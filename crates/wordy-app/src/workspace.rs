//! The main window: title bar, space rail, dock area, status bar.
//! Owns the open editor tabs, the autosave timer, focus mode, the quick-open
//! palette and keyboard navigation between tabs and documents.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui_kit::base::dock::PanelId;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{panel_handle, DockArea, DockLayout, DockPlacement, DockSkin, PanelStyle};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::momentum::today;
use wordy_doc::{storage, NodeKind, Space, TreeID, Version, BULK_ORIGIN};
use wordy_editor::SpellState;

use crate::app::{
    self, CloseQuickOpen, CloseTab, Find, FindNext, FindPrev, FocusSidebar, NewItem, NextDocument, NextTab,
    PrevDocument, PrevTab, QuickOpen, QuickOpenDown, QuickOpenUp, Quit, Replace, Save, SearchProject, SharedProject,
    ShowHome, SpaceManuscript, SpaceNotes, SpaceWorld, ToggleFocusMode, ToggleReference, ToggleSpellcheck, ToggleTheme,
    ToggleTypewriter, QUICK_OPEN_CONTEXT,
};
use crate::layout::Layout;
use crate::panels::editor::{EditorPanel, EditorPanelEvent};
use crate::panels::home::{HomeEvent, HomePanel};
use crate::panels::reference::{ReferenceEvent, ReferencePanel};
use crate::panels::sidebar::{SidebarEvent, SidebarPanel};
use crate::sync::{SyncEvent, SyncManager};

const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);
/// Layout changes (tabs, docks, collapse state) are written after this pause.
const LAYOUT_DELAY: Duration = Duration::from_millis(400);
/// Gaps between edits longer than this do not count as writing time.
const SESSION_IDLE: Duration = Duration::from_secs(60);
/// A snapshot copy goes to `snapshots/` on the first save of a session and
/// then at most this often (Ctrl-S always writes one).
const BACKUP_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Rows shown in the quick-open palette.
const QUICK_OPEN_LIMIT: usize = 12;
/// Commands mixed into a node search (a `>` prefix shows commands only).
const QUICK_OPEN_MIXED_ACTIONS: usize = 3;

/// The palette (Ctrl-P): a filter box over every node and every command.
struct QuickOpenState {
    input: Entity<InputState>,
    selected: usize,
    _sub: Subscription,
}

/// One palette row.
enum QuickOpenHit {
    Node {
        id: TreeID,
        title: String,
        kind: &'static str,
        space: Space,
        has_body: bool,
    },
    Action {
        group: &'static str,
        label: &'static str,
        action: Box<dyn Action>,
    },
}

/// How well a title matches the palette query; higher sorts first.
fn match_rank(title: &str, query: &str, terms: &[&str]) -> Option<u8> {
    if terms.is_empty() {
        return Some(1);
    }
    let lower = title.to_lowercase();
    if lower.starts_with(terms[0]) && terms.iter().all(|t| lower.contains(t)) {
        return Some(3);
    }
    if terms.iter().all(|t| lower.contains(t)) {
        return Some(2);
    }
    // Fuzzy: the query's letters appear in order ("chp1" finds "Chapter 1").
    let mut chars = query.chars().filter(|c| !c.is_whitespace());
    let mut want = chars.next()?;
    for c in lower.chars() {
        if c == want {
            match chars.next() {
                Some(n) => want = n,
                None => return Some(1),
            }
        }
    }
    None
}

/// Every command the palette offers. Editor commands only when a document is open.
fn palette_actions(has_editor: bool) -> Vec<(&'static str, &'static str, Box<dyn Action>)> {
    let mut v: Vec<(&'static str, &'static str, Box<dyn Action>)> = vec![
        ("Go", "Home", Box::new(ShowHome)),
        ("Go", "Manuscript", Box::new(SpaceManuscript)),
        ("Go", "World", Box::new(SpaceWorld)),
        ("Go", "Notes", Box::new(SpaceNotes)),
        ("Go", "Next tab", Box::new(NextTab)),
        ("Go", "Previous tab", Box::new(PrevTab)),
        ("Go", "Close tab", Box::new(CloseTab)),
        ("Go", "Next document", Box::new(NextDocument)),
        ("Go", "Previous document", Box::new(PrevDocument)),
        ("Go", "Focus the sidebar", Box::new(FocusSidebar)),
        ("View", "Toggle reference pane", Box::new(ToggleReference)),
        ("View", "Toggle focus mode", Box::new(ToggleFocusMode)),
        ("View", "Toggle typewriter scrolling", Box::new(ToggleTypewriter)),
        ("View", "Toggle light / dark theme", Box::new(ToggleTheme)),
        ("Project", "Search the project", Box::new(SearchProject)),
        ("Project", "New scene / entity / note", Box::new(NewItem)),
        ("Project", "Save now", Box::new(Save)),
        ("Project", "Toggle spellcheck", Box::new(ToggleSpellcheck)),
    ];
    if has_editor {
        use wordy_editor::*;
        v.extend([
            ("Edit", "Find", Box::new(Find) as Box<dyn Action>),
            ("Edit", "Find and replace", Box::new(Replace)),
            ("Edit", "Next match", Box::new(FindNext)),
            ("Edit", "Previous match", Box::new(FindPrev)),
            ("Edit", "Undo", Box::new(Undo)),
            ("Edit", "Redo", Box::new(Redo)),
            ("Edit", "Select all", Box::new(SelectAll)),
            ("Format", "Bold", Box::new(ToggleBold)),
            ("Format", "Italic", Box::new(ToggleItalic)),
            ("Format", "Underline", Box::new(ToggleUnderline)),
            ("Format", "Strikethrough", Box::new(ToggleStrike)),
            ("Format", "Small caps", Box::new(ToggleSmallCaps)),
            ("Format", "Cycle highlight colour", Box::new(ToggleHighlight)),
            ("Format", "Clear formatting", Box::new(ClearFormatting)),
            ("Format", "Paragraph", Box::new(SetParagraph)),
            ("Format", "Heading 1", Box::new(SetHeading1)),
            ("Format", "Heading 2", Box::new(SetHeading2)),
            ("Format", "Heading 3", Box::new(SetHeading3)),
            ("Format", "Quote", Box::new(SetQuote)),
            ("Format", "Scene break", Box::new(InsertSceneBreak)),
            ("Notes", "Add comment", Box::new(AddComment)),
            ("Notes", "Edit comment at caret", Box::new(EditCommentAtCaret)),
            (
                "Notes",
                "Show / hide resolved comments",
                Box::new(ToggleResolvedComments),
            ),
            ("Notes", "Insert link", Box::new(InsertLink)),
            ("Notes", "Remove link", Box::new(RemoveLink)),
        ]);
    }
    v
}

