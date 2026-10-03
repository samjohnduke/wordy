//! The space sidebar: tree of nodes for the current space, with creation,
//! rename, drag-and-drop reorder, duplicate, status, compile toggle, and trash.

use std::collections::HashSet;

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::Panel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::{NodeKind, Space, Status, TreeID};

use crate::app::{
    SharedProject, SidebarActivate, SidebarBack, SidebarDown, SidebarLeft, SidebarRename, SidebarRight, SidebarTrash,
    SidebarUp, SIDEBAR_CONTEXT,
};

pub enum SidebarEvent {
    /// Open a node with a body in an editor tab.
    Open(TreeID),
    /// Titles, order, or metadata changed.
    Changed,
    /// A node was trashed or deleted; close its tab if open.
    Removed(TreeID),
    /// Escape: hand focus back to the active editor.
    FocusEditor,
    /// Collapse state changed (persisted in layout.json).
    LayoutChanged,
}

/// The dragged row's floating preview.
#[derive(Clone)]
struct DragNode {
    id: TreeID,
    title: SharedString,
}

impl Render for DragNode {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .text_sm()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_md()
            .child(self.title.clone())
    }
}

/// Where a dragged row would land relative to the row under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropZone {
    Before,
    Into,
    After,
}

struct Rename {
    id: TreeID,
    input: Entity<InputState>,
    _sub: Subscription,
}

pub struct SidebarPanel {
    project: SharedProject,
    space: Space,
    selected: Option<TreeID>,
    collapsed: HashSet<TreeID>,
    rename: Option<Rename>,
    /// Row under a dragged node and where it would drop.
    drop_hint: Option<(TreeID, DropZone)>,
    show_trash: bool,
    search: Entity<InputState>,
    query: String,
    weak: WeakEntity<Self>,
    pub focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl SidebarPanel {
    pub fn new(project: SharedProject, space: Space, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search project… (#tag)"));
        let sub = cx.subscribe_in(&search, window, |this, input, ev: &InputEvent, _window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query = input.read(cx).value().trim().to_string();
                cx.notify();
            }
        });
        Self {
            project,
            space,
            selected: None,
            collapsed: HashSet::new(),
            rename: None,
            drop_hint: None,
            show_trash: false,
            search,
            query: String::new(),
            weak: cx.weak_entity(),
            focus: cx.focus_handle(),
            _subs: vec![sub],
        }
    }

