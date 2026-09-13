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
pub(crate) enum Language {
    Plain,
    Markdown,
    Rust,
    Json,
    Script,
    Python,
    Shell,
    Css,
    Markup,
    Config,
    Sql,
    Lua,
    Ruby,
    PowerShell,
    Batch,
    Tex,
    Diff,
    Data(u8),
    Doc,
    Ignore,
}
impl Language {
    pub(crate) fn name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "md" | "markdown" | "mdown" => Self::Markdown,
            "rs" | "rust" => Self::Rust,
            "json" | "jsonc" | "jsonl" | "ndjson" | "ipynb" => Self::Json,
            "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "javascript" | "typescript" | "c"
            | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "c++" | "cs" | "csharp" | "java"
            | "go" | "kt" | "kts" | "kotlin" | "swift" | "dart" | "gradle" | "groovy" | "scala"
            | "sc" | "php" | "phtml" | "vue" | "svelte" => Self::Script,
            "py" | "pyw" | "python" | "r" | "rscript" => Self::Python,
            "rb" | "ruby" | "rake" | "gemspec" | "pl" | "pm" | "perl" => Self::Ruby,
            "lua" => Self::Lua,
            "sh" | "bash" | "zsh" | "fish" | "shell" | "shellscript" => Self::Shell,
            "ps1" | "psm1" | "psd1" | "powershell" | "pwsh" => Self::PowerShell,
            "bat" | "cmd" | "batch" => Self::Batch,
            "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "config" | "env" | "properties"
            | "editorconfig" | "lock" | "make" | "makefile" | "cmake" | "dockerfile" => {
                Self::Config
            }
            "sql" => Self::Sql,
            "css" | "scss" | "sass" | "less" => Self::Css,
            "html" | "htm" | "xml" | "svg" | "xhtml" | "xaml" => Self::Markup,
            "tex" | "latex" | "bib" | "sty" | "cls" => Self::Tex,
            "diff" | "patch" => Self::Diff,
            "csv" => Self::Data(b','),
            "tsv" => Self::Data(b'\t'),
            "rst" | "rest" | "adoc" | "asciidoc" => Self::Doc,
            "gitignore" | "gitattributes" | "dockerignore" | "ignore" => Self::Ignore,
            _ => Self::Plain,
        }
    }

    pub(crate) fn detect(path: &Path, source: &str) -> Self {
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let special = match filename.as_str() {
            "dockerfile" | "containerfile" | "makefile" | "gnumakefile" | "cmakelists.txt" => {
                Some(Self::Config)
            }
            "gemfile" | "rakefile" => Some(Self::Ruby),
            ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => Some(Self::Shell),
            _ if filename == ".env" || filename.starts_with(".env.") => Some(Self::Config),
            _ if filename.starts_with('.')
                && Self::name(filename.trim_start_matches('.')) != Self::Plain =>
            {
                Some(Self::name(filename.trim_start_matches('.')))
            }
            _ => None,
        };
        let language = special
            .unwrap_or_else(|| Self::name(path.extension().and_then(|s| s.to_str()).unwrap_or("")));
        if language != Self::Plain {
            return language;
        }
        if let Some(line) = source.strip_prefix("#!").and_then(|s| s.lines().next()) {
            for (interpreter, language) in [
                ("python", Self::Python),
                ("ruby", Self::Ruby),
                ("perl", Self::Ruby),
                ("node", Self::Script),
                ("deno", Self::Script),
                ("lua", Self::Lua),
                ("pwsh", Self::PowerShell),
                ("sh", Self::Shell),
            ] {
                if line.contains(interpreter) {
                    return language;
                }
            }
        }
        language
    }
}

