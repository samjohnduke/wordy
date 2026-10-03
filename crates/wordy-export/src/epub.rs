//! EPUB 3: a title page, an inline table of contents, and one XHTML file per
//! chapter with a small stylesheet for the inline marks.

use std::fmt::Write as _;

use anyhow::{anyhow, Result};
use epub_builder::{EpubBuilder, EpubContent, EpubVersion, ReferenceType, ZipLibrary};
use wordy_doc::{Block, Marks};

use crate::compile::{Chapter, Compiled};

const CSS: &str = r#"
body { font-family: Georgia, "Libertinus Serif", serif; line-height: 1.5; margin: 1em; }
h1 { text-align: center; font-weight: normal; font-size: 1.7em; margin: 3em 0 2em; }
h2 { text-align: center; font-weight: normal; font-style: italic; font-size: 1.1em; margin: 1.6em 0 0.8em; }
h3 { font-size: 1.05em; margin: 1.2em 0 0.4em; }
h4 { font-size: 1em; font-style: italic; font-weight: normal; margin: 1em 0 0.3em; }
p { margin: 0; text-indent: 1.4em; text-align: justify; }
h1 + p, h2 + p, h3 + p, h4 + p, p.sep + p, p.first, blockquote + p { text-indent: 0; }
p.sep { text-align: center; text-indent: 0; margin: 1.2em 0; }
blockquote { margin: 1em 2em; }
blockquote p { text-indent: 0; }
.u { text-decoration: underline; }
.sc { font-variant: small-caps; }
mark { background: #fff2a8; color: inherit; }
.title { text-align: center; margin-top: 30%; }
.title h1 { font-size: 2em; margin: 0 0 0.5em; }
.title p { text-indent: 0; text-align: center; font-size: 1.1em; }
"#;

pub fn render(c: &Compiled) -> Result<Vec<u8>> {
    let err = |e: epub_builder::Error| anyhow!("epub: {e}");
    let mut book = EpubBuilder::new(ZipLibrary::new().map_err(err)?).map_err(err)?;
    book.epub_version(EpubVersion::V30);
    book.set_title(&c.title);
    if !c.author.is_empty() {
        book.add_author(&c.author);
    }
    book.add_language("en");
    book.set_generator("Wordy");
    book.stylesheet(CSS.as_bytes()).map_err(err)?;

    let title_page = xhtml(&c.title, &title_body(c));
    book.add_content(
        EpubContent::new("title.xhtml", title_page.as_bytes())
            .title("Title Page")
            .reftype(ReferenceType::TitlePage),
    )
    .map_err(err)?;
    book.inline_toc();

    for (i, chapter) in c.chapters.iter().enumerate() {
        let href = format!("chapter-{:03}.xhtml", i + 1);
        let page = xhtml(&chapter.title, &chapter_body(c, chapter));
        book.add_content(
            EpubContent::new(href, page.as_bytes())
                .title(&chapter.title)
                .reftype(ReferenceType::Text),
        )
        .map_err(err)?;
    }

    let mut out = Vec::new();
    book.generate(&mut out).map_err(err)?;
    Ok(out)
}

fn title_body(c: &Compiled) -> String {
    let mut s = format!("<div class=\"title\"><h1>{}</h1>", esc(&c.title));
    if !c.author.is_empty() {
        let _ = write!(s, "<p>{}</p>", esc(&c.author));
    }
    s.push_str("</div>");
    s
}

fn chapter_body(c: &Compiled, chapter: &Chapter) -> String {
    let mut s = format!("<h1>{}</h1>\n", esc(&chapter.title));
    for (i, scene) in chapter.scenes.iter().enumerate() {
        if c.scene_titles && (chapter.scenes.len() > 1 || scene.title != chapter.title) {
            let _ = writeln!(s, "<h2>{}</h2>", esc(&scene.title));
        } else if i > 0 {
            let _ = writeln!(s, "<p class=\"sep\">{}</p>", esc(&c.separator));
        }
        for p in &scene.paragraphs {
            let inner: String = p.runs.iter().map(|r| styled(&esc(&r.text), &r.marks)).collect();
            match p.block {
                Block::Paragraph => {
                    let _ = writeln!(s, "<p>{inner}</p>");
                }
                Block::H1 => {
                    let _ = writeln!(s, "<h3>{inner}</h3>");
                }
                Block::H2 | Block::H3 => {
                    let _ = writeln!(s, "<h4>{inner}</h4>");
                }
                Block::Quote => {
                    let _ = writeln!(s, "<blockquote><p>{inner}</p></blockquote>");
                }
                Block::Break => {
                    let _ = writeln!(s, "<p class=\"sep\">{}</p>", esc(&c.separator));
                }
            }
        }
    }
    s
}

fn styled(text: &str, m: &Marks) -> String {
    let mut s = text.to_string();
    if m.smallcaps {
        s = format!("<span class=\"sc\">{s}</span>");
    }
    if m.underline {
        s = format!("<span class=\"u\">{s}</span>");
    }
    if m.strike {
        s = format!("<s>{s}</s>");
    }
    if m.highlight {
        s = format!("<mark>{s}</mark>");
    }
    if m.italic {
        s = format!("<em>{s}</em>");
    }
    if m.bold {
        s = format!("<strong>{s}</strong>");
    }
    s
}

fn xhtml(title: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head>
<meta charset="utf-8"/>
<title>{}</title>
<link rel="stylesheet" type="text/css" href="stylesheet.css"/>
</head>
<body>
{}
</body>
</html>
"#,
        esc(title),
        body
    )
}

pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn epub_has_mimetype_and_chapters() {
        let bytes = render(&crate::docx::tests::sample()).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut mt = String::new();
        zip.by_name("mimetype").unwrap().read_to_string(&mut mt).unwrap();
        assert_eq!(mt, "application/epub+zip");
        let mut ch = String::new();
        zip.by_name("OEBPS/chapter-001.xhtml")
            .unwrap()
            .read_to_string(&mut ch)
            .unwrap();
        assert!(ch.contains("<h1>One</h1>"));
        assert!(ch.contains("Hello &lt;world&gt;<strong><em> loud</em></strong>"));
        assert!(ch.contains("<p class=\"sep\">#</p>"), "scene separator between scenes");
        assert!(zip.by_name("OEBPS/chapter-002.xhtml").is_ok());
    }
}
