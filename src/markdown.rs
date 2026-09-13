use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::fmt::Write;
use std::{borrow::Cow, collections::BTreeSet};

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
    formatted(source, table_width, false, None, None)
}
#[cfg(test)]
pub fn preview(source: &str, table_width: usize) -> String {
    formatted(source, table_width, true, None, None)
}
pub fn with_images(
    source: &str,
    width: usize,
    dark: bool,
    base: Option<&std::path::Path>,
) -> String {
    formatted(source, width, dark, base, None)
}
pub fn folding_preview(
    source: &str,
    width: usize,
    base: Option<&std::path::Path>,
    folded: &BTreeSet<usize>,
) -> String {
    formatted(
        &fold_source(source, folded),
        width,
        true,
        base,
        Some(folded),
    )
}
pub fn scroll_anchors(source: &str, folded: &BTreeSet<usize>) -> Vec<(i32, String)> {
    let view = fold_source(source, folded);
    let mut anchors = Vec::new();
    let (mut block, mut image, mut byte, mut cp, mut last) = (false, false, 0, 0, 0);
    // Keep anchors distributed through the document with bounded extra memory.
    let spacing = (source.len() / 8192).max(1);
    for (event, range) in Parser::new_ext(
        &view,
        Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_MATH,
    )
    .into_offset_iter()
    {
        match event {
            Event::Start(
                Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::Item
                | Tag::CodeBlock(_)
                | Tag::TableCell,
            ) => block = true,
            Event::Start(Tag::Image { .. }) => image = true,
            Event::End(TagEnd::Image) => image = false,
            Event::Text(text) | Event::Code(text) if block && !image => {
                block = false;
                if !anchors.is_empty() && range.start < last + spacing {
                    continue;
                }
                cp += source[byte..range.start].encode_utf16().count() as i32
                    - source[byte..range.start].matches("\r\n").count() as i32;
                byte = range.start;
                let snippet: String = text
                    .chars()
                    .take_while(|c| *c != '\r' && *c != '\n')
                    .take(48)
                    .collect();
                if !snippet.trim().is_empty() {
                    anchors.push((cp, snippet));
                    last = range.start;
                }
            }
            _ => (),
        }
    }
    anchors
}
fn fold_source<'a>(source: &'a str, folded: &BTreeSet<usize>) -> Cow<'a, str> {
    if folded.is_empty() {
        return Cow::Borrowed(source);
    }
    let headings: Vec<_> = Parser::new(source)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            if let Event::Start(Tag::Heading { level, .. }) = event {
                Some((range, level as u8))
            } else {
                None
            }
        })
        .collect();
    let mut bytes = source.as_bytes().to_vec();
    for (i, (range, level)) in headings.iter().enumerate() {
        if folded.contains(&range.start) {
            let end = headings[i + 1..]
                .iter()
                .find(|(_, next)| next <= level)
                .map_or(source.len(), |(r, _)| r.start);
            for byte in &mut bytes[range.end..end] {
                if *byte != b'\n' && *byte != b'\r' {
                    *byte = b' ';
                }
            }
        }
    }
    // Spaces preserve original byte offsets and remove hidden Markdown from the preview parser.
    Cow::Owned(String::from_utf8(bytes).expect("masked UTF-8"))
}
// ponytail: support image tags, not a general HTML layout engine.
fn html_image(html: &str) -> Option<(String, Option<usize>)> {
    let mut rest = html.trim().strip_prefix('<')?;
    if !rest.get(..3)?.eq_ignore_ascii_case("img") || !rest.as_bytes().get(3)?.is_ascii_whitespace()
    {
        return None;
    }
    rest = &rest[3..];
    let (mut src, mut width) = (None, None);
    loop {
        rest = rest.trim_start();
        if rest.starts_with('>') || rest.starts_with("/>") {
            break;
        }
        let end = rest.find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '>' | '/'))?;
        if end == 0 {
            return None;
        }
        let key = &rest[..end];
        rest = rest[end..].trim_start();
        if !rest.starts_with('=') {
            continue;
        }
        rest = rest[1..].trim_start();
        let first = *rest.as_bytes().first()?;
        let value;
        if first == b'\'' || first == b'"' {
            rest = &rest[1..];
            let end = rest.find(first as char)?;
            value = &rest[..end];
            rest = &rest[end + 1..];
        } else {
            let end = rest.find(|c: char| c.is_ascii_whitespace() || c == '>')?;
            value = &rest[..end];
            rest = &rest[end..];
        }
        if key.eq_ignore_ascii_case("src") {
            src = Some(value.replace("&amp;", "&"));
        }
        if key.eq_ignore_ascii_case("width") {
            width = value.parse::<usize>().ok().filter(|n| *n > 0);
        }
    }
    Some((src?, width))
}
fn formatted(
    source: &str,
    table_width: usize,
    dark: bool,
    base: Option<&std::path::Path>,
    folded: Option<&BTreeSet<usize>>,
) -> String {
    let mut out = String::from("{\\rtf1\\ansi\\deff0\\uc1{\\fonttbl{\\f0 Segoe UI;}{\\f1 Consolas;}{\\f2 Segoe UI Symbol;}{\\f3 Cambria Math;}}{\\colortbl;\\red34\\green48\\blue64;\\red35\\green96\\blue154;}\\f0\\fs22\\cf1 ");
    if dark {
        out = out.replace(r"\fs22", r"\fs30").replace(
            r"\red34\green48\blue64;\red35\green96\blue154;",
            r"\red222\green232\blue233;\red63\green221\blue207;",
        );
    }
    // Shared lexer colors, with a paper palette for PDF export.
    let palette = [
        crate::theme::INK,
        crate::theme::ACCENT,
        crate::theme::rgb(111, 185, 246),
        crate::theme::rgb(145, 206, 180),
        crate::theme::rgb(218, 185, 130),
        crate::theme::rgb(178, 165, 223),
        crate::theme::MUTED,
    ];
    let paper = [
        0x403022, 0x9a6023, 0x9a6023, 0x466b20, 0x235d94, 0x884d79, 0x72685d,
    ];
    let mut extra = String::new();
    for color in if dark { palette } else { paper } {
        let _ = write!(
            extra,
            "\\red{}\\green{}\\blue{};",
            color & 255,
            color >> 8 & 255,
            color >> 16 & 255
        );
    }
    extra.push_str(if dark {
        "\\red24\\green33\\blue44;\\red59\\green75\\blue92;"
    } else {
        "\\red242\\green245\\blue248;\\red180\\green190\\blue200;"
    });
    let end = out.find("}\\f0").unwrap();
    out.insert_str(end, &extra);
    let mut code_language = None;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut alignments = Vec::new();
    let mut quote_depth = 0;
    let mut column = 0;
    let mut table_head = false;
    let mut image_budget = (16 * 1024 * 1024, 16 * 1024 * 1024);
    let mut image_rendered = false;
    let mut in_comment = false;
    for (event, range) in Parser::new_ext(
        source,
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_MATH,
    )
    .into_offset_iter()
    {
        if image_rendered {
            if matches!(event, Event::End(TagEnd::Image)) {
                image_rendered = false;
            }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if lists.is_empty() {
                        let _ = write!(out, "{{\\pard\\li{}\\sa140 ", quote_depth * 360);
                    } else {
                        out.push_str("{\\sa100 ");
                    }
                }
                Tag::Heading { level, .. } => {
                    let _ = write!(
                        out,
                        "{{\\pard\\sb200\\sa120\\b\\cf1\\fs{} ",
                        (40 - (level as u8 - 1) * 4).max(if dark { 30 } else { 20 })
                    );
                    if let Some(folded) = folded {
                        let arrow = if folded.contains(&range.start) {
                            9656
                        } else {
                            9662
                        };
                        let _ = write!(out, "{{\\field{{\\*\\fldinst HYPERLINK \"plumetxt-fold:{}\"}}{{\\fldrslt \\u{}?}}}} ", range.start, arrow);
                    }
                }
                Tag::Strong => out.push_str("{\\b "),
                Tag::Emphasis => out.push_str("{\\i "),
                Tag::Strikethrough => out.push_str("{\\strike "),
                Tag::CodeBlock(kind) => {
                    code_language = Some(match kind {
                        CodeBlockKind::Fenced(name) => crate::syntax::Language::name(
                            name.split_whitespace().next().unwrap_or(""),
                        ),
                        _ => crate::syntax::Language::Plain,
                    });
                    // RichEdit cell shading suppresses token colours; borders distinguish code without that override.
                    let _ = write!(out, "{{\\trowd\\trgaph160\\trpaddt120\\trpaddb120\\trpaddft3\\trpaddfb3\\clbrdrt\\brdrs\\brdrw10\\brdrcf11\\clbrdrl\\brdrs\\brdrw10\\brdrcf11\\clbrdrb\\brdrs\\brdrw10\\brdrcf11\\clbrdrr\\brdrs\\brdrw10\\brdrcf11\\cellx{}\\pard\\intbl\\sb100\\sa100\\f1\\cf3\\fs{} ", table_width.max(240), if dark { 26 } else { 20 });
                }
                Tag::BlockQuote(_) => {
                    quote_depth += 1;
                    let _ = write!(out, "{{\\pard\\li{}\\cf9 ", quote_depth * 360);
                }
                Tag::List(start) => {
                    if !lists.is_empty() {
                        out.push_str("\\par ");
                    }
                    lists.push(start);
                }
                Tag::Item => {
                    let indent = lists.len() * 360 + quote_depth * 360;
                    let _ = write!(out, "{{\\pard\\li{indent}\\fi-240\\tx{indent}\\sa80 ");
                    let task = source[range.clone()]
                        .split_once(char::is_whitespace)
                        .is_some_and(|(_, body)| {
                            ["[ ] ", "[x] ", "[X] "]
                                .iter()
                                .any(|m| body.trim_start().starts_with(m))
                        });
                    if !task {
                        match lists.last_mut() {
                            Some(Some(n)) => {
                                let _ = write!(out, "{}.\\~", n);
                                *n += 1;
                            }
                            _ => out.push_str("\\u8226?\\tab "),
                        }
                    }
                }
                Tag::Link { dest_url, .. } => {
                    out.push_str("{\\field{\\*\\fldinst HYPERLINK \"");
                    escape(
                        &mut out,
                        &dest_url.replace('"', "%22").replace(['\r', '\n', '\0'], ""),
                    );
                    out.push_str("\"}{\\fldrslt\\ul\\cf2 ");
                }
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
                Tag::Table(align) => alignments = align,
                Tag::TableHead | Tag::TableRow => {
                    table_head = matches!(tag, Tag::TableHead);
                    column = 0;
                    out.push_str("{\\trowd\\trgaph140\\trpaddt80\\trpaddb80\\trpaddft3\\trpaddfb3");
                    for c in 1..=alignments.len() {
                        for side in ["t", "l", "b", "r"] {
                            let _ = write!(out, "\\clbrdr{side}\\brdrs\\brdrw10\\brdrcf11");
                        }
                        if table_head {
                            out.push_str("\\clcbpat10");
                        }
                        let _ = write!(
                            out,
                            "\\cellx{}",
                            c * table_width.max(240) / alignments.len().max(1)
                        );
                    }
                    out.push(' ');
                }
                Tag::TableCell => {
                    out.push_str("{\\pard\\intbl\\cf1\\sb80\\sa80 ");
                    out.push_str(match alignments.get(column) {
                        Some(Alignment::Center) => "\\qc ",
                        Some(Alignment::Right) => "\\qr ",
                        _ => "\\ql ",
                    });
                    if table_head {
                        out.push_str("\\b ");
                    }
                    column += 1;
                }
                _ => (),
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => out.push_str("\\par}"),
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => out.push('}'),
                TagEnd::BlockQuote(_) => {
                    quote_depth -= 1;
                    out.push('}');
                }
                TagEnd::Link => out.push_str("}}"),
                TagEnd::CodeBlock => {
                    code_language = None;
                    out.push_str("\\cell\\row}\\pard\\sa120\\par ");
                }
                TagEnd::Image => {
                    escape(&mut out, "]");
                    out.push('}');
                }
                TagEnd::List(_) => {
                    lists.pop();
                }
                TagEnd::TableCell => out.push_str("\\cell} "),
                TagEnd::TableHead | TagEnd::TableRow => out.push_str("\\row}"),
                TagEnd::Table => out.push_str("\\pard\\par "),
                _ => (),
            },
            Event::Text(text) if code_language.is_some() && text.len() <= 64 * 1024 => {
                // ponytail: cap lexer allocation per text event; larger blocks remain plain.
                let colors = crate::syntax::colors(&text, code_language.unwrap());
                let mut start = 0;
                for end in text
                    .char_indices()
                    .map(|(i, _)| i)
                    .skip(1)
                    .chain(std::iter::once(text.len()))
                {
                    if end == text.len() || colors[end] != colors[start] {
                        let color = palette
                            .iter()
                            .position(|c| *c == colors[start])
                            .unwrap_or(0)
                            + 3;
                        let _ = write!(out, "{{\\cf{color} ");
                        escape(&mut out, &text[start..end]);
                        out.push('}');
                        start = end;
                    }
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                let mut html = html.as_ref();
                while !html.is_empty() {
                    if in_comment {
                        let Some(end) = html.find("-->") else {
                            break;
                        };
                        html = &html[end + 3..];
                        in_comment = false;
                        continue;
                    }
                    let (visible, rest) = html.split_once("<!--").unwrap_or((html, ""));
                    in_comment = visible.len() != html.len();
                    html = rest;
                    if visible.trim().is_empty() {
                        continue;
                    }
                    let picture = html_image(visible).and_then(|(src, width)| {
                        base.and_then(|base| {
                            crate::assets::picture(
                                base,
                                &src,
                                width.map_or(table_width, |w| w.saturating_mul(15).min(table_width)),
                                &mut image_budget,
                                dark,
                            )
                        })
                    });
                    if let Some(picture) = picture {
                        out.push_str(&picture);
                    } else {
                        escape(&mut out, visible);
                    }
                }
            }
            Event::Text(text) => escape(&mut out, &text),
            ref math_event @ (Event::InlineMath(_) | Event::DisplayMath(_)) => {
                let (math, display) = match math_event {
                    Event::InlineMath(m) => (m, false),
                    Event::DisplayMath(m) => (m, true),
                    _ => unreachable!(),
                };
                out.push_str(if display {
                    "{\\pard\\qc\\sb160\\sa160\\f3\\cf1 "
                } else {
                    "{\\f3\\cf1 "
                });
                out.push_str("\\u-8192?");
                escape(&mut out, math);
                out.push_str("\\u-8191?");
                out.push_str(if display { "\\par}" } else { "}" });
            }
            Event::Code(text) => {
                out.push_str("{\\f1\\highlight10\\cf3 ");
                escape(&mut out, &text);
                out.push('}');
            }
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push_str("\\line "),
            Event::Rule => {
                let _ = write!(out, "{{\\pard\\sa100\\par}}{{\\trowd\\trrh-20\\trgaph0\\clbrdrt\\brdrnone\\clbrdrb\\brdrnone\\clbrdrl\\brdrnone\\clbrdrr\\brdrnone\\clcbpat11\\cellx{}\\pard\\intbl\\fs1\\cell\\row}}{{\\pard\\sa100\\par}}", table_width.max(240));
            }
            Event::TaskListMarker(done) => out.push_str(if done {
                "{\\f2\\cf2\\u9745?}\\tab "
            } else {
                "{\\f2\\cf9\\u9744?}\\tab "
            }),
            _ => (),
        }
    }
    out.push('}');
    out
}

