use crate::theme::{rgb, wide, ACCENT, INK, MUTED};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::{path::Path, ptr::null_mut};
use windows::{
    core::{IUnknown, Interface},
    Win32::UI::Controls::RichEdit::{tomResume, tomSuspend, ITextDocument},
};
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    UI::{
        Controls::{EM_GETFIRSTVISIBLELINE, EM_GETMODIFY, EM_LINEINDEX, EM_SETMODIFY},
        WindowsAndMessaging::*,
    },
};

const BLUE: u32 = rgb(111, 185, 246);
const STRING: u32 = rgb(145, 206, 180);
const NUMBER: u32 = rgb(218, 185, 130);
const LILAC: u32 = rgb(178, 165, 223);
const WINDOW: i32 = 32 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language {
    Plain,
    Markdown,
    Rust,
    Json,
    Script,
    Python,
    Shell,
    Css,
    Markup,
}
impl Language {
    fn name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "md" | "markdown" | "mdown" => Self::Markdown,
            "rs" | "rust" => Self::Rust,
            "json" | "jsonc" => Self::Json,
            "js" | "jsx" | "ts" | "tsx" | "javascript" | "typescript" | "c" | "h" | "cpp"
            | "hpp" | "cs" | "java" | "go" => Self::Script,
            "py" | "python" => Self::Python,
            "sh" | "bash" | "ps1" | "powershell" | "toml" | "yaml" | "yml" => Self::Shell,
            "css" | "scss" => Self::Css,
            "html" | "htm" | "xml" | "svg" => Self::Markup,
            _ => Self::Plain,
        }
    }
}

