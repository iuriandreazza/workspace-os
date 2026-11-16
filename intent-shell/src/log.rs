//! The decision log (ADR 0002): one JSON object per line, appended.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::ports::DecisionLog;

pub struct JsonlLog {
    file: File,
}

impl JsonlLog {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        // The log holds requests, paths and file content: private to the user.
        if let Some(dir) = path.parent() {
            DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        Ok(Self {
            file: OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(path)?,
        })
    }
}

impl DecisionLog for JsonlLog {
    fn record(&mut self, mut event: Value) -> Result<(), String> {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Some(object) = event.as_object_mut() {
            object.insert("unix_time".to_owned(), seconds.into());
        }
        // One write call per line, flushed before anything runs, so a crash
        // cannot lose the record of a decision that was acted on.
        let line = format!("{event}\n");
        self.file
            .write_all(line.as_bytes())
            .and_then(|()| self.file.sync_data())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_log_is_private_to_the_user() {
        let dir = std::env::temp_dir().join(format!("intent-shell-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("state/decisions.jsonl");
        let mut log = JsonlLog::open(&path).unwrap();
        log.record(serde_json::json!({"event": "x"})).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
