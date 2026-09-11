//! Bounded, read-only text viewport. No full-file allocation or line index.
use crate::theme::*;
use std::{
    cell::RefCell,
    fs::File,
    io::{Read, Seek, SeekFrom},
    mem::zeroed,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        mpsc::{channel, Sender},
        Arc, Mutex,
    },
};
use unicode_width::UnicodeWidthChar;
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::SetScrollInfo, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
const READY: u32 = WM_APP + 130;
// ponytail: byte-based navigation avoids an O(file size) line index; add a background index only when exact line navigation is needed.
const RANGE: i32 = 1_000_000;
const BLOCK: u64 = 256 * 1024;
pub const THRESHOLD: u64 = 8 * 1024 * 1024;
#[derive(Clone, Copy)]
struct Request {
    id: u64,
    offset: u64,
    columns: usize,
    back: usize,
}
struct Row {
    start: u64,
    text: String,
    colors: Vec<u32>,
    line_end: bool,
}
struct Page {
    id: u64,
    len: u64,
    rows: Vec<Row>,
    #[cfg(test)]
    read: usize,
}
struct Source {
    file: File,
    utf16: Option<bool>,
    bom: u64,
    language: crate::syntax::Language,
}
impl Source {
    fn open(path: &std::path::Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let mut prefix = [0; 1024];
        let n = file.read(&mut prefix).map_err(|e| e.to_string())?;
        let utf16 = if n >= 2 && prefix[..2] == [0xff, 0xfe] {
            Some(true)
        } else if n >= 2 && prefix[..2] == [0xfe, 0xff] {
            Some(false)
        } else {
            None
        };
        let bom = if utf16.is_some() {
            2
        } else if n >= 3 && prefix[..3] == [0xef, 0xbb, 0xbf] {
            3
        } else {
            0
        };
        let header = if let Some(le) = utf16 {
            let units: Vec<_> = prefix[2..n]
                .chunks_exact(2)
                .map(|p| {
                    if le {
                        u16::from_le_bytes([p[0], p[1]])
                    } else {
                        u16::from_be_bytes([p[0], p[1]])
                    }
                })
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(&prefix[bom as usize..n]).into_owned()
        };
        let language = crate::syntax::Language::detect(path, &header);
        Ok(Self {
            file,
            utf16,
            bom,
            language,
        })
    }
    fn page(&mut self, req: Request) -> Result<Page, String> {
        let len = self.file.metadata().map_err(|e| e.to_string())?.len();
        let target = req.offset.min(len).max(self.bom);
        let mut start = if req.back > 0 {
            target.saturating_sub(BLOCK)
        } else {
            target
        }
        .max(self.bom);
        if self.utf16.is_some() {
            start -= (start - self.bom) % 2;
        }
        self.file
            .seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::with_capacity((BLOCK * 2) as usize);
        (&mut self.file)
            .take(BLOCK * 2)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        #[cfg(test)]
        let read = bytes.len();
        let mut pos = 0;
        if self.utf16.is_none() {
            while pos < bytes.len() && bytes[pos] & 0xc0 == 0x80 {
                pos += 1;
            }
        } else if bytes.len() >= 2 {
            let u = if self.utf16 == Some(true) {
                u16::from_le_bytes([bytes[0], bytes[1]])
            } else {
                u16::from_be_bytes([bytes[0], bytes[1]])
            };
            if (0xdc00..=0xdfff).contains(&u) {
                pos = 2;
            }
        }
        let columns = req.columns.clamp(16, 240);
        let mut rows = Vec::new();
        let mut text = String::new();
        let mut row_start = start + pos as u64;
        let mut count = 0;
        while pos < bytes.len() {
            let (ch, width) = if let Some(le) = self.utf16 {
                if pos + 1 >= bytes.len() {
                    break;
                }
                let unit = |p| {
                    if le {
                        u16::from_le_bytes([bytes[p], bytes[p + 1]])
                    } else {
                        u16::from_be_bytes([bytes[p], bytes[p + 1]])
                    }
                };
                let a = unit(pos);
                if (0xd800..=0xdbff).contains(&a)
                    && pos + 3 < bytes.len()
                    && (0xdc00..=0xdfff).contains(&unit(pos + 2))
                {
                    (
                        char::from_u32(
                            0x10000 + ((a as u32 - 0xd800) << 10) + unit(pos + 2) as u32 - 0xdc00,
                        )
                        .unwrap(),
                        4,
                    )
                } else {
                    (char::from_u32(a as u32).unwrap_or('\u{fffd}'), 2)
                }
            } else {
                let b = bytes[pos];
                let n = if b < 128 {
                    1
                } else if b & 0xe0 == 0xc0 {
                    2
                } else if b & 0xf0 == 0xe0 {
                    3
                } else if b & 0xf8 == 0xf0 {
                    4
                } else {
                    1
                };
                if pos + n > bytes.len() {
                    break;
                }
                match std::str::from_utf8(&bytes[pos..pos + n])
                    .ok()
                    .and_then(|s| s.chars().next())
                {
                    Some(c) => (c, n),
                    None => ('\u{fffd}', 1),
                }
            };
            pos += width;
            let cells = if ch == '\t' {
                8 - count % 8
            } else {
                ch.width().unwrap_or(1).max(1)
            };
            if ch == '\n' || count + cells > columns {
                rows.push(Row {
                    start: row_start,
                    text: std::mem::take(&mut text),
                    colors: Vec::new(),
                    line_end: ch == '\n',
                });
                row_start = start + pos as u64 - if ch == '\n' { 0 } else { width as u64 };
                count = 0;
            }
            if ch != '\n' && ch != '\r' {
                text.push(if ch == '\0' { '\u{fffd}' } else { ch });
                count += cells;
            }
            if rows.len() >= 512 && row_start >= target {
                break;
            }
        }
        if !text.is_empty() {
            rows.push(Row {
                start: row_start,
                text,
                colors: Vec::new(),
                line_end: false,
            });
        }
        let at = rows
            .partition_point(|r| r.start < target)
            .saturating_sub(req.back);
        let mut rows: Vec<Row> = rows.into_iter().skip(at).take(256).collect();
        // Only colour the bounded page on the worker; never scan the full file.
        let mut source = String::new();
        for row in &rows {
            source.push_str(&row.text);
            if row.line_end {
                source.push('\n');
            }
        }
        let colors = crate::syntax::colors(&source, self.language);
        let mut offset = 0;
        for row in &mut rows {
            let raw = std::mem::take(&mut row.text);
            let mut column = 0;
            for (byte, ch) in raw.char_indices() {
                let width = if ch == '\t' {
                    8 - column % 8
                } else {
                    ch.width().unwrap_or(1).max(1)
                };
                if ch == '\t' {
                    row.text.extend(std::iter::repeat_n(' ', width));
                    row.colors
                        .extend(std::iter::repeat_n(colors[offset + byte], width));
                } else {
                    row.text.push(ch);
                    row.colors
                        .extend_from_slice(&colors[offset + byte..offset + byte + ch.len_utf8()]);
                }
                column += width;
            }
            offset += raw.len() + usize::from(row.line_end);
        }
        Ok(Page {
            id: req.id,
            len,
            rows,
            #[cfg(test)]
            read,
        })
    }
}
pub struct Large(pub HWND);
struct State {
    sender: Sender<Request>,
    result: Arc<Mutex<Option<Result<Page, String>>>>,
    page: Option<Page>,
    font: HFONT,
    buffer: Buffer,
    generation: u64,
    requested: u64,
    columns: usize,
    message: String,
    wheel: i32,
}
impl Large {
    pub unsafe fn create(parent: HWND, path: PathBuf, font: HFONT) -> Self {
        let class = wide("FeatherPadLarge");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        });
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Large file · Read only").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_CLIPCHILDREN,
            0,
            0,
            800,
            600,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let (sender, rx) = channel::<Request>();
        let result = Arc::new(Mutex::new(None));
        let output = result.clone();
        let address = hwnd as usize;
        std::thread::spawn(move || {
            let mut source = Source::open(&path);
            while let Ok(mut req) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    req = newer;
                }
                let page = match &mut source {
                    Ok(s) => s.page(req),
                    Err(e) => Err(e.clone()),
                };
                *output.lock().unwrap() = Some(page);
                PostMessageW(address as HWND, READY, 0, 0);
            }
        });
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::into_raw(Box::new(RefCell::new(State {
                sender,
                result,
                page: None,
                font,
                buffer: Buffer::default(),
                generation: 0,
                requested: 0,
                columns: 80,
                message: "Loading…".into(),
                wheel: 0,
            }))) as isize,
        );
        crate::scroll::attach(hwnd, CANVAS);
        SendMessageW(hwnd, WM_SIZE, 0, 0);
        Self(hwnd)
    }
}
impl Drop for Large {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
impl State {
    fn request(&mut self, offset: u64, back: usize) {
        self.generation += 1;
        self.requested = offset;
        let _ = self.sender.send(Request {
            id: self.generation,
            offset,
            columns: self.columns,
            back,
        });
    }
    fn move_rows(&mut self, delta: i32) {
        if let Some(page) = &self.page {
            if delta < 0 {
                self.request(
                    page.rows.first().map_or(0, |r| r.start),
                    delta.unsigned_abs() as usize,
                );
            } else if let Some(row) = page.rows.get(delta as usize).or_else(|| page.rows.last()) {
                let offset = row.start;
                self.request(offset, 0);
            }
        }
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_ERASEBKGND {
        return 1;
    }
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<State>;
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !p.is_null() {
            drop(Box::from_raw(p));
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if p.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let Ok(mut s) = (*p).try_borrow_mut() else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        WM_SIZE => {
            // Native scrollbar visibility can send WM_SIZE without resizing the window.
            // Use the outer width so hiding its track cannot trigger a read/resize loop.
            let mut bounds = zeroed();
            GetWindowRect(hwnd, &mut bounds);
            let columns = ((bounds.right - bounds.left - 80) / 11).clamp(16, 240) as usize;
            if columns != s.columns || s.generation == 0 {
                s.columns = columns;
                let offset = s.requested;
                s.request(offset, 0);
            }
            invalidate(hwnd);
        }
        READY => {
            let result = s.result.lock().unwrap().take();
            match result {
                Some(Ok(page)) if page.id == s.generation => {
                    s.message = String::new();
                    s.requested = page.rows.first().map_or(0, |r| r.start);
                    let info = SCROLLINFO {
                        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                        fMask: SIF_RANGE | SIF_POS | SIF_PAGE,
                        nMin: 0,
                        nMax: RANGE,
                        nPage: 1,
                        nPos: (s.requested as f64 / page.len.max(1) as f64 * RANGE as f64) as i32,
                        ..zeroed()
                    };
                    SetScrollInfo(hwnd, SB_VERT, &info, 0);
                    s.page = Some(page);
                    PostMessageW(hwnd, WM_APP + 93, 0, 0);
                }
                Some(Err(e)) => s.message = e,
                _ => (),
            }
            invalidate(hwnd);
        }
        crate::reader::SCROLL_TO => {
            if let Some(page) = &s.page {
                let offset = (lp as f64 / RANGE as f64 * page.len as f64) as u64;
                let back = if lp >= RANGE as isize {
                    ((client(hwnd).bottom - 48) / 28).max(1) as usize
                } else {
                    0
                };
                s.request(offset, back);
            }
        }
        WM_MOUSEWHEEL => {
            s.wheel += (wp >> 16) as u16 as i16 as i32;
            let rows = s.wheel / 40;
            s.wheel %= 40;
            if rows != 0 {
                s.move_rows(-rows);
            }
        }
        WM_LBUTTONDOWN => {
            SetFocus(hwnd);
        }
        WM_KEYDOWN => match wp as u16 {
            VK_DOWN => s.move_rows(1),
            VK_UP => s.move_rows(-1),
            VK_NEXT => s.move_rows(((client(hwnd).bottom - 48) / 28).max(1)),
            VK_PRIOR => s.move_rows(-((client(hwnd).bottom - 48) / 28).max(1)),
            VK_HOME => s.request(0, 0),
            VK_END => {
                if let Some(page) = &s.page {
                    let end = page.len;
                    s.request(end, ((client(hwnd).bottom - 48) / 28).max(1) as usize);
                }
            }
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        },
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let rc = client(hwnd);
            if s.buffer.ensure(dc, rc.right, rc.bottom) {
                let canvas = s.buffer.dc;
                fill(canvas, rc, CANVAS);
                if let Some(page) = &s.page {
                    for (i, row) in page
                        .rows
                        .iter()
                        .take(((rc.bottom - 48) / 28).max(1) as usize)
                        .enumerate()
                    {
                        let old = SelectObject(canvas, s.font);
                        SetBkMode(canvas, TRANSPARENT as i32);
                        let mut x = 32;
                        let mut start = 0;
                        for end in row
                            .text
                            .char_indices()
                            .map(|(n, _)| n)
                            .skip(1)
                            .chain(std::iter::once(row.text.len()))
                        {
                            if end < row.text.len() && row.colors[end] == row.colors[start] {
                                continue;
                            }
                            let run = wide(&row.text[start..end]);
                            SetTextColor(canvas, row.colors.get(start).copied().unwrap_or(INK));
                            TextOutW(
                                canvas,
                                x,
                                24 + i as i32 * 28,
                                run.as_ptr(),
                                run.len() as i32 - 1,
                            );
                            let mut size: SIZE = zeroed();
                            GetTextExtentPoint32W(
                                canvas,
                                run.as_ptr(),
                                run.len() as i32 - 1,
                                &mut size,
                            );
                            x += size.cx;
                            start = end;
                            if x >= rc.right - 28 {
                                break;
                            }
                        }
                        SelectObject(canvas, old);
                    }
                }
                if !s.message.is_empty() {
                    label(
                        canvas,
                        &s.message,
                        RECT {
                            left: 32,
                            top: 24,
                            right: rc.right - 32,
                            bottom: 80,
                        },
                        s.font,
                        MUTED,
                        DT_SINGLELINE,
                    );
                }
                s.buffer.blit(dc, &ps.rcPaint);
            }
            EndPaint(hwnd, &ps);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

#[test]
fn tabular_highlighting_survives_display_wrapping() {
    let path = std::env::temp_dir().join(format!("featherpad-table-{}.tsv", std::process::id()));
    std::fs::write(
        &path,
        "name\tcity\tnote\nAlice\tParis\t\"long quoted field across wrapping\"\n",
    )
    .unwrap();
    let page = Source::open(&path)
        .unwrap()
        .page(Request {
            id: 1,
            offset: 0,
            columns: 16,
            back: 0,
        })
        .unwrap();
    let paris = page.rows.iter().find(|r| r.text.contains("Paris")).unwrap();
    assert_eq!(
        paris.colors[paris.text.find("Paris").unwrap()],
        rgb(218, 185, 130)
    );
    for row in &page.rows {
        assert_eq!(row.text.len(), row.colors.len());
        if let Some(at) = row.text.find("wrapping") {
            assert_eq!(row.colors[at], rgb(145, 206, 180));
        }
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn bounded_reads_and_unicode_boundaries() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("featherpad-large-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("large.md");
    let mut f = File::create(&path).unwrap();
    f.write_all("# Start\n中文 😀\n".as_bytes()).unwrap();
    f.set_len(1024 * 1024 * 1024).unwrap();
    f.seek(SeekFrom::End(-11)).unwrap();
    f.write_all(b"\n# The end\n").unwrap();
    drop(f);
    let mut source = Source::open(&path).unwrap();
    let page = source
        .page(Request {
            id: 1,
            offset: 0,
            columns: 80,
            back: 0,
        })
        .unwrap();
    assert_eq!(page.len, 1024 * 1024 * 1024);
    assert_eq!(page.rows[0].colors[0], ACCENT);
    assert!(page
        .rows
        .iter()
        .all(|row| row.colors.len() == row.text.len()));
    assert!(page.read <= BLOCK as usize * 2);
    assert_eq!(page.rows[1].text, "中文 😀");
    let end = source
        .page(Request {
            id: 2,
            offset: page.len,
            columns: 80,
            back: 10,
        })
        .unwrap();
    assert!(end.rows.iter().any(|r| r.text == "# The end"));
    assert!(end.rows.windows(2).all(|r| r[1].start > r[0].start));
    drop(source);
    for encoding in [
        crate::document::Encoding::Utf16Le,
        crate::document::Encoding::Utf16Be,
    ] {
        std::fs::write(
            &path,
            crate::document::encode("中文 😀\nSecond\n", encoding, false),
        )
        .unwrap();
        let mut source = Source::open(&path).unwrap();
        let req = Request {
            id: 1,
            offset: 0,
            columns: 80,
            back: 0,
        };
        assert_eq!(source.page(req).unwrap().rows[0].text, "中文 😀");
        let boundary = source.page(Request { offset: 9, ..req }).unwrap();
        assert!(boundary.rows.iter().all(|r| !r.text.contains('\u{fffd}')));
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
#[ignore = "Creates a 1 GiB Markdown benchmark; requires Windows controls"]
fn native_gigabyte_first_viewport() {
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    let path = std::env::temp_dir().join(format!("featherpad-gigabyte-{}.md", std::process::id()));
    let mut file = File::create(&path).unwrap();
    let mut block = b"# Markdown performance\n\nA large document should show its first screen without loading everything.\n\n".repeat(65536);
    block.resize(8 * 1024 * 1024, b' ');
    for _ in 0..128 {
        file.write_all(&block).unwrap();
    }
    file.sync_all().unwrap();
    drop(file);
    drop(block);
    unsafe {
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("Large file test").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            1000,
            700,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let fonts = Fonts::new();
        let started = Instant::now();
        let view = Large::create(parent, path.clone(), fonts.code);
        let state = GetWindowLongPtrW(view.0, GWLP_USERDATA) as *const RefCell<State>;
        let wait = || {
            let start = Instant::now();
            loop {
                let mut msg: MSG = zeroed();
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let s = (*state).borrow();
                if s.page.as_ref().is_some_and(|p| p.id == s.generation) {
                    break;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "Large viewport timed out: {}",
                    s.message
                );
                drop(s);
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        wait();
        eprintln!(
            "1 GiB full-content file, warm-cache first viewport: {:?}; bytes read: {}",
            started.elapsed(),
            (*state).borrow().page.as_ref().unwrap().read
        );
        assert!((*state).borrow().page.as_ref().unwrap().rows[0]
            .text
            .starts_with("# Markdown"));
        let jumped = Instant::now();
        SendMessageW(view.0, crate::reader::SCROLL_TO, 1, (RANGE / 2) as isize);
        wait();
        assert!((*state).borrow().requested > 500 * 1024 * 1024);
        SendMessageW(view.0, WM_KEYDOWN, VK_END as usize, 0);
        wait();
        assert!((*state).borrow().requested > 1023 * 1024 * 1024);
        let generation = (*state).borrow().generation;
        SendMessageW(view.0, WM_SIZE, 0, 0);
        assert_eq!(
            (*state).borrow().generation,
            generation,
            "Unchanged size must not reload or lose the end position"
        );
        eprintln!("Middle + end seeks: {:?}", jumped.elapsed());
        drop(view);
        DestroyWindow(parent);
    }
    std::fs::remove_file(path).unwrap();
}
