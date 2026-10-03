//! Editor tab: hosts one `ProseEditor` bound to a node's body, plus the
//! find/replace bar.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::loro::LoroText;
use wordy_doc::{storage, NodeKind, Status, TreeID, Version};
use wordy_editor::{EditorEvent, LinkTarget, ProseEditor};
use wordy_export::{SnippetOptions, SnippetSize};

use crate::app::{CloseFind, Find, FindNext, FindPrev, Replace, SharedProject, EDITOR_PANEL_CONTEXT};
use crate::panels::sheet::{EntitySheet, SheetEvent};

pub enum EditorPanelEvent {
    /// The body changed.
    Edited,
    /// This tab became the displayed one in its group.
    Activated,
    /// The tab was closed.
    Closed,
    /// Entity names changed (aliases); the matcher and index must refresh.
    NamesChanged,
    /// Follow a link: show `id` in the reference pane, or open it when `navigate`.
    OpenLink { id: TreeID, navigate: bool },
    /// A word was added to the custom dictionary.
    DictionaryChanged(String),
    /// Status, goal, tags, or versions changed (metadata, not the body).
    MetaChanged,
    /// Show a saved version read-only in the reference pane.
    ViewVersion(Version),
}

struct FindBar {
    find: Entity<InputState>,
    replace: Entity<InputState>,
    show_replace: bool,
    _subs: Vec<Subscription>,
}

/// The status / goal / tags / versions strip above a scene or note.
struct MetaBar {
    goal: Entity<InputState>,
    tag: Entity<InputState>,
    /// Inline label input while saving a version.
    version_label: Option<(Entity<InputState>, Subscription)>,
    _subs: Vec<Subscription>,
}

