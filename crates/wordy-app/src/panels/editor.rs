//! Editor tab: hosts one `ProseEditor` bound to a node's body.

use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use wordy_doc::loro::LoroText;
use wordy_doc::TreeID;
use wordy_editor::{EditorEvent, ProseEditor};

use crate::app::SharedProject;

pub enum EditorPanelEvent {
    /// The body changed.
    Edited,
    /// This tab became the displayed one in its group.
    Activated,
    /// The tab was closed.
    Closed,
}

pub struct EditorPanel {
    project: SharedProject,
    node: Option<TreeID>,
    editor: Option<Entity<ProseEditor>>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EditorPanel {
    pub fn placeholder(project: SharedProject, cx: &mut Context<Self>) -> Self {
        Self { project, node: None, editor: None, focus: cx.focus_handle(), _subs: Vec::new() }
    }

    pub fn open(project: SharedProject, id: TreeID, body: LoroText, cx: &mut Context<Self>) -> Self {
        let doc = project.project.doc.clone();
        let editor = cx.new(|cx| ProseEditor::new(doc, body, cx));
        let sub = cx.subscribe(&editor, |_, _, ev: &EditorEvent, cx| {
            if matches!(ev, EditorEvent::Edited) {
                cx.emit(EditorPanelEvent::Edited);
            }
        });
        Self { project, node: Some(id), editor: Some(editor), focus: cx.focus_handle(), _subs: vec![sub] }
    }

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
        match &self.editor {
            Some(editor) => div().size_full().bg(cx.theme().background).child(editor.clone()),
            None => div().size_full().child(
                v_flex()
                    .size_full()
                    .track_focus(&self.focus)
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().muted_foreground)
                    .child("Open a scene from the sidebar to start writing."),
            ),
        }
    }
}
