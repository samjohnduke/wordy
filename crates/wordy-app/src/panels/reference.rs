//! Reference pane: read-only view of a pinned entity or note, with its
//! sheet summary and "Appears in", so you can glance at it while writing.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::dock::Panel;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::templates;
use wordy_doc::{NodeKind, TreeID};
use wordy_editor::{EditorEvent, ProseEditor};

use crate::app::SharedProject;

pub enum ReferenceEvent {
    /// Open the pinned node (or a node it links to) in an editor tab.
    Open(TreeID),
    /// Show another node here.
    Pin(TreeID),
}

struct Pinned {
    id: TreeID,
    editor: Entity<ProseEditor>,
    _sub: Subscription,
}

pub struct ReferencePanel {
    project: SharedProject,
    pinned: Option<Pinned>,
    pub focus: FocusHandle,
}

impl ReferencePanel {
    pub fn new(project: SharedProject, cx: &mut Context<Self>) -> Self {
        Self { project, pinned: None, focus: cx.focus_handle() }
    }

    pub fn pinned_id(&self) -> Option<TreeID> {
        self.pinned.as_ref().map(|p| p.id)
    }

    pub fn pin(&mut self, id: TreeID, cx: &mut Context<Self>) {
        if self.pinned_id() == Some(id) {
            self.refresh(cx);
            return;
        }
        let Ok(node) = self.project.project.node(id) else { return };
        let Ok(body) = node.body() else { return };
        let doc = self.project.project.doc.clone();
        let editor = cx.new(|cx| ProseEditor::new(doc, body, cx));
        editor.update(cx, |e, cx| {
            e.set_read_only(true);
            e.set_comments(self.project.project.comments(), id.to_string());
            e.set_self_id(Some(id));
            e.set_link_targets(self.project.link_targets(), cx);
        });
        let sub = cx.subscribe(&editor, |_, _, ev: &EditorEvent, cx| {
            if let EditorEvent::OpenLink { id, navigate } = ev {
                if *navigate {
                    cx.emit(ReferenceEvent::Open(*id));
                } else {
                    cx.emit(ReferenceEvent::Pin(*id));
                }
            }
        });
        self.pinned = Some(Pinned { id, editor, _sub: sub });
        cx.notify();
    }

    pub fn unpin(&mut self, cx: &mut Context<Self>) {
        self.pinned = None;
        cx.notify();
    }

    /// Re-read the pinned body and metadata (edited in another tab).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &self.pinned {
            p.editor.update(cx, |e, cx| e.reload(cx));
        }
        cx.notify();
    }

    pub fn refresh_if(&mut self, id: TreeID, cx: &mut Context<Self>) {
        if self.pinned_id() == Some(id) {
            self.refresh(cx);
        }
    }

    pub fn set_link_targets(&mut self, targets: Vec<wordy_editor::LinkTarget>, cx: &mut Context<Self>) {
        if let Some(p) = &self.pinned {
            p.editor.update(cx, |e, cx| e.set_link_targets(targets, cx));
        }
        cx.notify();
    }

    fn render_pinned(&self, pinned: &Pinned, cx: &mut Context<Self>) -> AnyElement {
        let Ok(node) = self.project.project.node(pinned.id) else {
            return div().p_2().child("This item no longer exists.").into_any_element();
        };
        let id = pinned.id;
        let muted = cx.theme().muted_foreground;
        let is_entity = node.kind() == NodeKind::Entity;

        let mut header = v_flex().gap_1().px_3().pt_2().pb_2().border_b_1().border_color(cx.theme().border).child(
            h_flex()
                .items_center()
                .justify_between()
                .child(div().font_semibold().text_base().child(node.title()))
                .child(
                    h_flex()
                        .gap_0p5()
                        .child(
                            Button::new("ref-open")
                                .ghost()
                                .xsmall()
                                .label("Open")
                                .tooltip("Open in an editor tab")
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ReferenceEvent::Open(id)))),
                        )
                        .child(
                            Button::new("ref-unpin")
                                .ghost()
                                .xsmall()
                                .label("×")
                                .tooltip("Unpin")
                                .on_click(cx.listener(|this, _, _, cx| this.unpin(cx))),
                        ),
                ),
        );

        if is_entity {
            let template = node.template().map(|t| templates::by_id(&t)).unwrap_or(templates::DEFAULT);
            let aliases = node.aliases();
            let mut line = template.label.to_string();
            if !aliases.is_empty() {
                line.push_str(" · aka ");
                line.push_str(&aliases.join(", "));
            }
            header = header.child(div().text_xs().text_color(muted).child(line));
            for spec in template.fields {
                let value = node.field(spec.key);
                if value.is_empty() {
                    continue;
                }
                header = header.child(
                    v_flex()
                        .gap_0()
                        .child(div().text_xs().text_color(muted).child(spec.label))
                        .child(div().text_sm().child(value)),
                );
            }
            let backlinks = self.project.appears_in(id);
            if !backlinks.is_empty() {
                let mut list = v_flex().gap_0().child(div().text_xs().text_color(muted).child("Appears in"));
                for (ix, b) in backlinks.iter().enumerate() {
                    let target = b.node;
                    list = list.child(
                        div()
                            .id(ElementId::Name(format!("ref-bl-{ix}").into()))
                            .text_sm()
                            .text_color(cx.theme().primary)
                            .cursor_pointer()
                            .child(format!("{} ×{}", b.title, b.count))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ReferenceEvent::Open(target)))),
                    );
                }
                header = header.child(list);
            }
        }

        v_flex()
            .size_full()
            .child(div().id("ref-head").max_h(px(320.)).overflow_y_scroll().child(header))
            .child(div().flex_1().min_h_0().w_full().child(pinned.editor.clone()))
            .into_any_element()
    }
}

super::impl_panel_boilerplate!(ReferencePanel, "Reference", closable = false);

impl Panel for ReferencePanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Reference".into())
    }
}

impl EventEmitter<ReferenceEvent> for ReferencePanel {}

impl Render for ReferencePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.pinned {
            Some(p) => div().size_full().track_focus(&self.focus).child(self.render_pinned(p, cx)),
            None => div().size_full().track_focus(&self.focus).child(
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .px_4()
                    .text_sm()
                    .text_align(TextAlign::Center)
                    .text_color(cx.theme().muted_foreground)
                    .child("Click an entity link or mention in a scene to pin it here."),
            ),
        }
    }
}
