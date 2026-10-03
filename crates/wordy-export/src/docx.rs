//! Standard manuscript format as .docx: 12pt serif, double spaced, one inch
//! margins, chapter headings a third of the way down a fresh page, `#`
//! between scenes, author / title / page in the header.

use std::io::Cursor;

use anyhow::Result;
use docx_rs::*;
use wordy_doc::{Block, Marks, Paragraph as DocParagraph};

use crate::compile::Compiled;

const FONT: &str = "Times New Roman";
/// Half-points.
const SIZE: usize = 24;
/// 240ths of a line: 480 = double.
const DOUBLE: i32 = 480;
const INCH: i32 = 1440;

pub fn render(c: &Compiled) -> Result<Vec<u8>> {
    let mut docx = Docx::new()
        .default_fonts(RunFonts::new().ascii(FONT).hi_ansi(FONT).cs(FONT))
        .default_size(SIZE)
        .default_line_spacing(
            LineSpacing::new()
                .line(DOUBLE)
                .line_rule(LineSpacingType::Auto),
        )
        .page_size(12240, 15840)
        .page_margin(
            PageMargin::new()
                .top(INCH)
                .bottom(INCH)
                .left(INCH)
                .right(INCH)
                .header(720)
                .footer(720),
        )
        .add_style(
            Style::new("ChapterTitle", StyleType::Paragraph)
                .name("Chapter Title")
                .based_on("Normal")
                .align(AlignmentType::Center)
                .outline_lvl(0)
                .bold(),
        )
        .header(header(c));

    // Title page.
    for _ in 0..6 {
        docx = docx.add_paragraph(Paragraph::new());
    }
    docx = docx.add_paragraph(centered(&c.title.to_uppercase(), true));
    if !c.author.is_empty() {
        docx = docx
            .add_paragraph(centered("by", false))
            .add_paragraph(centered(&c.author, false));
    }
    docx = docx.add_paragraph(Paragraph::new()).add_paragraph(centered(
        &format!("About {} words", round_words(c.word_count())),
        false,
    ));

    for chapter in &c.chapters {
        // Chapter title about a third of the way down a fresh page. Empty
        // paragraphs rather than space-before: Word and LibreOffice both drop
        // space-before at the top of a page.
        docx = docx.add_paragraph(Paragraph::new().page_break_before(true));
        for _ in 0..3 {
            docx = docx.add_paragraph(Paragraph::new());
        }
        docx = docx.add_paragraph(
            Paragraph::new()
                .style("ChapterTitle")
                .keep_next(true)
                .line_spacing(
                    LineSpacing::new()
                        .after(DOUBLE as u32)
                        .line(DOUBLE)
                        .line_rule(LineSpacingType::Auto),
                )
                .add_run(Run::new().add_text(&chapter.title).bold()),
        );
        for (i, scene) in chapter.scenes.iter().enumerate() {
            if c.scene_titles && (chapter.scenes.len() > 1 || scene.title != chapter.title) {
                docx = docx.add_paragraph(centered(&scene.title, true).keep_next(true));
            } else if i > 0 {
                docx = docx.add_paragraph(centered(&c.separator, false));
            }
            for p in &scene.paragraphs {
                docx = docx.add_paragraph(body_paragraph(p, &c.separator));
            }
        }
    }

    let mut buf = Cursor::new(Vec::new());
    docx.build().pack(&mut buf)?;
    Ok(buf.into_inner())
}

fn header(c: &Compiled) -> Header {
    let mut lead = String::new();
    if !c.author.is_empty() {
        let surname = c.author.rsplit(' ').next().unwrap_or(&c.author);
        lead.push_str(surname);
        lead.push_str(" / ");
    }
    lead.push_str(&short_title(&c.title));
    lead.push_str(" / ");
    Header::new().add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Right)
            .line_spacing(
                LineSpacing::new()
                    .line(240)
                    .line_rule(LineSpacingType::Auto),
            )
            .add_run(Run::new().add_text(lead))
            .add_run(
                Run::new()
                    .add_field_char(FieldCharType::Begin, false)
                    .add_instr_text(InstrText::PAGE(InstrPAGE::new()))
                    .add_field_char(FieldCharType::Separate, false)
                    .add_text("1")
                    .add_field_char(FieldCharType::End, false),
            ),
    )
}

