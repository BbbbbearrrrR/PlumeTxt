use crate::theme::*;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::{
    cell::RefCell,
    io::{Read, Write},
    mem::zeroed,
    path::Path,
    ptr::{null, null_mut},
    sync::mpsc::{channel, sync_channel, Receiver, Sender},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{DataExchange::*, LibraryLoader::GetModuleHandleW, Memory::*},
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
const READY: u32 = WM_APP + 110;
const RESIZE: u32 = WM_APP + 111;
struct Session {
    writer: Sender<Vec<u8>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
enum Event {
    Started(Session),
    Output(Vec<u8>),
    Error(String),
    Exited,
}
struct State {
    hwnd: HWND,
    events: Receiver<Event>,
    session: Option<Session>,
    exited: bool,
    parser: vt100::Parser,
    font: HFONT,
    cell_w: i32,
    cell_h: i32,
    buffer: Buffer,
    surrogate: Option<u16>,
    selection: Option<(usize, usize)>,
    selecting: bool,
}
pub struct Terminal(pub HWND);
impl Terminal {
    pub unsafe fn create(parent: HWND, cwd: &Path) -> Self {
        let name = wide("FeatherPadTerminal");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_IBEAM),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("PowerShell").as_ptr(),
            WS_CHILD | WS_CLIPSIBLINGS | WS_TABSTOP,
            0,
            0,
            800,
            260,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let font = font(17, 400, "Consolas");
        let dc = GetDC(hwnd);
        let old = SelectObject(dc, font);
        let mut metrics = zeroed();
        GetTextMetricsW(dc, &mut metrics);
        SelectObject(dc, old);
        ReleaseDC(hwnd, dc);
        let (tx, events) = sync_channel(8);
        let target = hwnd as usize;
        let cwd = cwd.to_path_buf();
        std::thread::spawn(move || {
            let start = (|| -> Result<(Session, Box<dyn Read + Send>), String> {
                let pair = native_pty_system()
                    .openpty(PtySize {
                        rows: 12,
                        cols: 90,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .map_err(|e| e.to_string())?;
                let mut command = CommandBuilder::new("powershell.exe");
                command.args(["-NoLogo", "-NoProfile"]);
                command.cwd(&cwd);
                command.env("TERM", "xterm-256color");
                let child = pair
                    .slave
                    .spawn_command(command)
                    .map_err(|e| e.to_string())?;
                drop(pair.slave);
                let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
                let mut raw_writer = pair.master.take_writer().map_err(|e| e.to_string())?;
                let (writer, input) = channel::<Vec<u8>>();
                std::thread::spawn(move || {
                    while let Ok(bytes) = input.recv() {
                        if raw_writer.write_all(&bytes).is_err() {
                            break;
                        }
                    }
                });
                Ok((
                    Session {
                        master: pair.master,
                        writer,
                        child,
                    },
                    reader,
                ))
            })();
            let (session, mut reader) = match start {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Event::Error(e));
                    PostMessageW(target as HWND, READY, 0, 0);
                    return;
                }
            };
            if tx.send(Event::Started(session)).is_err() {
                return;
            }
            PostMessageW(target as HWND, READY, 0, 0);
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(Event::Output(buf[..n].to_vec())).is_err() {
                            return;
                        }
                        PostMessageW(target as HWND, READY, 0, 0);
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(Event::Exited);
            PostMessageW(target as HWND, READY, 0, 0);
        });
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::into_raw(Box::new(RefCell::new(State {
                hwnd,
                session: None,
                exited: false,
                events,
                parser: vt100::Parser::new(12, 90, 2000),
                font,
                cell_w: metrics.tmAveCharWidth.max(8),
                cell_h: metrics.tmHeight.max(18),
                buffer: Buffer::default(),
                surrogate: None,
                selection: None,
                selecting: false,
            }))) as isize,
        );
        Self(hwnd)
    }
    pub unsafe fn exited(&self) -> bool {
        let p = GetWindowLongPtrW(self.0, GWLP_USERDATA) as *const RefCell<State>;
        !p.is_null() && (*p).borrow().exited
    }
    pub unsafe fn contains(&self, hwnd: HWND) -> bool {
        hwnd == self.0 || IsChild(self.0, hwnd) != 0
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
        }
    }
}
fn color(c: vt100::Color, default: u32) -> u32 {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => rgb(r as u32, g as u32, b as u32),
        vt100::Color::Idx(n) => {
            let basic = [
                rgb(20, 26, 29),
                rgb(237, 111, 117),
                rgb(110, 207, 146),
                rgb(232, 196, 114),
                rgb(114, 167, 230),
                rgb(188, 144, 218),
                ACCENT,
                rgb(222, 232, 233),
                rgb(113, 132, 137),
                rgb(255, 145, 151),
                rgb(146, 229, 177),
                rgb(246, 219, 153),
                rgb(151, 195, 249),
                rgb(212, 177, 236),
                rgb(121, 239, 223),
                WHITE,
            ];
            if n < 16 {
                basic[n as usize]
            } else if n >= 232 {
                let v = 8 + 10 * (n as u32 - 232);
                rgb(v, v, v)
            } else {
                let n = n as u32 - 16;
                let part = |v| if v == 0 { 0 } else { 55 + v * 40 };
                rgb(part(n / 36), part(n / 6 % 6), part(n % 6))
            }
        }
    }
}
impl State {
    fn write(&mut self, bytes: &[u8]) {
        self.parser.screen_mut().set_scrollback(0);
        self.selection = None;
        if let Some(s) = &mut self.session {
            if let Err(e) = s.writer.send(bytes.to_vec()) {
                self.parser.process(format!("\r\n{e}").as_bytes());
            }
        }
    }
    unsafe fn resize(&mut self) {
        let rc = client(self.hwnd);
        let rows = ((rc.bottom - 16) / self.cell_h).clamp(2, 200) as u16;
        let cols = ((rc.right - 32) / self.cell_w).clamp(10, 400) as u16;
        self.parser.screen_mut().set_size(rows, cols);
        if let Some(s) = &self.session {
            let _ = s.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        invalidate(self.hwnd);
    }
    unsafe fn output(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Started(s) => {
                    self.session = Some(s);
                    self.resize();
                }
                Event::Output(bytes) => {
                    self.parser.process(&bytes);
                    if bytes.windows(4).any(|b| b == b"\x1b[6n") {
                        let (r, c) = self.parser.screen().cursor_position();
                        if let Some(s) = &mut self.session {
                            let _ = s
                                .writer
                                .send(format!("\x1b[{};{}R", r + 1, c + 1).into_bytes());
                        }
                    }
                }
                Event::Error(e) => {
                    self.exited = true;
                    self.parser.process(e.as_bytes());
                }
                Event::Exited => {
                    self.exited = true;
                    self.parser.process(b"\r\n[Process exited]");
                }
            }
        }
        if IsWindowVisible(self.hwnd) != 0 {
            invalidate(self.hwnd);
        }
    }
    unsafe fn paint(&mut self, dc: HDC, dirty: &RECT) {
        let rc = client(self.hwnd);
        if !self.buffer.ensure(dc, rc.right, rc.bottom) {
            return;
        }
        let target = self.buffer.dc;
        fill(target, rc, SURFACE);
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let cursor = (GetFocus() == self.hwnd && !screen.hide_cursor() && screen.scrollback() == 0)
            .then(|| {
                let (row, col) = screen.cursor_position();
                let continuation = screen
                    .cell(row, col)
                    .is_some_and(|cell| cell.is_wide_continuation());
                (row, col.saturating_sub(u16::from(continuation)))
            });
        let old = SelectObject(target, self.font);
        SetBkMode(target, TRANSPARENT as i32);
        for row in 0..rows {
            for col in 0..cols {
                if let Some(cell) = screen.cell(row, col) {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let mut fg = color(cell.fgcolor(), INK);
                    let mut bg = color(cell.bgcolor(), SURFACE);
                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    let n = row as usize * cols as usize + col as usize;
                    if self
                        .selection
                        .is_some_and(|(a, b)| n >= a.min(b) && n <= a.max(b))
                    {
                        bg = SELECTED;
                    }
                    if cursor == Some((row, col)) {
                        bg = ACCENT;
                        fg = SURFACE;
                    }
                    let x = 16 + col as i32 * self.cell_w;
                    let y = 8 + row as i32 * self.cell_h;
                    let r = RECT {
                        left: x,
                        top: y,
                        right: x + self.cell_w * if cell.is_wide() { 2 } else { 1 },
                        bottom: y + self.cell_h,
                    };
                    if bg != SURFACE {
                        fill(target, r, bg);
                    }
                    if cell.contents().is_empty() {
                        continue;
                    }
                    SetTextColor(target, fg);
                    let text = wide(cell.contents());
                    ExtTextOutW(
                        target,
                        x,
                        y,
                        ETO_CLIPPED,
                        &r,
                        text.as_ptr(),
                        (text.len() - 1) as u32,
                        null(),
                    );
                }
            }
        }
        SelectObject(target, old);
        self.buffer.blit(dc, dirty);
    }
    fn index(&self, lp: isize) -> usize {
        let (rows, cols) = self.parser.screen().size();
        let x = (lp as u16 as i16 as i32 - 16) / self.cell_w;
        let y = ((lp >> 16) as u16 as i16 as i32 - 8) / self.cell_h;
        y.clamp(0, rows as i32 - 1) as usize * cols as usize + x.clamp(0, cols as i32 - 1) as usize
    }
    unsafe fn paste(&mut self) {
        if OpenClipboard(self.hwnd) == 0 {
            return;
        }
        let handle = GetClipboardData(13);
        if !handle.is_null() {
            let p = GlobalLock(handle) as *const u16;
            if !p.is_null() {
                let len = (GlobalSize(handle) / 2).min(1024 * 1024);
                let slice = std::slice::from_raw_parts(p, len);
                let end = slice.iter().position(|&c| c == 0).unwrap_or(len);
                let text = String::from_utf16_lossy(&slice[..end]);
                GlobalUnlock(handle);
                self.write(text.as_bytes());
            }
        }
        CloseClipboard();
    }
    unsafe fn copy(&self) {
        let screen = self.parser.screen();
        let (_, cols) = screen.size();
        let text = if let Some((a, b)) = self.selection {
            let mut text = String::new();
            for n in a.min(b)..=a.max(b) {
                if n != a.min(b) && n % cols as usize == 0 {
                    text.push('\n');
                }
                if let Some(c) = screen.cell((n / cols as usize) as u16, (n % cols as usize) as u16)
                {
                    if !c.is_wide_continuation() {
                        text.push_str(c.contents());
                    }
                }
            }
            text
        } else {
            screen.contents()
        };
        let data = wide(&text);
        if OpenClipboard(self.hwnd) == 0 {
            return;
        }
        let mem = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
        if !mem.is_null() {
            let p = GlobalLock(mem);
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u16, data.len());
                GlobalUnlock(mem);
                EmptyClipboard();
                if SetClipboardData(13, mem).is_null() {
                    GlobalFree(mem);
                }
            }
        }
        CloseClipboard();
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_ERASEBKGND {
        return 1;
    }
    if msg == WM_SIZE {
        let rc = client(hwnd);
        SetWindowRgn(
            hwnd,
            CreateRoundRectRgn(0, 0, rc.right + 1, rc.bottom + 1, 14, 14),
            1,
        );
        PostMessageW(hwnd, RESIZE, 0, 0);
        return 0;
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
        READY => s.output(),
        RESIZE => s.resize(),
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            s.paint(dc, &ps.rcPaint);
            EndPaint(hwnd, &ps);
        }
        WM_SETFOCUS | WM_KILLFOCUS => invalidate(hwnd),
        WM_LBUTTONDOWN => {
            SetFocus(hwnd);
            let n = s.index(lp);
            s.selection = Some((n, n));
            s.selecting = true;
            SetCapture(hwnd);
            invalidate(hwnd);
        }
        WM_MOUSEMOVE if s.selecting => {
            let n = s.index(lp);
            if let Some((a, _)) = s.selection {
                s.selection = Some((a, n));
            }
            invalidate(hwnd);
        }
        WM_LBUTTONUP => {
            s.selecting = false;
            ReleaseCapture();
        }
        WM_CAPTURECHANGED => s.selecting = false,
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as u16 as i16 as i32;
            let offset = (s.parser.screen().scrollback() as i32 + delta / 120 * 3).max(0) as usize;
            s.parser.screen_mut().set_scrollback(offset);
            invalidate(hwnd);
        }
        WM_KEYDOWN => {
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            let shift = GetKeyState(VK_SHIFT as i32) < 0;
            if ctrl && wp == 0x56 {
                s.paste();
            } else if ctrl && shift && wp == 0x43 {
                s.copy();
            } else {
                let bytes: Option<&[u8]> = match wp as u16 {
                    VK_UP => Some(b"\x1b[A"),
                    VK_DOWN => Some(b"\x1b[B"),
                    VK_RIGHT => Some(b"\x1b[C"),
                    VK_LEFT => Some(b"\x1b[D"),
                    VK_HOME => Some(b"\x1b[H"),
                    VK_END => Some(b"\x1b[F"),
                    VK_DELETE => Some(b"\x1b[3~"),
                    VK_PRIOR => Some(b"\x1b[5~"),
                    VK_NEXT => Some(b"\x1b[6~"),
                    _ => None,
                };
                if let Some(bytes) = bytes {
                    s.write(bytes);
                }
            }
            invalidate(hwnd);
        }
        WM_CHAR => {
            let ch = wp as u16;
            if ch == 22 || (ch == 3 && GetKeyState(VK_SHIFT as i32) < 0) {
                return 0;
            }
            if (0xd800..=0xdbff).contains(&ch) {
                s.surrogate = Some(ch);
            } else {
                let units = if let Some(high) = s.surrogate.take() {
                    vec![high, ch]
                } else {
                    vec![ch]
                };
                let text = String::from_utf16_lossy(&units);
                s.write(text.as_bytes());
            }
            invalidate(hwnd);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

#[test]
#[ignore = "Requires Windows ConPTY and PowerShell"]
fn native_terminal_uses_file_directory_and_keeps_session() {
    use std::time::{Duration, Instant};
    unsafe {
        let cwd = std::env::current_dir()
            .unwrap()
            .join("tmp")
            .join("terminal workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("Terminal test").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            900,
            500,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let terminal = Terminal::create(parent, &cwd);
        let state = GetWindowLongPtrW(terminal.0, GWLP_USERDATA) as *const RefCell<State>;
        let start = Instant::now();
        let mut sent = false;
        loop {
            let mut msg: MSG = zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let mut s = (*state).borrow_mut();
            if !sent && s.session.is_some() {
                s.write(b"Write-Output ('PTY_'+'OK'); (Get-Location).Path\r");
                sent = true;
            }
            let contents = s.parser.screen().contents();
            if contents.contains("PTY_OK") && contents.contains("terminal workspace") {
                assert!(s.session.is_some());
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "Terminal output: {contents}"
            );
            drop(s);
            std::thread::sleep(Duration::from_millis(15));
        }
        ShowWindow(terminal.0, SW_HIDE);
        assert!((*state).borrow().session.is_some());
        (*state)
            .borrow_mut()
            .write(b"1..100000 | ForEach-Object { 'terminal output backpressure test' }\r");
        std::thread::sleep(Duration::from_millis(200));
        let closing = Instant::now();
        drop(terminal);
        assert!(
            closing.elapsed() < Duration::from_secs(5),
            "Terminal shutdown blocked on output"
        );
        DestroyWindow(parent);
    }
}
