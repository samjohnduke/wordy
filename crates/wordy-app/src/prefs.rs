//! Per-user preferences that are not about any one project: theme,
//! appearance, scrollbars and the editor's type. Stored beside the sync
//! settings as `prefs.json` in `wordy_sync::config::config_dir()`, next to
//! the `fonts/` and `themes/` folders a user can drop files into.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::StreamExt as _;
use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry, ThemeSet};
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
/// half shows. The bundled families other than the stock one come from the
/// JSON files under `themes/`, registered by [`register_themes`]; `User`
/// names a set the user dropped in their own themes folder, see
/// [`UserThemes`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeFamily {
    #[default]
    Default,
    Wordy,
    Catppuccin,
    HighContrast,
    User(String),
}

impl ThemeFamily {
    /// The families that ship with Wordy.
    pub const BUNDLED: [ThemeFamily; 4] = [
        ThemeFamily::Default,
        ThemeFamily::Wordy,
        ThemeFamily::Catppuccin,
        ThemeFamily::HighContrast,
    ];

    pub fn label(&self) -> String {
        match self {
            ThemeFamily::Default => "Default".into(),
            ThemeFamily::Wordy => "Wordy".into(),
            ThemeFamily::Catppuccin => "Catppuccin".into(),
            ThemeFamily::HighContrast => "High contrast".into(),
            ThemeFamily::User(name) => name.clone(),
        }
    }

    pub fn blurb(&self, cx: &App) -> String {
        match self {
            ThemeFamily::Default => "Neutral greys: the stock look.".into(),
            ThemeFamily::Wordy => "Cream page, ink and oxblood, like the website and the icon.".into(),
            ThemeFamily::Catppuccin => "Catppuccin Latte by day and Mocha by night.".into(),
            ThemeFamily::HighContrast => "Black on white or white on black, hard borders, no shadows.".into(),
            ThemeFamily::User(name) => match UserThemes::global(cx).get(name) {
                Some(set) => {
                    let halves = match (&set.light, &set.dark) {
                        (Some(_), Some(_)) => "Light and dark halves",
                        (Some(_), None) => "Light half only; the stock dark fills in",
                        (None, Some(_)) => "Dark half only; the stock light fills in",
                        (None, None) => "No themes in the file",
                    };
                    format!("{halves}, from {} in your themes folder.", set.file_name())
                }
                None => "This theme's file is gone from your themes folder; the stock look shows instead.".into(),
            },
        }
    }

    /// The registered light and dark theme names of a bundled family; none
    /// for the stock pair or a user theme.
    fn names(&self) -> Option<(&'static str, &'static str)> {
        match self {
            ThemeFamily::Default | ThemeFamily::User(_) => None,
            ThemeFamily::Wordy => Some(("Wordy Light", "Wordy Dark")),
            ThemeFamily::Catppuccin => Some(("Catppuccin Latte", "Catppuccin Mocha")),
            ThemeFamily::HighContrast => Some(("High Contrast Light", "High Contrast Dark")),
        }
    }

