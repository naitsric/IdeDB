//! Writing exported result data to a file the user picked.

use std::fs::OpenOptions;
use std::io::Write;

use crate::error::{CommandError, CommandResult, ErrorCode};

/// Writes one chunk of an export. The UI formats rows in chunks so a large
/// result never becomes one giant IPC message; `first` truncates the file.
#[tauri::command]
pub async fn export_write(path: String, chunk: String, first: bool) -> CommandResult<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(first)
        .append(!first)
        .open(&path)
        .map_err(|e| CommandError::new(ErrorCode::Storage, format!("cannot write {path}: {e}")))?;
    file.write_all(chunk.as_bytes())
        .map_err(|e| CommandError::new(ErrorCode::Storage, format!("cannot write {path}: {e}")))
}
