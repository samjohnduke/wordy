//! Reference pane: read-only view of a pinned entity or note, with its
//! sheet summary and "Appears in", so you can glance at it while writing.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::Panel;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::templates;
use wordy_doc::{diff, DiffStats, NodeKind, TreeID, Version};
use wordy_editor::{EditorEvent, ProseEditor};

use crate::app::SharedProject;

pub enum ReferenceEvent {
    /// Open the pinned node (or a node it links to) in an editor tab.
    Open(TreeID),
    /// Show another node here.
    Pin(TreeID),
    /// Replace the node's body with this saved version.
    RestoreVersion(Version),
}

struct Pinned {
    id: TreeID,
    /// Set when showing a saved version of `id` instead of its live body.
    version: Option<Version>,
    /// Version view: highlight word-level changes against the live body.
    compare: bool,
    stats: Option<DiffStats>,
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
        Self {
            project,
            pinned: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn pinned_id(&self) -> Option<TreeID> {
        self.pinned.as_ref().filter(|p| p.version.is_none()).map(|p| p.id)
    }

    /// Show a saved version of a body, read-only, with a Restore button and
    /// word-level diff highlighting against the live body.
    pub fn pin_version(&mut self, v: Version, cx: &mut Context<Self>) {
        let compare = self
            .pinned
            .as_ref()
            .filter(|p| p.version.is_some())
            .map(|p| p.compare)
            .unwrap_or(true);
        self.show_version(v, compare, cx);
    }

    fn show_version(&mut self, v: Version, compare: bool, cx: &mut Context<Self>) {
        let mut stats = None;
        let built = if compare {
            self.project.project.version_diff(&v).map(|spans| {
                stats = Some(diff::stats(&spans));
                diff::diff_doc(&spans)
            })
        } else {
            self.project.project.version_doc(&v)
        };
        let (doc, text) = match built {
            Ok(x) => x,
            Err(e) => {
                tracing::error!("version view: {e:#}");
                return;
            }
        };
        let editor = cx.new(|cx| ProseEditor::new(doc, text, cx));
        editor.update(cx, |e, cx| {
            e.spellcheck = false;
            e.set_read_only(true);
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
        self.pinned = Some(Pinned {
            id: v.node,
            version: Some(v),
            compare,
            stats,
            editor,
            _sub: sub,
        });
        cx.notify();
    }

    fn toggle_compare(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.pinned.as_ref() else { return };
        let Some(v) = p.version.clone() else { return };
        let compare = !p.compare;
        self.show_version(v, compare, cx);
    }

    pub fn pin(&mut self, id: TreeID, cx: &mut Context<Self>) {
        if self.pinned_id() == Some(id) {
            self.refresh(cx);
            return;
        }
        let Ok(node) = self.project.project.node(id) else {
            return;
        };
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
        self.pinned = Some(Pinned {
            id,
            version: None,
            compare: false,
            stats: None,
            editor,
            _sub: sub,
        });
        cx.notify();
    }

    pub fn unpin(&mut self, cx: &mut Context<Self>) {
        self.pinned = None;
        cx.notify();
    }

    /// Re-read the pinned body and metadata (edited in another tab).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.pinned.as_ref() else {
            cx.notify();
            return;
        };
        match p.version.clone() {
            None => p.editor.update(cx, |e, cx| e.reload(cx)),
            // The live side of the comparison moved: rebuild the diff.
            Some(v) if p.compare => self.show_version(v, true, cx),
            Some(_) => {}
        }
        cx.notify();
    }

    pub fn refresh_if(&mut self, id: TreeID, cx: &mut Context<Self>) {
        if self.pinned.as_ref().map(|p| p.id) == Some(id) {
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

        if let Some(v) = &pinned.version {
            let when = wordy_doc::chrono::DateTime::from_timestamp_millis(v.created)
                .map(|d| {
                    d.with_timezone(&wordy_doc::chrono::Local)
                        .format("%b %-d, %Y %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            let words = match pinned.stats {
                Some(_) => self.project.project.node(id).map(|n| n.word_count()).unwrap_or(0),
                None => pinned.editor.read(cx).word_count(),
            };
            let restore = v.clone();
            let compare = pinned.compare;
            let changes: Option<AnyElement> = pinned.stats.map(|s| {
                let chip = |label: String, color: Hsla| div().px_1().rounded_sm().bg(color).text_xs().child(label);
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .text_color(muted)
                    .child(chip(format!("+{} added", s.inserted), hsla(0.36, 0.7, 0.5, 0.35)))
                    .child(chip(format!("−{} removed", s.deleted), hsla(0.0, 0.8, 0.55, 0.3)))
                    .child(if s.inserted == 0 && s.deleted == 0 {
                        "Live body matches this version."
                    } else {
                        "Green: live only. Red: version only."
                    })
                    .into_any_element()
            });
            let header = v_flex()
                .gap_1()
                .px_3()
                .pt_2()
                .pb_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(div().font_semibold().text_base().child(node.title()))
                        .child(
                            h_flex()
                                .gap_0p5()
                                .child(
                                    Button::new("ref-compare")
                                        .ghost()
                                        .xsmall()
                                        .label(if compare { "Hide changes" } else { "Compare" })
                                        .tooltip("Highlight word-level differences against the live body")
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_compare(cx))),
                                )
                                .child(
                                    Button::new("ref-restore")
                                        .primary()
                                        .xsmall()
                                        .label("Restore")
                                        .tooltip("Replace the current body with this version (undoable)")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(ReferenceEvent::RestoreVersion(restore.clone()))
                                        })),
                                )
                                .child(
                                    Button::new("ref-unpin")
                                        .ghost()
                                        .xsmall()
                                        .label("×")
                                        .tooltip("Close")
                                        .on_click(cx.listener(|this, _, _, cx| this.unpin(cx))),
                                ),
                        ),
                )
                .child(div().text_xs().text_color(muted).child(format!(
                    "Version · {} · {when} · {words} words{}",
                    v.label,
                    if compare { " live" } else { "" }
                )))
                .children(changes);
            return v_flex()
                .size_full()
                .child(header)
                .child(div().flex_1().min_h_0().w_full().child(pinned.editor.clone()))
                .into_any_element();
        }

        let mut header = v_flex()
            .gap_1()
            .px_3()
            .pt_2()
            .pb_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
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
            let template = node
                .template()
                .map(|t| templates::by_id(&t))
                .unwrap_or(templates::DEFAULT);
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
                let mut list = v_flex()
                    .gap_0()
                    .child(div().text_xs().text_color(muted).child("Appears in"));
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
            Some(p) => div()
                .size_full()
                .track_focus(&self.focus)
                .child(self.render_pinned(p, cx)),
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
