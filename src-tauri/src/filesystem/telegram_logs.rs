//! Physical Telegram log operations; formatting and redaction stay with callers.

use std::fmt::Arguments;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub(crate) type TelegramLogFile = File;

pub(crate) fn ensure_telegram_log_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub(crate) fn open_telegram_bridge_log(path: &Path) -> io::Result<TelegramLogFile> {
    OpenOptions::new().create(true).append(true).open(path)
}

pub(crate) fn open_telegram_diagnostic_log(path: &Path) -> io::Result<TelegramLogFile> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
}

pub(crate) fn telegram_log_metadata(file: &TelegramLogFile) -> io::Result<Metadata> {
    file.metadata()
}

pub(crate) fn write_telegram_log(
    file: &mut TelegramLogFile,
    args: Arguments<'_>,
) -> io::Result<()> {
    file.write_fmt(args)
}

pub(crate) fn flush_telegram_log(file: &mut TelegramLogFile) -> io::Result<()> {
    file.flush()
}