// Byte colours are converted to UTF-16 runs once, so Unicode never shifts offsets.
fn code(source: &str, language: Language, colors: &mut [u32]) {
    if language == Language::Plain {
        return;
    }
    if matches!(language, Language::Diff | Language::Doc | Language::Ignore) {
        let mut at = 0;
        for line in source.split_inclusive('\n') {
            let text = line.trim_start();
            let color = match language {
                Language::Diff if text.starts_with("@@") || text.starts_with("diff ") => BLUE,
                Language::Diff if text.starts_with('+') => STRING,
                Language::Diff if text.starts_with('-') => rgb(238, 130, 139),
                Language::Ignore if text.starts_with('#') => MUTED,
                Language::Ignore if text.starts_with('!') => LILAC,
                Language::Ignore => STRING,
                Language::Doc if text.starts_with(".. ") || text.starts_with("//") => MUTED,
                Language::Doc
                    if text.starts_with(['=', '#', ':'])
                        || text.trim().chars().all(|c| "=-~^*".contains(c)) =>
                {
                    ACCENT
                }
                Language::Doc if text.starts_with(['*', '-', '`', '[']) => BLUE,
                _ => INK,
            };
            colors[at..at + line.len()].fill(color);
            at += line.len();
        }
        return;
    }
    let b = source.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let mut color = INK;
        if (language == Language::Lua && b[i..].starts_with(b"--[["))
            || (language == Language::PowerShell && b[i..].starts_with(b"<#"))
        {
            let close = if language == Language::Lua {
                "]]"
            } else {
                "#>"
            };
            i += source[i..]
                .find(close)
                .map_or(b.len() - i, |n| n + close.len());
            color = MUTED;
        } else if language == Language::Lua && b[i..].starts_with(b"[[") {
            i += 2;
            i += source[i..].find("]]").map_or(b.len() - i, |n| n + 2);
            color = STRING;
        } else if b[i..].starts_with(b"<!--")
            && matches!(language, Language::Markup | Language::Script)
        {
            i += source[i..].find("-->").map_or(b.len() - i, |n| n + 3);
            color = MUTED;
        } else if (b[i..].starts_with(b"//")
            && matches!(
                language,
                Language::Rust | Language::Script | Language::Json | Language::Css
            ))
            || (b[i] == b'#'
                && matches!(
                    language,
                    Language::Python
                        | Language::Shell
                        | Language::PowerShell
                        | Language::Config
                        | Language::Ruby
                ))
            || (b[i..].starts_with(b"--") && matches!(language, Language::Sql | Language::Lua))
            || (b[i] == b';'
                && language == Language::Config
                && source[..i]
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .is_empty())
            || (b[i] == b'%' && language == Language::Tex)
            || (language == Language::Batch
                && (b[i..].starts_with(b"::")
                    || source[i..]
                        .get(..4)
                        .is_some_and(|s| s.eq_ignore_ascii_case("rem "))))
        {
            i += source[i..].find('\n').unwrap_or(b.len() - i);
            color = MUTED;
        } else if b[i..].starts_with(b"/*")
            && matches!(
                language,
                Language::Rust | Language::Script | Language::Json | Language::Css | Language::Sql
            )
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
            let triple = matches!(
                language,
                Language::Python | Language::Config | Language::Script
            ) && b[i..].starts_with(&[quote; 3]);
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
        } else if b[i] == b'\\' && language == Language::Tex {
            i += 1;
            if i < b.len() && !b[i].is_ascii_alphabetic() {
                i += 1;
            }
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            color = ACCENT;
        } else if b[i] == b'$'
            && matches!(
                language,
                Language::Shell | Language::PowerShell | Language::Ruby | Language::Script
            )
        {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b"_{:?}".contains(&b[i])) {
                i += 1;
            }
            color = LILAC;
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
            let lower = word.to_ascii_lowercase();
            let extra_keywords = match language {
                Language::Sql => "select from where insert into update delete create alter drop table index view join inner left right outer on group by order having limit offset union all distinct values set as and or not null is like between exists primary key references begin commit rollback transaction case when then else end asc desc count sum avg min max",
                Language::Shell => "then fi do done esac case elif export local readonly declare unset source echo exit function in select until",
                Language::PowerShell => "param begin process end function filter foreach switch default try catch finally throw return class enum using exit trap dynamicparam data in parallel workflow",
                Language::Config => "from run cmd entrypoint workdir copy add expose env arg volume user label include endif ifdef ifndef define endef override export find_package add_executable add_library target_link_libraries project cmake_minimum_required set",
                Language::Lua => "local function end then elseif repeat until do and or not nil require",
                Language::Ruby => "end unless elsif then do module require include attr_reader attr_accessor rescue ensure begin puts sub my our package use given when",
                Language::Batch => "echo set setlocal endlocal goto call exit if else for in do not exist defined errorlevel shift pause",
                Language::Script => "val fun object companion data sealed internal override open inline reified suspend vararg package implements abstract final synchronized throws instanceof boolean byte char double float int long short signed unsigned typedef sizeof union volatile namespace template typename virtual friend operator delete nullptr bool string String auto include define endif elif extends extension protocol guard defer func init deinit associatedtype some any get set readonly keyof typeof declare constructor yield dynamic late factory required mixin part library import export implements echo require require_once endif endforeach public private protected",
                Language::Python => "assert del global nonlocal print library require function repeat next NULL NA Inf NaN",
                _ => "",
            };
            if extra_keywords
                .split_whitespace()
                .any(|k| k == lower || k == word)
                || matches!(
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
                )
            {
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
            } else if (language == Language::PowerShell
                && (source[i..].starts_with('-') || source[..start].ends_with('-')))
                || (matches!(
                    language,
                    Language::Config | Language::Css | Language::Markup
                ) && source[i..].trim_start().starts_with([':', '=']))
                || (language == Language::Markup && source[..start].ends_with(['<', '/']))
                || source[i..].trim_start().starts_with('(')
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
pub(crate) fn colors(source: &str, language: Language) -> Vec<u32> {
    let mut colors = vec![INK; source.len()];
    if let Language::Data(separator) = language {
        let palette = [ACCENT, NUMBER, STRING, LILAC, BLUE];
        let bytes = source.as_bytes();
        let (mut column, mut quoted, mut field_start, mut i) = (0, false, true, 0);
        while i < bytes.len() {
            let byte = bytes[i];
            colors[i] = palette[column % palette.len()];
            if byte == b'"' {
                if quoted && bytes.get(i + 1) == Some(&b'"') {
                    colors[i + 1] = colors[i];
                    i += 2;
                    continue;
                }
                if quoted || field_start {
                    quoted = !quoted;
                }
                field_start = false;
            } else if !quoted && byte == separator {
                colors[i] = MUTED;
                column += 1;
                field_start = true;
            } else if !quoted && matches!(byte, b'\r' | b'\n') {
                colors[i] = MUTED;
                column = 0;
                field_start = true;
            } else {
                field_start = false;
            }
            i += 1;
        }
        return colors;
    }
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
// Keep the first deadline: wheel/input messages must not postpone highlighting.
pub unsafe fn schedule(hwnd: HWND) {
    let key = wide("PlumeTxtHighlightPending");
    if GetPropW(hwnd, key.as_ptr()).is_null() && SetTimer(hwnd, 9, 16, None) != 0 {
        SetPropW(hwnd, key.as_ptr(), 1usize as _);
    }
}

pub unsafe fn scheduled(hwnd: HWND) {
    RemovePropW(hwnd, wide("PlumeTxtHighlightPending").as_ptr());
}

#[derive(Default)]
pub struct Highlighter {
    last: Option<(i32, Language, String, i32, i32)>,
}
impl Highlighter {
    pub fn clear(&mut self) {
        self.last = None;
    }
    pub unsafe fn update(&mut self, hwnd: HWND, path: Option<&Path>) {
        if windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture() == hwnd
            || !GetPropW(hwnd, wide("PlumeTxtComposing").as_ptr()).is_null()
        {
            return;
        }
        let Some(doc) = document(hwnd) else {
            return;
        };
        let language = path.map_or(Language::Markdown, |p| {
            let prefix = doc
                .Range(0, 256)
                .and_then(|r| r.GetText())
                .map(|s| s.to_string())
                .unwrap_or_default();
            Language::detect(p, &prefix)
        });
        let line = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
        let first = SendMessageW(hwnd, EM_LINEINDEX, line as usize, 0).max(0) as i32;
        let rect = crate::theme::client(hwnd);
        let bottom = POINT {
            x: rect.right - 1,
            y: rect.bottom - 1,
        };
        let visible_end = (SendMessageW(hwnd, WM_USER + 39, 0, &bottom as *const _ as isize)
            as i32)
            .max(first)
            .saturating_add(1); // EM_CHARFROMPOS (RichEdit)
        let paint_start = (first - 512).max(0);
        let paint_end = visible_end
            .saturating_add(512)
            .min(first.saturating_add(WINDOW / 2));
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
        if self.last.as_ref().is_some_and(|v| {
            v.0 == start && v.1 == language && v.2 == source && v.3 <= first && v.4 >= visible_end
        }) {
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
            doc.Range(paint_start, paint_end.min(offset))?
                .GetFont()?
                .SetForeColor(INK as i32)?;
            // Lex the look-behind for context, but format only the viewport and a
            // small margin. TOM formatting, not tokenization, dominates the cost.
            for &(a, b, color) in runs
                .iter()
                .filter(|r| r.1 > paint_start && r.0 < paint_end)
                .filter(|r| r.2 != INK)
                .take(4096)
            {
                doc.Range(a.max(paint_start), b.min(paint_end))?
                    .GetFont()?
                    .SetForeColor(color as i32)?;
            }
            Ok(())
        })();
        let _ = doc.Unfreeze();
        SendMessageW(hwnd, WM_USER + 222, 0, &scroll as *const _ as isize);
        let _ = doc.Undo(tomResume.0);
        SendMessageW(hwnd, EM_SETMODIFY, modified as usize, 0);
        SendMessageW(hwnd, WM_USER + 69, 0, mask);
        if result.is_ok() {
            self.last = Some((start, language, source, paint_start, paint_end));
        }
    }
}