    /// The bundled JSON this family was loaded from; none for the stock
    /// pair or a user theme.
    fn bundled_file(&self) -> Option<&'static str> {
        match self {
            ThemeFamily::Wordy => Some(THEME_FILES[0]),
            ThemeFamily::Catppuccin => Some(THEME_FILES[1]),
            ThemeFamily::HighContrast => Some(THEME_FILES[2]),
            ThemeFamily::Default | ThemeFamily::User(_) => None,
        }
    }

    /// The light and dark configs to install for this family. A user set
    /// missing a half, or missing altogether, gets the stock half.
    fn configs(&self, cx: &App) -> (Rc<ThemeConfig>, Rc<ThemeConfig>) {
        let registry = ThemeRegistry::global(cx);
        let stock = || {
            (
                registry.default_light_theme().clone(),
                registry.default_dark_theme().clone(),
            )
        };
        match self {
            ThemeFamily::User(name) => match UserThemes::global(cx).get(name) {
                Some(set) => {
                    let (l, d) = stock();
                    (set.light.clone().unwrap_or(l), set.dark.clone().unwrap_or(d))
                }
                None => stock(),
            },
            _ => self
                .names()
                .and_then(|(l, d)| Some((registry.themes().get(l)?.clone(), registry.themes().get(d)?.clone())))
                .unwrap_or_else(stock),
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

/// One file from the user's themes folder: a gpui-component theme set, of
/// which Wordy keeps the first light and the first dark theme.
#[derive(Clone, Debug)]
pub struct UserThemeSet {
    /// The set's name, or the file stem when the file names none. Unique
    /// across the folder; a clash gets the stem appended.
    pub name: String,
    pub file: PathBuf,
    pub light: Option<Rc<ThemeConfig>>,
    pub dark: Option<Rc<ThemeConfig>>,
}

impl UserThemeSet {
    pub fn file_name(&self) -> String {
        self.file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Every theme file in the user's themes folder, parsed, plus the files
/// that would not parse and why. Rebuilt by [`load_user_themes`]: at launch,
/// on Reload, and whenever the folder changes while the app runs.
#[derive(Clone, Debug, Default)]
pub struct UserThemes {
    pub sets: Vec<UserThemeSet>,
    /// `(file name, error)` for each file that failed.
    pub errors: Vec<(String, String)>,
}

impl Global for UserThemes {}

impl UserThemes {
    pub fn global(cx: &App) -> &UserThemes {
        cx.global::<UserThemes>()
    }

    pub fn get(&self, name: &str) -> Option<&UserThemeSet> {
        self.sets.iter().find(|s| s.name == name)
    }
}

/// The name of the JSON Schema Wordy writes into the themes folder, so an
/// editor can validate and complete a theme file.
pub const THEME_SCHEMA_FILE: &str = "theme.schema.json";

/// Where a user drops theme files: `themes/` beside `prefs.json`.
pub fn themes_dir() -> PathBuf {
    wordy_sync::config::config_dir().join("themes")
}

/// The theme files in [`themes_dir`], sorted; the schema file is not one.
pub fn user_theme_files() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(themes_dir()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                && p.file_name().and_then(|n| n.to_str()) != Some(THEME_SCHEMA_FILE)
        })
        .collect();
    files.sort();
    files
}

fn parse_user_theme(path: &Path) -> Result<UserThemeSet> {
    let text = std::fs::read_to_string(path)?;
    let set: ThemeSet = serde_json::from_str(&text)?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = if set.name.trim().is_empty() {
        stem
    } else {
        set.name.to_string()
    };
    let mut light = None;
    let mut dark = None;
    for theme in set.themes {
        let slot = if theme.mode.is_dark() { &mut dark } else { &mut light };
        if slot.is_none() {
            *slot = Some(Rc::new(theme));
        }
    }
    Ok(UserThemeSet {
        name,
        file: path.to_path_buf(),
        light,
        dark,
    })
}

/// Read the themes folder into the [`UserThemes`] global and, when the
/// preferences are loaded, reapply them so a change to the chosen theme
/// shows at once.
pub fn load_user_themes(cx: &mut App) {
    let mut themes = UserThemes::default();
    for path in user_theme_files() {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match parse_user_theme(&path) {
            Ok(mut set) => {
                if themes.get(&set.name).is_some() {
                    let stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    set.name = format!("{} ({stem})", set.name);
                }
                themes.sets.push(set);
            }
            Err(e) => {
                tracing::warn!("theme {file}: {e:#}");
                themes.errors.push((file, format!("{e:#}")));
            }
        }
    }
    cx.set_global(themes);
    if cx.has_global::<Prefs>() {
        Prefs::apply(None, cx);
    }
}

/// Write the theme file schema into the themes folder, creating the folder,
/// so an editor can check a theme as it is written. Skipped when the file
/// already says the same.
pub fn write_theme_schema() -> Result<()> {
    let dir = themes_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let schema = schemars::schema_for!(ThemeSet);
    let json = serde_json::to_vec_pretty(&schema)?;
    let path = dir.join(THEME_SCHEMA_FILE);
    if std::fs::read(&path).is_ok_and(|old| old == json) {
        return Ok(());
    }
    wordy_doc::storage::write_atomic(&path, &json)
}

/// Watch the themes folder and reload it when a file is added, changed or
/// removed, so a theme can be edited with the app open. Events are
/// coalesced for a moment so one save does not reload the folder twice.
pub fn watch_user_themes(cx: &mut App) {
    use notify::Watcher as _;

    let dir = themes_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("{}: {e}", dir.display());
        return;
    }
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            if matches!(
                event.kind,
                notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
            ) {
                _ = tx.unbounded_send(());
            }
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("cannot watch {}: {e}", dir.display());
            return;
        }
    };
    if let Err(e) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
        tracing::warn!("cannot watch {}: {e}", dir.display());
        return;
    }
    cx.spawn(async move |cx| {
        // The watcher stops when dropped, so it lives as long as this task.
        let _watcher = watcher;
        while rx.next().await.is_some() {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            while rx.try_recv().is_ok() {}
            cx.update(load_user_themes);
        }
    })
    .detach();
}

/// Write a copy of a family's theme file into the themes folder as a
/// starting point for the user's own, and return its path. The copy gets
/// its own set and theme names so it lists apart from the original.
pub fn save_theme_copy(family: &ThemeFamily, cx: &App) -> Result<PathBuf> {
    let mut value: serde_json::Value = match family {
        ThemeFamily::User(name) => {
            let set = UserThemes::global(cx).get(name).context("theme file is gone")?;
            serde_json::from_str(&std::fs::read_to_string(&set.file)?)?
        }
        _ => match family.bundled_file() {
            Some(json) => serde_json::from_str(json)?,
            None => {
                let registry = ThemeRegistry::global(cx);
                let set = ThemeSet {
                    name: "Default".into(),
                    author: None,
                    url: None,
                    themes: vec![
                        registry.default_light_theme().as_ref().clone(),
                        registry.default_dark_theme().as_ref().clone(),
                    ],
                };
                let mut v = serde_json::to_value(&set)?;
                strip_nulls(&mut v);
                v
            }
        },
    };
    let base = format!("{} copy", family.label());
    if let Some(obj) = value.as_object_mut() {
        obj.insert("$schema".into(), format!("./{THEME_SCHEMA_FILE}").into());
        obj.insert("name".into(), base.clone().into());
        obj.remove("author");
        obj.remove("url");
        if let Some(themes) = obj.get_mut("themes").and_then(|t| t.as_array_mut()) {
            for theme in themes {
                if let Some(t) = theme.as_object_mut() {
                    let old = t.get("name").and_then(|n| n.as_str()).unwrap_or("Theme");
                    t.insert("name".into(), format!("{old} copy").into());
                    t.insert("is_default".into(), false.into());
                }
            }
        }
    }
    let dir = themes_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let slug: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let mut path = dir.join(format!("{slug}.json"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{slug}-{n}.json"));
        n += 1;
    }
    let json = serde_json::to_vec_pretty(&value)?;
    wordy_doc::storage::write_atomic(&path, &json)?;
    Ok(path)
}

/// Drop every `null` so a serialised config reads like a hand-written file.
fn strip_nulls(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
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
        let (light, dark) = prefs.theme.configs(cx);
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