#[test]
fn heading_folds_respect_hierarchy_and_leave_export_complete() {
    let source = "# Parent\n\nPARENT_BODY\n\n## Child\n\nCHILD_BODY\n\n# Next\n\nNEXT_BODY\n\n```md\n# Not a heading\n```\n";
    let folded = BTreeSet::from([0]);
    let view = folding_preview(source, 9000, None, &folded);
    assert!(!view.contains("PARENT_BODY"));
    assert!(!view.contains("Child"));
    assert!(view.contains("Next") && view.contains("NEXT_BODY"));
    assert!(view.contains("plumetxt-fold:0") && view.contains("\\u9656?"));
    let child = source.find("## Child").unwrap();
    let view = folding_preview(source, 9000, None, &BTreeSet::from([child]));
    assert!(view.contains("PARENT_BODY") && !view.contains("CHILD_BODY"));
    assert!(rtf(source, 9000).contains("CHILD_BODY"));
    assert!(!rtf(source, 9000).contains("plumetxt-fold:"));
    assert!(
        !folding_preview("```md\n# Fake\n```", 9000, None, &BTreeSet::new())
            .contains("plumetxt-fold:")
    );
    let setext = folding_preview(
        "Heading\n=======\n\nHIDDEN\n\nOther\n=====\nSHOWN",
        9000,
        None,
        &folded,
    );
    assert!(!setext.contains("HIDDEN") && setext.contains("SHOWN"));
}

