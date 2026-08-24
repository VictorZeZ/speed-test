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
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".speed-test-history.json")
}

pub fn load() -> Vec<TestRecord> {
    std::fs::read(history_path())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

pub fn save(records: &[TestRecord]) -> Result<()> {
    let data = serde_json::to_vec_pretty(records)?;
    std::fs::write(history_path(), data)?;
    Ok(())
}