// Byte colours are converted to UTF-16 runs once, so Unicode never shifts offsets.
fn code(source: &str, language: Language, colors: &mut [u32]) {
    if language == Language::Plain {
        return;
    }
    let b = source.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let mut color = INK;
        if b[i..].starts_with(b"<!--") && language == Language::Markup {
            i += source[i..].find("-->").map_or(b.len() - i, |n| n + 3);
            color = MUTED;
        } else if (b[i..].starts_with(b"//")
            && !matches!(language, Language::Python | Language::Shell))
            || (b[i] == b'#' && matches!(language, Language::Python | Language::Shell))
        {
            i += source[i..].find('\n').unwrap_or(b.len() - i);
            color = MUTED;
        } else if b[i..].starts_with(b"/*")
            && !matches!(language, Language::Python | Language::Shell)
        {
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else if language == Language::Rust && b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            color = MUTED;
        } else if matches!(b[i], b'"' | b'\'' | b'`')
            && !(language == Language::Rust && b[i] == b'\'' && b.get(i + 2) != Some(&b'\''))
        {
            let quote = b[i];
            let triple = language == Language::Python && b[i..].starts_with(&[quote; 3]);
            i += if triple { 3 } else { 1 };
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                } else if triple && b[i..].starts_with(&[quote; 3]) {
                    i += 3;
                    break;
                } else if !triple && b[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            color = STRING;
        } else if b[i].is_ascii_digit() {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'.' | b'_')) {
                i += 1;
            }
            color = NUMBER;
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &source[start..i];
            if matches!(
                word,
                "fn" | "let"
                    | "mut"
                    | "pub"
                    | "use"
                    | "mod"
                    | "impl"
                    | "trait"
                    | "struct"
                    | "enum"
                    | "match"
                    | "self"
                    | "Self"
                    | "const"
                    | "static"
                    | "unsafe"
                    | "async"
                    | "await"
                    | "move"
                    | "ref"
                    | "return"
                    | "if"
                    | "else"
                    | "for"
                    | "while"
                    | "loop"
                    | "break"
                    | "continue"
                    | "in"
                    | "as"
                    | "where"
                    | "type"
                    | "class"
                    | "def"
                    | "import"
                    | "from"
                    | "with"
                    | "try"
                    | "except"
                    | "finally"
                    | "raise"
                    | "yield"
                    | "pass"
                    | "lambda"
                    | "and"
                    | "or"
                    | "not"
                    | "is"
                    | "function"
                    | "var"
                    | "new"
                    | "export"
                    | "default"
                    | "extends"
                    | "interface"
                    | "switch"
                    | "case"
                    | "throw"
                    | "catch"
                    | "public"
                    | "private"
                    | "protected"
                    | "void"
                    | "package"
            ) {
                color = ACCENT;
            } else if matches!(
                word,
                "true"
                    | "false"
                    | "null"
                    | "undefined"
                    | "None"
                    | "True"
                    | "False"
                    | "Some"
                    | "Ok"
                    | "Err"
            ) {
                color = LILAC;
            } else if source[i..].trim_start().starts_with('(')
                || source[i..].trim_start().starts_with("!(")
            {
                color = BLUE;
            }
        } else {
            i += 1;
            if b[start].is_ascii_punctuation() {
                color = BLUE;
            }
        }
        colors[start..i].fill(color);
    }
}
fn colors(source: &str, language: Language) -> Vec<u32> {
    let mut colors = vec![INK; source.len()];
    if language != Language::Markdown {
        code(source, language, &mut colors);
        return colors;
    }
    let mut fenced = None;
    for (event, range) in Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    )
    .into_offset_iter()
    {
        let color = match event {
            Event::Start(Tag::Heading { .. }) => Some(ACCENT),
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => Some(BLUE),
            Event::Start(Tag::Strong | Tag::Emphasis | Tag::Strikethrough) => Some(LILAC),
            Event::Start(Tag::BlockQuote(_)) => Some(MUTED),
            Event::Start(Tag::Item) => {
                let end = source[range.clone()]
                    .find(|c: char| c.is_whitespace())
                    .map_or(range.end, |n| range.start + n);
                colors[range.start..end].fill(ACCENT);
                None
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                fenced = Some(match kind {
                    CodeBlockKind::Fenced(info) => {
                        Language::name(info.split_whitespace().next().unwrap_or(""))
                    }
                    _ => Language::Plain,
                });
                Some(STRING)
            }
            Event::End(TagEnd::CodeBlock) => {
                fenced = None;
                None
            }
            Event::Text(_) if fenced.is_some() => {
                code(
                    &source[range.clone()],
                    fenced.unwrap(),
                    &mut colors[range.clone()],
                );
                None
            }
            Event::Code(_) => Some(STRING),
            Event::Html(_) | Event::InlineHtml(_) => Some(MUTED),
            Event::Rule | Event::TaskListMarker(_) => Some(ACCENT),
            _ => None,
        };
        if let Some(color) = color {
            colors[range].fill(color);
        }
    }
    colors
}

