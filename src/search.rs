use crate::{document::Encoding, theme::*};
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read},
    mem::zeroed,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub const OPEN_RESULT: u32 = WM_APP + 40;
pub const CLOSE: u32 = WM_APP + 41;
const LIMIT: usize = 2000;
const MAX_LINE: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Hit {
    pub pdf: Option<crate::pdftext::Match>,
    pub path: PathBuf,
    pub offset: u64,
    pub line: usize,
    pub column: usize,
    pub matched: String,
    snippet: String,
    snippet_match: usize,
}
impl Hit {
    fn prefix(&self) -> String {
        if self.pdf.is_some() {
            format!("Page {}  ", self.line)
        } else {
            format!("{}:{}  ", self.line, self.column)
        }
    }
    pub fn selection(&self, source: &str, encoding: Encoding, start: u64) -> Option<(i32, i32)> {
        let target = self.offset.checked_sub(start)?;
        let mut bytes = 0;
        for (i, ch) in source.char_indices() {
            if bytes == target {
                if !source[i..].starts_with(&self.matched) {
                    return None;
                }
                let a = source[..i].replace("\r\n", "\r").encode_utf16().count() as i32;
                return Some((a, a + self.matched.encode_utf16().count() as i32));
            }
            bytes += if matches!(encoding, Encoding::Utf16Le | Encoding::Utf16Be) {
                ch.len_utf16() * 2
            } else {
                ch.len_utf8()
            } as u64;
        }
        None
    }
    pub fn current_selection(&self, source: &str) -> Option<(i32, i32)> {
        let source = source.replace("\r\n", "\n").replace('\r', "\n");
        let mut offset = 0;
        for (line, content) in source.split('\n').enumerate() {
            if line + 1 == self.line {
                let (column, _) = content.char_indices().nth(self.column - 1)?;
                if !content[column..].starts_with(&self.matched) {
                    return None;
                }
                let a = offset + content[..column].encode_utf16().count() as i32;
                return Some((a, a + self.matched.encode_utf16().count() as i32));
            }
            offset += content.encode_utf16().count() as i32 + 1;
        }
        None
    }
}
enum Event {
    Hit(Hit),
    Error(String),
    Done { skipped: usize, limited: bool },
}

// Return original UTF-8 boundaries even when Unicode lowercasing changes byte length.
fn ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let folded = text.to_lowercase();
    let mut chars = text.char_indices().peekable();
    let mut at = 0;
    let mut result = Vec::new();
    for (start, _) in folded.match_indices(query).take(LIMIT) {
        let end = start + query.len();
        let mut a = None;
        let mut b = 0;
        while let Some(&(index, ch)) = chars.peek() {
            let next = at + ch.to_lowercase().map(char::len_utf8).sum::<usize>();
            if at <= start && start < next {
                a = Some(index);
            }
            chars.next();
            at = next;
            b = index + ch.len_utf8();
            if at >= end {
                break;
            }
        }
        if let Some(a) = a {
            result.push((a, b));
        }
    }
    result
}

