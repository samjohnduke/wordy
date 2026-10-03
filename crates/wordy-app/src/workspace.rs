//! The main window: title bar, space rail, dock area, status bar.
//! Owns the open editor tabs and the autosave timer.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::base::dock::PanelId;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{DockArea, DockLayout, DockPlacement, DockSkin, PanelStyle, panel_handle};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::{storage, Space, TreeID};

use crate::app::{self, NewItem, Quit, Save, SharedProject, ToggleTheme};
use crate::panels::editor::{EditorPanel, EditorPanelEvent};
use crate::panels::reference::ReferencePanel;
use crate::panels::sidebar::{SidebarEvent, SidebarPanel};

const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);

pub struct Workspace {
    project: SharedProject,
    space: Space,
    dock: Entity<DockArea>,
    sidebar: Entity<SidebarPanel>,
    placeholder: Option<Entity<EditorPanel>>,
    editors: HashMap<TreeID, Entity<EditorPanel>>,
    active: Option<TreeID>,
    dirty: bool,
    last_saved: Option<String>,
    save_task: Option<Task<()>>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(project: SharedProject, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dock, skin) = DockSkin::dock_area("wordy-main", Some(1), window, cx);
        skin.set_panel_style(PanelStyle::TabBar, cx);
        skin.set_close_button_visible(true, cx);

        let sidebar = cx.new(|cx| SidebarPanel::new(project.clone(), Space::Manuscript, cx));
        let placeholder = cx.new(|cx| EditorPanel::placeholder(project.clone(), cx));
        let reference = cx.new(|cx| ReferencePanel::new(cx));

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
                DockLayout::tabs().panel_view(panel_handle(reference), cx),
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
        });

        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        Self {
            project,
            space: Space::Manuscript,
            dock,
            sidebar,
            placeholder: Some(placeholder),
            editors: HashMap::new(),
            active: None,
            dirty: false,
            last_saved: None,
            save_task: None,
            focus,
            _subs: vec![sub],
        }
    }

    fn set_space(&mut self, space: Space, cx: &mut Context<Self>) {
        if self.space == space {
            return;
        }
        self.space = space;
        self.sidebar.update(cx, |s, cx| s.set_space(space, cx));
        cx.notify();
    }

    /// Show the editor tab for `id`, creating it on first open.
    fn open_node(&mut self, id: TreeID, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.editors.get(&id).cloned() {
            let pid = PanelId::from(panel.entity_id());
            self.dock.update(cx, |dock, cx| dock.select_panel(pid, window, cx));
            panel.update(cx, |p, cx| p.focus_editor(window, cx));
            self.active = Some(id);
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
        let panel = cx.new(|cx| EditorPanel::open(self.project.clone(), id, body, cx));
        // Keep each editor's undo stack to its own body: exclude every other
        // open editor's commit origin, in both directions.
        if let Some(new_editor) = panel.read(cx).editor().cloned() {
            let new_origin = new_editor.read(cx).origin().to_string();
            let others: Vec<Entity<wordy_editor::ProseEditor>> =
                self.editors.values().filter_map(|p| p.read(cx).editor().cloned()).collect();
            for other in others {
                let other_origin = other.read(cx).origin().to_string();
                new_editor.update(cx, |e, _| e.exclude_origin(&other_origin));
                other.update(cx, |e, _| e.exclude_origin(&new_origin));
            }
        }
        let sub = cx.subscribe_in(&panel, window, move |this, _, ev: &EditorPanelEvent, _window, cx| match ev {
            EditorPanelEvent::Edited => this.on_edited(cx),
            EditorPanelEvent::Activated => {
                this.active = Some(id);
                this.sidebar.update(cx, |s, cx| s.select(Some(id), cx));
                cx.notify();
            }
            EditorPanelEvent::Closed => {
                this.editors.remove(&id);
                if this.active == Some(id) {
                    this.active = None;
                }
                cx.notify();
            }
        });
        self._subs.push(sub);
        self.editors.insert(id, panel.clone());

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
        cx.notify();
    }

    fn close_node(&mut self, id: TreeID, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.editors.remove(&id) {
            self.dock.update(cx, |dock, cx| dock.remove_panel(panel, window, cx));
        }
        if self.active == Some(id) {
            self.active = None;
        }
        self.on_edited(cx);
    }

    fn on_tree_changed(&mut self, cx: &mut Context<Self>) {
        for panel in self.editors.values() {
            panel.update(cx, |_, cx| cx.notify());
        }
        self.dock.update(cx, |_, cx| cx.notify());
        self.on_edited(cx);
    }

    /// Mark dirty and (re)start the autosave timer.
    fn on_edited(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update(cx, |ws, cx| ws.save_now(cx)).ok();
        }));
        cx.notify();
    }

    fn save_now(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        match self.project.project.save() {
            Ok(()) => {
                self.dirty = false;
                self.last_saved = Some(chrono_time());
                tracing::info!("saved");
            }
            Err(e) => tracing::error!("save failed: {e:#}"),
        }
        cx.notify();
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.save_now(cx);
        if let Some(dir) = self.project.dir() {
            match storage::backup(dir) {
                Ok(Some(p)) => tracing::info!("backup written: {}", p.display()),
                Ok(None) => {}
                Err(e) => tracing::error!("backup failed: {e:#}"),
            }
        }
    }

    fn new_item(&mut self, _: &NewItem, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |s, cx| s.new_leaf(window, cx));
    }

    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.save_now(cx);
        }
        cx.quit();
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let spaces = [
            ("rail-manuscript", "Manuscript", Space::Manuscript),
            ("rail-world", "World", Space::World),
            ("rail-notes", "Notes", Space::Notes),
        ];
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
            .children(spaces.into_iter().map(|(id, label, space)| {
                let active = self.space == space;
                Button::new(id)
                    .ghost()
                    .small()
                    .w(px(48.))
                    .label(label.chars().next().unwrap().to_string())
                    .tooltip(label)
                    .toggled(active)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_space(space, cx)))
            }))
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.project.project.name();
        let dir = self
            .project
            .dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let save_state = if self.dirty {
            "unsaved".to_string()
        } else {
            match &self.last_saved {
                Some(t) => format!("saved {t}"),
                None => String::new(),
            }
        };
        let right = self.status_right(cx);

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::new_item))
            .on_action(|_: &ToggleTheme, window, cx| app::toggle_theme(window, cx))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .px_2()
                        .child(div().text_sm().child(format!("Wordy — {name}")))
                        .child(
                            Button::new("theme")
                                .ghost()
                                .xsmall()
                                .label(if cx.theme().mode.is_dark() { "Light" } else { "Dark" })
                                .on_click(|_, window, cx| app::toggle_theme(window, cx)),
                        ),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .child(div().flex_1().min_w_0().h_full().child(self.dock.clone())),
            )
            .child(
                StatusBar::new()
                    .left(
                        h_flex()
                            .gap_3()
                            .text_xs()
                            .child(dir)
                            .child(div().text_color(cx.theme().muted_foreground).child(save_state)),
                    )
                    .right(div().text_xs().child(right)),
            )
    }
}
