//! App-level state: the open project, actions, key bindings, window creation.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use gpui_kit::component::{ActiveTheme as _, TitleBar};
use gpui_kit::*;
use wordy_doc::{storage, Matcher, Project, TreeID};
use wordy_editor::LinkTarget;
use wordy_index::{Backlink, Index, SearchHit};

use crate::prefs::{Appearance, Prefs};
use crate::workspace::Workspace;

gpui_kit::actions!(
    wordy,
    [
        Quit,
        ToggleTheme,
        Save,
        NewItem,
        Find,
        FindNext,
        FindPrev,
        Replace,
        CloseFind,
        ToggleReference,
        SearchProject,
        ShowHome,
        ShowSettings,
        ToggleFocusMode,
        ToggleTypewriter,
        ToggleSpellcheck,
        QuickOpen,
        CloseQuickOpen,
        QuickOpenUp,
        QuickOpenDown,
        NextTab,
        PrevTab,
        CloseTab,
        NextDocument,
        PrevDocument,
        FocusSidebar,
        NewProject,
        OpenProjectFolder,
        SpaceManuscript,
        SpaceWorld,
        SpaceNotes,
        SidebarUp,
        SidebarDown,
        SidebarLeft,
        SidebarRight,
        SidebarActivate,
        SidebarBack,
        SidebarRename,
        SidebarTrash
    ]
);

pub const EDITOR_PANEL_CONTEXT: &str = "EditorPanel";
pub const QUICK_OPEN_CONTEXT: &str = "QuickOpen";
pub const SIDEBAR_CONTEXT: &str = "Sidebar";

/// The open project, shared by every view in the window.
///
/// Loro containers are internally synchronized, so `Project` is read through
/// `&` and edits go straight to Loro. Views hold an `Rc<ProjectHandle>` and
/// re-read what they need on render.
pub struct ProjectHandle {
    pub project: Project,
    /// SQLite side index: backlinks, search, word counts. Derived, rebuildable.
    index: RefCell<Index>,
    /// Entity-name matcher shared by the index and every editor.
    matcher: RefCell<Rc<Matcher>>,
}

impl ProjectHandle {
    pub fn new(project: Project) -> Self {
        let index = match project.dir.as_deref().map(Index::open) {
            Some(Ok(ix)) => ix,
            other => {
                if let Some(Err(e)) = other {
                    tracing::error!("open index: {e:#}; using an in-memory index");
                }
                Index::in_memory().expect("in-memory sqlite")
            }
        };
        let handle = Self {
            project,
            index: RefCell::new(index),
            matcher: RefCell::new(Rc::new(Matcher::empty())),
        };
        handle.refresh_matcher();
        handle.rebuild_index();
        handle
    }

    pub fn dir(&self) -> Option<&PathBuf> {
        self.project.dir.as_ref()
    }

    pub fn matcher(&self) -> Rc<Matcher> {
        self.matcher.borrow().clone()
    }

    /// Rebuild the name matcher after entities were added, renamed, or aliased.
    pub fn refresh_matcher(&self) {
        *self.matcher.borrow_mut() = Rc::new(Matcher::new(&self.project.entity_names()));
    }

    pub fn rebuild_index(&self) {
        let matcher = self.matcher();
        if let Err(e) = self.index.borrow_mut().rebuild(&self.project, &matcher) {
            tracing::error!("rebuild index: {e:#}");
        }
    }

    pub fn update_index_node(&self, id: TreeID) {
        let matcher = self.matcher();
        if let Err(e) = self.index.borrow_mut().update_node(&self.project, id, &matcher) {
            tracing::error!("update index: {e:#}");
        }
    }

    /// Every live entity as a link target, in World-tree order.
    pub fn link_targets(&self) -> Vec<LinkTarget> {
        self.project
            .entities()
            .into_iter()
            .filter_map(|id| self.project.node(id).ok())
            .map(|n| LinkTarget {
                id: n.id,
                title: n.title(),
                aliases: n.aliases(),
            })
            .collect()
    }

    pub fn appears_in(&self, entity: TreeID) -> Vec<Backlink> {
        self.index.borrow().appears_in(entity).unwrap_or_else(|e| {
            tracing::error!("appears_in: {e:#}");
            Vec::new()
        })
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.index.borrow().search(query, limit).unwrap_or_else(|e| {
            tracing::error!("search: {e:#}");
            Vec::new()
        })
    }

