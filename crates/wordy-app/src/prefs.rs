//! Per-user preferences that are not about any one project: theme,
//! appearance, scrollbars and the editor's type. Stored beside the sync
//! settings as `prefs.json` in `wordy_sync::config::config_dir()`.

use std::borrow::Cow;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use wordy_editor::EditorStyle;

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

/// A palette with a light half and a dark half; [`Appearance`] picks which
/// half shows. The families other than the stock one come from the JSON
/// files under `themes/`, registered by [`register_themes`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeFamily {
    #[default]
    Default,
    Wordy,
    Catppuccin,
    HighContrast,
}

impl ThemeFamily {
    pub const ALL: [ThemeFamily; 4] = [
        ThemeFamily::Default,
        ThemeFamily::Wordy,
        ThemeFamily::Catppuccin,
        ThemeFamily::HighContrast,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeFamily::Default => "Default",
            ThemeFamily::Wordy => "Wordy",
            ThemeFamily::Catppuccin => "Catppuccin",
            ThemeFamily::HighContrast => "High contrast",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            ThemeFamily::Default => "Neutral greys: the stock look.",
            ThemeFamily::Wordy => "Cream page, ink and oxblood, like the website and the icon.",
            ThemeFamily::Catppuccin => "Catppuccin Latte by day and Mocha by night.",
            ThemeFamily::HighContrast => "Black on white or white on black, hard borders, no shadows.",
        }
    }

    /// The registered light and dark theme names; none for the stock pair.
    fn names(self) -> Option<(&'static str, &'static str)> {
        match self {
            ThemeFamily::Default => None,
            ThemeFamily::Wordy => Some(("Wordy Light", "Wordy Dark")),
            ThemeFamily::Catppuccin => Some(("Catppuccin Latte", "Catppuccin Mocha")),
            ThemeFamily::HighContrast => Some(("High Contrast Light", "High Contrast Dark")),
        }
    }
}

/// The bundled theme files, one set per family.
const THEME_FILES: [&str; 3] = [
    include_str!("../themes/wordy.json"),
    include_str!("../themes/catppuccin.json"),
    include_str!("../themes/high-contrast.json"),
];

/// Put the bundled themes in gpui-component's registry. Once, after
/// `gpui_kit::init` and before the preferences are applied.
pub fn register_themes(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for file in THEME_FILES {
        if let Err(e) = registry.load_themes_from_str(file) {
            tracing::error!("bundled theme: {e:#}");
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

/// Body paragraphs: a first-line indent, or a blank half line between them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParagraphStyle {
    #[default]
    Indent,
    Spaced,
}

impl ParagraphStyle {
    pub const ALL: [ParagraphStyle; 2] = [ParagraphStyle::Indent, ParagraphStyle::Spaced];

    pub fn label(self) -> &'static str {
        match self {
            ParagraphStyle::Indent => "Indent first line",
            ParagraphStyle::Spaced => "Space between",
        }
    }
}

/// The type on the page you write on: font, size, spacing, measure. Maps
/// onto [`EditorStyle`]; the defaults are the editor's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextPrefs {
    pub font: String,
    /// Body size in pixels.
    pub size: f32,
    /// Line height as a multiple of the size.
    pub line_height: f32,
    /// The widest the text column gets, in pixels.
    pub width: f32,
    pub paragraphs: ParagraphStyle,
}

impl Default for TextPrefs {
    fn default() -> Self {
        let s = EditorStyle::default();
        Self {
            font: s.font_family.to_string(),
            size: f32::from(s.font_size),
            line_height: s.line_height,
            width: f32::from(s.max_width),
            paragraphs: ParagraphStyle::Indent,
        }
    }
}

impl TextPrefs {
    /// The serif that ships with Wordy, and that the PDF export uses.
    pub const BUNDLED_FONT: &str = "Libertinus Serif";
    pub const SIZE: (f32, f32, f32) = (12., 36., 1.);
    pub const LINE_HEIGHT: (f32, f32, f32) = (1.2, 2.2, 0.05);
    pub const WIDTH: (f32, f32, f32) = (440., 1100., 40.);

    pub fn style(&self) -> EditorStyle {
        let (paragraph_spacing, first_line_indent) = match self.paragraphs {
            ParagraphStyle::Indent => (0.0, px(28.)),
            ParagraphStyle::Spaced => (0.6, px(0.)),
        };
        EditorStyle {
            font_family: self.font.clone().into(),
            font_size: px(self.size),
            line_height: self.line_height,
            max_width: px(self.width),
            paragraph_spacing,
            first_line_indent,
            ..EditorStyle::default()
        }
    }

    /// Move one setting by `steps` of its increment, inside its range.
    pub fn step(value: &mut f32, (min, max, by): (f32, f32, f32), steps: f32) {
        let n = ((*value - min) / by).round() + steps;
        *value = (min + n * by).clamp(min, max);
        // Keep "1.65" from drifting to 1.6500001 in the file.
        *value = (*value * 100.).round() / 100.;
    }
}

/// Where a user drops font files Wordy should know about without installing
/// them: `fonts/` beside `prefs.json`.
pub fn fonts_dir() -> PathBuf {
    wordy_sync::config::config_dir().join("fonts")
}

/// The font files in [`fonts_dir`], sorted. Empty when the folder is missing.
pub fn user_font_files() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(fonts_dir()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
        })
        .collect();
    files.sort();
    files
}

/// Load every font file in [`fonts_dir`] into the text system. Safe to call
/// again after the folder changes; returns how many files were read. A file
/// that cannot be read is logged and skipped.
pub fn load_user_fonts(cx: &App) -> usize {
    let mut fonts: Vec<Cow<'static, [u8]>> = Vec::new();
    for path in user_font_files() {
        match std::fs::read(&path) {
            Ok(bytes) => fonts.push(Cow::Owned(bytes)),
            Err(e) => tracing::warn!("{}: {e}", path.display()),
        }
    }
    let n = fonts.len();
    if n > 0 {
        if let Err(e) = cx.text_system().add_fonts(fonts) {
            tracing::error!("user fonts: {e:#}");
            return 0;
        }
    }
    n
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemeFamily,
    pub appearance: Appearance,
    pub scrollbars: Scrollbars,
    pub text: TextPrefs,
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

    /// Push the preferences onto the theme (family, mode, scrollbars) and
    /// onto the editors (the global [`EditorStyle`]).
    pub fn apply(window: Option<&Window>, cx: &mut App) {
        let prefs = Prefs::global(cx).clone();
        cx.set_global(prefs.text.style());
        let mode = prefs.theme_mode(window, cx);
        let registry = ThemeRegistry::global(cx);
        let (light, dark) = prefs
            .theme
            .names()
            .and_then(|(l, d)| Some((registry.themes().get(l)?.clone(), registry.themes().get(d)?.clone())))
            .unwrap_or_else(|| {
                (
                    registry.default_light_theme().clone(),
                    registry.default_dark_theme().clone(),
                )
            });
        {
            // A theme file only sets the radius and shadow it names, so put
            // the stock values back before the chosen file is applied over
            // them; otherwise High contrast's square corners would outlive it.
            let fresh = Theme::default();
            let theme = Theme::global_mut(cx);
            theme.light_theme = light;
            theme.dark_theme = dark;
            theme.radius = fresh.radius;
            theme.radius_lg = fresh.radius_lg;
            theme.shadow = fresh.shadow;
        }
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