    /// Focus the project search box.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
    }

    /// `#tag` queries list every node carrying that tag.
    fn render_tag_results(&self, tag: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let ids = self.project.nodes_with_tag(tag);
        if ids.is_empty() {
            return vec![div()
                .p_2()
                .text_sm()
                .text_color(muted)
                .child(format!("Nothing tagged #{tag}."))
                .into_any_element()];
        }
        ids.into_iter()
            .filter_map(|id| self.project.project.node(id).ok())
            .enumerate()
            .map(|(ix, node)| {
                let id = node.id;
                let tags = node
                    .tags()
                    .iter()
                    .map(|t| format!("#{t}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                v_flex()
                    .id(ElementId::Name(format!("tag-hit-{ix}").into()))
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_0()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().secondary))
                    .child(div().text_sm().font_semibold().child(node.title()))
                    .child(div().text_xs().text_color(muted).child(tags))
                    .on_click(cx.listener(move |this, _, _, cx| this.activate(id, cx)))
                    .into_any_element()
            })
            .collect()
    }

    fn render_search_results(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if let Some(tag) = self.query.strip_prefix('#') {
            return self.render_tag_results(&tag.trim().to_lowercase(), cx);
        }
        let hits = self.project.search(&self.query, 50);
        let muted = cx.theme().muted_foreground;
        if hits.is_empty() {
            return vec![div()
                .p_2()
                .text_sm()
                .text_color(muted)
                .child("No matches.")
                .into_any_element()];
        }
        hits.into_iter()
            .enumerate()
            .map(|(ix, hit)| {
                let id = hit.node;
                let snippet: String = hit.snippet.chars().filter(|c| *c != '\u{1}' && *c != '\u{2}').collect();
                v_flex()
                    .id(ElementId::Name(format!("hit-{ix}").into()))
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_0()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().secondary))
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(div().text_sm().font_semibold().child(hit.title))
                            .child(div().text_xs().text_color(muted).child(hit.space)),
                    )
                    .child(div().text_xs().text_color(muted).child(snippet))
                    .on_click(cx.listener(move |this, _, _, cx| this.activate(id, cx)))
                    .into_any_element()
            })
            .collect()
    }

    pub fn set_space(&mut self, space: Space, cx: &mut Context<Self>) {
        self.space = space;
        self.rename = None;
        cx.notify();
    }

    pub fn select(&mut self, id: Option<TreeID>, cx: &mut Context<Self>) {
        self.selected = id;
        cx.notify();
    }

    pub fn collapsed_ids(&self) -> Vec<TreeID> {
        self.collapsed.iter().copied().collect()
    }

    pub fn set_collapsed(&mut self, ids: Vec<TreeID>, cx: &mut Context<Self>) {
        self.collapsed = ids.into_iter().collect();
        cx.notify();
    }

    fn set_folded(&mut self, id: TreeID, folded: bool, cx: &mut Context<Self>) {
        let changed = if folded {
            self.collapsed.insert(id)
        } else {
            self.collapsed.remove(&id)
        };
        if changed {
            cx.emit(SidebarEvent::LayoutChanged);
        }
        cx.notify();
    }

    fn toggle_folded(&mut self, id: TreeID, cx: &mut Context<Self>) {
        let folded = !self.collapsed.contains(&id);
        self.set_folded(id, folded, cx);
    }

    // ----- keyboard navigation --------------------------------------------

    /// Focus the tree for arrow-key navigation, selecting the first row if
    /// nothing is selected.
    pub fn focus_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            self.selected = self.visible_rows().first().copied();
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Rows currently shown, top to bottom (collapsed subtrees skipped).
    fn visible_rows(&self) -> Vec<TreeID> {
        let p = &self.project.project;
        let mut out = Vec::new();
        self.collect_visible(p.root(self.space), &mut out);
        if self.show_trash {
            self.collect_visible(p.trash(self.space), &mut out);
        }
        out
    }

    fn collect_visible(&self, parent: TreeID, out: &mut Vec<TreeID>) {
        for child in self.project.project.children(parent) {
            out.push(child);
            if !self.collapsed.contains(&child) {
                self.collect_visible(child, out);
            }
        }
    }

    /// Keys go to the search box or the rename field while one has focus.
    fn keyboard_blocked(&self, window: &Window, cx: &App) -> bool {
        self.rename.is_some() || self.search.read(cx).focus_handle(cx).is_focused(window)
    }

    fn step_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard_blocked(window, cx) {
            cx.propagate();
            return;
        }
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let ix = self.selected.and_then(|s| rows.iter().position(|r| *r == s));
        let next = match ix {
            Some(i) => (i as isize + delta).clamp(0, rows.len() as isize - 1) as usize,
            None if delta > 0 => 0,
            None => rows.len() - 1,
        };
        self.selected = Some(rows[next]);
        cx.notify();
    }

    fn on_up(&mut self, _: &SidebarUp, window: &mut Window, cx: &mut Context<Self>) {
        self.step_selection(-1, window, cx);
    }

    fn on_down(&mut self, _: &SidebarDown, window: &mut Window, cx: &mut Context<Self>) {
        self.step_selection(1, window, cx);
    }

    /// Left: collapse an open container, else jump to the parent.
    fn on_left(&mut self, _: &SidebarLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard_blocked(window, cx) {
            cx.propagate();
            return;
        }
        let Some(id) = self.selected else { return };
        let p = &self.project.project;
        let Ok(node) = p.node(id) else { return };
        if node.kind().is_container() && !self.collapsed.contains(&id) && !p.children(id).is_empty() {
            self.set_folded(id, true, cx);
        } else if let Some(parent) = node.parent() {
            let parent_is_root = p
                .node(parent)
                .map(|n| matches!(n.kind(), NodeKind::Root | NodeKind::Trash))
                .unwrap_or(true);
            if !parent_is_root {
                self.selected = Some(parent);
            }
        }
        cx.notify();
    }

    /// Right: expand a collapsed container, else step into its first child.
    fn on_right(&mut self, _: &SidebarRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard_blocked(window, cx) {
            cx.propagate();
            return;
        }
        let Some(id) = self.selected else { return };
        let p = &self.project.project;
        let Ok(node) = p.node(id) else { return };
        if !node.kind().is_container() {
            return;
        }
        if self.collapsed.contains(&id) {
            self.set_folded(id, false, cx);
            return;
        }
        if let Some(first) = p.children(id).first().copied() {
            self.selected = Some(first);
        }
        cx.notify();
    }

    /// Enter: open a document, or fold/unfold a container.
    fn on_activate(&mut self, _: &SidebarActivate, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard_blocked(window, cx) {
            cx.propagate();
            return;
        }
        let Some(id) = self.selected else { return };
        let Ok(node) = self.project.project.node(id) else {
            return;
        };
        if node.kind().is_container() {
            self.toggle_folded(id, cx);
        } else {
            self.activate(id, cx);
        }
    }

    fn on_back(&mut self, _: &SidebarBack, _: &mut Window, cx: &mut Context<Self>) {
        if self.rename.is_some() {
            cx.propagate();
            return;
        }
        cx.emit(SidebarEvent::FocusEditor);
    }

    fn on_rename(&mut self, _: &SidebarRename, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard_blocked(window, cx) {
            cx.propagate();
            return;
        }
        if let Some(id) = self.selected {
            self.begin_rename(id, window, cx);
        }
    }

    /// Delete: move the selection to the trash (never deletes for good).
    fn on_trash(&mut self, _: &SidebarTrash, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard_blocked(window, cx) {
            cx.propagate();
            return;
        }
        let Some(id) = self.selected else { return };
        if !self.project.project.is_live(id) {
            return;
        }
        let rows = self.visible_rows();
        let ix = rows.iter().position(|r| *r == id);
        self.trash(id, cx);
        // Keep the keyboard somewhere useful: the row that moved up into place.
        let rows = self.visible_rows();
        if let Some(ix) = ix {
            self.selected = rows.get(ix).or_else(|| rows.last()).copied();
        }
        cx.notify();
    }

    /// The kinds a user may create in this space: (leaf, container).
    fn kinds(&self) -> (NodeKind, NodeKind) {
        match self.space {
            Space::Manuscript => (NodeKind::Scene, NodeKind::Chapter),
            Space::World => (NodeKind::Entity, NodeKind::Folder),
            Space::Notes => (NodeKind::Note, NodeKind::Folder),
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.project.project.commit_meta();
        cx.emit(SidebarEvent::Changed);
        cx.notify();
    }

    /// Create the space's leaf kind next to the selection (or at the root).
    pub fn new_leaf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (leaf, _) = self.kinds();
        let anchor = self.selected.unwrap_or_else(|| self.project.project.root(self.space));
        self.create(anchor, leaf, window, cx);
    }

    /// Create `kind` inside `anchor` if it is a container, else after it.
    fn create(&mut self, anchor: TreeID, kind: NodeKind, window: &mut Window, cx: &mut Context<Self>) {
        let p = &self.project.project;
        let is_container = p.node(anchor).map(|n| n.kind().is_container()).unwrap_or(true);
        let result = if is_container {
            if self.collapsed.remove(&anchor) {
                cx.emit(SidebarEvent::LayoutChanged);
            }
            p.create_node(anchor, kind, kind.default_title())
        } else {
            p.create_node_after(anchor, kind, kind.default_title())
        };
        match result {
            Ok(id) => {
                // Name it first; committing the name opens it (see `commit_rename`).
                self.selected = Some(id);
                self.changed(cx);
                self.begin_rename(id, window, cx);
            }
            Err(e) => tracing::error!("create node: {e:#}"),
        }
    }

    fn begin_rename(&mut self, id: TreeID, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(node) = self.project.project.node(id) else {
            return;
        };
        let title = node.title();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(window, cx),
            _ => {}
        });
        self.rename = Some(Rename { id, input, _sub: sub });
        cx.notify();
    }

    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        let value = rename.input.read(cx).value().trim().to_string();
        if !value.is_empty() {
            if let Ok(node) = self.project.project.node(rename.id) {
                if node.title() != value {
                    if let Err(e) = node.set_title(&value) {
                        tracing::error!("rename: {e:#}");
                    }
                }
            }
        }
        self.changed(cx);
        // Back to writing: a named scene is the one you want to type into.
        if self
            .project
            .project
            .node(rename.id)
            .map(|n| n.kind().has_body())
            .unwrap_or(false)
        {
            cx.emit(SidebarEvent::Open(rename.id));
        } else {
            window.focus(&self.focus, cx);
        }
    }

    fn move_by(&mut self, id: TreeID, delta: i32, cx: &mut Context<Self>) {
        let p = &self.project.project;
        let Ok(node) = p.node(id) else { return };
        let Some(parent) = node.parent() else { return };
        let siblings = p.children(parent);
        let Some(ix) = siblings.iter().position(|s| *s == id) else {
            return;
        };
        let result = if delta < 0 {
            if ix == 0 {
                return;
            }
            p.move_before(id, siblings[ix - 1])
        } else {
            if ix + 1 >= siblings.len() {
                return;
            }
            p.move_after(id, siblings[ix + 1])
        };
        if let Err(e) = result {
            tracing::error!("move: {e:#}");
        }
        self.changed(cx);
    }

    fn duplicate(&mut self, id: TreeID, cx: &mut Context<Self>) {
        match self.project.project.duplicate_node(id) {
            Ok(new_id) => self.selected = Some(new_id),
            Err(e) => tracing::error!("duplicate: {e:#}"),
        }
        self.changed(cx);
    }

    /// Finish a drag: put `dragged` before/after/inside `target`.
    fn drop_node(&mut self, dragged: TreeID, target: TreeID, zone: DropZone, cx: &mut Context<Self>) {
        self.drop_hint = None;
        let p = &self.project.project;
        if dragged == target || p.is_descendant(target, dragged) || !p.is_live(dragged) {
            cx.notify();
            return;
        }
        let result = match zone {
            DropZone::Before => p.move_before(dragged, target),
            DropZone::After => p.move_after(dragged, target),
            DropZone::Into => p.move_into(dragged, target),
        };
        if let Err(e) = result {
            tracing::error!("drop: {e:#}");
        }
        if zone == DropZone::Into && self.collapsed.remove(&target) {
            cx.emit(SidebarEvent::LayoutChanged);
        }
        self.selected = Some(dragged);
        self.changed(cx);
    }

    /// Drop on the empty space under the tree: append to the space root.
    fn drop_at_end(&mut self, dragged: TreeID, cx: &mut Context<Self>) {
        self.drop_hint = None;
        let root = self.project.project.root(self.space);
        if let Err(e) = self.project.project.move_into(dragged, root) {
            tracing::error!("drop: {e:#}");
        }
        self.selected = Some(dragged);
        self.changed(cx);
    }

    /// Track where a drag would land over row `id`.
    fn drag_over_row(&mut self, id: TreeID, is_container: bool, ev: &DragMoveEvent<DragNode>, cx: &mut Context<Self>) {
        let b = ev.bounds;
        let pos = ev.event.position;
        if !b.contains(&pos) {
            if self.drop_hint.map(|h| h.0) == Some(id) {
                self.drop_hint = None;
                cx.notify();
            }
            return;
        }
        let dragged = ev.drag(cx).id;
        if dragged == id || self.project.project.is_descendant(id, dragged) {
            if self.drop_hint.is_some() {
                self.drop_hint = None;
                cx.notify();
            }
            return;
        }
        let frac = f32::from((pos.y - b.origin.y) / b.size.height);
        let zone = if is_container {
            if frac < 0.25 {
                DropZone::Before
            } else if frac > 0.75 {
                DropZone::After
            } else {
                DropZone::Into
            }
        } else if frac < 0.5 {
            DropZone::Before
        } else {
            DropZone::After
        };
        if self.drop_hint != Some((id, zone)) {
            self.drop_hint = Some((id, zone));
            cx.notify();
        }
    }

    fn trash(&mut self, id: TreeID, cx: &mut Context<Self>) {
        if let Err(e) = self.project.project.trash_node(id) {
            tracing::error!("trash: {e:#}");
        }
        if self.selected == Some(id) {
            self.selected = None;
        }
        cx.emit(SidebarEvent::Removed(id));
        self.changed(cx);
    }

    fn restore(&mut self, id: TreeID, cx: &mut Context<Self>) {
        if let Err(e) = self.project.project.restore_node(id) {
            tracing::error!("restore: {e:#}");
        }
        self.changed(cx);
    }

    fn delete_forever(&mut self, id: TreeID, cx: &mut Context<Self>) {
        if let Err(e) = self.project.project.delete_node(id) {
            tracing::error!("delete: {e:#}");
        }
        cx.emit(SidebarEvent::Removed(id));
        self.changed(cx);
    }

    fn set_status(&mut self, id: TreeID, status: Status, cx: &mut Context<Self>) {
        if let Ok(node) = self.project.project.node(id) {
            if let Err(e) = node.set_status(status) {
                tracing::error!("status: {e:#}");
            }
        }
        self.changed(cx);
    }

    fn toggle_compile(&mut self, id: TreeID, cx: &mut Context<Self>) {
        if let Ok(node) = self.project.project.node(id) {
            if let Err(e) = node.set_include_in_compile(!node.include_in_compile()) {
                tracing::error!("compile toggle: {e:#}");
            }
        }
        self.changed(cx);
    }

    fn activate(&mut self, id: TreeID, cx: &mut Context<Self>) {
        self.selected = Some(id);
        if let Ok(node) = self.project.project.node(id) {
            if node.kind().has_body() {
                cx.emit(SidebarEvent::Open(id));
            }
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn node_menu(
        project: &SharedProject,
        this: WeakEntity<Self>,
        (leaf, container): (NodeKind, NodeKind),
        id: TreeID,
        in_trash: bool,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let Ok(node) = project.project.node(id) else {
            return menu;
        };
        let kind = node.kind();

        if in_trash {
            return menu
                .item(PopupMenuItem::new("Restore").on_click({
                    let this = this.clone();
                    move |_, _, cx| this.update(cx, |s, cx| s.restore(id, cx)).ok().unwrap_or(())
                }))
                .separator()
                .item(PopupMenuItem::new("Delete permanently").on_click({
                    let this = this.clone();
                    move |_, _, cx| this.update(cx, |s, cx| s.delete_forever(id, cx)).ok().unwrap_or(())
                }));
        }

        let mut menu = menu
            .item(
                PopupMenuItem::new(format!("New {}", leaf.default_title().trim_start_matches("New "))).on_click({
                    let this = this.clone();
                    move |_, window, cx| {
                        this.update(cx, |s, cx| s.create(id, leaf, window, cx))
                            .ok()
                            .unwrap_or(())
                    }
                }),
            )
            .item(
                PopupMenuItem::new(format!("New {}", container.default_title().trim_start_matches("New "))).on_click({
                    let this = this.clone();
                    move |_, window, cx| {
                        this.update(cx, |s, cx| s.create(id, container, window, cx))
                            .ok()
                            .unwrap_or(())
                    }
                }),
            )
            .separator()
            .item(PopupMenuItem::new("Rename").on_click({
                let this = this.clone();
                move |_, window, cx| {
                    this.update(cx, |s, cx| s.begin_rename(id, window, cx))
                        .ok()
                        .unwrap_or(())
                }
            }))
            .item(PopupMenuItem::new("Duplicate").on_click({
                let this = this.clone();
                move |_, _, cx| this.update(cx, |s, cx| s.duplicate(id, cx)).ok().unwrap_or(())
            }))
            .item(PopupMenuItem::new("Move Up").on_click({
                let this = this.clone();
                move |_, _, cx| this.update(cx, |s, cx| s.move_by(id, -1, cx)).ok().unwrap_or(())
            }))
            .item(PopupMenuItem::new("Move Down").on_click({
                let this = this.clone();
                move |_, _, cx| this.update(cx, |s, cx| s.move_by(id, 1, cx)).ok().unwrap_or(())
            }));

        if kind == NodeKind::Scene {
            let current = node.status();
            let include = node.include_in_compile();
            menu = menu.separator().submenu("Status", window, cx, {
                let this = this.clone();
                move |menu, _, _| {
                    let mut menu = menu;
                    for st in Status::ALL {
                        let this = this.clone();
                        menu = menu.item(PopupMenuItem::new(st.label()).checked(st == current).on_click(
                            move |_, _, cx| this.update(cx, |s, cx| s.set_status(id, st, cx)).ok().unwrap_or(()),
                        ));
                    }
                    menu
                }
            });
            menu = menu.item(PopupMenuItem::new("Include in compile").checked(include).on_click({
                let this = this.clone();
                move |_, _, cx| this.update(cx, |s, cx| s.toggle_compile(id, cx)).ok().unwrap_or(())
            }));
        }

        menu.separator().item(PopupMenuItem::new("Move to Trash").on_click({
            let this = this.clone();
            move |_, _, cx| this.update(cx, |s, cx| s.trash(id, cx)).ok().unwrap_or(())
        }))
    }

    fn render_node(&self, id: TreeID, depth: usize, in_trash: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let Ok(node) = self.project.project.node(id) else {
            return out;
        };
        let kind = node.kind();
        let children = self.project.project.children(id);
        let is_container = kind.is_container();
        let collapsed = self.collapsed.contains(&id);
        let selected = self.selected == Some(id);
        let theme = cx.theme();
        let dim = kind == NodeKind::Scene && !node.include_in_compile();
        let renaming = self.rename.as_ref().filter(|r| r.id == id).map(|r| r.input.clone());
        let renaming_none = renaming.is_none();
        let menu_project = self.project.clone();
        let menu_this = self.weak.clone();
        let menu_kinds = self.kinds();

        let chevron = if is_container {
            div()
                .id(ElementId::Name(format!("chev-{id}").into()))
                .w(px(14.))
                .flex_shrink_0()
                .text_xs()
                .text_color(theme.muted_foreground)
                .cursor_pointer()
                .child(if collapsed { "▸" } else { "▾" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_folded(id, cx);
                    cx.stop_propagation();
                }))
                .into_any_element()
        } else {
            div().w(px(14.)).flex_shrink_0().into_any_element()
        };

        let label: AnyElement = match renaming {
            Some(input) => Input::new(&input).xsmall().into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .when(is_container, |d| d.font_semibold())
                .when(dim, |d| d.text_color(theme.muted_foreground).italic())
                .child(node.title())
                .into_any_element(),
        };

        // Containers show how much they hold, so a folded chapter is not a mystery.
        let count: Option<AnyElement> = (is_container && !children.is_empty()).then(|| {
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(children.len().to_string())
                .into_any_element()
        });

        let status: Option<AnyElement> = (kind == NodeKind::Scene).then(|| {
            let color = match node.status() {
                Status::Idea => theme.muted_foreground,
                Status::Draft => theme.foreground.opacity(0.5),
                Status::Revised => theme.accent,
                Status::Final => theme.primary,
            };
            div()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(color)
                .flex_shrink_0()
                .into_any_element()
        });

        let hint = self.drop_hint.filter(|h| h.0 == id).map(|h| h.1);
        let title: SharedString = node.title().into();
        let marker = |top: bool| {
            div()
                .absolute()
                .left(px(6. + 14. * depth as f32))
                .right_0()
                .h(px(2.))
                .bg(theme.primary)
                .when(top, |d| d.top_0())
                .when(!top, |d| d.bottom_0())
        };

        let row = h_flex()
            .id(ElementId::Name(format!("node-{id}").into()))
            .relative()
            .w_full()
            .items_center()
            .gap_1()
            .pl(px(6. + 14. * depth as f32))
            .pr_2()
            .py_0p5()
            .rounded_sm()
            .text_sm()
            .text_color(theme.foreground)
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.accent))
            .when(!selected, |d| d.hover(|s| s.bg(theme.secondary)))
            .when(hint == Some(DropZone::Into), |d| d.bg(theme.primary.opacity(0.18)))
            .child(chevron)
            .child(label)
            .children(count)
            .children(status)
            .children(hint.filter(|h| *h == DropZone::Before).map(|_| marker(true)))
            .children(hint.filter(|h| *h == DropZone::After).map(|_| marker(false)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.rename.as_ref().map(|r| r.id) == Some(id) {
                    return;
                }
                this.activate(id, cx);
            }))
            .when(!in_trash && renaming_none, |d| {
                d.on_drag(
                    DragNode {
                        id,
                        title: title.clone(),
                    },
                    |drag, _, _, cx| cx.new(|_| drag.clone()),
                )
                .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<DragNode>, _, cx| {
                    this.drag_over_row(id, is_container, ev, cx)
                }))
                .on_drop(cx.listener(move |this, drag: &DragNode, _, cx| {
                    let zone = this
                        .drop_hint
                        .filter(|h| h.0 == id)
                        .map(|h| h.1)
                        .unwrap_or(DropZone::After);
                    this.drop_node(drag.id, id, zone, cx);
                }))
            })
            .context_menu(move |menu, window, cx| {
                Self::node_menu(
                    &menu_project,
                    menu_this.clone(),
                    menu_kinds,
                    id,
                    in_trash,
                    menu,
                    window,
                    cx,
                )
            });
        out.push(row.into_any_element());

        if is_container && !collapsed {
            for child in children {
                out.extend(self.render_node(child, depth + 1, in_trash, cx));
            }
        }
        out
    }
}

