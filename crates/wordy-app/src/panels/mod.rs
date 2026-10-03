pub mod editor;
pub mod reference;
pub mod sidebar;

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