    pub fn nodes_with_tag(&self, tag: &str) -> Vec<TreeID> {
        self.index.borrow().nodes_with_tag(tag).unwrap_or_else(|e| {
            tracing::error!("nodes_with_tag: {e:#}");
            Vec::new()
        })
    }

    /// The project's custom spelling dictionary, one word per line.
    pub fn dictionary_path(&self) -> Option<PathBuf> {
        self.dir().map(|d| d.join("dictionary.txt"))
    }

    pub fn load_dictionary(&self) -> Vec<String> {
        let Some(path) = self.dictionary_path() else {
            return Vec::new();
        };
        match std::fs::read_to_string(&path) {
            Ok(s) => s
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::error!("read dictionary: {e:#}");
                Vec::new()
            }
        }
    }

    pub fn append_dictionary_word(&self, word: &str) {
        let Some(path) = self.dictionary_path() else {
            return;
        };
        let mut words = self.load_dictionary();
        if words.iter().any(|w| w == word) {
            return;
        }
        words.push(word.to_string());
        words.sort_unstable_by_key(|w| w.to_lowercase());
        let mut body = words.join("\n");
        body.push('\n');
        if let Err(e) = storage::write_atomic(&path, body.as_bytes()) {
            tracing::error!("write dictionary: {e:#}");
        }
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
        KeyBinding::new("secondary-n", NewItem, None),
        KeyBinding::new("cmd-shift-t", ToggleTheme, None),
        KeyBinding::new("ctrl-shift-t", ToggleTheme, None),
        KeyBinding::new("secondary-f", Find, Some(EDITOR_PANEL_CONTEXT)),
        KeyBinding::new("secondary-g", FindNext, Some(EDITOR_PANEL_CONTEXT)),
        KeyBinding::new("secondary-shift-g", FindPrev, Some(EDITOR_PANEL_CONTEXT)),
        KeyBinding::new("secondary-h", Replace, Some(EDITOR_PANEL_CONTEXT)),
        KeyBinding::new("escape", CloseFind, Some(EDITOR_PANEL_CONTEXT)),
        KeyBinding::new("secondary-shift-r", ToggleReference, None),
        KeyBinding::new("secondary-shift-f", SearchProject, None),
        KeyBinding::new("secondary-0", ShowHome, None),
        KeyBinding::new("secondary-,", ShowSettings, None),
        KeyBinding::new("secondary-shift-d", ToggleFocusMode, None),
        KeyBinding::new("secondary-shift-y", ToggleTypewriter, None),
        KeyBinding::new("secondary-p", QuickOpen, None),
        KeyBinding::new("escape", CloseQuickOpen, Some(QUICK_OPEN_CONTEXT)),
        KeyBinding::new("up", QuickOpenUp, Some(QUICK_OPEN_CONTEXT)),
        KeyBinding::new("ctrl-p", QuickOpenUp, Some(QUICK_OPEN_CONTEXT)),
        KeyBinding::new("down", QuickOpenDown, Some(QUICK_OPEN_CONTEXT)),
        KeyBinding::new("ctrl-n", QuickOpenDown, Some(QUICK_OPEN_CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("secondary-w", CloseTab, None),
        KeyBinding::new("secondary-alt-down", NextDocument, None),
        KeyBinding::new("secondary-alt-up", PrevDocument, None),
        KeyBinding::new("secondary-e", FocusSidebar, None),
        KeyBinding::new("secondary-shift-n", NewProject, None),
        KeyBinding::new("secondary-shift-o", OpenProjectFolder, None),
        KeyBinding::new("secondary-1", SpaceManuscript, None),
        KeyBinding::new("secondary-2", SpaceWorld, None),
        KeyBinding::new("secondary-3", SpaceNotes, None),
        KeyBinding::new("up", SidebarUp, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("down", SidebarDown, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("left", SidebarLeft, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("right", SidebarRight, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("enter", SidebarActivate, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("escape", SidebarBack, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("f2", SidebarRename, Some(SIDEBAR_CONTEXT)),
        KeyBinding::new("delete", SidebarTrash, Some(SIDEBAR_CONTEXT)),
    ]);
    crate::prefs::register_themes(cx);
    if let Err(e) = crate::prefs::write_theme_schema() {
        tracing::warn!("theme schema: {e:#}");
    }
    crate::prefs::load_user_themes(cx);
    cx.set_global(Prefs::load());
    Prefs::apply(None, cx);
    crate::prefs::watch_user_themes(cx);
}

/// Open the project folder given on the command line (`wordy <dir>`,
/// created if it does not exist yet), else the project opened last on this
/// machine (`recent`, from prefs), else the project under `~/Wordy` whose
/// file was written most recently, else create "My Novel".
pub fn open_or_create_default_project(recent: &[PathBuf]) -> Result<Project> {
    if let Some(arg) = std::env::args_os().nth(1) {
        let dir = PathBuf::from(arg);
        if storage::snapshot_path(&dir).exists() {
            tracing::info!("opening {}", dir.display());
            return Project::open(&dir);
        }
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("My Novel")
            .to_string();
        tracing::info!("creating {}", dir.display());
        return Project::create(&dir, &name);
    }
    // A folder that was moved or deleted since is skipped, not an error.
    if let Some(dir) = recent.iter().find(|d| storage::snapshot_path(d).exists()) {
        tracing::info!("opening last project {}", dir.display());
        return Project::open(dir);
    }
    let root = storage::projects_root();
    std::fs::create_dir_all(&root)?;
    if let Some(dir) = storage::most_recent_project(&root) {
        tracing::info!("opening {}", dir.display());
        return Project::open(&dir);
    }
    let dir = root.join("My Novel");
    tracing::info!("creating {}", dir.display());
    Project::create(&dir, "My Novel")
}

pub fn open_main_window(cx: &mut App) {
    let recent = Prefs::global(cx).recent_projects.clone();
    let project = match open_or_create_default_project(&recent) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("could not open a project: {e:#}");
            cx.quit();
            return;
        }
    };
    // Startup housekeeping: a project with more than a year of edit history
    // gets backed up and compacted so it never needs manual attention.
    let notice = match project.prune_if_old() {
        Ok(Some(r)) => Some(format!(
            "compacted edit history older than {} days: project.loro {} → {}; the full file was backed up to snapshots/",
            r.age_days,
            human_size(r.before.file_bytes),
            human_size(r.after.file_bytes)
        )),
        Ok(None) => None,
        Err(e) => {
            tracing::error!("startup pruning: {e:#}");
            None
        }
    };
    open_project_window(project, notice, cx);
}

/// Open `project` in its own window. The first call is the main window;
/// later ones (a copy fetched from the account) sit beside it. A window
/// that will not open is fatal: there is nothing else to show.
pub fn open_project_window(project: Project, notice: Option<String>, cx: &mut App) {
    if let Err(e) = open_project_window_at(project, notice, None, cx) {
        tracing::error!("open window: {e:#}");
        cx.quit();
    }
}

/// Open `project` in a new window, at `bounds` when given (switching
/// projects puts the new window where the old one was) else centred.
/// Remembers the folder as the most recent project.
pub fn open_project_window_at(
    project: Project,
    notice: Option<String>,
    bounds: Option<WindowBounds>,
    cx: &mut App,
) -> Result<()> {
    if let Some(dir) = project.dir.clone() {
        Prefs::remember_project(&dir, cx);
    }
    let shared: SharedProject = Rc::new(ProjectHandle::new(project));
    wordy_editor::SpellState::set_custom_words(cx, shared.load_dictionary());

    let bounds =
        bounds.unwrap_or_else(|| WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(820.)), cx)));
    let options = WindowOptions {
        window_bounds: Some(bounds),
        window_min_size: Some(size(px(720.), px(480.))),
        app_id: Some("dev.sam.wordy".into()),
        ..TitleBar::window_options()
    };

    gpui_kit::open_window(options, cx, move |window, cx| {
        // Follow the system theme when that is the preference.
        window
            .observe_window_appearance(|window, cx| Prefs::system_changed(window, cx))
            .detach();
        cx.new(|cx| {
            let mut ws = Workspace::new(shared, window, cx);
            ws.set_notice(notice);
            ws
        })
    })?;
    cx.activate(true);
    Ok(())
}

fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.0} KB", n as f64 / 1024.)
    } else {
        format!("{:.1} MB", n as f64 / (1024. * 1024.))
    }
}

/// Flip light / dark and pin the result as the preference (so "match the
/// system" stops following until it is chosen again in Settings).
pub fn toggle_theme(window: &mut Window, cx: &mut App) {
    let next = if cx.theme().mode.is_dark() {
        Appearance::Light
    } else {
        Appearance::Dark
    };
    Prefs::update(Some(window), cx, |p| p.appearance = next);
}
