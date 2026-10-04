//! Per-project window layout, saved next to the project as `layout.json`.
//! Derived state: safe to delete, and never synced.

use std::path::Path;

use serde::{Deserialize, Serialize};
use wordy_doc::TreeID;

pub const LAYOUT_FILE: &str = "layout.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Layout {
    /// "manuscript" | "world" | "notes"
    pub space: String,
    /// Open editor tabs in bar order, as `TreeID` strings.
    pub tabs: Vec<String>,
    /// The tab that was showing: a node id, or `None` for Home / nothing.
    pub active: Option<String>,
    pub home_open: bool,
    pub settings_open: bool,
    /// Which Settings page was showing.
    pub settings_section: Option<String>,
    /// `true` when Settings, not Home, was the displayed fixed tab.
    pub settings_front: bool,
    pub reference_open: bool,
    pub reference_pinned: Option<String>,
    pub sidebar_open: bool,
    pub sidebar_width: Option<f32>,
    pub reference_width: Option<f32>,
    pub typewriter: bool,
    pub focus_mode: bool,
    /// Collapsed containers in the sidebar trees.
    pub collapsed: Vec<String>,
}

impl Layout {
    pub fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join(LAYOUT_FILE)).ok()?;
        match serde_json::from_str(&text) {
            Ok(l) => Some(l),
            Err(e) => {
                tracing::warn!("ignoring unreadable {LAYOUT_FILE}: {e}");
                None
            }
        }
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        wordy_doc::storage::write_atomic(&dir.join(LAYOUT_FILE), text.as_bytes())
    }

    pub fn ids(list: &[String]) -> Vec<TreeID> {
        list.iter().filter_map(|s| TreeID::try_from(s.as_str()).ok()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tolerates_missing_fields() {
        let dir = tempfile_dir();
        let l = Layout {
            space: "world".into(),
            tabs: vec!["1@2".into()],
            active: Some("1@2".into()),
            typewriter: true,
            ..Default::default()
        };
        l.save(&dir).unwrap();
        assert_eq!(Layout::load(&dir), Some(l));
        std::fs::write(dir.join(LAYOUT_FILE), "{\"space\": \"notes\"}").unwrap();
        let partial = Layout::load(&dir).unwrap();
        assert_eq!(partial.space, "notes");
        assert!(partial.tabs.is_empty());
        std::fs::write(dir.join(LAYOUT_FILE), "not json").unwrap();
        assert_eq!(Layout::load(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wordy-layout-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
