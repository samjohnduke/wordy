//! Editor tab. Phase 0: a placeholder; the prose editor arrives in Phase 1.

use gpui_kit::component::dock::Panel;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub struct EditorPanel {
    title: SharedString,
    pub focus: FocusHandle,
}

impl EditorPanel {
    pub fn placeholder(cx: &mut Context<Self>) -> Self {
        Self { title: "Welcome".into(), focus: cx.focus_handle() }
    }
}

super::impl_panel_boilerplate!(EditorPanel, "Editor");

impl Panel for EditorPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }
}

impl Render for EditorPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child("Open a scene from the sidebar to start writing.")
    }
}
