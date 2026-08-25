use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRecord {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub profile: String,
    pub down_mbps: f64,
    pub up_mbps: f64,
    pub ping_ms: f64,
    pub jitter_ms: f64,
    pub grade: char,
}

fn history_path() -> PathBuf {
    // Overridable so tests never touch the user's real history file.
    if let Ok(custom) = std::env::var("SPEED_TEST_HISTORY_FILE") {
        return PathBuf::from(custom);
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".speed-test-history.json")
}

/// Loads the run history. A missing file is normal (first launch). A corrupt
/// file is moved aside to `*.json.bad` so the app keeps working and the data
/// can be inspected later instead of being overwritten silently.
pub fn load() -> Vec<TestRecord> {
    let path = history_path();
    match std::fs::read(&path) {
        Ok(data) => match serde_json::from_slice(&data) {
            Ok(records) => records,
            Err(e) => {
                let bad = path.with_extension("json.bad");
                let _ = std::fs::rename(&path, &bad);
                eprintln!(
                    "warning: history file was corrupt ({e}); moved to {} and starting fresh",
                    bad.display()
                );
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

pub fn save(records: &[TestRecord]) -> Result<()> {
    let path = history_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    // Write to a temp file first so an interruption can never truncate the
    // existing history.
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(records)?;
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