fn scan_file(
    path: &Path,
    query: &str,
    cancel: &AtomicBool,
    emit: &mut impl FnMut(Hit) -> bool,
) -> Result<bool, ()> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return scan_pdf(path, query, cancel, emit).map_err(|_| ());
    }
    let mut reader = BufReader::with_capacity(64 * 1024, File::open(path).map_err(|_| ())?);
    let header = reader.fill_buf().map_err(|_| ())?;
    let (encoding, bom) = if header.starts_with(&[0xff, 0xfe]) {
        (Encoding::Utf16Le, 2)
    } else if header.starts_with(&[0xfe, 0xff]) {
        (Encoding::Utf16Be, 2)
    } else if header.starts_with(&[0xef, 0xbb, 0xbf]) {
        (Encoding::Utf8Bom, 3)
    } else {
        (Encoding::Utf8, 0)
    };
    let utf16 = matches!(encoding, Encoding::Utf16Le | Encoding::Utf16Be);
    if !utf16 && header.contains(&0) {
        return Err(());
    }
    reader.consume(bom);
    let mut offset = bom as u64;
    let mut line = 1;
    let mut bytes = Vec::new();
    let mut skipped = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(skipped);
        }
        bytes.clear();
        let mut consumed = 0;
        let mut long = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(skipped);
            }
            let before = bytes.len();
            if utf16 {
                // Buffered reads keep UTF-16 code units aligned, including non-ASCII newlines.
                for _ in 0..32768 {
                    if reader.fill_buf().map_err(|_| ())?.is_empty() {
                        break;
                    }
                    let mut unit = [0; 2];
                    reader.read_exact(&mut unit).map_err(|_| ())?;
                    bytes.extend_from_slice(&unit);
                    let value = if encoding == Encoding::Utf16Le {
                        u16::from_le_bytes(unit)
                    } else {
                        u16::from_be_bytes(unit)
                    };
                    if value == 10 {
                        break;
                    }
                }
            } else {
                reader
                    .by_ref()
                    .take(65536)
                    .read_until(b'\n', &mut bytes)
                    .map_err(|_| ())?;
            }
            let n = bytes.len() - before;
            consumed += n;
            let newline = if utf16 {
                bytes.ends_with(if encoding == Encoding::Utf16Le {
                    &[10, 0]
                } else {
                    &[0, 10]
                })
            } else {
                bytes.last() == Some(&b'\n')
            };
            if bytes.len() > MAX_LINE {
                long = true;
            }
            if n == 0 || newline {
                break;
            }
            if long {
                bytes.clear();
            }
        }
        if consumed == 0 {
            break;
        }
        if long {
            skipped = true;
        } else {
            let source = if utf16 {
                let units: Vec<_> = bytes
                    .chunks_exact(2)
                    .map(|b| {
                        if encoding == Encoding::Utf16Le {
                            u16::from_le_bytes([b[0], b[1]])
                        } else {
                            u16::from_be_bytes([b[0], b[1]])
                        }
                    })
                    .collect();
                String::from_utf16(&units).map_err(|_| ())?
            } else {
                String::from_utf8(bytes.clone()).map_err(|_| ())?
            };
            if source.contains('\0') {
                return Err(());
            }
            let source = source.trim_end_matches(['\r', '\n']);
            let mut from = 0;
            let mut column = 1;
            let mut units = 0;
            for (a, b) in ranges(source, query) {
                column += source[from..a].chars().count();
                units += source[from..a].encode_utf16().count();
                let byte = if utf16 { units * 2 } else { a };
                let context = source[..a]
                    .char_indices()
                    .rev()
                    .nth(32)
                    .map_or(0, |(i, _)| i);
                let snippet: String = source[context..].chars().take(180).collect();
                if !emit(Hit {
                    pdf: None,
                    path: path.to_owned(),
                    offset: offset + byte as u64,
                    line,
                    column,
                    matched: source[a..b].into(),
                    snippet: format!(
                        "{}{}",
                        if context > 0 { "…" } else { "" },
                        snippet.replace('\t', "  ")
                    ),
                    snippet_match: source[context..a].replace('\t', "  ").len()
                        + if context > 0 { "…".len() } else { 0 },
                }) {
                    return Ok(skipped);
                }
                from = a;
                if cancel.load(Ordering::Relaxed) {
                    return Ok(skipped);
                }
            }
        }
        offset += consumed as u64;
        line += 1;
    }
    Ok(skipped)
}

fn scan_pdf(
    path: &Path,
    query: &str,
    cancel: &AtomicBool,
    emit: &mut impl FnMut(Hit) -> bool,
) -> Result<bool, String> {
    crate::pdftext::search(path, query, cancel, &mut |pdf, matched, snippet| {
        let snippet_match = snippet.find(&matched).unwrap_or(0);
        emit(Hit {
            line: pdf.page + 1,
            pdf: Some(pdf),
            path: path.to_owned(),
            offset: 0,
            column: 0,
            matched,
            snippet,
            snippet_match,
        })
    })
    .map(|has_text| !has_text)
}

