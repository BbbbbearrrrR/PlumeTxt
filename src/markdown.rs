use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::fmt::Write;

pub(crate) fn escape(out: &mut String, text: &str) {
    for unit in text.encode_utf16() {
        match unit {
            92 | 123 | 125 => {
                out.push('\\');
                out.push(char::from_u32(unit as u32).unwrap());
            }
            10 => out.push_str("\\line "),
            13 => (),
            9 => out.push_str("\\tab "),
            32..=126 => out.push(char::from_u32(unit as u32).unwrap()),
            _ => {
                let _ = write!(out, "\\u{}?", unit as i16);
            }
        }
    }
}

#[cfg(test)]
pub fn rtf(source: &str, table_width: usize) -> String {
    formatted(source, table_width, false, None)
}
#[cfg(test)]
pub fn preview(source: &str, table_width: usize) -> String {
    formatted(source, table_width, true, None)
}
pub fn with_images(
    source: &str,
    width: usize,
    dark: bool,
    base: Option<&std::path::Path>,
) -> String {
    formatted(source, width, dark, base)
}
fn formatted(
    source: &str,
    table_width: usize,
    dark: bool,
    base: Option<&std::path::Path>,
) -> String {
    let mut out = String::from("{\\rtf1\\ansi\\deff0\\uc1{\\fonttbl{\\f0 Segoe UI;}{\\f1 Consolas;}}{\\colortbl;\\red34\\green48\\blue64;\\red35\\green96\\blue154;}\\f0\\fs22\\cf1 ");
    if dark {
        out = out.replace(r"\fs22", r"\fs30").replace(
            r"\red34\green48\blue64;\red35\green96\blue154;",
            r"\red222\green232\blue233;\red63\green221\blue207;",
        );
    }
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut columns = 0;
    let mut image_budget = (16 * 1024 * 1024, 16 * 1024 * 1024);
    let mut image_rendered = false;
    for event in Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        if image_rendered {
            if matches!(event, Event::End(TagEnd::Image)) {
                image_rendered = false;
            }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("{\\pard\\sa140 "),
                Tag::Heading { level, .. } => {
                    let _ = write!(
                        out,
                        "{{\\pard\\sb200\\sa120\\b\\cf2\\fs{} ",
                        (40 - (level as u8 - 1) * 4).max(if dark { 30 } else { 20 })
                    );
                }
                Tag::Strong => out.push_str("{\\b "),
                Tag::Emphasis => out.push_str("{\\i "),
                Tag::Strikethrough => out.push_str("{\\strike "),
                Tag::CodeBlock(_) => out.push_str(if dark {
                    "{\\pard\\li280\\sa160\\f1\\fs28 "
                } else {
                    "{\\pard\\li280\\sa160\\f1\\fs20 "
                }),
                Tag::BlockQuote(_) => out.push_str("{\\li360\\i "),
                Tag::List(start) => lists.push(start),
                Tag::Item => {
                    let _ = write!(out, "{{\\pard\\li{}\\sa60 ", lists.len() * 240);
                    match lists.last_mut() {
                        Some(Some(n)) => {
                            let _ = write!(out, "{}. ", n);
                            *n += 1;
                        }
                        _ => out.push_str("\\u8226? "),
                    }
                }
                Tag::Link { .. } => out.push_str("{\\ul\\cf2 "),
                Tag::Image { dest_url, .. } => {
                    if let Some(picture) = base.and_then(|base| {
                        crate::assets::picture(
                            base,
                            &dest_url,
                            table_width,
                            &mut image_budget,
                            dark,
                        )
                    }) {
                        out.push_str(&picture);
                        image_rendered = true;
                        continue;
                    }
                    out.push('{');
                    escape(&mut out, "[Image: ");
                }
                Tag::Table(align) => {
                    columns = align.len();
                }
                Tag::TableHead | Tag::TableRow => {
                    out.push_str("{\\trowd\\trgaph80");
                    for c in 1..=columns {
                        let _ = write!(out, "\\cellx{}", c * table_width.max(240) / columns.max(1));
                    }
                    out.push(' ');
                }
                Tag::TableCell => out.push_str("\\intbl "),
                _ => (),
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item => {
                    out.push_str("\\par}")
                }
                TagEnd::Strong
                | TagEnd::Emphasis
                | TagEnd::Strikethrough
                | TagEnd::BlockQuote(_)
                | TagEnd::Link => out.push('}'),
                TagEnd::Image => {
                    escape(&mut out, "]");
                    out.push('}');
                }
                TagEnd::List(_) => {
                    lists.pop();
                }
                TagEnd::TableCell => out.push_str("\\cell "),
                TagEnd::TableHead | TagEnd::TableRow => out.push_str("\\row}"),
                TagEnd::Table => out.push_str("\\pard\\par "),
                _ => (),
            },
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                escape(&mut out, &text)
            }
            Event::Code(text) => {
                out.push_str("{\\f1 ");
                escape(&mut out, &text);
                out.push('}');
            }
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push_str("\\line "),
            Event::Rule => out.push_str("\\par ________________________________\\par "),
            Event::TaskListMarker(done) => out.push_str(if done { "[x] " } else { "[ ] " }),
            _ => (),
        }
    }
    out.push('}');
    out
}

#[test]
fn markdown_unicode_and_rtf_injection() {
    let screen = preview("Body\n\n```\ncode\n```", 9000);
    assert!(screen.contains(r"\fs30"));
    assert!(screen.contains(r"\fs28"));
    assert!(!screen.contains('\u{c}'));
    let doc = rtf("# 中文 😀\n\n**bold** `code`\n\n1. item\n\n| A | B |\n|---|---|\n| one | two |\n\n{\\rtf1 evil}", 9000);
    assert!(doc.contains("\\u20013?"));
    assert!(doc.contains("\\u-10179?\\u-8704?"));
    assert!(doc.contains("{\\b bold}"));
    assert!(doc.contains("\\cellx4500"));
    assert!(doc.contains("\\{\\\\rtf1 evil\\}"));
}
