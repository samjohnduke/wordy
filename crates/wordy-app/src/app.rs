//! App-level state: the open project, actions, key bindings, window creation.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, TitleBar};
use gpui_kit::*;
use wordy_doc::{storage, Matcher, Project, TreeID};
use wordy_editor::LinkTarget;
use wordy_index::{Backlink, Index, SearchHit};

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
        ShowHome
    ]
);

pub const EDITOR_PANEL_CONTEXT: &str = "EditorPanel";

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
        let handle = Self { project, index: RefCell::new(index), matcher: RefCell::new(Rc::new(Matcher::empty())) };
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
            .map(|n| LinkTarget { id: n.id, title: n.title(), aliases: n.aliases() })
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
        let Some(path) = self.dictionary_path() else { return Vec::new() };
        match std::fs::read_to_string(&path) {
            Ok(s) => s.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::error!("read dictionary: {e:#}");
                Vec::new()
            }
        }
    }

    pub fn append_dictionary_word(&self, word: &str) {
        let Some(path) = self.dictionary_path() else { return };
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
        KeyBinding::new("secondary-shift-h", ShowHome, None),
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
    let shared: SharedProject = Rc::new(ProjectHandle::new(project));
    wordy_editor::SpellState::set_custom_words(cx, shared.load_dictionary());

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
