use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrecencyData {
    timestamps: Vec<u64>,
}

impl FrecencyData {
    pub fn new() -> Self {
        Self { timestamps: Vec::new() }
    }

    pub fn record(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.timestamps.push(now);
        if self.timestamps.len() > 10 {
            self.timestamps.remove(0);
        }
    }

    pub fn score(&self) -> u32 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.timestamps.iter().map(|&t| {
            let age_hours = (now.saturating_sub(t)) / 3600;
            match age_hours {
                0..=4 => 100,
                5..=24 => 70,
                25..=168 => 50,
                169..=720 => 30,
                _ => 10,
            }
        }).sum()
    }
}

pub struct Frecency {
    data: HashMap<String, FrecencyData>,
    path: PathBuf,
}

impl Frecency {
    pub fn load() -> Self {
        let path = Self::path();
        let data = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { data, path }
    }

    fn path() -> PathBuf {
        let data_dir = env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap()).join(".local/share"));
        data_dir.join("launcher/frecency.json")
    }

    pub fn record(&mut self, key: &str) {
        self.data.entry(key.to_string())
            .or_insert_with(FrecencyData::new)
            .record();
        self.save();
    }

    pub fn score(&self, key: &str) -> u32 {
        self.data.get(key).map(|d| d.score()).unwrap_or(0)
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&self.path, serde_json::to_string(&self.data).unwrap_or_default());
    }
}