#[test]
fn markdown_unicode_and_rtf_injection() {
    assert_eq!(
        html_image(r#"<img alt="logo > here" width='128' src="assets/a&amp;b.png">"#),
        Some(("assets/a&b.png".into(), Some(128)))
    );
    assert_eq!(
        html_image("<IMG SRC=logo.png />"),
        Some(("logo.png".into(), None))
    );
    assert!(html_image("<img src='broken>").is_none());
    assert!(html_image("<script src='a.png'>").is_none());

    let rich = preview("[Docs](https://example.com)\n\n|Left|Center|Right|\n|:---|:---:|---:|\n|a|b|c|\n\n```rust\nlet n = 42; // 中文\n```", 9000);
    assert!(rich.contains("HYPERLINK \"https://example.com\""));
    assert!(rich.contains("\\qc ") && rich.contains("\\qr "));
    assert!(rich.contains("\\clcbpat10") && rich.contains("\\trgaph160"));
    assert!(rich.contains("{\\cf4 let}"));

    let screen = preview("Body\n\n```\ncode\n```", 9000);
    assert!(screen.contains(r"\fs30"));
    assert!(screen.contains(r"\fs26"));
    assert!(!screen.contains('\u{c}'));
    let doc = rtf("# 中文 😀\n\n**bold** `code`\n\n1. item\n\n| A | B |\n|---|---|\n| one | two |\n\n{\\rtf1 evil}", 9000);
    assert!(doc.contains("\\u20013?"));
    assert!(doc.contains("\\u-10179?\\u-8704?"));
    assert!(doc.contains("{\\b bold}"));
    assert!(doc.contains("\\cellx4500"));
    assert!(doc.contains("\\{\\\\rtf1 evil\\}"));
}

#[test]
fn html_comments_do_not_create_preview_gaps() {
    let source = format!("Before\n\n<!-- Exact-size padding: {} -->\n\n<!-- SIZE_PADDING_END -->\n\n# File end\n\nAfter", " ".repeat(7726));
    let expected = "Before\n\n# File end\n\nAfter";
    assert_eq!(preview(&source, 9000), preview(expected, 9000));
    assert_eq!(rtf(&source, 9000), rtf(expected, 9000));
    assert_eq!(preview("Before<!-- hidden\ncomment -->After", 9000), preview("BeforeAfter", 9000));
    assert_eq!(preview("Before\n\n<!-- hidden\ncomment\nstill hidden -->\n\nAfter", 9000), preview("Before\n\nAfter", 9000));
    assert!(preview("`<!-- literal -->`", 9000).contains("<!-- literal -->"));
    assert!(preview("```html\n<!-- literal -->\n```", 9000).contains("literal"));
}
