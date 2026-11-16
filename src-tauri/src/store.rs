// Persistent state: device name, save folder, transfer history (JSON, atomic writes).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub ts: u64,
    pub direction: String, // "sent" | "received"
    pub peer: String,
    pub files: Vec<String>,
    pub bytes: u64,
    pub status: String, // "done" | "failed" | "cancelled" | "rejected"
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct VaultState {
    pub device_name: Option<String>,
    pub save_dir: Option<String>,
    pub history: Vec<HistoryEntry>,
}

const HISTORY_CAP: usize = 200;

pub struct Store {
    path: PathBuf,
    pub state: VaultState,
}

impl Store {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("nearsend.json");
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Store { path, state }
    }

    pub fn save(&self) -> Result<(), String> {
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(&self.state).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn record(&mut self, entry: HistoryEntry) {
        self.state.history.insert(0, entry);
        self.state.history.truncate(HISTORY_CAP);
        let _ = self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nearsend-store-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn history_is_capped_and_persisted() {
        let dir = temp_dir("cap");
        let mut store = Store::load(&dir);
        for i in 0..220 {
            store.record(HistoryEntry {
                ts: i,
                direction: "sent".into(),
                peer: "p".into(),
                files: vec![format!("f{i}")],
                bytes: 1,
                status: "done".into(),
            });
        }
        assert_eq!(store.state.history.len(), HISTORY_CAP);
        let reloaded = Store::load(&dir);
        assert_eq!(reloaded.state.history.len(), HISTORY_CAP);
        assert_eq!(reloaded.state.history[0].files[0], "f219");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_state_falls_back_to_default() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("nearsend.json"), "{not json").unwrap();
        let store = Store::load(&dir);
        assert!(store.state.device_name.is_none());
        assert!(store.state.history.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
