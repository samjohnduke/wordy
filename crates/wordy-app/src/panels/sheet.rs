//! Entity sheet: template, aliases, template-driven fields, relations,
//! attachments, and "Appears in". Shown above an entity's description editor.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::templates::{self, FieldSpec, Template};
use wordy_doc::{storage, TreeID};
use wordy_index::LinkKind;

use crate::app::SharedProject;

pub enum SheetEvent {
    /// Metadata changed (fields, template, relations, attachments).
    Changed,
    /// Names changed (aliases): the matcher and index need refreshing.
    NamesChanged,
    /// Open a node in the editor.
    Open(TreeID),
    /// Show a node in the reference pane.
    Pin(TreeID),
}

enum FieldInput {
    Single(Entity<InputState>),
    Multi(Entity<TextareaState>),
}

struct Field {
    spec: FieldSpec,
    input: FieldInput,
    _sub: Subscription,
}

struct RelationForm {
    kind: Entity<InputState>,
    target: Entity<InputState>,
    _subs: Vec<Subscription>,
}

pub struct EntitySheet {
    project: SharedProject,
    id: TreeID,
    collapsed: bool,
    aliases: Entity<InputState>,
    fields: Vec<Field>,
    relation_form: Option<RelationForm>,
    _subs: Vec<Subscription>,
}

