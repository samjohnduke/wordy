//! Editor tab: hosts one `ProseEditor` bound to a node's body, plus the
//! find/replace bar.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::loro::LoroText;
use wordy_doc::TreeID;
use wordy_editor::{EditorEvent, ProseEditor};

use crate::app::{CloseFind, Find, FindNext, FindPrev, Replace, SharedProject, EDITOR_PANEL_CONTEXT};

pub enum EditorPanelEvent {
    /// The body changed.
    Edited,
    /// This tab became the displayed one in its group.
    Activated,
    /// The tab was closed.
    Closed,
}

struct FindBar {
    find: Entity<InputState>,
    replace: Entity<InputState>,
    show_replace: bool,
    _subs: Vec<Subscription>,
}

pub struct EditorPanel {
    project: SharedProject,
    node: Option<TreeID>,
    editor: Option<Entity<ProseEditor>>,
    find: Option<FindBar>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EditorPanel {
    pub fn placeholder(project: SharedProject, cx: &mut Context<Self>) -> Self {
        Self { project, node: None, editor: None, find: None, focus: cx.focus_handle(), _subs: Vec::new() }
    }

    pub fn open(project: SharedProject, id: TreeID, body: LoroText, cx: &mut Context<Self>) -> Self {
        let doc = project.project.doc.clone();
        let editor = cx.new(|cx| ProseEditor::new(doc, body, cx));
        editor.update(cx, |e, _| e.set_comments(project.project.comments(), id.to_string()));
        let sub = cx.subscribe(&editor, |_, _, ev: &EditorEvent, cx| {
            if matches!(ev, EditorEvent::Edited) {
                cx.emit(EditorPanelEvent::Edited);
            }
        });
        Self { project, node: Some(id), editor: Some(editor), find: None, focus: cx.focus_handle(), _subs: vec![sub] }
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

    pub fn focus_editor(&self, window: &mut Window, cx: &mut App) {
        if let Some(editor) = &self.editor {
            let focus = editor.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
    }

    // ----- find / replace --------------------------------------------------

    fn open_find(&mut self, with_replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else { return };
        if self.find.is_none() {
            let seed = {
                let e = editor.read(cx);
                let t = e.selected_text();
                if !t.is_empty() && !t.contains('\n') { t } else { String::new() }
            };
            let find = cx.new(|cx| InputState::new(window, cx).default_value(seed.clone()).placeholder("Find"));
            let replace = cx.new(|cx| InputState::new(window, cx).placeholder("Replace"));
            let ed = editor.clone();
            let s1 = cx.subscribe_in(&find, window, move |this, input, ev: &InputEvent, _window, cx| match ev {
                InputEvent::Change => {
                    let q = input.read(cx).value().to_string();
                    ed.update(cx, |e, cx| e.set_search(Some(q), cx));
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => {
                    this.step(if *shift { -1 } else { 1 }, cx);
                }
                _ => {}
            });
            let s2 = cx.subscribe_in(&replace, window, move |this, _, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.replace_current(cx);
                }
            });
            if !seed.is_empty() {
                editor.update(cx, |e, cx| e.set_search(Some(seed), cx));
            }
            self.find = Some(FindBar { find, replace, show_replace: with_replace, _subs: vec![s1, s2] });
        }
        let bar = self.find.as_mut().unwrap();
        bar.show_replace |= with_replace;
        let target = if with_replace { bar.replace.clone() } else { bar.find.clone() };
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
        self.find.as_ref().map(|b| b.replace.read(cx).value().to_string()).unwrap_or_default()
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
                div().w(px(260.)).child(Input::new(&bar.find).small()).into_any_element(),
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
                div().w(px(260.)).child(Input::new(&bar.replace).small()).into_any_element(),
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
                .child(div().flex_1().min_h_0().w_full().child(editor))
                .into_any_element(),
            None => div().size_full().child(
                v_flex()
                    .size_full()
                    .track_focus(&self.focus)
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().muted_foreground)
                    .child("Open a scene from the sidebar to start writing."),
            ).into_any_element(),
        }
    }
}