pub unsafe fn document(hwnd: HWND) -> Option<ITextDocument> {
    let mut raw: *mut std::ffi::c_void = null_mut();
    if SendMessageW(hwnd, WM_USER + 60, 0, &mut raw as *mut _ as isize) == 0 || raw.is_null() {
        return None;
    }
    IUnknown::from_raw(raw).cast().ok()
}
#[derive(Default)]
pub struct Highlighter {
    last: Option<(i32, Language, String)>,
}
impl Highlighter {
    pub fn clear(&mut self) {
        self.last = None;
    }
    pub unsafe fn update(&mut self, hwnd: HWND, path: Option<&Path>) {
        if windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture() == hwnd
            || !GetPropW(hwnd, wide("FeatherPadComposing").as_ptr()).is_null()
        {
            return;
        }
        let language = path.map_or(Language::Markdown, |p| {
            Language::name(p.extension().and_then(|s| s.to_str()).unwrap_or(""))
        });
        let Some(doc) = document(hwnd) else {
            return;
        };
        let line = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
        let first = SendMessageW(hwnd, EM_LINEINDEX, line as usize, 0).max(0) as i32;
        let start = (first - WINDOW / 2).max(0);
        let Ok(range) = doc.Range(start, start + WINDOW) else {
            return;
        };
        let Ok(raw) = range.GetText() else {
            return;
        };
        let mut source = raw.to_string().replace('\r', "\n");
        let mut start = start;
        // ponytail: bounded look-behind; multiline constructs beginning over 16K
        // before the viewport need a future incremental lexer for exact colouring.
        if start > 0 {
            if let Some(n) = source.find('\n') {
                start += source[..=n].encode_utf16().count() as i32;
                source.drain(..=n);
            }
        }
        if self
            .last
            .as_ref()
            .is_some_and(|v| v.0 == start && v.1 == language && v.2 == source)
        {
            return;
        }
        let colors = colors(&source, language);
        let mut runs = Vec::new();
        let mut offset = start;
        for (byte, ch) in source.char_indices() {
            let end = offset + ch.len_utf16() as i32;
            if let Some((_, last_end, color)) = runs.last_mut() {
                if *color == colors[byte] {
                    *last_end = end;
                    offset = end;
                    continue;
                }
            }
            runs.push((offset, end, colors[byte]));
            offset = end;
        }
        let mut scroll: POINT = std::mem::zeroed();
        SendMessageW(hwnd, WM_USER + 221, 0, &mut scroll as *mut _ as isize);
        let modified = SendMessageW(hwnd, EM_GETMODIFY, 0, 0);
        let mask = SendMessageW(hwnd, WM_USER + 69, 0, 0);
        if doc.Undo(tomSuspend.0).is_err() {
            SendMessageW(hwnd, WM_USER + 69, 0, mask);
            return;
        }
        let _ = doc.Freeze();
        let result = (|| -> windows::core::Result<()> {
            doc.Range(start, offset)?
                .GetFont()?
                .SetForeColor(INK as i32)?;
            // Pathological punctuation cannot turn one viewport into unbounded COM calls.
            if runs.len() <= 4096 {
                for (a, b, color) in runs {
                    if color != INK {
                        doc.Range(a, b)?.GetFont()?.SetForeColor(color as i32)?;
                    }
                }
            }
            Ok(())
        })();
        let _ = doc.Unfreeze();
        SendMessageW(hwnd, WM_USER + 222, 0, &scroll as *const _ as isize);
        let _ = doc.Undo(tomResume.0);
        SendMessageW(hwnd, EM_SETMODIFY, modified as usize, 0);
        SendMessageW(hwnd, WM_USER + 69, 0, mask);
        if result.is_ok() {
            self.last = Some((start, language, source));
        }
    }
}

#[test]
fn token_colors_keep_unicode_and_code_context() {
    let text = "# 中文 😀\n\n**strong** [link](https://example.com)\n\n```rust\nlet value = \"😀\"; // comment\n```\n";
    let c = colors(text, Language::Markdown);
    for (word, expected) in [
        ("中文", ACCENT),
        ("strong", LILAC),
        ("link", BLUE),
        ("let", ACCENT),
        ("comment", MUTED),
    ] {
        assert_eq!(c[text.find(word).unwrap()], expected, "{word}");
    }
    let json = "{\"key\": true, \"n\": 42}";
    let c = colors(json, Language::Json);
    assert_eq!(c[json.find("key").unwrap()], STRING);
    assert_eq!(c[json.find("true").unwrap()], LILAC);
    assert_eq!(c[json.find("42").unwrap()], NUMBER);
}