/// A keystroke the way the Home tab's shortcut list writes it.
fn pretty_keystroke(k: &Keystroke) -> String {
    let mac = cfg!(target_os = "macos");
    let m = &k.modifiers;
    let mut parts: Vec<String> = Vec::new();
    if m.control {
        parts.push(if mac { "⌃" } else { "Ctrl" }.into());
    }
    if m.alt {
        parts.push(if mac { "⌥" } else { "Alt" }.into());
    }
    if m.shift {
        parts.push(if mac { "⇧" } else { "Shift" }.into());
    }
    if m.platform {
        parts.push(if mac { "⌘" } else { "Super" }.into());
    }
    parts.push(match k.key.as_str() {
        "enter" => "Enter".into(),
        "escape" => "Esc".into(),
        "tab" => "Tab".into(),
        "space" => "Space".into(),
        "backspace" => "⌫".into(),
        "delete" => "Del".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        key if key.chars().count() == 1 => key.to_uppercase(),
        key => key.to_string(),
    });
    parts.join(if mac { "" } else { "+" })
}

pub struct Workspace {
    project: SharedProject,
    space: Space,
    dock: Entity<DockArea>,
    sidebar: Entity<SidebarPanel>,
    reference: Entity<ReferencePanel>,
    placeholder: Option<Entity<EditorPanel>>,
    home: Option<Entity<HomePanel>>,
    sync: Entity<SyncManager>,
    editors: HashMap<TreeID, Entity<EditorPanel>>,
    /// Open editor tabs in opening order, for Ctrl-Tab cycling.
    tab_order: Vec<TreeID>,
    active: Option<TreeID>,
    /// Focus mode: rail, status bar and both docks hidden; meta bars gone;
    /// paragraphs away from the caret dimmed.
    focus_mode: bool,
    /// Which docks were open when focus mode started, to restore on exit.
    docks_before_focus: (bool, bool),
    typewriter: bool,
    quick_open: Option<QuickOpenState>,
    last_backup: Option<Instant>,
    dirty: bool,
    /// Writing time since the last save, and when the last edit happened.
    session_seconds: f64,
    last_edit: Option<Instant>,
    /// Nodes edited since the last save; re-indexed on save.
    dirty_nodes: HashSet<TreeID>,
    last_saved: Option<String>,
    save_task: Option<Task<()>>,
    layout_task: Option<Task<()>>,
    /// Set once `restore_layout` has run; layout writes before that are noise.
    restoring: bool,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(project: SharedProject, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dock, skin) = DockSkin::dock_area("wordy-main", Some(1), window, cx);
        skin.set_panel_style(PanelStyle::TabBar, cx);
        skin.set_close_button_visible(true, cx);

        let sidebar = cx.new(|cx| SidebarPanel::new(project.clone(), Space::Manuscript, window, cx));
        let placeholder = cx.new(|cx| EditorPanel::placeholder(project.clone(), cx));
        let reference = cx.new(|cx| ReferencePanel::new(project.clone(), cx));

        dock.update(cx, |dock, cx| {
            dock.set_dock(
                DockPlacement::Left,
                DockLayout::tabs().panel_view(panel_handle(sidebar.clone()), cx),
                window,
                cx,
            );
            dock.set_dock_size(DockPlacement::Left, px(260.), window, cx);
            dock.set_center(
                DockLayout::tabs().panel_view(panel_handle(placeholder.clone()), cx),
                window,
                cx,
            );
            dock.set_dock(
                DockPlacement::Right,
                DockLayout::tabs().panel_view(panel_handle(reference.clone()), cx),
                window,
                cx,
            );
            dock.set_dock_size(DockPlacement::Right, px(320.), window, cx);
            dock.toggle_dock(DockPlacement::Right, window, cx);
        });

        let sub = cx.subscribe_in(&sidebar, window, |this, _, ev: &SidebarEvent, window, cx| match ev {
            SidebarEvent::Open(id) => this.open_node(*id, window, cx),
            SidebarEvent::Changed => this.on_tree_changed(cx),
            SidebarEvent::Removed(id) => this.close_node(*id, window, cx),
            SidebarEvent::FocusEditor => this.focus_active(window, cx),
            SidebarEvent::LayoutChanged => this.layout_changed(cx),
            SidebarEvent::Renamed { id, from, to } => this.on_renamed(*id, from.clone(), to.clone(), window, cx),
        });
        let ref_sub = cx.subscribe_in(
            &reference,
            window,
            |this, _, ev: &ReferenceEvent, window, cx| match ev {
                ReferenceEvent::Open(id) => this.open_node(*id, window, cx),
                ReferenceEvent::Pin(id) => this.pin_reference(*id, window, cx),
                ReferenceEvent::RestoreVersion(v) => this.restore_version(v.clone(), window, cx),
            },
        );

        let sync = cx.new(|cx| SyncManager::new(project.clone(), cx));
        let sync_sub = cx.subscribe(&sync, |this, _, ev: &SyncEvent, cx| match ev {
            SyncEvent::Applied(outcome) => this.after_sync(outcome, cx),
        });

        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        // Today's writing session starts from the current manuscript count.
        if let Err(e) = project
            .project
            .begin_session(today(), project.project.manuscript_word_count())
        {
            tracing::error!("begin session: {e:#}");
        }

        let dock_sub = cx.observe(&dock, |this, _, cx| this.layout_changed(cx));

