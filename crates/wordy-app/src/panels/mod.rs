pub mod editor;
pub mod empty;
pub mod home;
pub mod reference;
pub mod sheet;
pub mod sidebar;

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// The one heading style every panel uses for its section titles: small,
/// semibold, muted and upper-cased. Sidebar, sheet and Home share it so the
/// spaces read as one app.
pub fn heading(title: impl Into<SharedString>, cx: &App) -> Div {
    let title: SharedString = title.into();
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .whitespace_nowrap()
        .child(title.to_uppercase())
}

/// Boilerplate shared by every dock panel.
macro_rules! impl_panel_boilerplate {
    ($ty:ty, $name:literal) => {
        $crate::panels::impl_panel_boilerplate!($ty, $name, closable = true);
    };
    ($ty:ty, $name:literal, closable = $closable:expr) => {
        impl gpui_kit::component::dock::BasePanel for $ty {
            fn panel_name(&self) -> &'static str {
                $name
            }
            fn closable(&self, _: &gpui_kit::App) -> bool {
                $closable
            }
        }
        impl gpui_kit::EventEmitter<gpui_kit::component::dock::PanelEvent> for $ty {}
        impl gpui_kit::Focusable for $ty {
            fn focus_handle(&self, _: &gpui_kit::App) -> gpui_kit::FocusHandle {
                self.focus.clone()
            }
        }
    };
}
pub(crate) use impl_panel_boilerplate;