pub struct EditorPanel {
    project: SharedProject,
    node: Option<TreeID>,
    editor: Option<Entity<ProseEditor>>,
    sheet: Option<Entity<EntitySheet>>,
    meta: Option<MetaBar>,
    find: Option<FindBar>,
    /// Transient message in the meta bar (e.g. "Snippet copied").
    notice: Option<String>,
    _notice_task: Option<Task<()>>,
    /// Focus mode hides the meta bar and dims paragraphs away from the caret.
    focus_mode: bool,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EditorPanel {
    pub fn placeholder(project: SharedProject, cx: &mut Context<Self>) -> Self {
        Self {
            project,
            node: None,
            editor: None,
            sheet: None,
            meta: None,
            find: None,
            notice: None,
            _notice_task: None,
            focus_mode: false,
            focus: cx.focus_handle(),
            _subs: Vec::new(),
        }
    }

    pub fn open(
        project: SharedProject,
        id: TreeID,
        body: LoroText,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let doc = project.project.doc.clone();
        let editor = cx.new(|cx| ProseEditor::new(doc, body, cx));
        editor.update(cx, |e, cx| {
            e.set_comments(project.project.comments(), id.to_string());
            e.set_self_id(Some(id));
            e.set_link_targets(project.link_targets(), cx);
        });
        let mut subs = vec![cx.subscribe(&editor, |_, _, ev: &EditorEvent, cx| match ev {
            EditorEvent::Edited => cx.emit(EditorPanelEvent::Edited),
            EditorEvent::OpenLink { id, navigate } => cx.emit(EditorPanelEvent::OpenLink {
                id: *id,
                navigate: *navigate,
            }),
            EditorEvent::DictionaryChanged(w) => cx.emit(EditorPanelEvent::DictionaryChanged(w.clone())),
            EditorEvent::SelectionChanged => {}
        })];
        let is_entity = project
            .project
            .node(id)
            .map(|n| n.kind() == NodeKind::Entity)
            .unwrap_or(false);
        let sheet = is_entity.then(|| {
            let sheet = cx.new(|cx| EntitySheet::new(project.clone(), id, window, cx));
            subs.push(cx.subscribe(&sheet, |_, _, ev: &SheetEvent, cx| match ev {
                SheetEvent::Changed => cx.emit(EditorPanelEvent::Edited),
                SheetEvent::NamesChanged => cx.emit(EditorPanelEvent::NamesChanged),
                SheetEvent::Open(id) => cx.emit(EditorPanelEvent::OpenLink {
                    id: *id,
                    navigate: true,
                }),
                SheetEvent::Pin(id) => cx.emit(EditorPanelEvent::OpenLink {
                    id: *id,
                    navigate: false,
                }),
            }));
            sheet
        });
        let meta = (!is_entity).then(|| {
            let node = project.project.node(id).ok();
            let goal_seed = node
                .as_ref()
                .and_then(|n| n.word_goal())
                .map(|g| g.to_string())
                .unwrap_or_default();
            let goal = cx.new(|cx| InputState::new(window, cx).default_value(goal_seed).placeholder("goal"));
            let tag = cx.new(|cx| InputState::new(window, cx).placeholder("+ tag"));
            let s1 = cx.subscribe_in(&goal, window, move |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    let v = input.read(cx).value().trim().to_string();
                    let parsed = if v.is_empty() { None } else { v.parse::<i64>().ok() };
                    if !v.is_empty() && parsed.is_none() {
                        return;
                    }
                    if let Ok(node) = this.project.project.node(id) {
                        if node.word_goal() != parsed {
                            if let Err(e) = node.set_word_goal(parsed) {
                                tracing::error!("word goal: {e:#}");
                            }
                            this.meta_changed(cx);
                        }
                    }
                }
            });
            let s2 = cx.subscribe_in(&tag, window, move |this, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let v = input.read(cx).value().trim().trim_start_matches('#').to_lowercase();
                    if !v.is_empty() {
                        if let Ok(node) = this.project.project.node(id) {
                            if let Err(e) = node.add_tag(&v) {
                                tracing::error!("add tag: {e:#}");
                            }
                        }
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                        this.meta_changed(cx);
                    }
                }
            });
            MetaBar {
                goal,
                tag,
                version_label: None,
                _subs: vec![s1, s2],
            }
        });
        Self {
            project,
            node: Some(id),
            editor: Some(editor),
            sheet,
            meta,
            find: None,
            notice: None,
            _notice_task: None,
            focus_mode: false,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    /// Re-run the spellchecker (the custom dictionary changed).
    pub fn rescan_spelling(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.rescan_spelling(cx));
        }
    }

    /// Select `len` code points at `offset` and scroll them into view.
    pub fn reveal(&mut self, offset: usize, len: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.select_range(offset..offset + len, cx));
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    // ----- meta bar --------------------------------------------------------

    fn meta_changed(&mut self, cx: &mut Context<Self>) {
        self.project.project.commit_meta();
        cx.emit(EditorPanelEvent::MetaChanged);
        cx.notify();
    }

    fn set_status(&mut self, status: Status, cx: &mut Context<Self>) {
        if let Some(node) = self.node.and_then(|id| self.project.project.node(id).ok()) {
            if let Err(e) = node.set_status(status) {
                tracing::error!("status: {e:#}");
            }
        }
        self.meta_changed(cx);
    }

    fn remove_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        if let Some(node) = self.node.and_then(|id| self.project.project.node(id).ok()) {
            if let Err(e) = node.remove_tag(tag) {
                tracing::error!("remove tag: {e:#}");
            }
        }
        self.meta_changed(cx);
    }

    /// Show `text` in the meta bar for a few seconds.
    fn notify_user(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        cx.notify();
        self._notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
            this.update(cx, |t, cx| {
                t.notice = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// The selection, or the paragraph under the caret when nothing is selected.
    fn snippet_text(&self, cx: &App) -> Option<String> {
        let editor = self.editor.as_ref()?.read(cx);
        let selected = editor.selected_text();
        if !selected.trim().is_empty() {
            return Some(selected);
        }
        let paras = editor.paragraphs();
        let pos = paras.locate(editor.selection().head);
        let text = paras.get(pos.para)?.text.clone();
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Render the selection (or caret paragraph) as a PNG card, copy it to
    /// the clipboard and save it under the project's `exports/` folder.
    fn make_snippet(&mut self, size: SnippetSize, dark: bool, cx: &mut Context<Self>) {
        let Some(text) = self.snippet_text(cx) else {
            self.notify_user("Select some text first", cx);
            return;
        };
        let settings = self.project.project.settings_map();
        let get = |k: &str| match settings.get(k) {
            Some(wordy_doc::loro::ValueOrContainer::Value(wordy_doc::loro::LoroValue::String(s))) if !s.is_empty() => {
                Some(s.to_string())
            }
            _ => None,
        };
        let title = get("export.title").unwrap_or_else(|| self.project.project.name());
        let attribution = match get("export.author") {
            Some(a) => format!("{title} · {a}"),
            None => title,
        };
        let opts = SnippetOptions {
            size,
            dark,
            attribution,
        };
        let out_dir = self.project.dir().map(|d| d.join("exports"));
        let stamp = wordy_doc::chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        self.notify_user("Rendering snippet…", cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let png = wordy_export::snippet::render(&text, &opts)?;
                    let mut saved = None;
                    if let Some(dir) = out_dir {
                        std::fs::create_dir_all(&dir)?;
                        let path = dir.join(format!("snippet-{stamp}.png"));
                        storage::write_atomic(&path, &png)?;
                        saved = Some(path);
                    }
                    Ok::<_, anyhow::Error>((png, saved))
                })
                .await;
            this.update(cx, |t, cx| match result {
                Ok((png, saved)) => {
                    copy_png(png, cx);
                    let msg = match saved {
                        Some(p) => format!(
                            "Snippet copied and saved to exports/{}",
                            p.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        None => "Snippet copied to clipboard".to_string(),
                    };
                    t.notify_user(msg, cx);
                }
                Err(e) => t.notify_user(format!("Snippet failed: {e:#}"), cx),
            })
            .ok();
        })
        .detach();
    }

    fn begin_save_version(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(meta) = self.meta.as_mut() else {
            return;
        };
        let seed = format!("Version {}", wordy_doc::chrono::Local::now().format("%b %-d %H:%M"));
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(seed)
                .placeholder("Version label")
        });
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let label = input.read(cx).value().trim().to_string();
                this.save_version(&label, cx);
            }
        });
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        meta.version_label = Some((input, sub));
        cx.notify();
    }

    fn save_version(&mut self, label: &str, cx: &mut Context<Self>) {
        let Some(id) = self.node else { return };
        let label = if label.is_empty() { "Version" } else { label };
        match self.project.project.save_version(id, label) {
            Ok(v) => tracing::info!("saved version {} ({})", v.label, v.id),
            Err(e) => tracing::error!("save version: {e:#}"),
        }
        if let Some(meta) = self.meta.as_mut() {
            meta.version_label = None;
        }
        self.meta_changed(cx);
    }

    fn cancel_save_version(&mut self, cx: &mut Context<Self>) {
        if let Some(meta) = self.meta.as_mut() {
            meta.version_label = None;
        }
        cx.notify();
    }

    /// The document changed underneath us (a sync imported edits): re-read
    /// the body and the sheet.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.reload(cx));
        }
        if let Some(sheet) = &self.sheet {
            sheet.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// Replace the body with a saved version, as an undoable edit of this editor.
    pub fn restore_version(&mut self, v: &Version, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        let origin = editor.read(cx).origin().to_string();
        if let Err(e) = self.project.project.restore_version(v, &origin) {
            tracing::error!("restore version: {e:#}");
            return;
        }
        editor.update(cx, |e, cx| e.reload(cx));
        cx.emit(EditorPanelEvent::Edited);
        cx.notify();
    }

    fn delete_version(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(e) = self.project.project.remove_version(id) {
            tracing::error!("remove version: {e:#}");
        }
        self.meta_changed(cx);
    }

    fn render_meta_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.focus_mode {
            return None;
        }
        let meta = self.meta.as_ref()?;
        let id = self.node?;
        let node = self.project.project.node(id).ok()?;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let is_scene = node.kind() == NodeKind::Scene;
        let weak = cx.weak_entity();

        let mut bar = h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_xs()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.secondary.opacity(0.4));

        if is_scene {
            let current = node.status();
            let w = weak.clone();
            bar = bar.child(
                Button::new("meta-status")
                    .ghost()
                    .xsmall()
                    .label(current.label())
                    .tooltip("Scene status")
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu;
                        for st in Status::ALL {
                            let w = w.clone();
                            menu = menu.item(PopupMenuItem::new(st.label()).checked(st == current).on_click(
                                move |_, _, cx| w.update(cx, |p, cx| p.set_status(st, cx)).ok().unwrap_or(()),
                            ));
                        }
                        menu
                    }),
            );
        }

        // Word goal
        let words = self.word_count(cx);
        let goal_txt = match node.word_goal() {
            Some(g) if g > 0 => format!("{words} / {g} · {}%", (words as i64 * 100 / g).min(999)),
            _ => format!("{words} words"),
        };
        bar = bar
            .child(div().text_color(muted).child(goal_txt))
            .child(div().w(px(64.)).child(Input::new(&meta.goal).xsmall()));

        // Tags
        let mut tags = h_flex().gap_1().items_center().flex_wrap();
        for (ix, tag) in node.tags().into_iter().enumerate() {
            let t = tag.clone();
            tags = tags.child(
                h_flex()
                    .id(ElementId::Name(format!("tag-{ix}").into()))
                    .items_center()
                    .gap_0p5()
                    .px_1p5()
                    .rounded_full()
                    .bg(theme.accent)
                    .text_color(theme.accent_foreground)
                    .child(format!("#{tag}"))
                    .child(
                        div()
                            .text_color(muted)
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.foreground))
                            .child("×"),
                    )
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_tag(&t, cx))),
            );
        }
        tags = tags.child(div().w(px(80.)).child(Input::new(&meta.tag).xsmall()));
        bar = bar.child(tags).child(div().flex_1());

        if let Some(notice) = &self.notice {
            bar = bar.child(div().text_color(muted).child(notice.clone()));
        }

        // Snippet image of the selection.
        let ws = weak.clone();
        bar = bar.child(
            Button::new("meta-snippet")
                .ghost()
                .xsmall()
                .label("Snippet")
                .tooltip("Copy the selected text as a shareable PNG card")
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (size, dark, label) in [
                        (SnippetSize::Square, false, "Square · light"),
                        (SnippetSize::Square, true, "Square · dark"),
                        (SnippetSize::Wide, false, "Wide · light"),
                        (SnippetSize::Wide, true, "Wide · dark"),
                    ] {
                        let w = ws.clone();
                        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            w.update(cx, |p, cx| p.make_snippet(size, dark, cx)).ok().unwrap_or(())
                        }));
                    }
                    menu
                }),
        );

        // Versions
        let versions = self.project.project.versions_for(id);
        let count = versions.len();
        let w = weak.clone();
        bar = bar.child(
            Button::new("meta-versions")
                .ghost()
                .xsmall()
                .label(if count == 0 {
                    "Versions".to_string()
                } else {
                    format!("Versions ({count})")
                })
                .tooltip("Saved versions of this body")
                .dropdown_menu(move |menu, window, cx| {
                    let w0 = w.clone();
                    let mut menu = menu.item(PopupMenuItem::new("Save version…").on_click(move |_, window, cx| {
                        w0.update(cx, |p, cx| p.begin_save_version(window, cx))
                            .ok()
                            .unwrap_or(())
                    }));
                    if !versions.is_empty() {
                        menu = menu.separator();
                    }
                    for v in versions.iter().cloned() {
                        let when = wordy_doc::chrono::DateTime::from_timestamp_millis(v.created)
                            .map(|d| {
                                d.with_timezone(&wordy_doc::chrono::Local)
                                    .format("%b %-d, %H:%M")
                                    .to_string()
                            })
                            .unwrap_or_default();
                        let label = format!("{} · {when}", v.label);
                        let w = w.clone();
                        menu = menu.submenu(label, window, cx, move |menu, _, _| {
                            let (v1, v2, v3) = (v.clone(), v.clone(), v.clone());
                            let (w1, w2, w3) = (w.clone(), w.clone(), w.clone());
                            menu.item(PopupMenuItem::new("View").on_click(move |_, _, cx| {
                                w1.update(cx, |_, cx| cx.emit(EditorPanelEvent::ViewVersion(v1.clone())))
                                    .ok()
                                    .unwrap_or(())
                            }))
                            .item(PopupMenuItem::new("Restore").on_click(move |_, _, cx| {
                                w2.update(cx, |p, cx| p.restore_version(&v2, cx)).ok().unwrap_or(())
                            }))
                            .separator()
                            .item(PopupMenuItem::new("Delete").on_click(move |_, _, cx| {
                                w3.update(cx, |p, cx| p.delete_version(&v3.id, cx)).ok().unwrap_or(())
                            }))
                        });
                    }
                    menu
                }),
        );

        let mut out = v_flex().w_full().child(bar);
        if let Some((input, _)) = &meta.version_label {
            out = out.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_color(muted).child("Save version as"))
                    .child(div().w(px(260.)).child(Input::new(input).small()))
                    .child(
                        Button::new("ver-save")
                            .primary()
                            .xsmall()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let label = this
                                    .meta
                                    .as_ref()
                                    .and_then(|m| m.version_label.as_ref())
                                    .map(|(i, _)| i.read(cx).value().trim().to_string())
                                    .unwrap_or_default();
                                this.save_version(&label, cx);
                            })),
                    )
                    .child(
                        Button::new("ver-cancel")
                            .ghost()
                            .xsmall()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_save_version(cx))),
                    ),
            );
        }
        Some(out.into_any_element())
    }

    /// Push the current entity list into the editor (names changed somewhere).
    pub fn set_link_targets(&mut self, targets: Vec<LinkTarget>, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.set_link_targets(targets, cx));
        }
        cx.notify();
    }

    #[allow(dead_code)]
    pub fn node(&self) -> Option<TreeID> {
        self.node
    }

    pub fn editor(&self) -> Option<&Entity<ProseEditor>> {
        self.editor.as_ref()
    }

    pub fn word_count(&self, cx: &App) -> usize {
        self.editor.as_ref().map(|e| e.read(cx).word_count()).unwrap_or(0)
    }

    pub fn title(&self) -> SharedString {
        match self.node.and_then(|id| self.project.project.node(id).ok()) {
            Some(n) => n.title().into(),
            None => "Welcome".into(),
        }
    }

    /// Focus mode: no meta bar, inactive paragraphs dimmed.
    pub fn set_focus_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        self.focus_mode = on;
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.set_dim_inactive(on, cx));
        }
        cx.notify();
    }

    /// Typewriter scrolling keeps the caret line vertically centred.
    pub fn set_typewriter(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.set_typewriter(on, cx));
        }
    }

    pub fn focus_editor(&self, window: &mut Window, cx: &mut App) {
        if let Some(editor) = &self.editor {
            let focus = editor.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
    }

    // ----- find / replace --------------------------------------------------

    fn open_find(&mut self, with_replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };
        if self.find.is_none() {
            let seed = {
                let e = editor.read(cx);
                let t = e.selected_text();
                if !t.is_empty() && !t.contains('\n') {
                    t
                } else {
                    String::new()
                }
            };
            let find = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(seed.clone())
                    .placeholder("Find")
            });
            let replace = cx.new(|cx| InputState::new(window, cx).placeholder("Replace"));
            let ed = editor.clone();
            let s1 = cx.subscribe_in(
                &find,
                window,
                move |this, input, ev: &InputEvent, _window, cx| match ev {
                    InputEvent::Change => {
                        let q = input.read(cx).value().to_string();
                        ed.update(cx, |e, cx| e.set_search(Some(q), cx));
                        cx.notify();
                    }
                    InputEvent::PressEnter { shift, .. } => {
                        this.step(if *shift { -1 } else { 1 }, cx);
                    }
                    _ => {}
                },
            );
            let s2 = cx.subscribe_in(&replace, window, move |this, _, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.replace_current(cx);
                }
            });
            if !seed.is_empty() {
                editor.update(cx, |e, cx| e.set_search(Some(seed), cx));
            }
            self.find = Some(FindBar {
                find,
                replace,
                show_replace: with_replace,
                _subs: vec![s1, s2],
            });
        }
        let bar = self.find.as_mut().unwrap();
        bar.show_replace |= with_replace;
        let target = if with_replace {
            bar.replace.clone()
        } else {
            bar.find.clone()
        };
        target.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            if let Some(editor) = &self.editor {
                editor.update(cx, |e, cx| e.set_search(None, cx));
            }
            self.focus_editor(window, cx);
            cx.notify();
        }
    }

    fn step(&mut self, dir: i32, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.search_step(dir, false, cx));
        }
        cx.notify();
    }

    fn replacement(&self, cx: &App) -> String {
        self.find
            .as_ref()
            .map(|b| b.replace.read(cx).value().to_string())
            .unwrap_or_default()
    }

    fn replace_current(&mut self, cx: &mut Context<Self>) {
        let with = self.replacement(cx);
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.replace_current(&with, cx));
        }
        cx.notify();
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let with = self.replacement(cx);
        if let Some(editor) = &self.editor {
            editor.update(cx, |e, cx| e.replace_all(&with, cx));
        }
        cx.notify();
    }

    fn render_find_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let bar = self.find.as_ref()?;
        let editor = self.editor.as_ref()?;
        let (ix, n) = editor.read(cx).search_status();
        let status = match (ix, n) {
            (_, 0) if editor.read(cx).search_query().is_none() => String::new(),
            (_, 0) => "No matches".to_string(),
            (Some(i), n) => format!("{} of {n}", i + 1),
            (None, n) => format!("{n} matches"),
        };
        let row = |children: Vec<AnyElement>| h_flex().gap_1().items_center().children(children);
        let mut rows = v_flex()
            .gap_1()
            .px_2()
            .py_1()
            .bg(cx.theme().secondary)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(row(vec![
                div()
                    .w(px(260.))
                    .child(Input::new(&bar.find).small())
                    .into_any_element(),
                div()
                    .w(px(90.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status)
                    .into_any_element(),
                Button::new("find-prev")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronUp)
                    .tooltip("Previous match")
                    .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx)))
                    .into_any_element(),
                Button::new("find-next")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .tooltip("Next match")
                    .on_click(cx.listener(|this, _, _, cx| this.step(1, cx)))
                    .into_any_element(),
                div().flex_1().into_any_element(),
                Button::new("find-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Close")
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx)))
                    .into_any_element(),
            ]));
        if bar.show_replace {
            rows = rows.child(row(vec![
                div()
                    .w(px(260.))
                    .child(Input::new(&bar.replace).small())
                    .into_any_element(),
                Button::new("replace-one")
                    .ghost()
                    .xsmall()
                    .label("Replace")
                    .on_click(cx.listener(|this, _, _, cx| this.replace_current(cx)))
                    .into_any_element(),
                Button::new("replace-all")
                    .ghost()
                    .xsmall()
                    .label("Replace all")
                    .on_click(cx.listener(|this, _, _, cx| this.replace_all(cx)))
                    .into_any_element(),
            ]));
        }
        Some(rows)
    }
}

