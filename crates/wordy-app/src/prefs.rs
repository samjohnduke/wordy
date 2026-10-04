//! Per-user preferences that are not about any one project: appearance and
//! scrollbars. Stored beside the sync settings as `prefs.json` in
//! `wordy_sync::config::config_dir()`.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

/// Which theme to use: follow the system, or pin one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "Match the system",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }
}

/// When scrollbars are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scrollbars {
    /// While scrolling, then fade out.
    #[default]
    Scrolling,
    /// While the pointer is over the pane.
    Hover,
    Always,
}

impl Scrollbars {
    pub const ALL: [Scrollbars; 3] = [Scrollbars::Scrolling, Scrollbars::Hover, Scrollbars::Always];

    pub fn label(self) -> &'static str {
        match self {
            Scrollbars::Scrolling => "While scrolling",
            Scrollbars::Hover => "On hover",
            Scrollbars::Always => "Always",
        }
    }

    fn mode(self) -> ScrollbarMode {
        match self {
            Scrollbars::Scrolling => ScrollbarMode::Scrolling,
            Scrollbars::Hover => ScrollbarMode::Hover,
            Scrollbars::Always => ScrollbarMode::Always,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub appearance: Appearance,
    pub scrollbars: Scrollbars,
}

impl Global for Prefs {}

pub fn prefs_path() -> PathBuf {
    wordy_sync::config::config_dir().join("prefs.json")
}

impl Prefs {
    pub fn load() -> Self {
        let path = prefs_path();
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("{}: {e}; using defaults", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = prefs_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        wordy_doc::storage::write_atomic(&path, &json)
    }

    pub fn global(cx: &App) -> &Prefs {
        cx.global::<Prefs>()
    }

    /// The theme mode the appearance setting asks for right now.
    fn theme_mode(&self, window: Option<&Window>, cx: &App) -> ThemeMode {
        match self.appearance {
            Appearance::Light => ThemeMode::Light,
            Appearance::Dark => ThemeMode::Dark,
            Appearance::System => window
                .map(|w| w.appearance())
                .unwrap_or_else(|| cx.window_appearance())
                .into(),
        }
    }

    /// Push the preferences onto the theme: mode and scrollbars.
    pub fn apply(window: Option<&Window>, cx: &mut App) {
        let prefs = Prefs::global(cx).clone();
        let mode = prefs.theme_mode(window, cx);
        Theme::change(mode, None, cx);
        Theme::set_scrollbar_mode(prefs.scrollbars.mode(), cx);
    }

    /// Change one or more preferences, save them and apply them.
    pub fn update(window: Option<&Window>, cx: &mut App, edit: impl FnOnce(&mut Prefs)) {
        let mut prefs = Prefs::global(cx).clone();
        edit(&mut prefs);
        if *Prefs::global(cx) == prefs {
            return;
        }
        if let Err(e) = prefs.save() {
            tracing::error!("save prefs: {e:#}");
        }
        cx.set_global(prefs);
        Prefs::apply(window, cx);
    }

    /// The system appearance changed: follow it when that is the setting.
    pub fn system_changed(window: &Window, cx: &mut App) {
        if Prefs::global(cx).appearance == Appearance::System {
            Prefs::apply(Some(window), cx);
        }
    }
}