fn scan(root: PathBuf, query: String, cancel: Arc<AtomicBool>, tx: SyncSender<Event>) {
    use std::os::windows::fs::MetadataExt;
    let mut stack = vec![root];
    let mut count = 0;
    let mut skipped = 0;
    if stack[0].is_file() {
        let path = &stack[0];
        let mut emit = |hit| {
            count += 1;
            tx.send(Event::Hit(hit)).is_ok() && count < LIMIT && !cancel.load(Ordering::Relaxed)
        };
        let result = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            scan_pdf(path, &query, &cancel, &mut emit).and_then(|no_text| {
                if no_text {
                    Err("No searchable text · OCR required".into())
                } else {
                    Ok(false)
                }
            })
        } else {
            scan_file(path, &query, &cancel, &mut emit)
                .map_err(|_| "Cannot search this text file".to_string())
        };
        match result {
            Ok(partial) => {
                let _ = tx.send(Event::Done {
                    skipped: usize::from(partial),
                    limited: count >= LIMIT,
                });
            }
            Err(e) => {
                let _ = tx.send(Event::Error(e));
            }
        }
        return;
    }
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) || count >= LIMIT {
            break;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        for entry in entries {
            if cancel.load(Ordering::Relaxed) || count >= LIMIT {
                break;
            }
            let entry = match entry {
                Ok(v) => v,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            let meta = match entry.metadata() {
                Ok(v) => v,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            if meta.file_attributes() & 0x400 != 0 {
                continue;
            } // Do not follow junctions or symbolic links.
            let path = entry.path();
            if meta.is_dir() {
                if !matches!(
                    entry.file_name().to_string_lossy().as_ref(),
                    ".git" | "node_modules" | "target" | ".cargo-cache" | ".venv" | "__pycache__"
                ) {
                    stack.push(path);
                }
            } else if meta.is_file() {
                let result = scan_file(&path, &query, &cancel, &mut |hit| {
                    count += 1;
                    tx.send(Event::Hit(hit)).is_ok()
                        && count < LIMIT
                        && !cancel.load(Ordering::Relaxed)
                });
                if !matches!(result, Ok(false)) {
                    skipped += 1;
                }
            }
        }
    }
    let _ = tx.send(Event::Done {
        skipped,
        limited: count >= LIMIT,
    });
}

struct State {
    hwnd: HWND,
    edit: HWND,
    tree: HWND,
    status: HWND,
    root: PathBuf,
    hits: Vec<Hit>,
    pending: Option<Hit>,
    open_when_ready: bool,
    groups: HashMap<PathBuf, HTREEITEM>,
    receiver: Option<Receiver<Event>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
pub struct Search(pub HWND);
impl Search {
    pub unsafe fn create(parent: HWND, root: PathBuf, font: HFONT) -> Self {
        let name = wide("PlumeTxtSearch");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        });
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("Workspace search").as_ptr(),
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            300,
            500,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let child = |class: &str, title: &str, style: u32, id: usize| {
            CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(title).as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                1,
                1,
                hwnd,
                id as _,
                GetModuleHandleW(null()),
                null(),
            )
        };
        let edit = child("EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32, 1);
        attach_button(edit);
        SendMessageW(edit, EM_SETLIMITTEXT, 512, 0);
        SendMessageW(
            edit,
            EM_SETCUEBANNER,
            1,
            wide(if root.is_file() {
                "Find in file…"
            } else {
                "Search workspace…"
            })
            .as_ptr() as isize,
        );
        let tree = child(
            "SysTreeView32",
            "Search results",
            WS_TABSTOP
                | TVS_HASBUTTONS
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS
                | TVS_FULLROWSELECT
                | TVS_NOTOOLTIPS
                | TVS_NOHSCROLL,
            2,
        );
        SetWindowTheme(tree, wide("").as_ptr(), wide("").as_ptr());
        SendMessageW(tree, TVM_SETBKCOLOR, 0, SURFACE as isize);
        SendMessageW(tree, TVM_SETTEXTCOLOR, 0, INK as isize);
        SendMessageW(tree, TVM_SETITEMHEIGHT, px(hwnd, 26) as usize, 0);
        SendMessageW(
            tree,
            TVM_SETEXTENDEDSTYLE,
            TVS_EX_DOUBLEBUFFER as usize,
            TVS_EX_DOUBLEBUFFER as isize,
        );
        crate::scroll::attach(tree, SURFACE);
        let status = child("STATIC", "Files on disk · Esc to return", 0, 3);
        for control in [edit, tree, status] {
            SendMessageW(control, WM_SETFONT, font as usize, 0);
        }
        let state = State {
            hwnd,
            edit,
            tree,
            status,
            root,
            hits: Vec::new(),
            pending: None,
            open_when_ready: false,
            groups: HashMap::new(),
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::into_raw(Box::new(RefCell::new(state))) as isize,
        );
        Self(hwnd)
    }
    pub unsafe fn focus(&self) {
        with(self.0, |s| {
            SetFocus(s.edit);
            SendMessageW(s.edit, EM_SETSEL, 0, -1);
        });
    }
    pub unsafe fn contains(&self, hwnd: HWND) -> bool {
        IsChild(self.0, hwnd) != 0
    }
    pub unsafe fn is_scope(&self, path: &Path) -> bool {
        let mut same = false;
        with(self.0, |s| same = s.root == path);
        same
    }
    pub unsafe fn take_hit(&self) -> Option<Hit> {
        let mut hit = None;
        with(self.0, |s| hit = s.pending.take());
        hit
    }
    pub unsafe fn key(&self, msg: &MSG) -> bool {
        if msg.message != WM_KEYDOWN
            || IsWindowVisible(self.0) == 0
            || IsChild(self.0, msg.hwnd) == 0
        {
            return false;
        }
        let mut handled = false;
        with(self.0, |s| match msg.wParam as u16 {
            0x41 if msg.hwnd == s.edit && GetKeyState(VK_CONTROL as i32) < 0 => {
                SendMessageW(s.edit, EM_SETSEL, 0, -1);
                handled = true;
            }
            VK_ESCAPE => {
                PostMessageW(GetParent(s.hwnd), CLOSE, 0, 0);
                handled = true;
            }
            VK_RETURN => {
                if msg.hwnd == s.edit {
                    s.start();
                    s.open_when_ready = true;
                } else {
                    s.open_selected();
                }
                handled = true;
            }
            VK_DOWN | VK_TAB if msg.hwnd == s.edit => {
                SetFocus(s.tree);
                if SendMessageW(s.tree, TVM_GETNEXTITEM, TVGN_CARET as usize, 0) == 0 {
                    let root = SendMessageW(s.tree, TVM_GETNEXTITEM, TVGN_ROOT as usize, 0);
                    let first = SendMessageW(s.tree, TVM_GETNEXTITEM, TVGN_CHILD as usize, root);
                    SendMessageW(s.tree, TVM_SELECTITEM, TVGN_CARET as usize, first);
                }
                handled = true;
            }
            VK_TAB => {
                SetFocus(s.edit);
                handled = true;
            }
            _ => (),
        });
        handled
    }
}
impl Drop for Search {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
unsafe fn with(hwnd: HWND, f: impl FnOnce(&mut State)) {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>;
    if !p.is_null() {
        if let Ok(mut s) = (*p).try_borrow_mut() {
            f(&mut s);
        }
    }
}
impl State {
    unsafe fn start(&mut self) {
        self.open_when_ready = false;
        KillTimer(self.hwnd, 1);
        KillTimer(self.hwnd, 2);
        self.cancel.store(true, Ordering::Relaxed);
        self.receiver = None;
        self.hits.clear();
        self.groups.clear();
        SendMessageW(self.tree, TVM_DELETEITEM, 0, TVI_ROOT);
        let mut query = vec![0u16; GetWindowTextLengthW(self.edit) as usize + 1];
        let n = GetWindowTextW(self.edit, query.as_mut_ptr(), query.len() as i32) as usize;
        let query = String::from_utf16_lossy(&query[..n]).to_lowercase();
        if query.is_empty() {
            SetWindowTextW(self.status, wide("Files on disk · Esc to return").as_ptr());
            return;
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let root = self.root.clone();
        let (tx, rx) = mpsc::sync_channel(256);
        self.receiver = Some(rx);
        std::thread::spawn(move || scan(root, query, cancel, tx));
        SetWindowTextW(self.status, wide("Searching…").as_ptr());
        SetTimer(self.hwnd, 2, 40, None);
    }
    unsafe fn insert(&self, parent: HTREEITEM, title: &str, index: isize) -> HTREEITEM {
        let mut title = wide(title);
        let insert = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                item: TVITEMW {
                    mask: TVIF_TEXT | TVIF_PARAM,
                    pszText: title.as_mut_ptr(),
                    lParam: index,
                    ..zeroed()
                },
            },
        };
        SendMessageW(self.tree, TVM_INSERTITEMW, 0, &insert as *const _ as isize)
    }
    unsafe fn tick(&mut self) {
        let mut done = None;
        let before = self.hits.len();
        SendMessageW(self.tree, WM_SETREDRAW, 0, 0);
        for _ in 0..100 {
            let Some(event) = self.receiver.as_ref().and_then(|r| r.try_recv().ok()) else {
                break;
            };
            match event {
                Event::Hit(hit) => {
                    let parent = if let Some(parent) = self.groups.get(&hit.path) {
                        *parent
                    } else {
                        let label = if hit.path == self.root {
                            hit.path.file_name().unwrap_or_default().to_string_lossy()
                        } else {
                            hit.path
                                .strip_prefix(&self.root)
                                .unwrap_or(&hit.path)
                                .to_string_lossy()
                        };
                        let parent = self.insert(TVI_ROOT, &label, -1);
                        self.groups.insert(hit.path.clone(), parent);
                        parent
                    };
                    self.insert(
                        parent,
                        &format!("{}{}", hit.prefix(), hit.snippet),
                        self.hits.len() as isize,
                    );
                    self.hits.push(hit);
                    SendMessageW(self.tree, TVM_EXPAND, TVE_EXPAND as usize, parent);
                }
                Event::Done { skipped, limited } => {
                    done = Some((skipped, limited));
                    break;
                }
                Event::Error(e) => {
                    KillTimer(self.hwnd, 2);
                    self.receiver = None;
                    SendMessageW(self.tree, WM_SETREDRAW, 1, 0);
                    invalidate(self.tree);
                    SetWindowTextW(self.status, wide(&e).as_ptr());
                    return;
                }
            }
        }
        SendMessageW(self.tree, WM_SETREDRAW, 1, 0);
        if self.hits.len() != before {
            invalidate(self.tree);
        }
        if self.open_when_ready && !self.hits.is_empty() {
            self.open_when_ready = false;
            let root = SendMessageW(self.tree, TVM_GETNEXTITEM, TVGN_ROOT as usize, 0);
            let first = SendMessageW(self.tree, TVM_GETNEXTITEM, TVGN_CHILD as usize, root);
            SendMessageW(self.tree, TVM_SELECTITEM, TVGN_CARET as usize, first);
            self.open_selected();
        }
        if self.hits.len() == before && done.is_none() {
            return;
        }
        let status = if let Some((skipped, limited)) = done {
            KillTimer(self.hwnd, 2);
            self.receiver = None;
            format!(
                "{}{} matches · {} files{}",
                self.hits.len(),
                if limited { "+" } else { "" },
                self.groups.len(),
                if skipped > 0 {
                    format!(" · {skipped} skipped")
                } else {
                    String::new()
                }
            )
        } else {
            format!("Searching… {} matches", self.hits.len())
        };
        SetWindowTextW(self.status, wide(&status).as_ptr());
    }
    unsafe fn open_selected(&mut self) {
        let item = SendMessageW(self.tree, TVM_GETNEXTITEM, TVGN_CARET as usize, 0);
        let mut info = TVITEMW {
            mask: TVIF_PARAM,
            hItem: item,
            ..zeroed()
        };
        if item != 0
            && SendMessageW(self.tree, TVM_GETITEMW, 0, &mut info as *mut _ as isize) != 0
            && info.lParam >= 0
        {
            self.pending = self.hits.get(info.lParam as usize).cloned();
            PostMessageW(GetParent(self.hwnd), OPEN_RESULT, self.hwnd as usize, 0);
        }
    }
    unsafe fn highlight(&self, draw: &NMTVCUSTOMDRAW) {
        let Some(hit) = self.hits.get(draw.nmcd.lItemlParam as usize) else {
            return;
        };
        let at = hit.snippet_match;
        let prefix = wide(&format!("{}{}", hit.prefix(), &hit.snippet[..at]));
        let dc = draw.nmcd.hdc;
        let font = SendMessageW(self.tree, WM_GETFONT, 0, 0) as HFONT;
        let old = SelectObject(dc, font);
        let mut size = zeroed();
        GetTextExtentPoint32W(dc, prefix.as_ptr(), (prefix.len() - 1) as i32, &mut size);
        let mut rect: RECT = zeroed();
        std::ptr::write_unaligned(
            (&mut rect as *mut RECT).cast::<HTREEITEM>(),
            draw.nmcd.dwItemSpec as HTREEITEM,
        );
        if SendMessageW(self.tree, TVM_GETITEMRECT, 1, &mut rect as *mut _ as isize) == 0 {
            SelectObject(dc, old);
            return;
        }
        rect.left += size.cx + 2;
        let mut client = zeroed();
        GetClientRect(self.tree, &mut client);
        rect.right = rect.right.min(client.right);
        if rect.left < rect.right {
            label(
                dc,
                &hit.matched,
                rect,
                font,
                ACCENT,
                DT_SINGLELINE | DT_VCENTER,
            );
        }
        SelectObject(dc, old);
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_NCDESTROY => {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<State>;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
        }
        WM_SIZE => with(hwnd, |s| {
            let mut rc = zeroed();
            GetClientRect(hwnd, &mut rc);
            move_window(
                s.edit,
                px(hwnd, 12),
                px(hwnd, 16),
                (rc.right - px(hwnd, 24)).max(1),
                px(hwnd, 25),
                0,
            );
            move_window(
                s.status,
                px(hwnd, 12),
                px(hwnd, 49),
                (rc.right - px(hwnd, 24)).max(1),
                px(hwnd, 20),
                0,
            );
            crate::scroll::resize(
                s.tree,
                px(hwnd, 4),
                px(hwnd, 78),
                (rc.right - px(hwnd, 8)).max(1),
                (rc.bottom - px(hwnd, 82)).max(1),
            );
        }),
        WM_COMMAND if wp >> 16 == EN_CHANGE as usize => with(hwnd, |s| {
            s.open_when_ready = false;
            s.cancel.store(true, Ordering::Relaxed);
            s.receiver = None;
            KillTimer(hwnd, 2);
            SetTimer(hwnd, 1, 180, None);
        }),
        WM_TIMER => with(hwnd, |s| {
            if wp == 1 {
                s.start();
            } else if wp == 2 {
                s.tick();
            }
        }),
        WM_NOTIFY if lp != 0 => {
            let hdr = &*(lp as *const NMHDR);
            if hdr.code == NM_CUSTOMDRAW {
                let draw = &mut *(lp as *mut NMTVCUSTOMDRAW);
                if draw.nmcd.dwDrawStage == CDDS_PREPAINT {
                    return CDRF_NOTIFYITEMDRAW as isize;
                }
                if draw.nmcd.dwDrawStage == CDDS_ITEMPREPAINT {
                    let selected = draw.nmcd.uItemState & CDIS_SELECTED != 0;
                    draw.nmcd.uItemState &= !(CDIS_HOT | CDIS_FOCUS);
                    draw.clrText = if selected {
                        ACCENT
                    } else if draw.nmcd.lItemlParam < 0 {
                        MUTED
                    } else {
                        INK
                    };
                    draw.clrTextBk = if selected { SELECTED } else { SURFACE };
                    return (CDRF_NEWFONT | CDRF_NOTIFYPOSTPAINT) as isize;
                }
                if draw.nmcd.dwDrawStage == CDDS_ITEMPOSTPAINT {
                    with(hwnd, |s| s.highlight(draw));
                }
            }
            if hdr.code == NM_CLICK {
                let mut hit: TVHITTESTINFO = zeroed();
                GetCursorPos(&mut hit.pt);
                ScreenToClient(hdr.hwndFrom, &mut hit.pt);
                SendMessageW(hdr.hwndFrom, TVM_HITTEST, 0, &mut hit as *mut _ as isize);
                if hit.flags & (TVHT_ONITEMLABEL | TVHT_ONITEMICON) != 0 {
                    PostMessageW(hwnd, OPEN_RESULT, 0, 0);
                }
            }
        }
        OPEN_RESULT => with(hwnd, |s| s.open_selected()),
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let dc = wp as HDC;
            SetTextColor(dc, if msg == WM_CTLCOLORSTATIC { MUTED } else { INK });
            let bg = if msg == WM_CTLCOLOREDIT {
                FIELD
            } else {
                SURFACE
            };
            SetBkColor(dc, bg);
            SetDCBrushColor(dc, bg);
            return GetStockObject(DC_BRUSH) as isize;
        }
        WM_ERASEBKGND => {
            let mut rc = zeroed();
            GetClientRect(hwnd, &mut rc);
            fill(wp as HDC, rc, SURFACE);
            return 1;
        }
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            fill(dc, ps.rcPaint, SURFACE);
            with(hwnd, |s| input_frame(hwnd, dc, s.edit));
            EndPaint(hwnd, &ps);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

#[test]
fn streaming_search_offsets_limits_and_cancellation() {
    use crate::document::{decode, encode, Chunk};
    let root = std::env::temp_dir().join(format!("plumetxt-search-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/hidden.txt"), "needle").unwrap();
    std::fs::write(root.join("binary.bin"), b"\0needle").unwrap();
    let source = format!(
        "# 中文 😀\r\n{}NEEDLE 中文 needle\r\nlast",
        "x".repeat(70000)
    );
    let cancel = AtomicBool::new(false);
    for (index, encoding) in [
        Encoding::Utf8,
        Encoding::Utf8Bom,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ]
    .into_iter()
    .enumerate()
    {
        let path = root.join(format!("note{index}.md"));
        std::fs::write(&path, encode(&source, encoding, true)).unwrap();
        let mut hits = Vec::new();
        assert_eq!(
            scan_file(&path, "needle", &cancel, &mut |h| {
                hits.push(h);
                true
            }),
            Ok(false)
        );
        assert_eq!(hits.len(), 2);
        let (decoded, _) = decode(&std::fs::read(&path).unwrap()).unwrap();
        let bom = match encoding {
            Encoding::Utf8 => 0,
            Encoding::Utf8Bom => 3,
            _ => 2,
        };
        for hit in &hits {
            assert_eq!(hit.line, 2);
            let selected = hit.selection(&decoded, encoding, bom).unwrap();
            assert_eq!(Some(selected), hit.current_selection(&decoded));
            let normalized: Vec<u16> = decoded.replace("\r\n", "\r").encode_utf16().collect();
            assert_eq!(
                String::from_utf16(&normalized[selected.0 as usize..selected.1 as usize]).unwrap(),
                hit.matched
            );
            let (chunk, text) = Chunk::read(&path, hit.offset - 4096).unwrap();
            assert!(hit.selection(&text, chunk.encoding, chunk.start).is_some());
        }
    }
    let (tx, rx) = mpsc::sync_channel(256);
    scan(
        root.clone(),
        "needle".into(),
        Arc::new(AtomicBool::new(false)),
        tx,
    );
    let events: Vec<_> = rx.into_iter().collect();
    assert_eq!(
        events.iter().filter(|e| matches!(e, Event::Hit(_))).count(),
        8
    );
    assert!(matches!(
        events.last(),
        Some(Event::Done {
            skipped: 1,
            limited: false
        })
    ));
    assert_eq!(ranges("İ 中文 Ä ä", "ä"), vec![(10, 12), (13, 15)]);
    assert_eq!(ranges("中文 中文", "中文"), vec![(0, 6), (7, 13)]);
    assert!(ranges("anything", "").is_empty());
    let long = root.join("long.txt");
    std::fs::write(
        &long,
        format!("{}needle\nneedle", "x".repeat(MAX_LINE + 32)),
    )
    .unwrap();
    let mut hits = Vec::new();
    assert_eq!(
        scan_file(&long, "needle", &cancel, &mut |h| {
            hits.push(h);
            true
        }),
        Ok(true)
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 2);
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        scan_file(&long, "needle", &cancel, &mut |_| panic!(
            "Cancelled scan emitted a result"
        )),
        Ok(false)
    );
    let capped = root.join("cap");
    std::fs::create_dir_all(&capped).unwrap();
    std::fs::write(capped.join("many.txt"), "needle\n".repeat(LIMIT + 2)).unwrap();
    let (tx, rx) = mpsc::sync_channel(256);
    let worker = std::thread::spawn(move || {
        scan(
            capped,
            "needle".into(),
            Arc::new(AtomicBool::new(false)),
            tx,
        )
    });
    let events: Vec<_> = rx.into_iter().collect();
    worker.join().unwrap();
    assert_eq!(
        events.iter().filter(|e| matches!(e, Event::Hit(_))).count(),
        LIMIT
    );
    assert!(matches!(
        events.last(),
        Some(Event::Done { limited: true, .. })
    ));
    std::fs::remove_dir_all(root).unwrap();
}