pub unsafe fn attach(hwnd: HWND) {
    let services = text_services(hwnd)
        .map(Box::new)
        .map_or(0, |s| Box::into_raw(s) as usize);
    if windows_sys::Win32::UI::Shell::SetWindowSubclass(hwnd, Some(editor_proc), 903, services) == 0
        && services != 0
    {
        drop(Box::from_raw(
            services as *mut windows::Win32::UI::Controls::RichEdit::ITextServices,
        ));
    }
}
thread_local! {
    static CARET_UPDATING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
unsafe extern "system" fn editor_proc(
    hwnd: HWND,
    msg: u32,
    wp: usize,
    lp: isize,
    _id: usize,
    data: usize,
) -> isize {
    use windows::Win32::UI::Controls::RichEdit::{
        EM_EXSETSEL, EM_REDO, EM_SETCHARFORMAT, EM_SETSCROLLPOS, EM_SETZOOM, EM_STREAMIN,
    };
    use windows_sys::Win32::System::SystemServices::MK_LBUTTON;
    use windows_sys::Win32::UI::Controls::{EM_REPLACESEL, EM_SETRECT, EM_SETRECTNP, EM_SETSEL};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(editor_proc), 903);
        RemovePropW(hwnd, wide("FeatherPadComposing").as_ptr());
        if data != 0 {
            drop(Box::from_raw(
                data as *mut windows::Win32::UI::Controls::RichEdit::ITextServices,
            ));
        }
        return DefSubclassProc(hwnd, msg, wp, lp);
    }
    if msg == WM_IME_STARTCOMPOSITION {
        SetPropW(hwnd, wide("FeatherPadComposing").as_ptr(), 1usize as _);
    }
    if msg == WM_IME_ENDCOMPOSITION {
        RemovePropW(hwnd, wide("FeatherPadComposing").as_ptr());
        SetTimer(GetAncestor(hwnd, GA_ROOT), 9, 80, None);
    }
    if msg == WM_PASTE {
        PostMessageW(GetAncestor(hwnd, GA_ROOT), crate::ui::PASTE, 0, 0);
        return 0;
    }
    // Draw the active text view directly: WM_PRINTCLIENT recreates a thin caret.
    if msg == WM_PRINTCLIENT && wp != 0 && data != 0 {
        let mut before: GUITHREADINFO = std::mem::zeroed();
        before.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        GetGUIThreadInfo(GetWindowThreadProcessId(hwnd, null_mut()), &mut before);
        if paint_text(data, wp).is_some() {
            let mut after = before;
            GetGUIThreadInfo(GetWindowThreadProcessId(hwnd, null_mut()), &mut after);
            // A pending layout may replace the caret once; idle draws must keep it.
            if after.hwndCaret == hwnd
                && (before.rcCaret.right - before.rcCaret.left
                    != after.rcCaret.right - after.rcCaret.left)
            {
                CARET_UPDATING.with(|updating| {
                    if !updating.replace(true) {
                        block_caret(hwnd);
                        updating.set(false);
                    }
                });
            }
            return 0;
        }
    }
    let result = DefSubclassProc(hwnd, msg, wp, lp);
    if matches!(msg, WM_LBUTTONUP | WM_CAPTURECHANGED) {
        SetTimer(GetAncestor(hwnd, GA_ROOT), 9, 80, None);
    }
    // Passive queries, paints and timers must not recreate the caret or restart
    // its native blink cycle. Only input/layout changes need new cell geometry.
    let caret_changed = matches!(
        msg,
        WM_SETFOCUS
            | WM_CHAR
            | WM_KEYDOWN
            | WM_KEYUP
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_CAPTURECHANGED
            | WM_SIZE
            | WM_SETFONT
            | WM_SETTEXT
            | WM_UNDO
            | WM_CUT
            | WM_CLEAR
            | WM_VSCROLL
            | WM_HSCROLL
            | WM_MOUSEWHEEL
            | WM_IME_STARTCOMPOSITION
            | WM_IME_COMPOSITION
            | WM_IME_ENDCOMPOSITION
    ) || (msg == WM_MOUSEMOVE && wp & MK_LBUTTON as usize != 0)
        || matches!(
            msg,
            EM_SETSEL
                | EM_SETRECT
                | EM_SETRECTNP
                | EM_REPLACESEL
                | EM_EXSETSEL
                | EM_SETCHARFORMAT
                | EM_STREAMIN
                | EM_REDO
                | EM_SETSCROLLPOS
                | EM_SETZOOM
        );
    if caret_changed && windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd {
        CARET_UPDATING.with(|updating| {
            if !updating.replace(true) {
                block_caret(hwnd);
                updating.set(false);
            }
        });
    }
    result
}