fn short_title(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.len() <= 3 {
        title.to_uppercase()
    } else {
        words[..3].join(" ").to_uppercase()
    }
}

fn round_words(n: usize) -> usize {
    if n < 1000 {
        (n + 50) / 100 * 100
    } else {
        (n + 250) / 500 * 500
    }
}

fn centered(text: &str, bold: bool) -> Paragraph {
    let mut run = Run::new().add_text(text);
    if bold {
        run = run.bold();
    }
    Paragraph::new().align(AlignmentType::Center).add_run(run)
}

fn body_paragraph(p: &DocParagraph, separator: &str) -> Paragraph {
    let mut para = Paragraph::new();
    match p.block {
        Block::Break => return centered(separator, false),
        Block::Paragraph => {
            para = para.indent(
                None,
                Some(SpecialIndentType::FirstLine(INCH / 2)),
                None,
                None,
            );
        }
        Block::Quote => {
            para = para.indent(Some(INCH / 2), None, Some(INCH / 2), None);
        }
        Block::H1 => para = para.align(AlignmentType::Center).keep_next(true),
        Block::H2 | Block::H3 => para = para.keep_next(true),
    }
    for run in &p.runs {
        let mut marks = run.marks.clone();
        match p.block {
            Block::H1 | Block::H2 => marks.bold = true,
            Block::H3 => marks.italic = true,
            _ => {}
        }
        para = para.add_run(styled_run(&run.text, &marks));
    }
    para
}

fn styled_run(text: &str, m: &Marks) -> Run {
    let mut run = Run::new().add_text(text);
    if m.bold {
        run = run.bold();
    }
    if m.italic {
        run = run.italic();
    }
    if m.underline {
        run = run.underline("single");
    }
    if m.strike {
        run = run.strike();
    }
    if m.highlight {
        run = run.highlight("yellow");
    }
    if m.smallcaps {
        run.run_property = run.run_property.caps();
    }
    run
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::compile::{Chapter, Scene};
    use wordy_doc::Run as DocRun;

    pub(crate) fn sample() -> Compiled {
        let para = |text: &str, block: Block| DocParagraph {
            block,
            runs: vec![
                DocRun {
                    text: text.to_string(),
                    marks: Marks::default(),
                },
                DocRun {
                    text: " loud".to_string(),
                    marks: Marks {
                        bold: true,
                        italic: true,
                        ..Default::default()
                    },
                },
            ],
            text: format!("{text} loud"),
            start_cp: 0,
            len_cp: 0,
            terminated: true,
        };
        Compiled {
            title: "The Lighthouse & the Sea".into(),
            author: "Ada Example".into(),
            scene_titles: false,
            separator: "#".into(),
            chapters: vec![
                Chapter {
                    title: "One".into(),
                    scenes: vec![
                        Scene {
                            title: "Dawn".into(),
                            paragraphs: vec![
                                para("Hello <world>", Block::Paragraph),
                                para("Quoted", Block::Quote),
                            ],
                        },
                        Scene {
                            title: "Dusk".into(),
                            paragraphs: vec![para("Again", Block::Paragraph)],
                        },
                    ],
                },
                Chapter {
                    title: "Two".into(),
                    scenes: vec![Scene {
                        title: "End".into(),
                        paragraphs: vec![para("Fin", Block::H1)],
                    }],
                },
            ],
        }
    }

    #[test]
    fn docx_is_a_zip_with_document_xml() {
        let bytes = render(&sample()).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "word/document.xml"), "{names:?}");
        let mut doc = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("word/document.xml").unwrap(), &mut doc)
            .unwrap();
        assert!(doc.contains("Hello &lt;world&gt;"));
        assert!(doc.contains("<w:b />") || doc.contains("<w:b/>"));
        assert!(doc.contains("THE LIGHTHOUSE &amp; THE SEA"));
    }
}