impl BasePanel for EditorPanel {
    fn panel_name(&self) -> &'static str {
        "Editor"
    }
    fn closable(&self, _: &App) -> bool {
        true
    }
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            self.focus_editor(window, cx);
            cx.emit(EditorPanelEvent::Activated);
        }
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorPanelEvent::Closed);
    }
}

impl EventEmitter<PanelEvent> for EditorPanel {}
impl EventEmitter<EditorPanelEvent> for EditorPanel {}

impl Focusable for EditorPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.editor {
            Some(e) => e.read(cx).focus.clone(),
            None => self.focus.clone(),
        }
    }
}

impl Panel for EditorPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.title())
    }
}

impl Render for EditorPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.editor.clone() {
            Some(editor) => v_flex()
                .id("editor-panel")
                .key_context(EDITOR_PANEL_CONTEXT)
                .size_full()
                .bg(cx.theme().background)
                .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(false, window, cx)))
                .on_action(cx.listener(|this, _: &Replace, window, cx| this.open_find(true, window, cx)))
                .on_action(cx.listener(|this, _: &FindNext, _, cx| this.step(1, cx)))
                .on_action(cx.listener(|this, _: &FindPrev, _, cx| this.step(-1, cx)))
                .on_action(cx.listener(|this, _: &CloseFind, window, cx| this.close_find(window, cx)))
                .children(self.render_find_bar(cx))
                .children(self.render_meta_bar(cx))
                .children(self.sheet.clone())
                .child(div().flex_1().min_h_0().w_full().child(editor))
                .into_any_element(),
            None => div()
                .size_full()
                .child(
                    v_flex()
                        .size_full()
                        .track_focus(&self.focus)
                        .items_center()
                        .justify_center()
                        .text_color(cx.theme().muted_foreground)
                        .child("Open a scene from the sidebar to start writing."),
                )
                .into_any_element(),
        }
    }
}

/// Put a PNG on the system clipboard. gpui's Wayland/X11 backends only offer
/// text MIME types, so on Linux we hand the bytes to `wl-copy` or `xclip`.
fn copy_png(png: Vec<u8>, cx: &mut App) {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let candidates: &[(&str, &[&str])] = if wayland {
            &[
                ("wl-copy", &["--type", "image/png"]),
                ("xclip", &["-selection", "clipboard", "-t", "image/png"]),
            ]
        } else {
            &[
                ("xclip", &["-selection", "clipboard", "-t", "image/png"]),
                ("wl-copy", &["--type", "image/png"]),
            ]
        };
        for (bin, args) in candidates {
            let child = Command::new(bin)
                .args(*args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = child {
                let ok = child
                    .stdin
                    .take()
                    .map(|mut stdin| stdin.write_all(&png).is_ok())
                    .unwrap_or(false);
                // wl-copy forks and serves the selection; xclip likewise stays alive.
                let _ = child.wait();
                if ok {
                    return;
                }
            }
        }
        tracing::warn!("no wl-copy or xclip found; image left in gpui clipboard only");
    }
    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png)));
}