        let mut this = Self {
            project,
            space: Space::Manuscript,
            dock,
            sidebar,
            reference,
            placeholder: Some(placeholder),
            home: None,
            sync,
            editors: HashMap::new(),
            tab_order: Vec::new(),
            active: None,
            focus_mode: false,
            docks_before_focus: (true, false),
            typewriter: false,
            quick_open: None,
            last_backup: None,
            dirty: false,
            session_seconds: 0.,
            last_edit: None,
            dirty_nodes: HashSet::new(),
            last_saved: None,
            save_task: None,
            layout_task: None,
            restoring: true,
            focus,
            _subs: vec![sub, ref_sub, sync_sub, dock_sub],
        };
        this.restore_layout(window, cx);
        this.restoring = false;
        this
    }

    // ----- layout persistence ---------------------------------------------

    /// Everything about the window worth putting back on the next launch.
    fn capture_layout(&self, cx: &App) -> Layout {
        let dock = self.dock.read(cx);
        let sidebar = self.sidebar.read(cx);
        Layout {
            space: self.space.as_str().to_string(),
            tabs: self.tab_order.iter().map(|id| id.to_string()).collect(),
            active: self.active.map(|id| id.to_string()),
            home_open: self.home.is_some(),
            reference_open: dock.is_dock_open(DockPlacement::Right),
            reference_pinned: self.reference.read(cx).pinned_id().map(|id| id.to_string()),
            sidebar_open: dock.is_dock_open(DockPlacement::Left),
            sidebar_width: dock.dock_size(DockPlacement::Left).map(f32::from),
            reference_width: dock.dock_size(DockPlacement::Right).map(f32::from),
            typewriter: self.typewriter,
            focus_mode: self.focus_mode,
            collapsed: sidebar.collapsed_ids().iter().map(|id| id.to_string()).collect(),
        }
    }

    /// Something layout-ish changed: write `layout.json` after a short pause.
    fn layout_changed(&mut self, cx: &mut Context<Self>) {
        if self.restoring {
            return;
        }
        self.layout_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LAYOUT_DELAY).await;
            this.update(cx, |ws, cx| ws.save_layout(cx)).ok();
        }));
    }

    fn save_layout(&mut self, cx: &mut Context<Self>) {
        self.layout_task = None;
        let Some(dir) = self.project.dir().cloned() else {
            return;
        };
        if let Err(e) = self.capture_layout(cx).save(&dir) {
            tracing::warn!("layout save failed: {e:#}");
        }
    }

    /// Put the window back the way it was when the project was last open.
    fn restore_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.project.dir().and_then(|d| Layout::load(d)) else {
            return;
        };
        let p = self.project.clone();
        let p = &p.project;
        let space = match layout.space.as_str() {
            "world" => Space::World,
            "notes" => Space::Notes,
            _ => Space::Manuscript,
        };
        self.set_space(space, cx);
        self.dock.update(cx, |dock, cx| {
            if let Some(w) = layout.sidebar_width.filter(|w| *w >= 120.) {
                dock.set_dock_size(DockPlacement::Left, px(w), window, cx);
            }
            if let Some(w) = layout.reference_width.filter(|w| *w >= 160.) {
                dock.set_dock_size(DockPlacement::Right, px(w), window, cx);
            }
            if dock.is_dock_open(DockPlacement::Left) != layout.sidebar_open {
                dock.toggle_dock(DockPlacement::Left, window, cx);
            }
            if dock.is_dock_open(DockPlacement::Right) != layout.reference_open {
                dock.toggle_dock(DockPlacement::Right, window, cx);
            }
        });
        if let Some(id) = layout
            .reference_pinned
            .as_deref()
            .and_then(|s| TreeID::try_from(s).ok())
        {
            if p.is_live(id) {
                self.reference.update(cx, |r, cx| r.pin(id, cx));
            }
        }
        let collapsed: Vec<TreeID> = Layout::ids(&layout.collapsed)
            .into_iter()
            .filter(|id| p.is_live(*id))
            .collect();
        self.sidebar.update(cx, |s, cx| s.set_collapsed(collapsed, cx));
        self.typewriter = layout.typewriter;
        if layout.home_open {
            self.show_home(window, cx);
        }
        let active = layout.active.as_deref().and_then(|s| TreeID::try_from(s).ok());
        for id in Layout::ids(&layout.tabs) {
            if self.project.project.is_live(id) {
                self.open_node(id, window, cx);
            }
        }
        match active {
            Some(id) if self.editors.contains_key(&id) => self.open_node(id, window, cx),
            _ if layout.home_open => self.show_home(window, cx),
            _ => {}
        }
        if layout.focus_mode {
            self.set_focus_mode(true, window, cx);
        }
        self.sidebar.update(cx, |s, cx| s.select(self.active, cx));
        cx.notify();
    }

    /// Show `id` in the reference pane, opening the pane if it is hidden.
    fn pin_reference(&mut self, id: TreeID, window: &mut Window, cx: &mut Context<Self>) {
        self.reference.update(cx, |r, cx| r.pin(id, cx));
        self.dock.update(cx, |dock, cx| {
            if !dock.is_dock_open(DockPlacement::Right) {
                dock.toggle_dock(DockPlacement::Right, window, cx);
            }
        });
        self.layout_changed(cx);
        cx.notify();
    }

    fn follow_link(&mut self, id: TreeID, navigate: bool, window: &mut Window, cx: &mut Context<Self>) {
        if navigate {
            self.open_node(id, window, cx);
        } else {
            self.pin_reference(id, window, cx);
        }
    }

    /// Show only the Manuscript scenes that mention `entity` in the sidebar.
    fn filter_mentions(&mut self, entity: TreeID, cx: &mut Context<Self>) {
        self.set_space(Space::Manuscript, cx);
        self.sidebar.update(cx, |s, cx| s.set_mention_filter(Some(entity), cx));
        cx.notify();
    }

    /// An entity was renamed: offer to rewrite the text under its explicit
    /// links so the prose follows the new name.
    fn on_renamed(&mut self, id: TreeID, from: String, to: String, window: &mut Window, cx: &mut Context<Self>) {
        let is_entity = self
            .project
            .project
            .node(id)
            .map(|n| n.kind() == NodeKind::Entity)
            .unwrap_or(false);
        if !is_entity || from.trim().is_empty() || to.trim().is_empty() {
            return;
        }
        let hits = self.project.project.linked_mentions(id, &from);
        let n: usize = hits.iter().map(|(_, c)| c).sum();
        if n == 0 {
            return;
        }
        let scenes = hits.len();
        let message = format!(
            "Also replace {n} linked mention{} in the text?",
            if n == 1 { "" } else { "s" }
        );
        let detail = format!(
            "“{from}” becomes “{to}” under its links, in {scenes} scene{}.",
            if scenes == 1 { "" } else { "s" }
        );
        let rx = window.prompt(
            PromptLevel::Info,
            &message,
            Some(&detail),
            &["Replace", "Keep text"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if rx.await != Ok(0) {
                return;
            }
            this.update(cx, |ws, cx| ws.replace_linked_mentions(id, &hits, &from, &to, cx))
                .ok();
        })
        .detach();
    }

    /// Rewrite linked mentions scene by scene: through the open editor where
    /// there is one (so it lands on that tab's undo stack), directly on the
    /// document otherwise.
    fn replace_linked_mentions(
        &mut self,
        entity: TreeID,
        hits: &[(TreeID, usize)],
        from: &str,
        to: &str,
        cx: &mut Context<Self>,
    ) {
        let mut changed = 0;
        for (node, _) in hits {
            let node = *node;
            let editor = self.editors.get(&node).and_then(|p| p.read(cx).editor().cloned());
            match editor {
                Some(editor) => {
                    changed += editor.update(cx, |e, cx| e.replace_linked_mentions(entity, from, to, cx));
                }
                None => match self
                    .project
                    .project
                    .replace_linked_mentions(node, entity, from, to, BULK_ORIGIN)
                {
                    Ok(n) => {
                        changed += n;
                        if n > 0 {
                            self.dirty_nodes.insert(node);
                        }
                    }
                    Err(e) => tracing::error!("replace linked mentions: {e:#}"),
                },
            }
            self.reference.update(cx, |r, cx| r.refresh_if(node, cx));
        }
        tracing::info!("renamed {changed} linked mention(s) of {entity}");
        self.on_edited(cx);
    }

    /// Show a saved version in the reference pane.
    fn view_version(&mut self, v: Version, window: &mut Window, cx: &mut Context<Self>) {
        self.reference.update(cx, |r, cx| r.pin_version(v, cx));
        self.dock.update(cx, |dock, cx| {
            if !dock.is_dock_open(DockPlacement::Right) {
                dock.toggle_dock(DockPlacement::Right, window, cx);
            }
        });
        cx.notify();
    }

    /// Restore a version into its node, opening the node's tab first so the
    /// restore lands on that editor's undo stack.
    fn restore_version(&mut self, v: Version, window: &mut Window, cx: &mut Context<Self>) {
        self.open_node(v.node, window, cx);
        if let Some(panel) = self.editors.get(&v.node).cloned() {
            panel.update(cx, |p, cx| p.restore_version(&v, cx));
        }
        self.reference.update(cx, |r, cx| r.unpin(cx));
    }

    /// Open (or focus) the Home tab.
    fn show_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(home) = self.home.clone() {
            let pid = PanelId::from(home.entity_id());
            self.dock.update(cx, |dock, cx| dock.select_panel(pid, window, cx));
            home.update(cx, |_, cx| cx.notify());
            self.layout_changed(cx);
            return;
        }
        let home = cx.new(|cx| HomePanel::new(self.project.clone(), self.sync.clone(), window, cx));
        let sub = cx.subscribe_in(&home, window, |this, _, ev: &HomeEvent, window, cx| match ev {
            HomeEvent::Open(id) => this.open_node(*id, window, cx),
            HomeEvent::Reveal { id, offset, len } => {
                this.open_node(*id, window, cx);
                if let Some(panel) = this.editors.get(id).cloned() {
                    panel.update(cx, |p, cx| p.reveal(*offset, *len, window, cx));
                }
            }
            HomeEvent::Changed => this.on_edited(cx),
            HomeEvent::Activated => {
                this.active = None;
                this.layout_changed(cx);
                cx.notify();
            }
            HomeEvent::Closed => {
                this.home = None;
                this.layout_changed(cx);
                cx.notify();
            }
        });
        self._subs.push(sub);
        let placeholder = self.placeholder.take();
        let pid = PanelId::from(home.entity_id());
        self.dock.update(cx, |dock, cx| {
            dock.add_panel_view(panel_handle(home.clone()), DockPlacement::Center, None, window, cx);
            if let Some(ph) = placeholder {
                dock.remove_panel(ph, window, cx);
            }
            dock.select_panel(pid, window, cx);
        });
        self.home = Some(home);
        self.layout_changed(cx);
        cx.notify();
    }

    fn on_show_home(&mut self, _: &ShowHome, window: &mut Window, cx: &mut Context<Self>) {
        self.show_home(window, cx);
    }

    /// A sync imported edits from the other machine (already saved): refresh
    /// the index, the matcher, every open editor and the spell checker.
    fn after_sync(&mut self, outcome: &wordy_sync::SyncOutcome, cx: &mut Context<Self>) {
        if !outcome.new_words.is_empty() {
            SpellState::set_custom_words(cx, self.project.load_dictionary());
        }
        self.project.refresh_matcher();
        self.project.rebuild_index();
        let targets = self.project.link_targets();
        for panel in self.editors.values() {
            panel.update(cx, |p, cx| {
                p.reload(cx);
                p.set_link_targets(targets.clone(), cx);
                p.rescan_spelling(cx);
            });
        }
        self.reference.update(cx, |r, cx| {
            r.set_link_targets(targets, cx);
            cx.notify();
        });
        self.dock.update(cx, |_, cx| cx.notify());
        self.dirty = false;
        self.dirty_nodes.clear();
        self.last_saved = Some(chrono_time());
        if let Some(home) = &self.home {
            home.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// A word was added to the custom dictionary: persist it and re-check
    /// every open editor.
    fn on_dictionary_changed(&mut self, word: &str, cx: &mut Context<Self>) {
        self.project.append_dictionary_word(word);
        let _ = SpellState::custom_words(cx);
        for panel in self.editors.values() {
            panel.update(cx, |p, cx| p.rescan_spelling(cx));
        }
    }

    fn toggle_reference(&mut self, _: &ToggleReference, window: &mut Window, cx: &mut Context<Self>) {
        self.dock
            .update(cx, |dock, cx| dock.toggle_dock(DockPlacement::Right, window, cx));
    }

    fn search_project(&mut self, _: &SearchProject, window: &mut Window, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| {
            if !dock.is_dock_open(DockPlacement::Left) {
                dock.toggle_dock(DockPlacement::Left, window, cx);
            }
        });
        self.sidebar.update(cx, |s, cx| s.focus_search(window, cx));
    }

    fn set_space(&mut self, space: Space, cx: &mut Context<Self>) {
        if self.space == space {
            return;
        }
        self.space = space;
        self.sidebar.update(cx, |s, cx| s.set_space(space, cx));
        self.layout_changed(cx);
        cx.notify();
    }

    /// Show the editor tab for `id`, creating it on first open.
    fn open_node(&mut self, id: TreeID, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.editors.get(&id).cloned() {
            let pid = PanelId::from(panel.entity_id());
            self.dock.update(cx, |dock, cx| dock.select_panel(pid, window, cx));
            panel.update(cx, |p, cx| p.focus_editor(window, cx));
            self.active = Some(id);
            self.layout_changed(cx);
            cx.notify();
            return;
        }
        let body = match self.project.project.node(id).and_then(|n| n.body()) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("open node: {e:#}");
                return;
            }
        };
        let panel = cx.new(|cx| EditorPanel::open(self.project.clone(), id, body, window, cx));
        // Keep each editor's undo stack to its own body: exclude every other
        // open editor's commit origin, in both directions.
        if let Some(new_editor) = panel.read(cx).editor().cloned() {
            let new_origin = new_editor.read(cx).origin().to_string();
            let others: Vec<Entity<wordy_editor::ProseEditor>> = self
                .editors
                .values()
                .filter_map(|p| p.read(cx).editor().cloned())
                .collect();
            for other in others {
                let other_origin = other.read(cx).origin().to_string();
                new_editor.update(cx, |e, _| e.exclude_origin(&other_origin));
                other.update(cx, |e, _| e.exclude_origin(&new_origin));
            }
        }
        let sub = cx.subscribe_in(
            &panel,
            window,
            move |this, _, ev: &EditorPanelEvent, window, cx| match ev {
                EditorPanelEvent::Edited => {
                    this.dirty_nodes.insert(id);
                    this.reference.update(cx, |r, cx| r.refresh_if(id, cx));
                    this.on_edited(cx);
                }
                EditorPanelEvent::NamesChanged => this.on_tree_changed(cx),
                EditorPanelEvent::OpenLink { id, navigate } => this.follow_link(*id, *navigate, window, cx),
                EditorPanelEvent::DictionaryChanged(w) => this.on_dictionary_changed(w, cx),
                EditorPanelEvent::MetaChanged => {
                    this.dirty_nodes.insert(id);
                    this.sidebar.update(cx, |_, cx| cx.notify());
                    this.on_edited(cx);
                }
                EditorPanelEvent::ViewVersion(v) => this.view_version(v.clone(), window, cx),
                EditorPanelEvent::FilterMentions(id) => this.filter_mentions(*id, cx),
                EditorPanelEvent::Activated => {
                    this.active = Some(id);
                    this.sidebar.update(cx, |s, cx| s.select(Some(id), cx));
                    this.layout_changed(cx);
                    cx.notify();
                }
                EditorPanelEvent::Closed => {
                    this.editors.remove(&id);
                    this.tab_order.retain(|t| *t != id);
                    if this.active == Some(id) {
                        this.active = None;
                    }
                    this.layout_changed(cx);
                    cx.notify();
                }
            },
        );
        self._subs.push(sub);
        self.editors.insert(id, panel.clone());
        self.tab_order.push(id);
        let (focus_mode, typewriter) = (self.focus_mode, self.typewriter);
        panel.update(cx, |p, cx| {
            p.set_focus_mode(focus_mode, cx);
            p.set_typewriter(typewriter, cx);
        });

        let placeholder = self.placeholder.take();
        let pid = PanelId::from(panel.entity_id());
        self.dock.update(cx, |dock, cx| {
            dock.add_panel_view(panel_handle(panel.clone()), DockPlacement::Center, None, window, cx);
            if let Some(ph) = placeholder {
                dock.remove_panel(ph, window, cx);
            }
            dock.select_panel(pid, window, cx);
        });
        panel.update(cx, |p, cx| p.focus_editor(window, cx));
        self.active = Some(id);
        self.layout_changed(cx);
        cx.notify();
    }

    fn close_node(&mut self, id: TreeID, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.editors.remove(&id) {
            self.tab_order.retain(|t| *t != id);
            self.dock.update(cx, |dock, cx| dock.remove_panel(panel, window, cx));
        }
        if self.active == Some(id) {
            self.active = None;
        }
        self.on_edited(cx);
    }

    // ----- focus mode, typewriter ------------------------------------------

    fn toggle_focus_mode(&mut self, _: &ToggleFocusMode, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.focus_mode;
        self.set_focus_mode(on, window, cx);
    }

    fn set_focus_mode(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_mode == on {
            return;
        }
        self.focus_mode = on;
        let (left, right) = {
            let d = self.dock.read(cx);
            (
                d.is_dock_open(DockPlacement::Left),
                d.is_dock_open(DockPlacement::Right),
            )
        };
        let (want_left, want_right) = if on {
            self.docks_before_focus = (left, right);
            (false, false)
        } else {
            self.docks_before_focus
        };
        self.dock.update(cx, |dock, cx| {
            if left != want_left {
                dock.toggle_dock(DockPlacement::Left, window, cx);
            }
            if right != want_right {
                dock.toggle_dock(DockPlacement::Right, window, cx);
            }
        });
        for panel in self.editors.values() {
            panel.update(cx, |p, cx| p.set_focus_mode(on, cx));
        }
        self.focus_active(window, cx);
        self.layout_changed(cx);
        cx.notify();
    }

    /// Flip the project's spellcheck setting and apply it to every open editor.
    fn toggle_spellcheck(&mut self, _: &ToggleSpellcheck, _: &mut Window, cx: &mut Context<Self>) {
        let on = !self.project.project.spellcheck();
        if let Err(e) = self.project.project.set_spellcheck(on) {
            tracing::error!("spellcheck setting: {e:#}");
        }
        for panel in self.editors.values() {
            panel.update(cx, |p, cx| p.set_spellcheck(on, cx));
        }
        self.on_edited(cx);
        cx.notify();
    }

    fn toggle_typewriter(&mut self, _: &ToggleTypewriter, _: &mut Window, cx: &mut Context<Self>) {
        self.typewriter = !self.typewriter;
        let on = self.typewriter;
        for panel in self.editors.values() {
            panel.update(cx, |p, cx| p.set_typewriter(on, cx));
        }
        self.layout_changed(cx);
        cx.notify();
    }

    /// Put keyboard focus back where writing happens: the active editor, else
    /// the Home tab, else the workspace itself.
    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.active.and_then(|id| self.editors.get(&id)).cloned() {
            panel.update(cx, |p, cx| p.focus_editor(window, cx));
        } else if let Some(home) = &self.home {
            let f = home.read(cx).focus_handle(cx);
            window.focus(&f, cx);
        } else {
            window.focus(&self.focus, cx);
        }
    }

    // ----- tabs and documents ----------------------------------------------

    /// Tabs in bar order: Home first when open, then editors by opening order.
    fn tab_items(&self) -> Vec<Option<TreeID>> {
        let mut items = Vec::with_capacity(self.tab_order.len() + 1);
        if self.home.is_some() {
            items.push(None);
        }
        items.extend(self.tab_order.iter().copied().map(Some));
        items
    }

    fn current_tab(&self) -> Option<Option<TreeID>> {
        match self.active {
            Some(id) => Some(Some(id)),
            None => self.home.is_some().then_some(None),
        }
    }

    fn show_tab(&mut self, tab: Option<TreeID>, window: &mut Window, cx: &mut Context<Self>) {
        match tab {
            None => self.show_home(window, cx),
            Some(id) => self.open_node(id, window, cx),
        }
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.tab_items();
        if items.is_empty() {
            return;
        }
        let cur = self.current_tab().and_then(|c| items.iter().position(|i| *i == c));
        let next = match cur {
            Some(ix) => (ix as isize + delta).rem_euclid(items.len() as isize) as usize,
            None => 0,
        };
        self.show_tab(items[next], window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(1, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(-1, window, cx);
    }

    /// Close the displayed tab and show its neighbour.
    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cur) = self.current_tab() else {
            return;
        };
        let items = self.tab_items();
        let ix = items.iter().position(|i| *i == cur).unwrap_or(0);
        match cur {
            Some(id) => {
                if let Some(panel) = self.editors.remove(&id) {
                    self.tab_order.retain(|t| *t != id);
                    self.dock.update(cx, |dock, cx| dock.remove_panel(panel, window, cx));
                }
                self.active = None;
            }
            None => {
                if let Some(home) = self.home.take() {
                    self.dock.update(cx, |dock, cx| dock.remove_panel(home, window, cx));
                }
            }
        }
        let items = self.tab_items();
        if items.is_empty() {
            self.ensure_placeholder(window, cx);
        } else {
            let next = items[ix.min(items.len() - 1)];
            self.show_tab(next, window, cx);
        }
        cx.notify();
    }

    /// With no tabs left, show the "open a scene" placeholder again.
    fn ensure_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.placeholder.is_some() || !self.editors.is_empty() || self.home.is_some() {
            return;
        }
        let placeholder = cx.new(|cx| EditorPanel::placeholder(self.project.clone(), cx));
        self.dock.update(cx, |dock, cx| {
            dock.add_panel_view(
                panel_handle(placeholder.clone()),
                DockPlacement::Center,
                None,
                window,
                cx,
            );
        });
        self.placeholder = Some(placeholder);
        window.focus(&self.focus, cx);
    }

    /// Open the next (or previous) document with a body in the active
    /// document's space, in tree order.
    fn step_document(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let docs = {
            let p = &self.project.project;
            let space = self
                .active
                .and_then(|a| p.node(a).ok())
                .map(|n| n.space())
                .unwrap_or(self.space);
            let mut docs = Vec::new();
            p.walk(p.root(space), &mut |id, n| {
                if n.kind().has_body() {
                    docs.push(id);
                }
            });
            docs
        };
        if docs.is_empty() {
            return;
        }
        let cur = self.active.and_then(|a| docs.iter().position(|d| *d == a));
        let next = match cur {
            Some(ix) => (ix as isize + delta).rem_euclid(docs.len() as isize) as usize,
            None if delta > 0 => 0,
            None => docs.len() - 1,
        };
        let id = docs[next];
        self.open_node(id, window, cx);
        self.sidebar.update(cx, |s, cx| s.select(Some(id), cx));
    }

    fn next_document(&mut self, _: &NextDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.step_document(1, window, cx);
    }

    fn prev_document(&mut self, _: &PrevDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.step_document(-1, window, cx);
    }

    fn open_left_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| {
            if !dock.is_dock_open(DockPlacement::Left) {
                dock.toggle_dock(DockPlacement::Left, window, cx);
            }
        });
    }

    fn focus_sidebar(&mut self, _: &FocusSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.open_left_dock(window, cx);
        self.sidebar.update(cx, |s, cx| s.focus_tree(window, cx));
    }

    fn space_shortcut(&mut self, space: Space, window: &mut Window, cx: &mut Context<Self>) {
        self.open_left_dock(window, cx);
        self.set_space(space, cx);
    }

    // ----- quick open --------------------------------------------------------

    fn quick_open(&mut self, _: &QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(q) = &self.quick_open {
            q.input.update(cx, |s, cx| {
                s.focus(window, cx);
                s.select_all(window, cx);
            });
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Jump to a scene, entity, or note…"));
        input.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                if let Some(q) = &mut this.quick_open {
                    q.selected = 0;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.quick_open_confirm(window, cx),
            _ => {}
        });
        self.quick_open = Some(QuickOpenState {
            input,
            selected: 0,
            _sub: sub,
        });
        cx.notify();
    }

    fn close_quick_open(&mut self, _: &CloseQuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_open.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// Nodes and commands matching the query. Nodes whose title starts with
    /// the first word come first, then other matches in tree order, then a
    /// few matching commands. A `>` prefix lists commands only.
    fn quick_open_hits(&self, cx: &App) -> Vec<QuickOpenHit> {
        let raw = self
            .quick_open
            .as_ref()
            .map(|q| q.input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let (actions_only, query) = match raw.strip_prefix('>') {
            Some(rest) => (true, rest.trim().to_string()),
            None => (false, raw),
        };
        let terms: Vec<&str> = query.split_whitespace().collect();

        let has_editor = self.active.is_some_and(|id| self.editors.contains_key(&id));
        let mut actions: Vec<(u8, QuickOpenHit)> = Vec::new();
        if actions_only || !terms.is_empty() {
            for (group, label, action) in palette_actions(has_editor) {
                if let Some(rank) = match_rank(label, &query, &terms) {
                    actions.push((rank, QuickOpenHit::Action { group, label, action }));
                }
            }
            actions.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
        }
        if actions_only {
            let mut out: Vec<QuickOpenHit> = actions.into_iter().map(|(_, h)| h).collect();
            out.truncate(QUICK_OPEN_LIMIT);
            return out;
        }

        let p = &self.project.project;
        let mut ranked: Vec<(u8, usize, QuickOpenHit)> = Vec::new();
        for (order, id) in p.all_nodes().into_iter().enumerate() {
            let Ok(n) = p.node(id) else { continue };
            let title = n.title();
            let Some(rank) = match_rank(&title, &query, &terms) else {
                continue;
            };
            let kind = n.kind();
            ranked.push((
                rank,
                order,
                QuickOpenHit::Node {
                    id,
                    title,
                    kind: kind.as_str(),
                    space: n.space(),
                    has_body: kind.has_body(),
                },
            ));
        }
        ranked.sort_by_key(|(rank, order, _)| (std::cmp::Reverse(*rank), *order));
        let n_actions = actions.len().min(QUICK_OPEN_MIXED_ACTIONS);
        let mut out: Vec<QuickOpenHit> = ranked.into_iter().map(|(_, _, h)| h).collect();
        out.truncate(QUICK_OPEN_LIMIT - n_actions);
        out.extend(actions.into_iter().take(n_actions).map(|(_, h)| h));
        out
    }

    fn quick_open_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.quick_open_hits(cx).len();
        if let Some(q) = &mut self.quick_open {
            if n > 0 {
                q.selected = (q.selected as isize + delta).rem_euclid(n as isize) as usize;
            }
        }
        cx.notify();
    }

    fn quick_open_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self.quick_open.as_ref().map(|q| q.selected) else {
            return;
        };
        let mut hits = self.quick_open_hits(cx);
        if sel >= hits.len() {
            return;
        }
        let (id, space, has_body) = match hits.swap_remove(sel) {
            QuickOpenHit::Action { action, .. } => {
                self.quick_open = None;
                self.focus_active(window, cx);
                window.dispatch_action(action, cx);
                cx.notify();
                return;
            }
            QuickOpenHit::Node {
                id, space, has_body, ..
            } => (id, space, has_body),
        };
        self.quick_open = None;
        self.set_space(space, cx);
        self.sidebar.update(cx, |s, cx| s.select(Some(id), cx));
        if has_body {
            self.open_node(id, window, cx);
        } else {
            self.open_left_dock(window, cx);
            self.sidebar.update(cx, |s, cx| s.focus_tree(window, cx));
        }
        cx.notify();
    }

    fn render_quick_open(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let q = self.quick_open.as_ref()?;
        let hits = self.quick_open_hits(cx);
        let selected = q.selected.min(hits.len().saturating_sub(1));
        let (accent, secondary, muted, border, popover) = {
            let t = cx.theme();
            (t.accent, t.secondary, t.muted_foreground, t.border, t.popover)
        };
        // Keybinding hints are resolved where the command would run.
        let focus = self
            .active
            .and_then(|id| self.editors.get(&id))
            .and_then(|p| p.read(cx).editor_focus(cx))
            .unwrap_or_else(|| self.focus.clone());
        let empty = hits.is_empty();
        let rows: Vec<AnyElement> = hits
            .into_iter()
            .enumerate()
            .map(|(ix, hit)| {
                let (title, detail) = match &hit {
                    QuickOpenHit::Node { title, space, kind, .. } => {
                        (title.clone(), format!("{} · {}", space.label(), kind))
                    }
                    QuickOpenHit::Action { group, label, action } => {
                        let keys = window
                            .highest_precedence_binding_for_action_in(action.as_ref(), &focus)
                            .map(|b| {
                                b.keystrokes()
                                    .iter()
                                    .map(|k| pretty_keystroke(k.inner()))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .unwrap_or_default();
                        (format!("› {label}"), format!("{group}  {keys}").trim_end().to_string())
                    }
                };
                h_flex()
                    .id(ElementId::Name(format!("qo-{ix}").into()))
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .text_sm()
                    .cursor_pointer()
                    .when(ix == selected, |d| d.bg(accent))
                    .when(ix != selected, |d| d.hover(|s| s.bg(secondary)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    )
                    .child(div().text_xs().text_color(muted).flex_shrink_0().child(detail))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(q) = &mut this.quick_open {
                            q.selected = ix;
                        }
                        this.quick_open_confirm(window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        Some(
            div()
                .id("quick-open-layer")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .key_context(QUICK_OPEN_CONTEXT)
                .on_action(cx.listener(Self::close_quick_open))
                .on_action(cx.listener(|this, _: &QuickOpenUp, _, cx| this.quick_open_move(-1, cx)))
                .on_action(cx.listener(|this, _: &QuickOpenDown, _, cx| this.quick_open_move(1, cx)))
                // The input swallows plain up/down, so steer the list from the
                // raw key event before bindings are consulted.
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                    let k = &ev.keystroke;
                    let delta = match (k.key.as_str(), k.modifiers.control) {
                        ("up", _) | ("p", true) => -1,
                        ("down", _) | ("n", true) => 1,
                        _ => return,
                    };
                    this.quick_open_move(delta, cx);
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_quick_open(&CloseQuickOpen, window, cx)),
                )
                .child(
                    v_flex()
                        .id("quick-open")
                        .w(px(560.))
                        .max_w_full()
                        .mx_auto()
                        .mt(px(72.))
                        .p_2()
                        .gap_1()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .bg(popover)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(Input::new(&q.input).small())
                        .children(rows)
                        .when(empty, |d| {
                            d.child(div().px_2().py_1().text_sm().text_color(muted).child("No matches."))
                        })
                        .child(
                            div()
                                .px_2()
                                .pt_1()
                                .text_xs()
                                .text_color(muted)
                                .child("Type a title to jump to it, or > for commands."),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Titles, aliases, or structure changed: refresh names everywhere and
    /// rebuild the index so backlinks and search see the new titles.
    fn on_tree_changed(&mut self, cx: &mut Context<Self>) {
        self.project.refresh_matcher();
        self.project.rebuild_index();
        let targets = self.project.link_targets();
        for panel in self.editors.values() {
            panel.update(cx, |p, cx| p.set_link_targets(targets.clone(), cx));
        }
        self.reference.update(cx, |r, cx| r.set_link_targets(targets, cx));
        self.sidebar.update(cx, |s, cx| {
            s.refresh_filter();
            cx.notify();
        });
        self.dock.update(cx, |_, cx| cx.notify());
        self.on_edited(cx);
    }

    /// Mark dirty and (re)start the autosave timer.
    fn on_edited(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        let now = Instant::now();
        if let Some(last) = self.last_edit {
            let gap = now.duration_since(last);
            if gap <= SESSION_IDLE {
                self.session_seconds += gap.as_secs_f64();
            }
        }
        self.last_edit = Some(now);
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update(cx, |ws, cx| ws.save_now(cx)).ok();
        }));
        cx.notify();
    }

    fn save_now(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        self.record_session();
        match self.project.project.save() {
            Ok(()) => {
                self.dirty = false;
                self.last_saved = Some(chrono_time());
                for id in std::mem::take(&mut self.dirty_nodes) {
                    self.project.update_index_node(id);
                }
                self.sidebar.update(cx, |s, cx| {
                    s.refresh_filter();
                    cx.notify();
                });
                self.reference.update(cx, |_, cx| cx.notify());
                for panel in self.editors.values() {
                    panel.update(cx, |_, cx| cx.notify());
                }
                tracing::info!("saved");
                if self.last_backup.map(|t| t.elapsed() >= BACKUP_INTERVAL).unwrap_or(true) {
                    self.backup_now();
                }
            }
            Err(e) => tracing::error!("save failed: {e:#}"),
        }
        if let Some(home) = &self.home {
            home.update(cx, |_, cx| cx.notify());
        }
        self.save_layout(cx);
        cx.notify();
    }

    /// Fold the pending writing time and the current manuscript count into
    /// today's session (starting a new one after midnight).
    fn record_session(&mut self) {
        let p = &self.project.project;
        let day = today();
        let words = p.manuscript_word_count();
        if p.session(day).is_none() {
            // A new day: yesterday's end count is where today starts.
            let start = p.sessions().last().map(|s| s.words_end).unwrap_or(words);
            if let Err(e) = p.begin_session(day, start) {
                tracing::error!("begin session: {e:#}");
            }
        }
        let secs = self.session_seconds.round() as i64;
        self.session_seconds = 0.;
        if let Err(e) = p.update_session(day, words, secs) {
            tracing::error!("update session: {e:#}");
        }
    }

    /// Copy the saved snapshot into `snapshots/` (pruned there).
    fn backup_now(&mut self) {
        let Some(dir) = self.project.dir().cloned() else {
            return;
        };
        match storage::backup(&dir) {
            Ok(Some(p)) => tracing::info!("backup written: {}", p.display()),
            Ok(None) => {}
            Err(e) => tracing::error!("backup failed: {e:#}"),
        }
        self.last_backup = Some(Instant::now());
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.save_now(cx);
        self.backup_now();
    }

    fn new_item(&mut self, _: &NewItem, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |s, cx| s.new_leaf(window, cx));
    }

    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.save_now(cx);
        } else {
            self.save_layout(cx);
        }
        cx.quit();
    }

    /// One rail entry: an icon button with an accent bar on its left edge
    /// when it is the current space, so the active one reads at a glance.
    fn rail_item(active: bool, button: Button, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .w_full()
            .items_center()
            .child(div().w(px(3.)).h(px(22.)).rounded_r_sm().bg(if active {
                theme.primary
            } else {
                gpui::transparent_black()
            }))
            .child(div().flex_1().flex().justify_center().child(button))
            .child(div().w(px(3.)))
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let spaces = [
            (
                "rail-manuscript",
                "Manuscript (Ctrl-1)",
                IconName::BookOpen,
                Space::Manuscript,
            ),
            ("rail-world", "World (Ctrl-2)", IconName::Globe, Space::World),
            ("rail-notes", "Notes (Ctrl-3)", IconName::FileText, Space::Notes),
        ];
        let home_active = self.home.is_some() && self.active.is_none();
        let dark = cx.theme().mode.is_dark();
        v_flex()
            .w(px(56.))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .py_2()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(Self::rail_item(
                home_active,
                Button::new("rail-home")
                    .ghost()
                    .icon(IconName::LayoutDashboard)
                    .tooltip("Home: dashboard, reports, tasks, export, sync (Ctrl-Shift-H)")
                    .toggled(home_active)
                    .on_click(cx.listener(|this, _, window, cx| this.show_home(window, cx))),
                cx,
            ))
            .child(div().h(px(8.)))
            .children(spaces.into_iter().map(|(id, label, icon, space)| {
                let active = self.space == space && !home_active;
                Self::rail_item(
                    active,
                    Button::new(id)
                        .ghost()
                        .icon(icon)
                        .tooltip(label)
                        .toggled(active)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_space(space, cx))),
                    cx,
                )
            }))
            .child(div().flex_1())
            .child(Self::rail_item(
                false,
                Button::new("rail-theme")
                    .ghost()
                    .icon(if dark { IconName::Sun } else { IconName::Moon })
                    .tooltip(if dark {
                        "Switch to light theme (Ctrl-Shift-T)"
                    } else {
                        "Switch to dark theme (Ctrl-Shift-T)"
                    })
                    .on_click(|_, window, cx| app::toggle_theme(window, cx)),
                cx,
            ))
    }

    fn status_right(&self, cx: &App) -> String {
        let mut parts = Vec::new();
        if let Some(panel) = self.active.and_then(|id| self.editors.get(&id)) {
            let n = panel.read(cx).word_count(cx);
            parts.push(format!("{n} words"));
        }
        let manuscript: usize = self
            .project
            .project
            .manuscript_scenes()
            .into_iter()
            .filter_map(|id| self.project.project.node(id).ok())
            .filter(|n| n.include_in_compile())
            .map(|n| n.word_count())
            .sum();
        parts.push(format!("manuscript {manuscript}"));
        let day = today();
        let p = &self.project.project;
        // Live today count: the saved session plus whatever is unsaved.
        let start = p
            .session(day)
            .map(|s| s.words_start)
            .unwrap_or(p.manuscript_word_count());
        let today_words = manuscript as i64 - start;
        parts.push(format!("today {today_words:+}"));
        let streak = p.streak(day);
        if streak > 0 || today_words > 0 {
            let streak = streak.max(if today_words > 0 { 1 } else { 0 });
            parts.push(format!("streak {streak}"));
        }
        if self.typewriter {
            parts.push("typewriter".to_string());
        }
        parts.join("  ·  ")
    }
}

