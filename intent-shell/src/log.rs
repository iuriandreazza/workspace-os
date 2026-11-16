//! The decision log (ADR 0002): one JSON object per line, appended.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::ports::DecisionLog;

pub struct JsonlLog {
    file: File,
}

impl JsonlLog {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(Self {
            file: OpenOptions::new().create(true).append(true).open(path)?,
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