super::impl_panel_boilerplate!(SidebarPanel, "Sidebar", closable = false);

impl Panel for SidebarPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.space.label().into())
    }
}

impl EventEmitter<SidebarEvent> for SidebarPanel {}

impl Render for SidebarPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.drop_hint.is_some() && !cx.has_active_drag() {
            self.drop_hint = None;
        }
        let root = self.project.project.root(self.space);
        let trash = self.project.project.trash(self.space);
        let (leaf, container) = self.kinds();
        let items: Vec<AnyElement> = self
            .project
            .project
            .children(root)
            .into_iter()
            .flat_map(|c| self.render_node(c, 0, false, cx))
            .collect();
        let trashed = self.project.project.children(trash);
        let muted = cx.theme().muted_foreground;

        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(self.space.label().to_uppercase()),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .child(
                        Button::new("new-leaf")
                            .ghost()
                            .xsmall()
                            .label("+")
                            .tooltip(format!("New {}", leaf.default_title().trim_start_matches("New ")))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let anchor = this.selected.unwrap_or(root);
                                this.create(anchor, leaf, window, cx)
                            })),
                    )
                    .child(
                        Button::new("new-container")
                            .ghost()
                            .xsmall()
                            .label("▣")
                            .tooltip(format!("New {}", container.default_title().trim_start_matches("New ")))
                            .on_click(cx.listener(move |this, _, window, cx| this.create(root, container, window, cx))),
                    ),
            );

        let searching = !self.query.is_empty();
        let trash_section = (!searching && !trashed.is_empty()).then(|| {
            let show = self.show_trash;
            let mut section = v_flex().w_full().mt_2().child(
                div()
                    .id("trash-toggle")
                    .px_2()
                    .py_0p5()
                    .text_xs()
                    .text_color(muted)
                    .cursor_pointer()
                    .child(format!("{} TRASH ({})", if show { "▾" } else { "▸" }, trashed.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_trash = !this.show_trash;
                        cx.notify();
                    })),
            );
            if show {
                let items: Vec<AnyElement> = trashed
                    .into_iter()
                    .flat_map(|c| self.render_node(c, 0, true, cx))
                    .collect();
                section = section.children(items);
            }
            section
        });

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .key_context(SIDEBAR_CONTEXT)
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .on_action(cx.listener(Self::on_activate))
            .on_action(cx.listener(Self::on_back))
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_trash))
            .p_1()
            .child(header)
            .child(div().px_1().pb_1().child(Input::new(&self.search).small()))
            .child(
                div()
                    .id("tree-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(searching, |d| d.children(self.render_search_results(cx)))
                    .when(!searching && items.is_empty(), |d| {
                        d.child(
                            div()
                                .p_2()
                                .text_sm()
                                .text_color(muted)
                                .child("Nothing here yet. Use + to add one."),
                        )
                    })
                    .when(!searching, |d| d.children(items))
                    .children(trash_section)
                    .when(!searching, |d| {
                        d.child(
                            // Dropping below the last row appends to the space root.
                            div()
                                .id("tree-tail")
                                .w_full()
                                .flex_1()
                                .min_h(px(32.))
                                .drag_over::<DragNode>(|style, _, _, cx| {
                                    style.border_t_2().border_color(cx.theme().primary)
                                })
                                .on_drop(cx.listener(|this, drag: &DragNode, _, cx| this.drop_at_end(drag.id, cx))),
                        )
                    }),
            )
    }
}
