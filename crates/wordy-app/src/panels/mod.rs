pub mod editor;
pub mod empty;
pub mod home;
pub mod reference;
pub mod settings;
pub mod sheet;
pub mod sidebar;

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::{v_flex, ActiveTheme as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::loro::{LoroMap, LoroValue, ValueOrContainer};

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

/// A bordered card with a heading, the unit Home and Settings pages are
/// built from.
pub fn section(title: &str, cx: &App) -> Div {
    v_flex()
        .w_full()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().secondary.opacity(0.35))
        .child(heading(title.to_string(), cx))
}

pub fn setting_str(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

pub fn setting_bool(map: &LoroMap, key: &str) -> Option<bool> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::Bool(b))) => Some(b),
        _ => None,
    }
}

pub fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.0} KB", n as f64 / 1024.)
    } else {
        format!("{:.1} MB", n as f64 / (1024. * 1024.))
    }
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