fn chrono_time() -> String {
    wordy_doc::chrono::Local::now().format("%H:%M").to_string()
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.project.project.name();
        let dir = self.project.dir().map(|d| d.display().to_string()).unwrap_or_default();
        let spell_on = self.project.project.spellcheck();
        let save_state = if self.dirty {
            "unsaved".to_string()
        } else {
            match &self.last_saved {
                Some(t) => format!("saved {t}"),
                None => String::new(),
            }
        };
        let right = self.status_right(cx);
        let recovered = self
            .project
            .project
            .recovered_from()
            .and_then(|p| p.file_name())
            .map(|n| {
                format!(
                    "recovered from backup {}; the damaged file was kept next to it",
                    n.to_string_lossy()
                )
            });
        let focus_mode = self.focus_mode;

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::new_item))
            .on_action(cx.listener(Self::toggle_reference))
            .on_action(cx.listener(Self::search_project))
            .on_action(cx.listener(Self::on_show_home))
            .on_action(cx.listener(Self::toggle_focus_mode))
            .on_action(cx.listener(Self::toggle_typewriter))
            .on_action(cx.listener(Self::toggle_spellcheck))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_document))
            .on_action(cx.listener(Self::prev_document))
            .on_action(cx.listener(Self::focus_sidebar))
            .on_action(
                cx.listener(|this, _: &SpaceManuscript, window, cx| this.space_shortcut(Space::Manuscript, window, cx)),
            )
            .on_action(cx.listener(|this, _: &SpaceWorld, window, cx| this.space_shortcut(Space::World, window, cx)))
            .on_action(cx.listener(|this, _: &SpaceNotes, window, cx| this.space_shortcut(Space::Notes, window, cx)))
            .on_action(|_: &ToggleTheme, window, cx| app::toggle_theme(window, cx))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .px_2()
                        .child(div().text_sm().child(format!("Wordy — {name}"))),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(!focus_mode, |d| d.child(self.render_rail(cx)))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.dock.clone())
                            .children(self.render_quick_open(window, cx)),
                    ),
            )
            .when(!focus_mode, |d| {
                d.child(
                    // One flex row rather than StatusBar's left/right regions so
                    // the project path is the only thing that ever gets truncated.
                    StatusBar::new().child(
                        h_flex()
                            .w_full()
                            .gap_3()
                            .text_xs()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(dir),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(save_state),
                            )
                            .children(recovered.map(|r| div().flex_shrink_0().text_color(cx.theme().danger).child(r)))
                            .child(
                                div()
                                    .id("status-spell")
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .text_color(cx.theme().muted_foreground)
                                    .hover(|s| s.text_color(cx.theme().foreground))
                                    .child(if spell_on { "spelling en-US" } else { "spelling off" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_spellcheck(&ToggleSpellcheck, window, cx)
                                    })),
                            )
                            .child(div().flex_shrink_0().whitespace_nowrap().child(right)),
                    ),
                )
            })
    }
}
