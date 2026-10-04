//! The tab bars the dock draws: gpui-component's skin for everything except
//! the bar itself, which Wordy draws without the skin's "..." menu and its
//! zoom control. Tabs, close buttons, drag-to-reorder and the two dock
//! collapse buttons stay.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::dock::{
    AnyDrag, DockArea, DockPlacement, DragPanel, DropIndicator, NodeId, PaneNode, PaneRef, TabGroupContext,
    TabGroupRenderer,
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::dock::PanelHandle;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{h_flex, ActiveTheme as _, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;

const DRAG_PREVIEW_SIZE: Size<Pixels> = size(px(96.), px(30.));

/// One tab group's bar, built per group so the scroll position is its own.
pub struct WordyTabs {
    skin: Rc<dyn TabGroupRenderer>,
    area: WeakEntity<DockArea>,
    scroll_handle: ScrollHandle,
    last_active_ix: Cell<Option<usize>>,
}

impl WordyTabs {
    pub fn new(skin: Rc<dyn TabGroupRenderer>, area: WeakEntity<DockArea>) -> Self {
        Self {
            skin,
            area,
            scroll_handle: ScrollHandle::default(),
            last_active_ix: Cell::new(None),
        }
    }

    /// The collapse button for a side dock, drawn in the centre group nearest
    /// to it: the left dock's at the top-left group, the right dock's at the
    /// top-right one.
    fn dock_toggle(&self, placement: DockPlacement, group: &TabGroupContext, cx: &App) -> Option<Button> {
        if group.is_zoomed() {
            return None;
        }
        let area = self.area.upgrade()?;
        let area = area.read(cx);
        if !area.is_dock_collapsible(placement) {
            return None;
        }
        let centre = area.layout(DockPlacement::Center)?;
        let designated = match placement {
            DockPlacement::Left => left_top_group(centre.root()),
            DockPlacement::Right => right_top_group(centre.root()),
            _ => None,
        };
        if designated != Some(group.node()) {
            return None;
        }
        let open = area.is_dock_open(placement);
        let (icon, tooltip) = match (placement, open) {
            (DockPlacement::Left, true) => (IconName::PanelLeft, "Hide the sidebar"),
            (DockPlacement::Left, false) => (IconName::PanelLeftOpen, "Show the sidebar"),
            (DockPlacement::Right, true) => (IconName::PanelRight, "Hide the reference pane"),
            (DockPlacement::Right, false) => (IconName::PanelRightOpen, "Show the reference pane"),
            _ => return None,
        };
        let area = self.area.clone();
        Some(
            Button::new(SharedString::from(format!("toggle-dock:{placement:?}")))
                .icon(icon)
                .xsmall()
                .ghost()
                .tab_stop(false)
                .tooltip(tooltip)
                .on_click(move |_, window, cx| {
                    _ = area.update(cx, |area, cx| area.toggle_dock(placement, window, cx));
                }),
        )
    }
}

fn left_top_group(node: &PaneNode) -> Option<NodeId> {
    match node.kind() {
        PaneRef::Tabs { .. } => Some(node.id()),
        PaneRef::Split { children, .. } => children.first().and_then(left_top_group),
    }
}

fn right_top_group(node: &PaneNode) -> Option<NodeId> {
    match node.kind() {
        PaneRef::Tabs { .. } => Some(node.id()),
        PaneRef::Split { axis, children, .. } => match axis {
            Axis::Vertical => children.first(),
            Axis::Horizontal => children.last(),
        }
        .and_then(right_top_group),
    }
}

/// The ghost that follows the pointer while a tab is dragged.
struct TabPreview {
    title: SharedString,
}

impl Render for TabPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("drag-tab")
            .cursor_grab()
            .py_1()
            .px_3()
            .w_24()
            .overflow_hidden()
            .whitespace_nowrap()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .text_color(cx.theme().tab_foreground)
            .bg(cx.theme().tokens.tab_active)
            .opacity(0.75)
            .child(self.title.clone())
    }
}

impl TabGroupRenderer for WordyTabs {
    fn frame(&self, group: &TabGroupContext, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.frame(group, window, cx)
    }

    fn content_frame(&self, group: &TabGroupContext, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.content_frame(group, window, cx)
    }

    fn render_active_panel(
        &self,
        panel: AnyView,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.skin.render_active_panel(panel, group, window, cx)
    }

    fn render_drop_indicator(&self, indicator: DropIndicator, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        self.skin.render_drop_indicator(indicator, window, cx)
    }

    fn render_empty(&self, group: &TabGroupContext, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        self.skin.render_empty(group, window, cx)
    }

    fn render_tab_bar(&self, group: &TabGroupContext, window: &mut Window, cx: &mut App) -> AnyElement {
        let visible: Vec<usize> = group
            .panels()
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.visible(cx))
            .map(|(ix, _)| ix)
            .collect();
        if visible.is_empty() {
            return Empty.into_any_element();
        }
        let left_button = self.dock_toggle(DockPlacement::Left, group, cx);
        let right_button = self.dock_toggle(DockPlacement::Right, group, cx);
        let collapsed = group.is_collapsed();
        let droppable = group.is_droppable();
        let tabs_count = group.panels().len();
        let active_ix = group.active_ix();
        let displayed = group.active_panel().map(|panel| panel.panel_id(cx));
        let displayed_ix =
            displayed.and_then(|displayed| group.panels().iter().position(|panel| panel.panel_id(cx) == displayed));
        // A collapsed group shows no tab as active: the strip is a way back
        // in, not a selection.
        let selected = displayed_ix
            .and_then(|d| visible.iter().position(|ix| *ix == d))
            .filter(|_| !collapsed);

