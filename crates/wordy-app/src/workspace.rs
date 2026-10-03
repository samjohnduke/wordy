//! The main window: title bar, space rail, dock area, status bar.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{DockArea, DockLayout, DockPlacement, DockSkin, PanelStyle, panel_handle};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::Space;

use crate::app::{self, SharedProject, Save, ToggleTheme};
use crate::panels::{editor::EditorPanel, reference::ReferencePanel, sidebar::SidebarPanel};

pub struct Workspace {
    project: SharedProject,
    space: Space,
    dock: Entity<DockArea>,
    sidebar: Entity<SidebarPanel>,
    focus: FocusHandle,
}

impl Workspace {
    pub fn new(project: SharedProject, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dock, skin) = DockSkin::dock_area("wordy-main", Some(1), window, cx);
        skin.set_panel_style(PanelStyle::TabBar, cx);
        skin.set_close_button_visible(true, cx);

        let sidebar = cx.new(|cx| SidebarPanel::new(project.clone(), Space::Manuscript, cx));
        let editor = cx.new(|cx| EditorPanel::placeholder(cx));
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
                DockLayout::tabs().panel_view(panel_handle(editor), cx),
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

        Self {
            project,
            space: Space::Manuscript,
            dock,
            sidebar,
            focus: cx.focus_handle(),
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

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        match self.project.project.save() {
            Ok(()) => tracing::info!("saved"),
            Err(e) => tracing::error!("save failed: {e:#}"),
        }
        cx.notify();
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

        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::save))
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
                    .left(div().text_xs().child(dir))
                    .right(div().text_xs().child(format!("{} space", self.space.label()))),
            )
    }
}
