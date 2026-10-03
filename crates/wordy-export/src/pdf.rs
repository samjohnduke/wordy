//! PDF through typst, compiled in-process. The template is a string; the
//! `World` serves exactly one source file and the bundled fonts.

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use typst::diag::{FileError, FileResult, Warned};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt as _, World};
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;
use wordy_doc::chrono::Datelike;
use wordy_doc::{Block, Marks};

use crate::compile::Compiled;
use crate::fonts;

/// Bundled fonts are parsed once per process.
fn fonts() -> &'static (LazyHash<FontBook>, Vec<Font>) {
    static FONTS: OnceLock<(LazyHash<FontBook>, Vec<Font>)> = OnceLock::new();
    FONTS.get_or_init(|| {
        let fonts: Vec<Font> = fonts::ALL
            .iter()
            .filter_map(|data| Font::new(Bytes::new(*data), 0))
            .collect();
        (LazyHash::new(FontBook::from_fonts(&fonts)), fonts)
    })
}

fn library() -> &'static LazyHash<Library> {
    static LIB: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIB.get_or_init(|| LazyHash::new(Library::default()))
}

struct ExportWorld {
    main: Source,
}

impl World for ExportWorld {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }
    fn book(&self) -> &LazyHash<FontBook> {
        &fonts().0
    }
    fn main(&self) -> FileId {
        self.main.id()
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::NotFound(PathBuf::from(format!("{id:?}"))))
        }
    }
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(PathBuf::from(format!("{id:?}"))))
    }
    fn font(&self, index: usize) -> Option<Font> {
        fonts().1.get(index).cloned()
    }
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        let d = wordy_doc::chrono::Local::now().date_naive();
        Datetime::from_ymd(d.year(), d.month() as u8, d.day() as u8)
    }
}

pub fn render(c: &Compiled) -> Result<Vec<u8>> {
    let markup = typst_source(c);
    compile_typst(&markup)
}

/// Compile arbitrary typst markup with the bundled fonts.
pub fn compile_typst(markup: &str) -> Result<Vec<u8>> {
    let vpath = VirtualPath::new("main.typ").map_err(|e| anyhow!("typst path: {e}"))?;
    let id = FileId::new(RootedPath::new(VirtualRoot::Project, vpath));
    let world = ExportWorld {
        main: Source::new(id, markup.to_string()),
    };
    let Warned { output, warnings } = typst::compile::<PagedDocument>(&world);
    for w in &warnings {
        tracing::debug!("typst warning: {}", w.message);
    }
    let doc = output.map_err(|errs| {
        let msgs: Vec<String> = errs.iter().map(|e| e.message.to_string()).collect();
        anyhow!("typst: {}", msgs.join("; "))
    })?;
    typst_pdf::pdf(&doc, &PdfOptions::default()).map_err(|errs| {
        let msgs: Vec<String> = errs.iter().map(|e| e.message.to_string()).collect();
        anyhow!("pdf: {}", msgs.join("; "))
    })
}

/// The whole manuscript as typst markup. Prose goes through string literals
/// so nothing in the text is ever parsed as markup.
pub fn typst_source(c: &Compiled) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "#set document(title: {t}{a})\n",
        t = lit(&c.title),
        a = if c.author.is_empty() {
            String::new()
        } else {
            format!(", author: {}", lit(&c.author))
        }
    ));
    s.push_str(&format!(
        r##"#set page(paper: "a5", margin: (x: 18mm, top: 20mm, bottom: 22mm), numbering: "1",
  header: context {{ if counter(page).get().first() > 1 {{ align(center, text(size: 8.5pt, fill: luma(45%), smallcaps(lower({title})))) }} }})
#set text(font: "{font}", size: 11pt, lang: "en")
#set par(justify: true, first-line-indent: (amount: 1.4em, all: false), leading: 0.72em, spacing: 0.72em)
#show heading.where(level: 1): it => {{ pagebreak(weak: true); v(22%); align(center, text(size: 20pt, weight: "regular", it.body)); v(2.2em) }}
#show heading.where(level: 2): it => {{ v(1em); align(center, text(size: 12pt, style: "italic", weight: "regular", it.body)); v(0.6em) }}
#show heading.where(level: 3): it => {{ v(0.8em); text(size: 11pt, weight: "bold", it.body); v(0.3em) }}
#show heading.where(level: 4): it => {{ v(0.6em); text(size: 11pt, style: "italic", weight: "regular", it.body); v(0.2em) }}
#let sep = align(center, block(above: 1.3em, below: 1.3em, text({sep})))
#let bq(body) = block(inset: (x: 2em), above: 1em, below: 1em, body)

#page(numbering: none, header: none)[
  #v(30%)
  #align(center)[
    #text(size: 26pt, {title})
    {author}
  ]
]
#counter(page).update(1)