        // Bring a newly displayed tab into view.
        if self.last_active_ix.replace(Some(active_ix)) != Some(active_ix) {
            if let Some(visible_ix) = visible.iter().position(|ix| *ix == active_ix) {
                self.scroll_handle.scroll_to_item(visible_ix);
            }
        }

        let theme = cx.theme().clone();
        let tabs: Vec<Tab> = visible
            .into_iter()
            .map(|ix| {
                let panel = &group.panels()[ix];
                let handle = PanelHandle::of(panel);
                let title: SharedString = handle
                    .and_then(|handle| handle.tab_name(cx))
                    .unwrap_or_else(|| panel.panel_name(cx).into());
                let panel_id = panel.panel_id(cx);
                let closable = !collapsed && group.is_panel_closable(panel_id, cx);
                let drag = (!collapsed && group.is_draggable())
                    .then(|| group.drag_panel(ix, cx))
                    .flatten();
                Tab::new()
                    .child(title.clone())
                    .when(closable, |this| {
                        this.suffix(
                            Button::new(("close-tab", ix))
                                .icon(IconName::Close)
                                .xsmall()
                                .custom(
                                    ButtonCustomVariant::new(cx)
                                        .foreground(theme.secondary_foreground)
                                        .hover(*theme.tokens.secondary_hover)
                                        .active(*theme.tokens.secondary_active),
                                )
                                .ml(-px(8.))
                                .mr_2()
                                .tab_stop(false)
                                .on_click({
                                    let group = group.clone();
                                    move |_, window, cx| {
                                        cx.stop_propagation();
                                        group.close(panel_id, window, cx);
                                    }
                                }),
                        )
                    })
                    .on_click({
                        let group = group.clone();
                        move |_, window, cx| group.select_tab(ix, window, cx)
                    })
                    .when_some(drag, |this, drag| {
                        this.on_drag(drag, move |drag, offset, _, cx| {
                            cx.stop_propagation();
                            drag.set_drag_offset(offset);
                            drag.set_preview_size(DRAG_PREVIEW_SIZE);
                            let title = title.clone();
                            cx.new(|_| TabPreview { title })
                        })
                    })
                    .when(!collapsed && droppable, |this| {
                        this.drag_over::<DragPanel>(|this, _, _, cx| {
                            this.rounded_l_none()
                                .border_l_2()
                                .border_r_0()
                                .border_color(cx.theme().drag_border)
                        })
                        .on_drop({
                            let group = group.clone();
                            move |drag: &DragPanel, window, cx| {
                                group.drop_panel(drag.clone(), Some(ix), true, window, cx);
                            }
                        })
                        .drag_over::<AnyDrag>(|this, _, _, cx| {
                            this.rounded_l_none()
                                .border_l_2()
                                .border_r_0()
                                .border_color(cx.theme().drag_border)
                        })
                        .on_drop({
                            let group = group.clone();
                            move |item: &AnyDrag, window, cx| {
                                group.drop_item(item.clone(), None, window, cx);
                            }
                        })
                    })
            })
            .collect();

        let title_suffix = group
            .active_panel()
            .and_then(PanelHandle::of)
            .and_then(|handle| handle.title_suffix(window, cx));
        let has_suffix = !collapsed && (title_suffix.is_some() || right_button.is_some());

        TabBar::new("tab-bar")
            .track_scroll(&self.scroll_handle)
            .when_some(selected, |this, ix| this.selected_index(ix))
            .when_some(left_button, |this, button| {
                this.prefix(
                    h_flex()
                        .items_center()
                        .top_0()
                        .right(-px(1.))
                        .border_r_1()
                        .border_b_1()
                        .h_full()
                        .border_color(theme.border)
                        .bg(theme.tokens.tab_bar)
                        .px_2()
                        .child(button),
                )
            })
            .children(tabs)
            .last_empty_space(
                // Empty space so a tab can be moved past the last one.
                div()
                    .id("tab-bar-empty-space")
                    .h_full()
                    .flex_grow_1()
                    .min_w_16()
                    .when(droppable, |this| {
                        this.drag_over::<DragPanel>(|this, _, _, cx| this.bg(cx.theme().tokens.drop_target))
                            .on_drop({
                                let group = group.clone();
                                let node = group.node();
                                move |drag: &DragPanel, window, cx| {
                                    let ix = (drag.source() == node).then(|| tabs_count - 1);
                                    group.drop_panel(drag.clone(), ix, false, window, cx);
                                }
                            })
                            .drag_over::<AnyDrag>(|this, _, _, cx| this.bg(cx.theme().tokens.drop_target))
                            .on_drop({
                                let group = group.clone();
                                move |item: &AnyDrag, window, cx| {
                                    group.drop_item(item.clone(), None, window, cx);
                                }
                            })
                    }),
            )
            .when(has_suffix, |this| {
                this.suffix(
                    h_flex()
                        .items_center()
                        .top_0()
                        .right_0()
                        .border_l_1()
                        .border_b_1()
                        .h_full()
                        .border_color(theme.border)
                        .bg(theme.tokens.tab_bar)
                        .px_2()
                        .gap_1()
                        .children(title_suffix)
                        .children(right_button),
                )
            })
            .into_any_element()
    }
}
