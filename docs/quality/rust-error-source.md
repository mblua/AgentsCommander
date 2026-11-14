# Rust errors keep their cause

Issue #4547. Applies in `src-tauri/` and `crates/` to new error paths and to modules a plan
converts. It does not ask you to convert other code in a file you touch.

1. Inside the backend, a failure that wraps another error is a typed error. Use `thiserror`
   (already a dependency) and keep the inner error with `#[source]` or `#[from]`, or name the
   field `source`. Do not write `map_err(|e| format!("...: {}", e))` in internal code.
2. Turn the error into `String` only where it leaves the process boundary: a
   `#[tauri::command]` return value, a web dispatch reply, or CLI output. Do it once, there,
   with `.to_string()` or `impl From<MyError> for String`.
3. Keep the `#[error(...)]` text equal to the old message when you convert existing code.
   Prove it with a test that compares the full string.
4. Add a test that `std::error::Error::source()` returns the inner error.
5. Convert one module per change. Existing `Result<_, String>` signatures can stay until a
   later plan moves the boundary; then convert at their last internal caller.

Pilot: `src-tauri/src/config/local_config_io/json_object.rs` (`ConfigJsonError`).