"##,
        title = lit(&c.title),
        font = fonts::FAMILY,
        sep = lit(&c.separator),
        author = if c.author.is_empty() { String::new() } else { format!("#v(1.6em)\n    #text(size: 13pt, {})", lit(&c.author)) },
    ));

    for chapter in &c.chapters {
        s.push_str(&format!("#heading(level: 1, {})\n\n", lit(&chapter.title)));
        for (i, scene) in chapter.scenes.iter().enumerate() {
            if c.scene_titles && (chapter.scenes.len() > 1 || scene.title != chapter.title) {
                s.push_str(&format!("#heading(level: 2, {})\n\n", lit(&scene.title)));
            } else if i > 0 {
                s.push_str("#sep\n\n");
            }
            for p in &scene.paragraphs {
                let inner: String = p.runs.iter().map(|r| styled(&r.text, &r.marks)).collect();
                match p.block {
                    Block::Paragraph => {
                        s.push_str(&inner);
                        s.push_str("\n\n");
                    }
                    Block::H1 => s.push_str(&format!("#heading(level: 3)[{inner}]\n\n")),
                    Block::H2 | Block::H3 => {
                        s.push_str(&format!("#heading(level: 4)[{inner}]\n\n"))
                    }
                    Block::Quote => s.push_str(&format!("#bq[{inner}]\n\n")),
                    Block::Break => s.push_str("#sep\n\n"),
                }
            }
        }
    }
    s
}

/// A typst string literal.
fn lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One run as a code-mode expression. Runs are emitted back to back with no
/// whitespace between them so spacing is exactly the prose's own.
fn styled(text: &str, m: &Marks) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut args = Vec::new();
    if m.bold {
        args.push("weight: \"bold\"");
    }
    if m.italic {
        args.push("style: \"italic\"");
    }
    let mut expr = if args.is_empty() {
        format!("text({})", lit(text))
    } else {
        format!("text({}, {})", args.join(", "), lit(text))
    };
    if m.smallcaps {
        expr = format!("smallcaps({expr})");
    }
    if m.underline {
        expr = format!("underline({expr})");
    }
    if m.strike {
        expr = format!("strike({expr})");
    }
    if m.highlight {
        expr = format!("highlight({expr})");
    }
    format!("#{expr}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_escapes() {
        assert_eq!(lit(r#"a "b" \ c"#), r#""a \"b\" \\ c""#);
    }

    #[test]
    fn runs_are_contiguous_expressions() {
        let m = Marks {
            bold: true,
            ..Default::default()
        };
        assert_eq!(styled("x", &m), r#"#text(weight: "bold", "x")"#);
        let m = Marks {
            italic: true,
            smallcaps: true,
            ..Default::default()
        };
        assert_eq!(styled("y", &m), r#"#smallcaps(text(style: "italic", "y"))"#);
    }

    #[test]
    fn pdf_compiles_with_bundled_fonts() {
        let bytes = render(&crate::docx::tests::sample()).expect("pdf");
        assert!(bytes.starts_with(b"%PDF-"), "not a pdf");
        assert!(bytes.len() > 10_000, "suspiciously small: {}", bytes.len());
    }

    #[test]
    fn markup_characters_in_prose_are_inert() {
        let mut c = crate::docx::tests::sample();
        c.chapters[0].scenes[0].paragraphs[0].runs[0].text =
            "*not bold* _not italic_ #not-code $x$ // no [brackets] \\ \"quotes\"".into();
        let bytes = render(&c).expect("pdf with hostile text");
        assert!(bytes.starts_with(b"%PDF-"));
    }
}