#[test]
fn delimited_fields_have_visible_stable_column_colors() {
    let source = "姓名,城市,备注\r\n张三,北京,\"甲,乙\n丙\"\r\n李四,上海,\"他说\"\"你好\"\"\"\r\n";
    let c = colors(source, Language::Data(b','));
    for (text, color) in [
        ("姓名", ACCENT),
        ("张三", ACCENT),
        ("李四", ACCENT),
        ("城市", NUMBER),
        ("北京", NUMBER),
        ("上海", NUMBER),
        ("甲,乙\n丙", STRING),
        ("你好", STRING),
    ] {
        let start = source.find(text).unwrap();
        assert!(
            c[start..start + text.len()]
                .iter()
                .all(|value| *value == color),
            "{text}"
        );
    }
    let tsv = "name\tcity\tnote\nAlice\tParis\tcomma, inside";
    let c = colors(tsv, Language::Data(b'\t'));
    assert_eq!(c[tsv.find("Alice").unwrap()], ACCENT);
    assert_eq!(c[tsv.find("Paris").unwrap()], NUMBER);
    assert_eq!(c[tsv.find("inside").unwrap()], STRING);
}

#[test]
fn registered_syntax_types_and_language_rules() {
    // Keep syntax coverage in step with Explorer's supported file types.
    for row in include_str!("../assets/file-types.tsv").lines().skip(1) {
        let fields: Vec<_> = row.split('\t').collect();
        if fields[2].contains("image")
            || matches!(fields[1], "TXT" | "TEXT" | "LOG" | "PDF" | "ICO")
        {
            continue;
        }
        for extension in fields[3].split_whitespace() {
            assert_ne!(
                Language::name(extension),
                Language::Plain,
                "Missing syntax: {extension}"
            );
        }
    }
    for (path, source, token, expected) in [
        (
            "query.sql",
            "SELECT name FROM items -- 中文\nWHERE id = 42",
            "SELECT",
            ACCENT,
        ),
        ("query.sql", "SELECT 'http://host' -- 中文", "中文", MUTED),
        (
            "script.lua",
            "--[[中文\ncomment]]\nlocal x = [[hello]]",
            "comment",
            MUTED,
        ),
        (
            "script.ps1",
            "<#中文\ncomment#>\nparam($name)",
            "comment",
            MUTED,
        ),
        ("script.psm1", "$value = Get-Item 'file'", "$value", LILAC),
        (
            "config.ini",
            "[section]\nport = 8080\n; comment",
            "port",
            BLUE,
        ),
        ("Cargo.toml", "name = \"中文\"\n# comment", "name", BLUE),
        ("config.yaml", "title: \"中文\"", "title", BLUE),
        ("Dockerfile", "FROM alpine\nRUN echo hello", "FROM", ACCENT),
        ("Makefile", "include rules.mk\nall: app", "include", ACCENT),
        (".env.local", "PORT=8080", "PORT", BLUE),
        (
            ".gitignore",
            "# comment\n*.log\n!important.log",
            "!important",
            LILAC,
        ),
        (
            "run",
            "#!/usr/bin/env python3\ndef main(): pass",
            "def",
            ACCENT,
        ),
        ("file.tex", "\\section{中文} % comment", "\\section", ACCENT),
        ("file.tex", "\\% text", "text", INK),
        (
            "file.diff",
            "@@ -1 +1 @@\n-old\n+new",
            "-old",
            rgb(238, 130, 139),
        ),
        ("file.csv", "name,count\n\"中文\",42", "42", NUMBER),
        ("file.rb", "require 'json'\n# comment", "require", ACCENT),
        ("file.cmd", "REM comment\nset value=1", "comment", MUTED),
        ("file.kt", "fun main() { val n = 1 }", "fun", ACCENT),
        (
            "file.vue",
            "<!-- comment -->\n<script>const n = 1</script>",
            "comment",
            MUTED,
        ),
        (
            "file.jsonc",
            "{\"url\":\"https://host\", // comment\n\"n\":42}",
            "https://host",
            STRING,
        ),
        (".settings.json", "{\"enabled\": true}", "true", LILAC),
    ] {
        let palette = colors(source, Language::detect(Path::new(path), source));
        assert_eq!(palette.len(), source.len());
        assert_eq!(
            palette[source.find(token).unwrap()],
            expected,
            "{path}: {token}"
        );
    }
    let fenced = "```sql\nSELECT 1\n```\n";
    assert_eq!(
        colors(fenced, Language::Markdown)[fenced.find("SELECT").unwrap()],
        ACCENT
    );
    assert!(colors("普通文本 123", Language::Plain)
        .iter()
        .all(|c| *c == INK));
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
const CARET_REFRESH: u32 = WM_APP + 171;
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
        RemovePropW(hwnd, wide("PlumeTxtComposing").as_ptr());
        if data != 0 {
            drop(Box::from_raw(
                data as *mut windows::Win32::UI::Controls::RichEdit::ITextServices,
            ));
        }
        return DefSubclassProc(hwnd, msg, wp, lp);
    }
    if msg == WM_IME_STARTCOMPOSITION {
        SetPropW(hwnd, wide("PlumeTxtComposing").as_ptr(), 1usize as _);
    }
    if msg == WM_IME_ENDCOMPOSITION {
        RemovePropW(hwnd, wide("PlumeTxtComposing").as_ptr());
        schedule(GetAncestor(hwnd, GA_ROOT));
    }
    if msg == WM_PASTE {
        if GetWindowLongW(hwnd, GWL_STYLE) as u32 & ES_READONLY as u32 != 0 {
            return 0;
        }
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
                // BeginPaint temporarily hides the caret. Repair geometry only
                // after EndPaint has restored its visibility/count.
                PostMessageW(hwnd, CARET_REFRESH, 0, 0);
            }
            return 0;
        }
    }
    let result = DefSubclassProc(hwnd, msg, wp, lp);
    // RichEdit can reveal its rectangular selection again after native input,
    // especially in read-only rich text. Our shared overlay owns the highlight.
    if matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_KEYDOWN
            | EM_SETSEL
            | EM_EXSETSEL
            | EM_STREAMIN
    ) || msg == WM_MOUSEMOVE && wp & MK_LBUTTON as usize != 0
    {
        let (mut a, mut b) = (0u32, 0u32);
        SendMessageW(
            hwnd,
            windows_sys::Win32::UI::Controls::EM_GETSEL,
            &mut a as *mut _ as usize,
            &mut b as *mut _ as isize,
        );
        if a != b {
            SendMessageW(hwnd, WM_USER + 63, 1, 0);
        }
    }
    if matches!(msg, WM_LBUTTONUP | WM_CAPTURECHANGED) {
        schedule(GetAncestor(hwnd, GA_ROOT));
    }
    // Passive queries, paints and timers must not recreate the caret or restart
    // its native blink cycle. Only input/layout changes need new cell geometry.
    let caret_changed = matches!(
        msg,
        CARET_REFRESH
            | WM_SETFOCUS
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
    if start == end && GetPropW(hwnd, wide("PlumeTxtComposing").as_ptr()).is_null() {
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
