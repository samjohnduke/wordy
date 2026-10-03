//! App-level state: the open project, actions, key bindings, window creation.

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, TitleBar};
use gpui_kit::*;
use wordy_doc::{storage, Project};

use crate::workspace::Workspace;

gpui_kit::actions!(wordy, [Quit, ToggleTheme, Save]);

/// The open project, shared by every view in the window.
///
/// Loro containers are internally synchronized, so `Project` is read through
/// `&` and edits go straight to Loro. Views hold an `Rc<ProjectHandle>` and
/// re-read what they need on render.
pub struct ProjectHandle {
    pub project: Project,
}

impl ProjectHandle {
    pub fn dir(&self) -> Option<&PathBuf> {
        self.project.dir.as_ref()
    }
}

pub type SharedProject = Rc<ProjectHandle>;

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("ctrl-s", Save, None),
        KeyBinding::new("cmd-shift-t", ToggleTheme, None),
        KeyBinding::new("ctrl-shift-t", ToggleTheme, None),
    ]);
    Theme::sync_system_appearance(None, cx);
}

/// Open the most recent project under `~/Wordy`, or create "My Novel".
pub fn open_or_create_default_project() -> Result<Project> {
    let root = storage::projects_root();
    std::fs::create_dir_all(&root)?;
    let mut projects = storage::list_projects(&root);
    if let Some(dir) = projects.pop() {
        tracing::info!("opening {}", dir.display());
        return Project::open(&dir);
    }
    let dir = root.join("My Novel");
    tracing::info!("creating {}", dir.display());
    Project::create(&dir, "My Novel")
}

pub fn open_main_window(cx: &mut App) {
    let project = match open_or_create_default_project() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("could not open a project: {e:#}");
            cx.quit();
            return;
        }
    };
    let shared: SharedProject = Rc::new(ProjectHandle { project });

    let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(720.), px(480.))),
        app_id: Some("dev.sam.wordy".into()),
        ..TitleBar::window_options()
    };

    if let Err(e) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| Workspace::new(shared, window, cx))
    }) {
        tracing::error!("open window: {e:#}");
        cx.quit();
    }
    cx.activate(true);
}

pub fn toggle_theme(window: &mut Window, cx: &mut App) {
    let next = if cx.theme().mode.is_dark() { ThemeMode::Light } else { ThemeMode::Dark };
    Theme::change(next, Some(window), cx);
}
