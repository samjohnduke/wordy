//! Reference pane: read-only view of a pinned entity, note, or version.

use gpui_kit::component::dock::Panel;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub struct ReferencePanel {
    pub focus: FocusHandle,
}

impl ReferencePanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { focus: cx.focus_handle() }
    }
}

super::impl_panel_boilerplate!(ReferencePanel, "Reference", closable = false);

impl Panel for ReferencePanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Reference".into())
    }
}

impl Render for ReferencePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("Pin an entity or note here.")
    }
}
