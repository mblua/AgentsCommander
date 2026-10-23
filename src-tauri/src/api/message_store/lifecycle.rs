use super::locks::verify_existing_sqlite_files;
#[cfg(test)]
use super::PtyInputStoreTestFaults;
use super::{
    response_work_key, MessageDeliveryReservation, MessageStore, MessageStoreError,
    PtyInputMaintenanceCursors, DB_FILENAME, RESPONSE_CLOSING_DEFERRED,
};
use crate::pty::manager::{ResponseCloseError, ResponseCloseState, WorkAdmission};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

impl MessageStore {
    pub(crate) fn bind_response_close(&self, state: ResponseCloseState) {
        if let Some(bound) = self.response_close.get() {
            if !bound.same_state(&state) {
                bound.fail_closed();
                state.fail_closed();
            }
            return;
        }
        if self.response_close.set(state.clone()).is_err() {
            state.fail_closed();
            return;
        }
        let recovered = (|| -> Result<(), MessageStoreError> {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            for (table, plane, digest, statuses) in [
                (
                    "messages",
                    "db",
                    "body_sha256",
                    "'queued','retry','delivering'",
                ),
                (
                    "pty_input_operations",
                    "pty",
                    "request_fingerprint",
                    "'queued','preparing','retry','actuating'",
                ),
            ] {
                let mut stmt = conn.prepare(&format!(
                    "SELECT target_fqn,sender_fqn,op_id,{} FROM {} WHERE status IN ({})",
                    digest, table, statuses
                ))?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?;
                for row in rows {
                    let (target, sender, op, digest) = row?;
                    let key = response_work_key(plane, &sender, &op);
                    let mut admission = state
                        .reserve_work(&target, &key)
                        .map_err(|_| MessageStoreError::StoreCorrupt)?;
                    admission
                        .accept_durable(&digest)
                        .map_err(|_| MessageStoreError::StoreCorrupt)?;
                }
            }
            Ok(())
        })();
        if recovered.is_err() {
            state.fail_closed();
        } else {
            state.mark_work_bootstrapped();
        }
    }

    pub(crate) fn response_target_is_closing(&self, target: &str) -> bool {
        self.response_close
            .get()
            .is_some_and(|state| state.target_is_closing(target))
    }

    pub(crate) fn begin_response_delivery(
        &self,
        target: &str,
    ) -> Result<MessageDeliveryReservation, String> {
        let inner = match self.response_close.get() {
            None => None,
            Some(state) => match state.begin_delivery(target) {
                Ok(reservation) => Some(reservation),
                Err(ResponseCloseError::Unavailable) => None,
                Err(ResponseCloseError::ClosingDeferred) => {
                    return Err(RESPONSE_CLOSING_DEFERRED.to_string())
                }
                Err(error) => return Err(format!("response close admission: {:?}", error)),
            },
        };
        Ok(MessageDeliveryReservation { _inner: inner })
    }

    pub(super) fn reserve_response_work(&self, target: &str, key: &str) -> Option<WorkAdmission> {
        let state = self.response_close.get()?;
        match state.reserve_work(target, key) {
            Ok(admission) => Some(admission),
            Err(_) => {
                state.fail_closed();
                None
            }
        }
    }

    pub(super) fn accept_response_work(&self, admission: &mut Option<WorkAdmission>, digest: &str) {
        if admission
            .as_mut()
            .is_some_and(|admission| admission.accept_durable(digest).is_err())
        {
            if let Some(state) = self.response_close.get() {
                state.fail_closed();
            }
        }
    }

    pub(super) fn settle_response_row(&self, target: &str, key: &str) {
        if let Some(state) = self.response_close.get() {
            state.settle_terminal(target, key, std::time::Instant::now());
        }
    }

    pub fn at_config_dir() -> Result<Self, MessageStoreError> {
        let dir = crate::config::config_dir().ok_or(MessageStoreError::MissingConfigDir)?;
        Self::open(dir.join(DB_FILENAME))
    }

    pub fn open(path: PathBuf) -> Result<Self, MessageStoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            crate::path_identity::verify_directory(parent)
                .map_err(|_| MessageStoreError::UnsafePath)?;
        }
        verify_existing_sqlite_files(&path)?;
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let conn = rusqlite::Connection::open_with_flags(&path, flags)?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let _: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        verify_existing_sqlite_files(&path)?;
        let store = Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
            maintenance_cursors: Arc::new(Mutex::new(PtyInputMaintenanceCursors::default())),
            response_close: Arc::new(std::sync::OnceLock::new()),
            #[cfg(test)]
            test_faults: Arc::new(Mutex::new(PtyInputStoreTestFaults::default())),
        };
        store.migrate()?;
        store.recover_pty_input_startup()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Test-only teardown: release the SQLite/WAL/SHM file handles held by the
    /// live connection even while other `Arc` clones of this store (managed
    /// app state, API state) still exist. The connection is replaced with an
    /// in-memory one so the stored `Mutex<Connection>` stays usable and the
    /// dropped file-backed connection closes its handles; callers must not use
    /// the store afterwards. `AcceptanceFixture` calls this before its
    /// temporary directory is removed, because on Windows a directory cannot
    /// be deleted while the SQLite connection keeps it open.
    #[cfg(test)]
    pub(crate) fn close_for_test(&self) {
        let mut guard = match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Ok(in_memory) = rusqlite::Connection::open_in_memory() {
            let _ = std::mem::replace(&mut *guard, in_memory);
        }
    }

    #[cfg(test)]
    pub(super) fn inject_actuation_commit_ambiguous(&self) {
        self.test_faults
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .actuation_commit_ambiguous = true;
    }

    #[cfg(test)]
    pub(super) fn count_by_status(&self, status: &str) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE status = ?1",
                [status],
                |row| row.get(0),
            )
            .unwrap()
    }
}
