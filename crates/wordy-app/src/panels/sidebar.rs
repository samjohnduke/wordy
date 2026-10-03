//! The space sidebar: tree of nodes for the current space.

use gpui_kit::component::dock::Panel;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::{Space, TreeID};

use crate::app::SharedProject;

pub struct SidebarPanel {
    project: SharedProject,
    space: Space,
    pub focus: FocusHandle,
}

impl SidebarPanel {
    pub fn new(project: SharedProject, space: Space, cx: &mut Context<Self>) -> Self {
        Self { project, space, focus: cx.focus_handle() }
    }

    pub fn set_space(&mut self, space: Space, cx: &mut Context<Self>) {
        self.space = space;
        cx.notify();
    }

    fn render_node(&self, id: TreeID, depth: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let Ok(node) = self.project.project.node(id) else { return out };
        out.push(
            div()
                .pl(px(8. + 14. * depth as f32))
                .py_0p5()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(node.title())
                .into_any_element(),
        );
        for child in self.project.project.children(id) {
            out.extend(self.render_node(child, depth + 1, cx));
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

impl Render for SidebarPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = self.project.project.root(self.space);
        let items: Vec<AnyElement> = self
            .project
            .project
            .children(root)
            .into_iter()
            .flat_map(|c| self.render_node(c, 0, cx))
            .collect();
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .p_1()
            .when(items.is_empty(), |d| {
                d.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Nothing here yet."),
                )
            })
            .children(items)
    }
}
