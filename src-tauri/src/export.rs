//! Writing exported result data to a file the user picks.
//!
//! The save dialog runs here, not in the webview, and the webview only gets
//! a token for the chosen file: nothing it sends can name a path to write.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::error::{CommandError, CommandResult, ErrorCode};

/// Files chosen in a save dialog, by token, until their export finishes.
#[derive(Default)]
pub struct Exports {
    next: AtomicU64,
    open: Mutex<HashMap<u64, PathBuf>>,
}

impl Exports {
    /// Registers a chosen file and returns its token.
    fn register(&self, path: PathBuf) -> u64 {
        let token = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.open.lock().unwrap().insert(token, path);
        token
    }

    fn path(&self, token: u64) -> CommandResult<PathBuf> {
        self.open
            .lock()
            .unwrap()
            .get(&token)
            .cloned()
            .ok_or_else(|| CommandError::new(ErrorCode::NotFound, "this export is no longer open"))
    }

    /// Appends a chunk to the file behind `token`.
    fn append(&self, token: u64, chunk: &str) -> CommandResult<()> {
        let path = self.path(token)?;
        let fail = |e: std::io::Error| CommandError::new(ErrorCode::Storage, format!("cannot write {}: {e}", path.display()));
        OpenOptions::new().append(true).open(&path).map_err(fail)?.write_all(chunk.as_bytes()).map_err(fail)
    }

    fn finish(&self, token: u64) {
        self.open.lock().unwrap().remove(&token);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTarget {
    token: u64,
    /// For messages only.
    file_name: String,
}

/// Shows a save dialog and creates (or empties) the chosen file. `None`
/// when the user cancels.
#[tauri::command]
pub async fn export_begin(
    app: AppHandle,
    file_name: String,
    format_name: String,
    extension: String,
    exports: State<'_, Exports>,
) -> CommandResult<Option<ExportTarget>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(file_name)
        .add_filter(format_name, &[extension.as_str()])
        .save_file(move |path| {
            let _ = tx.send(path);
        });
    let Some(chosen) = rx.await.ok().flatten() else {
        return Ok(None);
    };
    let path = chosen
        .into_path()
        .map_err(|e| CommandError::new(ErrorCode::Storage, format!("unusable file location: {e}")))?;
    File::create(&path)
        .map_err(|e| CommandError::new(ErrorCode::Storage, format!("cannot write {}: {e}", path.display())))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Some(ExportTarget { token: exports.register(path), file_name: name }))
}

/// Appends one chunk of an export. The UI formats rows in chunks so a large
/// result never becomes one giant IPC message.
#[tauri::command]
pub async fn export_write(token: u64, chunk: String, exports: State<'_, Exports>) -> CommandResult<()> {
    exports.append(token, &chunk)
}

#[tauri::command]
pub fn export_finish(token: u64, exports: State<'_, Exports>) {
    exports.finish(token);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_only_to_registered_files_and_forgets_finished_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        File::create(&path).unwrap();
        let exports = Exports::default();
        let token = exports.register(path.clone());

        exports.append(token, "a,b\n").unwrap();
        exports.append(token, "1,2\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a,b\n1,2\n");

        // An unknown token names no file: there is no way to pass a path.
        assert!(exports.append(token + 1, "x").is_err());
        exports.finish(token);
        assert!(exports.append(token, "x").is_err());
    }
}
