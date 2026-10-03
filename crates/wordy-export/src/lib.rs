//! wordy-export: compile the manuscript and render it to docx, epub, PDF
//! (via typst), Markdown, a shareable PNG snippet, or a zip backup.
//!
//! Everything here is pure data in, bytes out; the app owns file dialogs
//! and writing to disk.

pub mod archive;
pub mod compile;
pub mod docx;
pub mod epub;
pub mod fonts;
pub mod markdown;
pub mod pdf;
pub mod snippet;

pub use compile::{compile, Chapter, CompileOptions, Compiled, Scene};
pub use snippet::{SnippetOptions, SnippetSize};

use anyhow::Result;

/// A manuscript output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Docx,
    Epub,
    Pdf,
    Markdown,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Docx, Format::Epub, Format::Pdf, Format::Markdown];

    pub fn label(self) -> &'static str {
        match self {
            Format::Docx => "Word (.docx)",
            Format::Epub => "EPUB",
            Format::Pdf => "PDF",
            Format::Markdown => "Markdown",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Docx => "docx",
            Format::Epub => "epub",
            Format::Pdf => "pdf",
            Format::Markdown => "md",
        }
    }
}

/// Render a compiled manuscript to `format`.
pub fn render(compiled: &Compiled, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Docx => docx::render(compiled),
        Format::Epub => epub::render(compiled),
        Format::Pdf => pdf::render(compiled),
        Format::Markdown => Ok(markdown::render(compiled).into_bytes()),
    }
}

/// A filesystem-safe file stem derived from a title.
pub fn file_stem(title: &str) -> String {
    let mut out = String::new();
    for ch in title.trim().chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() && !out.ends_with(' ') {
            out.push(' ');
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        "Manuscript".to_string()
    } else {
        out
    }
}
