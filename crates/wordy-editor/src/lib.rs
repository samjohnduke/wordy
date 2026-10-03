//! wordy-editor: the prose editor widget over a Loro rich-text body.
//!
//! `ProseEditor` is a gpui entity holding editing state (selection, scroll,
//! undo) for one `LoroText`; `ProseElement` is the custom element that lays
//! out, paints, and hit-tests it. Keyboard behavior is expressed as gpui
//! actions bound under the `ProseEditor` key context.

mod editor;
mod element;
pub mod ime;
pub mod spell;
mod style;
mod typography;

pub use editor::{
    CommentAnchor, CursorMarks, EditorEvent, LinkTarget, MentionSpan, ProseEditor, RichClipboard, RichFragment,
    Selection,
};
pub use element::ProseElement;
pub use spell::SpellState;
pub use style::EditorStyle;

use gpui_kit::{App, KeyBinding};

pub const KEY_CONTEXT: &str = "ProseEditor";

gpui_kit::actions!(
    prose_editor,
    [
        Backspace,
        Delete,
        DeleteWordBackward,
        DeleteWordForward,
        NewParagraph,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveWordLeft,
        MoveWordRight,
        MoveLineStart,
        MoveLineEnd,
        MoveDocStart,
        MoveDocEnd,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectLineStart,
        SelectLineEnd,
        SelectDocStart,
        SelectDocEnd,
        SelectAll,
        Undo,
        Redo,
        Copy,
        Cut,
        Paste,
        ToggleBold,
        ToggleItalic,
        ToggleUnderline,
        ToggleStrike,
        ToggleSmallCaps,
        ToggleHighlight,
        ClearFormatting,
        AddComment,
        InsertLink,
        RemoveLink,
        EditCommentAtCaret,
        ToggleResolvedComments,
        Cancel,
        SetParagraph,
        SetHeading1,
        SetHeading2,
        SetHeading3,
        SetQuote,
        InsertSceneBreak,
    ]
);

/// Register the editor's key bindings. Call once at startup.
pub fn init(cx: &mut App) {
    let c = Some(KEY_CONTEXT);
    let mut b = vec![
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("enter", NewParagraph, c),
        KeyBinding::new("shift-enter", NewParagraph, c),
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("home", MoveLineStart, c),
        KeyBinding::new("end", MoveLineEnd, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("secondary-z", Undo, c),
        KeyBinding::new("secondary-shift-z", Redo, c),
        KeyBinding::new("secondary-c", Copy, c),
        KeyBinding::new("secondary-x", Cut, c),
        KeyBinding::new("secondary-v", Paste, c),
        KeyBinding::new("secondary-b", ToggleBold, c),
        KeyBinding::new("secondary-i", ToggleItalic, c),
        KeyBinding::new("secondary-u", ToggleUnderline, c),
        KeyBinding::new("secondary-shift-x", ToggleStrike, c),
        KeyBinding::new("secondary-shift-k", ToggleSmallCaps, c),
        KeyBinding::new("secondary-shift-h", ToggleHighlight, c),
        KeyBinding::new("secondary-\\", ClearFormatting, c),
        KeyBinding::new("secondary-shift-m", AddComment, c),
        KeyBinding::new("secondary-shift-e", EditCommentAtCaret, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("secondary-k", InsertLink, c),
        KeyBinding::new("secondary-shift-l", RemoveLink, c),
        KeyBinding::new("secondary-alt-0", SetParagraph, c),
        KeyBinding::new("secondary-alt-1", SetHeading1, c),
        KeyBinding::new("secondary-alt-2", SetHeading2, c),
        KeyBinding::new("secondary-alt-3", SetHeading3, c),
        KeyBinding::new("secondary-shift-q", SetQuote, c),
        KeyBinding::new("secondary-shift-enter", InsertSceneBreak, c),
    ];
    if cfg!(target_os = "macos") {
        b.extend([
            KeyBinding::new("alt-left", MoveWordLeft, c),
            KeyBinding::new("alt-right", MoveWordRight, c),
            KeyBinding::new("alt-shift-left", SelectWordLeft, c),
            KeyBinding::new("alt-shift-right", SelectWordRight, c),
            KeyBinding::new("alt-backspace", DeleteWordBackward, c),
            KeyBinding::new("alt-delete", DeleteWordForward, c),
            KeyBinding::new("cmd-left", MoveLineStart, c),
            KeyBinding::new("cmd-right", MoveLineEnd, c),
            KeyBinding::new("cmd-shift-left", SelectLineStart, c),
            KeyBinding::new("cmd-shift-right", SelectLineEnd, c),
            KeyBinding::new("cmd-up", MoveDocStart, c),
            KeyBinding::new("cmd-down", MoveDocEnd, c),
            KeyBinding::new("cmd-shift-up", SelectDocStart, c),
            KeyBinding::new("cmd-shift-down", SelectDocEnd, c),
        ]);
    } else {
        b.extend([
            KeyBinding::new("ctrl-left", MoveWordLeft, c),
            KeyBinding::new("ctrl-right", MoveWordRight, c),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
            KeyBinding::new("ctrl-backspace", DeleteWordBackward, c),
            KeyBinding::new("ctrl-delete", DeleteWordForward, c),
            KeyBinding::new("ctrl-home", MoveDocStart, c),
            KeyBinding::new("ctrl-end", MoveDocEnd, c),
            KeyBinding::new("ctrl-shift-home", SelectDocStart, c),
            KeyBinding::new("ctrl-shift-end", SelectDocEnd, c),
            KeyBinding::new("ctrl-y", Redo, c),
        ]);
    }
    cx.bind_keys(b);
}