impl EntitySheet {
    pub fn new(project: SharedProject, id: TreeID, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let aliases_now = project
            .project
            .node(id)
            .map(|n| n.aliases().join(", "))
            .unwrap_or_default();
        let aliases = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(aliases_now)
                .placeholder("Aliases, comma separated")
        });
        let sub = cx.subscribe_in(&aliases, window, |this, input, ev: &InputEvent, _window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let value = input.read(cx).value().to_string();
                this.commit_aliases(&value, cx);
            }
        });
        let mut this = Self {
            project,
            id,
            collapsed: false,
            aliases,
            fields: Vec::new(),
            relation_form: None,
            _subs: vec![sub],
        };
        this.build_fields(window, cx);
        this
    }

    pub fn template(&self) -> Template {
        let id = self.project.project.node(self.id).ok().and_then(|n| n.template());
        match id {
            Some(id) => templates::by_id(&id),
            None => templates::DEFAULT,
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.project.project.commit_meta();
        cx.emit(SheetEvent::Changed);
        cx.notify();
    }

    fn build_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fields.clear();
        let Ok(node) = self.project.project.node(self.id) else {
            return;
        };
        for spec in self.template().fields {
            let value = node.field(spec.key);
            let key = spec.key;
            let (input, sub) = if spec.multiline {
                let state = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 12).default_value(value));
                let sub = cx.subscribe_in(&state, window, move |this, input, ev: &InputEvent, _window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        let v = input.read(cx).value().to_string();
                        this.set_field(key, &v, cx);
                    }
                });
                (FieldInput::Multi(state), sub)
            } else {
                let state = cx.new(|cx| InputState::new(window, cx).default_value(value));
                let sub = cx.subscribe_in(&state, window, move |this, input, ev: &InputEvent, _window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        let v = input.read(cx).value().to_string();
                        this.set_field(key, &v, cx);
                    }
                });
                (FieldInput::Single(state), sub)
            };
            self.fields.push(Field {
                spec: *spec,
                input,
                _sub: sub,
            });
        }
    }

    fn set_field(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        if let Ok(node) = self.project.project.node(self.id) {
            if node.field(key) != value {
                if let Err(e) = node.set_field(key, value) {
                    tracing::error!("set field: {e:#}");
                }
                self.changed(cx);
            }
        }
    }

    fn set_template(&mut self, template: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(node) = self.project.project.node(self.id) {
            if let Err(e) = node.set_template(template) {
                tracing::error!("set template: {e:#}");
            }
        }
        self.build_fields(window, cx);
        self.changed(cx);
    }

    fn commit_aliases(&mut self, value: &str, cx: &mut Context<Self>) {
        let Ok(node) = self.project.project.node(self.id) else {
            return;
        };
        let wanted: Vec<String> = value
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let current = node.aliases();
        if wanted == current {
            return;
        }
        for a in &current {
            if !wanted.contains(a) {
                let _ = node.remove_alias(a);
            }
        }
        for a in &wanted {
            if !current.contains(a) {
                let _ = node.add_alias(a);
            }
        }
        self.project.project.commit_meta();
        cx.emit(SheetEvent::NamesChanged);
        cx.emit(SheetEvent::Changed);
        cx.notify();
    }

    // ----- relations -------------------------------------------------------

    fn begin_relation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kind = cx.new(|cx| InputState::new(window, cx).placeholder("Relation (e.g. sister)"));
        let target = cx.new(|cx| InputState::new(window, cx).placeholder("Entity name"));
        let s1 = cx.subscribe_in(&kind, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.commit_relation(cx);
            }
        });
        let s2 = cx.subscribe_in(&target, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.commit_relation(cx);
            }
        });
        kind.update(cx, |s, cx| s.focus(window, cx));
        self.relation_form = Some(RelationForm {
            kind,
            target,
            _subs: vec![s1, s2],
        });
        cx.notify();
    }

    /// Resolve a typed entity name: exact title/alias match first, then a unique prefix.
    fn resolve_entity(&self, name: &str) -> Option<TreeID> {
        let name = name.trim().to_lowercase();
        if name.is_empty() {
            return None;
        }
        let targets = self.project.link_targets();
        let exact = targets.iter().find(|t| {
            t.id != self.id && (t.title.to_lowercase() == name || t.aliases.iter().any(|a| a.to_lowercase() == name))
        });
        if let Some(t) = exact {
            return Some(t.id);
        }
        let mut prefix = targets
            .iter()
            .filter(|t| t.id != self.id && t.title.to_lowercase().starts_with(&name));
        let first = prefix.next()?;
        if prefix.next().is_some() {
            return None;
        }
        Some(first.id)
    }

    fn commit_relation(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.relation_form.as_ref() else {
            return;
        };
        let kind = form.kind.read(cx).value().trim().to_string();
        let target = form.target.read(cx).value().to_string();
        let Some(to) = self.resolve_entity(&target) else {
            tracing::warn!("no entity matches {target:?}");
            return;
        };
        if let Ok(node) = self.project.project.node(self.id) {
            if let Err(e) = node.add_relation(to, &kind, "") {
                tracing::error!("add relation: {e:#}");
            }
        }
        self.relation_form = None;
        self.changed(cx);
    }

    fn remove_relation(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Ok(node) = self.project.project.node(self.id) {
            if let Err(e) = node.remove_relation(ix) {
                tracing::error!("remove relation: {e:#}");
            }
        }
        self.changed(cx);
    }

    // ----- attachments -----------------------------------------------------

    fn assets_dir(&self) -> Option<PathBuf> {
        self.project.dir().map(|d| d.join(storage::ASSETS_DIR))
    }

    fn add_attachments(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let picked = match rx.await {
                Ok(Ok(Some(paths))) => paths,
                _ => return,
            };
            this.update(cx, |sheet, cx| {
                for path in picked {
                    if let Err(e) = sheet.attach(&path) {
                        tracing::error!("attach {}: {e:#}", path.display());
                    }
                }
                sheet.changed(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Copy `src` into `assets/` under a content-hash name and record it.
    fn attach(&self, src: &Path) -> anyhow::Result<()> {
        let assets = self
            .assets_dir()
            .ok_or_else(|| anyhow::anyhow!("project has no folder"))?;
        std::fs::create_dir_all(&assets)?;
        let bytes = std::fs::read(src)?;
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let rel = format!("{:016x}{ext}", h.finish());
        let dest = assets.join(&rel);
        if !dest.exists() {
            std::fs::write(&dest, &bytes)?;
        }
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment")
            .to_string();
        let mime = mime_guess::from_path(src)
            .first_raw()
            .unwrap_or("application/octet-stream");
        self.project.project.node(self.id)?.add_attachment(&name, &rel, mime)?;
        Ok(())
    }

    fn open_attachment(&self, rel: &str, cx: &mut App) {
        if let Some(assets) = self.assets_dir() {
            cx.open_with_system(&assets.join(rel));
        }
    }

    fn remove_attachment(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Ok(node) = self.project.project.node(self.id) {
            if let Err(e) = node.remove_attachment(ix) {
                tracing::error!("remove attachment: {e:#}");
            }
        }
        self.changed(cx);
    }

    // ----- render ----------------------------------------------------------

    fn label(text: impl Into<SharedString>, cx: &App) -> Div {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(text.into())
    }

    fn render_templates(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.template().id;
        h_flex().flex_wrap().gap_1().children(templates::ALL.iter().map(|t| {
            let id = t.id;
            Button::new(ElementId::Name(format!("tpl-{id}").into()))
                .ghost()
                .xsmall()
                .label(t.label)
                .toggled(current == id)
                .on_click(cx.listener(move |this, _, window, cx| this.set_template(id, window, cx)))
        }))
    }

    fn render_fields(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.fields
            .iter()
            .map(|f| {
                let input: AnyElement = match &f.input {
                    FieldInput::Single(s) => Input::new(s).small().into_any_element(),
                    FieldInput::Multi(s) => Textarea::new(s).small().into_any_element(),
                };
                v_flex()
                    .gap_0p5()
                    .w_full()
                    .child(Self::label(f.spec.label, cx))
                    .child(input)
                    .into_any_element()
            })
            .collect()
    }

    fn render_relations(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Ok(node) = self.project.project.node(self.id) else {
            return v_flex();
        };
        let relations = node.relations();
        let mut section = v_flex().gap_0p5().w_full().child(
            h_flex()
                .justify_between()
                .items_center()
                .child(Self::label("Relations", cx))
                .child(
                    Button::new("rel-add")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("Add relation")
                        .on_click(cx.listener(|this, _, window, cx| this.begin_relation(window, cx))),
                ),
        );
        for (ix, r) in relations.iter().enumerate() {
            let to = r.to;
            let title = self
                .project
                .project
                .node(to)
                .map(|n| n.title())
                .unwrap_or_else(|_| "(missing)".into());
            section = section.child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_sm()
                    .child(div().text_color(cx.theme().muted_foreground).child(r.kind.clone()))
                    .child(
                        div()
                            .id(ElementId::Name(format!("rel-{ix}").into()))
                            .text_color(cx.theme().primary)
                            .cursor_pointer()
                            .child(title)
                            .on_click(cx.listener(move |_, ev: &ClickEvent, _, cx| {
                                if ev.modifiers().secondary() {
                                    cx.emit(SheetEvent::Open(to));
                                } else {
                                    cx.emit(SheetEvent::Pin(to));
                                }
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(ElementId::Name(format!("rel-rm-{ix}").into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_relation(ix, cx))),
                    ),
            );
        }
        if let Some(form) = &self.relation_form {
            section = section.child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(div().w(px(140.)).child(Input::new(&form.kind).xsmall()))
                    .child(div().flex_1().child(Input::new(&form.target).xsmall()))
                    .child(
                        Button::new("rel-ok")
                            .ghost()
                            .xsmall()
                            .label("Add")
                            .on_click(cx.listener(|this, _, _, cx| this.commit_relation(cx))),
                    )
                    .child(
                        Button::new("rel-cancel")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.relation_form = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        section
    }

    fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Ok(node) = self.project.project.node(self.id) else {
            return v_flex();
        };
        let attachments = node.attachments();
        let mut section = v_flex().gap_0p5().w_full().child(
            h_flex()
                .justify_between()
                .items_center()
                .child(Self::label("Attachments", cx))
                .child(
                    Button::new("att-add")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("Add files…")
                        .on_click(cx.listener(|this, _, _, cx| this.add_attachments(cx))),
                ),
        );
        for (ix, a) in attachments.iter().enumerate() {
            let rel = a.path.clone();
            section = section.child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_sm()
                    .child(
                        div()
                            .id(ElementId::Name(format!("att-{ix}").into()))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(cx.theme().primary)
                            .cursor_pointer()
                            .child(a.name.clone())
                            .on_click(cx.listener(move |this, _, _, cx| this.open_attachment(&rel, cx))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(a.mime.clone()),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("att-rm-{ix}").into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_attachment(ix, cx))),
                    ),
            );
        }
        section
    }

    fn render_appears_in(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let backlinks = self.project.appears_in(self.id);
        let mut section = v_flex().gap_0p5().w_full().child(Self::label("Appears in", cx));
        if backlinks.is_empty() {
            section = section.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Not mentioned anywhere yet."),
            );
        }
        for (ix, b) in backlinks.iter().enumerate() {
            let id = b.node;
            let kind = match b.kind {
                LinkKind::Explicit => "linked",
                LinkKind::Auto => "mentioned",
            };
            section = section.child(
                h_flex()
                    .id(ElementId::Name(format!("bl-{ix}").into()))
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .cursor_pointer()
                    .rounded_sm()
                    .px_1()
                    .hover(|s| s.bg(cx.theme().secondary))
                    .child(div().text_color(cx.theme().primary).child(b.title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} · {kind} ×{}", b.space, b.count)),
                    )
                    .on_click(cx.listener(move |_, ev: &ClickEvent, _, cx| {
                        if ev.modifiers().secondary() {
                            cx.emit(SheetEvent::Pin(id));
                        } else {
                            cx.emit(SheetEvent::Open(id));
                        }
                    })),
            );
        }
        section
    }
}

impl EventEmitter<SheetEvent> for EntitySheet {}

impl Render for EntitySheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.collapsed;
        let header = h_flex()
            .id("sheet-toggle")
            .w_full()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .cursor_pointer()
            .child(if collapsed { "▸" } else { "▾" })
            .child("SHEET")
            .child(div().flex_1())
            .child(self.template().label)
            .on_click(cx.listener(|this, _, _, cx| {
                this.collapsed = !this.collapsed;
                cx.notify();
            }));

        let mut sheet = v_flex()
            .w_full()
            .max_w(px(760.))
            .mx_auto()
            .px_4()
            .pt_2()
            .pb_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(header);
        if !collapsed {
            sheet = sheet
                .child(self.render_templates(cx))
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(Self::label("Aliases", cx))
                        .child(Input::new(&self.aliases).small()),
                )
                .children(self.render_fields(cx))
                .child(self.render_relations(cx))
                .child(self.render_attachments(cx))
                .child(self.render_appears_in(cx));
        }
        sheet
    }
}
