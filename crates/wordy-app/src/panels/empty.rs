//! What the centre shows when no tab is open: a short line and a search box
//! that jumps to any scene, entity or note. It is not a panel, so it carries
//! no tab bar; the dock renderer overlays it on an empty centre.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::dock::{DockAreaRenderer, DockContext, NodeId, PanelState, PanelView, TabGroupRenderer};
use gpui_kit::base::ResizeHandleContext;
use gpui_kit::component::dock::DockSkin;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::{Space, TreeID};

use crate::app::SharedProject;
use crate::workspace::match_rank;

/// How many matches the empty centre lists.
const EMPTY_LIMIT: usize = 8;

#[derive(Clone)]
pub enum EmptyCenterEvent {
    /// A match was chosen; the workspace opens it like a palette hit.
    Open { id: TreeID, space: Space, has_body: bool },
}

struct Hit {
    id: TreeID,
    title: String,
    kind: &'static str,
    space: Space,
    has_body: bool,
}

pub struct EmptyCenter {
    project: SharedProject,
    input: Entity<InputState>,
    selected: usize,
    /// Set by the workspace: true only while no editor or Home tab is open.
    shown: bool,
    _sub: Subscription,
}

impl EmptyCenter {
    pub fn new(project: SharedProject, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Jump to a scene, entity, or note…"));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                this.selected = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.confirm(window, cx),
            _ => {}
        });
        Self {
            project,
            input,
            selected: 0,
            shown: false,
            _sub: sub,
        }
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown != shown {
            self.shown = shown;
            cx.notify();
        }
    }

    /// Put the caret in the search box with the last query cleared.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0;
        self.input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
    }

    fn hits(&self, cx: &App) -> Vec<Hit> {
        let query = self.input.read(cx).value().trim().to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let p = &self.project.project;
        let mut ranked: Vec<(u8, usize, Hit)> = Vec::new();
        for (order, id) in p.all_nodes().into_iter().enumerate() {
            let Ok(n) = p.node(id) else { continue };
            let title = n.title();
            let Some(rank) = match_rank(&title, &query, &terms) else {
                continue;
            };
            let kind = n.kind();
            ranked.push((
                rank,
                order,
                Hit {
                    id,
                    title,
                    kind: kind.as_str(),
                    space: n.space(),
                    has_body: kind.has_body(),
                },
            ));
        }
        ranked.sort_by_key(|(rank, order, _)| (std::cmp::Reverse(*rank), *order));
        let mut out: Vec<Hit> = ranked.into_iter().map(|(_, _, h)| h).collect();
        out.truncate(EMPTY_LIMIT);
        out
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.hits(cx).len();
        if n > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(n as isize) as usize;
            cx.notify();
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut hits = self.hits(cx);
        if hits.is_empty() {
            return;
        }
        let sel = self.selected.min(hits.len() - 1);
        let hit = hits.swap_remove(sel);
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.selected = 0;
        cx.emit(EmptyCenterEvent::Open {
            id: hit.id,
            space: hit.space,
            has_body: hit.has_body,
        });
        cx.notify();
    }
}

impl EventEmitter<EmptyCenterEvent> for EmptyCenter {}

impl Render for EmptyCenter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hits = self.hits(cx);
        let searching = !self.input.read(cx).value().trim().is_empty();
        let selected = self.selected.min(hits.len().saturating_sub(1));
        let (accent, secondary, muted, border, popover) = {
            let t = cx.theme();
            (t.accent, t.secondary, t.muted_foreground, t.border, t.popover)
        };
        let rows: Vec<AnyElement> = hits
            .into_iter()
            .enumerate()
            .map(|(ix, hit)| {
                let detail = format!("{} · {}", hit.space.label(), hit.kind);
                h_flex()
                    .id(ElementId::Name(format!("empty-hit-{ix}").into()))
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .text_sm()
                    .cursor_pointer()
                    .when(ix == selected, |d| d.bg(accent))
                    .when(ix != selected, |d| d.hover(|s| s.bg(secondary)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(hit.title),
                    )
                    .child(div().text_xs().text_color(muted).flex_shrink_0().child(detail))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected = ix;
                        this.confirm(window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let none = searching && rows.is_empty();

        v_flex()
            .id("empty-center")
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .px_6()
            .bg(cx.theme().background)
            // The input swallows plain up/down, so steer the list from the
            // raw key event before bindings are consulted.
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let k = &ev.keystroke;
                let delta = match (k.key.as_str(), k.modifiers.control) {
                    ("up", _) | ("p", true) => -1,
                    ("down", _) | ("n", true) => 1,
                    _ => return,
                };
                this.step(delta, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Nothing open. Pick something from the sidebar, or search."),
            )
            .child(
                v_flex()
                    .w(px(440.))
                    .max_w_full()
                    .gap_1()
                    .child(Input::new(&self.input).small())
                    .when(searching, |d| {
                        d.child(
                            v_flex()
                                .w_full()
                                .p_1()
                                .gap_1()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .bg(popover)
                                .children(rows)
                                .when(none, |d| {
                                    d.child(div().px_2().py_1().text_sm().text_color(muted).child("No matches."))
                                }),
                        )
                    }),
            )
    }
}

/// The app's dock appearance: gpui-component's skin, plus the empty-centre
/// view laid over the centre while no tab is open. Every other hook is the
/// skin's.
pub struct WordyDockRenderer {
    skin: Rc<DockSkin>,
    empty: Entity<EmptyCenter>,
}

impl WordyDockRenderer {
    pub fn new(skin: Rc<DockSkin>, empty: Entity<EmptyCenter>) -> Self {
        Self { skin, empty }
    }
}

impl DockAreaRenderer for WordyDockRenderer {
    fn frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.frame(window, cx)
    }

    fn split_frame(&self, node: NodeId, axis: Axis, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.split_frame(node, axis, window, cx)
    }

    fn center_frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        let frame = self.skin.center_frame(window, cx).relative();
        if !self.empty.read(cx).shown() {
            return frame;
        }
        frame.child(div().absolute().inset_0().size_full().child(self.empty.clone()))
    }

    fn render_split_handle(
        &self,
        handle: &ResizeHandleContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.skin.render_split_handle(handle, window, cx)
    }

    fn render_dock(&self, dock: &DockContext, content: AnyElement, window: &mut Window, cx: &mut App) -> AnyElement {
        self.skin.render_dock(dock, content, window, cx)
    }

    fn build_placeholder(&self, state: &PanelState, window: &mut Window, cx: &mut App) -> Option<Arc<dyn PanelView>> {
        self.skin.build_placeholder(state, window, cx)
    }

    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        self.skin.tab_group_renderer()
    }
}