unsafe fn text_services(
    hwnd: HWND,
) -> Option<windows::Win32::UI::Controls::RichEdit::ITextServices> {
    use windows::Win32::UI::Controls::RichEdit::ITextServices;
    let doc = document(hwnd)?;
    let mut raw = null_mut();
    // ITextServices has a zero IID in win32metadata; use the native interface IID.
    let queried = doc.query(
        &windows::core::GUID::from_u128(0x8d33f740_cf58_11ce_a89d_00aa006cadc5),
        &mut raw,
    );
    queried.ok().ok()?;
    Some(ITextServices::from_raw(raw))
}

unsafe fn paint_text(data: usize, dc: usize) -> Option<()> {
    use windows::Win32::{
        Graphics::Gdi::HDC, System::Com::DVASPECT_CONTENT, UI::Controls::RichEdit::ITextServices,
    };
    let services = &*(data as *const ITextServices);
    services
        .TxDraw(
            DVASPECT_CONTENT,
            0,
            null_mut(),
            null_mut(),
            HDC(dc as _),
            HDC::default(),
            null_mut(),
            null_mut(),
            null_mut(),
            0,
            0,
            0,
        )
        .ok()
}

unsafe fn block_caret(hwnd: HWND) {
    let mut info: GUITHREADINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    if GetGUIThreadInfo(GetWindowThreadProcessId(hwnd, null_mut()), &mut info) == 0
        || info.hwndCaret != hwnd
        || info.flags & GUI_CARETBLINKING == 0
    {
        return;
    }
    let height = info.rcCaret.bottom - info.rcCaret.top;
    let mut width = 2;
    let mut left = info.rcCaret.left;
    let (mut start, mut end) = (0u32, 0u32);
    SendMessageW(
        hwnd,
        windows_sys::Win32::UI::Controls::EM_GETSEL,
        &mut start as *mut _ as usize,
        &mut end as *mut _ as isize,
    );
    // A selection has an active end, not an insertion point at its start.
    // Leave that edge under RichEdit's control while dragging or using Shift+arrows.
    if start == end && GetPropW(hwnd, wide("FeatherPadComposing").as_ptr()).is_null() {
        use windows::Win32::UI::Controls::RichEdit::{
            tomClientCoord, tomCluster, tomConstants, tomStart,
        };
        if let Some(doc) = document(hwnd) {
            let measured = (|| -> windows::core::Result<(i32, i32)> {
                let selection = doc.GetSelection()?;
                let cp = selection.GetStart()?;
                let range = doc.Range(cp, cp)?;
                range.Expand(tomCluster.0)?;
                let (mut x, mut y, mut right) = (0, 0, 0);
                range.GetPoint(tomConstants(tomStart.0 | tomClientCoord.0), &mut x, &mut y)?;
                range.GetPoint(
                    tomConstants(tomStart.0 | tomClientCoord.0 | 2),
                    &mut right,
                    &mut y,
                )?;
                Ok((x.min(right), (right - x).abs()))
            })();
            if let Ok((x, w)) = measured {
                left = x;
                width = w;
            }
        }
        if width <= 2 {
            // Empty lines and tabs use a measured half-width cell, as in VS Code.
            use windows_sys::Win32::Graphics::Gdi::*;
            let dc = GetDC(hwnd);
            let old = SelectObject(dc, SendMessageW(hwnd, WM_GETFONT, 0, 0) as _);
            let mut size = std::mem::zeroed();
            GetTextExtentPoint32W(dc, wide("0").as_ptr(), 1, &mut size);
            SelectObject(dc, old);
            ReleaseDC(hwnd, dc);
            let (mut numerator, mut denominator) = (0i32, 0i32);
            SendMessageW(
                hwnd,
                WM_USER + 224,
                &mut numerator as *mut _ as usize,
                &mut denominator as *mut _ as isize,
            );
            width = if denominator > 0 {
                size.cx * numerator / denominator
            } else {
                size.cx
            };
        }
        width = width.max(2);
    }
    if height > 0
        && info.rcCaret.right - info.rcCaret.left != width
        && CreateCaret(hwnd, null_mut(), width, height) != 0
    {
        SetCaretPos(left, info.rcCaret.top);
        ShowCaret(hwnd);
    } else if height > 0 && left != info.rcCaret.left {
        SetCaretPos(left, info.rcCaret.top);
    }
}
