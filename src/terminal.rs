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
    UI::{
        Input::{Ime::*, KeyboardAndMouse::*},
        WindowsAndMessaging::*,
    },
};
const READY: u32 = WM_APP + 110;
const RESIZE: u32 = WM_APP + 111;
struct Session {
    writer: Sender<Vec<u8>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    size: (u16, u16),
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
#[derive(Default)]
struct Replies {
    bytes: Vec<u8>,
    sync_started: Option<std::time::Instant>,
    cursor_style: u16,
}
impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if i1 == Some(b'?') && params.contains(&&[2026][..]) {
            match (i2, c) {
                (None, 'h') => {
                    self.sync_started
                        .get_or_insert_with(std::time::Instant::now);
                }
                (None, 'l') => self.sync_started = None,
                (Some(b'$'), 'p') => self
                    .bytes
                    .extend_from_slice(if self.sync_started.is_some() {
                        b"\x1b[?2026;1$y"
                    } else {
                        b"\x1b[?2026;2$y"
                    }),
                _ => (),
            }
            return;
        }
        if i1 == Some(b' ') && i2.is_none() && c == 'q' {
            let style = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
            if style <= 6 {
                self.cursor_style = style;
            }
            return;
        }
        if i1.is_some() || i2.is_some() || c != 'n' {
            return;
        }
        if params == [&[6][..]] {
            let (row, col) = screen.cursor_position();
            self.bytes
                .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
        } else if params == [&[5][..]] {
            self.bytes.extend_from_slice(b"\x1b[0n");
        }
    }
}
#[derive(Default)]
struct Cursor {
    position: Option<(u16, u16)>,
    pending: Option<(Option<(u16, u16)>, std::time::Instant)>,
}
impl Cursor {
    fn update(&mut self, position: (u16, u16), hidden: bool, now: std::time::Instant) -> bool {
        let next = (!hidden).then_some(position);
        if let Some((candidate, since)) = self.pending {
            if now.duration_since(since).as_millis() >= 32 {
                self.position = candidate;
                self.pending = None;
            }
        }
        if next == self.position {
            self.pending = None;
            return false;
        }
        let (candidate, since) = self.pending.get_or_insert((next, now));
        if *candidate != next {
            *candidate = next;
            *since = now;
        }
        // ConPTY can restore a visible cursor one flush (~16 ms) AFTER DEC 2026 ends.
        // Keep the presented cursor until both its position and visibility settle.
        true
    }
}
struct State {
    hwnd: HWND,
    events: Receiver<Event>,
    session: Option<Session>,
    exited: bool,
    parser: vt100::Parser<Replies>,
    font: HFONT,
    cell_w: i32,
    cell_h: i32,
    renderer: crate::render::Renderer,
    surrogate: Option<u16>,
    selection: Option<(usize, usize)>,
    selecting: bool,
    frame_pending: bool,
    cursor: Cursor,
}
pub struct Terminal(pub HWND);
impl Terminal {
    pub unsafe fn create(parent: HWND, cwd: &Path) -> Self {
        let name = wide("PlumeTxtTerminal");
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
        let font = font(px(hwnd, 17), 400, "Consolas");
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
                        size: (12, 90),
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
                parser: vt100::Parser::new_with_callbacks(12, 90, 2000, Replies::default()),
                font,
                cell_w: metrics.tmAveCharWidth.max(8),
                cell_h: metrics.tmHeight.max(18),
                renderer: crate::render::Renderer::default(),
                surrogate: None,
                selection: None,
                selecting: false,
                frame_pending: false,
                cursor: Cursor::default(),
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
fn key_sequence(
    key: u16,
    ctrl: bool,
    shift: bool,
    alt: bool,
    application: bool,
) -> Option<Vec<u8>> {
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let letter = match key {
        VK_UP => Some('A'),
        VK_DOWN => Some('B'),
        VK_RIGHT => Some('C'),
        VK_LEFT => Some('D'),
        VK_HOME => Some('H'),
        VK_END => Some('F'),
        VK_F1 => Some('P'),
        VK_F2 => Some('Q'),
        VK_F3 => Some('R'),
        VK_F4 => Some('S'),
        _ => None,
    };
    if let Some(letter) = letter {
        return Some(if modifier != 1 {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else if application || (VK_F1..=VK_F4).contains(&key) {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        });
    }
    let number = match key {
        VK_INSERT => 2,
        VK_DELETE => 3,
        VK_PRIOR => 5,
        VK_NEXT => 6,
        VK_F5 => 15,
        VK_F6 => 17,
        VK_F7 => 18,
        VK_F8 => 19,
        VK_F9 => 20,
        VK_F10 => 21,
        VK_F11 => 23,
        VK_F12 => 24,
        VK_TAB if shift => return Some(b"\x1b[Z".to_vec()),
        VK_BACK => {
            return Some(if ctrl {
                vec![23]
            } else if alt {
                vec![27, 127]
            } else {
                vec![127]
            })
        }
        _ => return None,
    };
    Some(
        if modifier == 1 {
            format!("\x1b[{number}~")
        } else {
            format!("\x1b[{number};{modifier}~")
        }
        .into_bytes(),
    )
}
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let clean: String = normalized
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    if bracketed {
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        clean.replace('\n', "\r").into_bytes()
    }
}
impl State {
    unsafe fn position_ime(&self) {
        if GetFocus() != self.hwnd {
            return;
        }
        let (row, col) = self.parser.screen().cursor_position();
        let rc = client(self.hwnd);
        let point = POINT {
            x: (px(self.hwnd, 16) + i32::from(col) * self.cell_w)
                .clamp(0, (rc.right - self.cell_w).max(0)),
            y: (px(self.hwnd, 8) + i32::from(row) * self.cell_h)
                .clamp(0, (rc.bottom - self.cell_h).max(0)),
        };
        SetCaretPos(point.x, point.y);
        let context = ImmGetContext(self.hwnd);
        if !context.is_null() {
            ImmSetCompositionWindow(
                context,
                &COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: point,
                    rcArea: rc,
                },
            );
            ImmSetCandidateWindow(
                context,
                &CANDIDATEFORM {
                    dwIndex: 0,
                    dwStyle: CFS_EXCLUDE,
                    ptCurrentPos: point,
                    rcArea: RECT {
                        left: point.x,
                        top: point.y,
                        right: point.x + self.cell_w,
                        bottom: point.y + self.cell_h,
                    },
                },
            );
            ImmReleaseContext(self.hwnd, context);
        }
    }
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
        let rows = ((rc.bottom - px(self.hwnd, 16)) / self.cell_h).clamp(2, 200) as u16;
        let cols = ((rc.right - px(self.hwnd, 32)) / self.cell_w).clamp(10, 400) as u16;
        if self.parser.screen().size() != (rows, cols) {
            self.parser.screen_mut().set_size(rows, cols);
        }
        if let Some(s) = &mut self.session {
            if s.size != (rows, cols)
                && s.master
                    .resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .is_ok()
            {
                s.size = (rows, cols);
            }
        }
        self.position_ime();
        invalidate(self.hwnd);
    }
    unsafe fn output(&mut self) {
        // Bound each dispatch so continuous output cannot starve keyboard messages.
        for batch in 0..16 {
            let Ok(event) = self.events.try_recv() else {
                break;
            };
            if batch == 15 {
                PostMessageW(self.hwnd, READY, 0, 0);
            }
            match event {
                Event::Started(s) => {
                    self.session = Some(s);
                    self.resize();
                }
                Event::Output(bytes) => {
                    self.parser.process(&bytes);
                    self.cursor.update(
                        self.parser.screen().cursor_position(),
                        self.parser.screen().hide_cursor(),
                        std::time::Instant::now(),
                    );
                    let replies = std::mem::take(&mut self.parser.callbacks_mut().bytes);
                    if !replies.is_empty() {
                        if let Some(session) = &self.session {
                            let _ = session.writer.send(replies);
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
        if IsWindowVisible(self.hwnd) != 0 && !self.frame_pending {
            self.frame_pending = SetTimer(self.hwnd, 2, 8, None) != 0;
            if !self.frame_pending {
                invalidate(self.hwnd);
            }
        }
    }
    unsafe fn paint(&mut self, dc: HDC, dirty: &RECT) {
        let screen = self.parser.screen();
        let pending = self.cursor.update(
            screen.cursor_position(),
            screen.hide_cursor(),
            std::time::Instant::now(),
        );
        if pending {
            SetTimer(self.hwnd, 3, 32, None);
        } else {
            KillTimer(self.hwnd, 3);
        }
        let rc = client(self.hwnd);
        let mut renderer = std::mem::take(&mut self.renderer);
        renderer.paint(self.hwnd, dc, dirty, |canvas| self.draw(canvas, rc));
        self.renderer = renderer;
    }
    unsafe fn draw(&self, canvas: &mut crate::render::Canvas, rc: RECT) {
        canvas.fill(rc, SURFACE);
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let cursor = self
            .cursor
            .position
            .filter(|_| GetFocus() == self.hwnd && screen.scrollback() == 0)
            .map(|(row, col)| {
                let continuation = screen
                    .cell(row, col)
                    .is_some_and(|cell| cell.is_wide_continuation());
                (row, col.saturating_sub(u16::from(continuation)))
            });
        let mut run = String::with_capacity(cols as usize);
        let flush = |canvas: &mut crate::render::Canvas, run: &mut String, x, y, fg| {
            if !run.is_empty() {
                canvas.label(
                    run,
                    RECT {
                        left: x,
                        top: y,
                        right: x + run.len() as i32 * self.cell_w,
                        bottom: y + self.cell_h,
                    },
                    self.font,
                    fg,
                    DT_SINGLELINE,
                );
                run.clear();
            }
        };
        for row in 0..rows {
            let y = px(self.hwnd, 8) + row as i32 * self.cell_h;
            let (mut run_x, mut run_color) = (0, INK);
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
                    if cursor == Some((row, col)) && self.parser.callbacks().cursor_style <= 2 {
                        bg = ACCENT;
                        fg = SURFACE;
                    }
                    let x = px(self.hwnd, 16) + col as i32 * self.cell_w;
                    let r = RECT {
                        left: x,
                        top: y,
                        right: x + self.cell_w * if cell.is_wide() { 2 } else { 1 },
                        bottom: y + self.cell_h,
                    };
                    if bg != SURFACE {
                        canvas.fill(r, bg);
                    }
                    let text = cell.contents();
                    if text.is_ascii() && !text.is_empty() && !cell.is_wide() {
                        if fg != run_color || x != run_x + run.len() as i32 * self.cell_w {
                            flush(canvas, &mut run, run_x, y, run_color);
                        }
                        if run.is_empty() {
                            run_x = x;
                            run_color = fg;
                        }
                        run.push_str(text);
                    } else {
                        flush(canvas, &mut run, run_x, y, run_color);
                        if !text.is_empty() {
                            canvas.label(text, r, self.font, fg, DT_SINGLELINE);
                        }
                    }
                }
            }
            flush(canvas, &mut run, run_x, y, run_color);
        }
        if let Some((row, col)) = cursor {
            let style = self.parser.callbacks().cursor_style;
            if style >= 3 {
                let x = px(self.hwnd, 16) + i32::from(col) * self.cell_w;
                let y = px(self.hwnd, 8) + i32::from(row) * self.cell_h;
                let stroke = px(self.hwnd, 2).max(1);
                canvas.fill(
                    if style <= 4 {
                        RECT {
                            left: x,
                            top: y + self.cell_h - stroke,
                            right: x + self.cell_w,
                            bottom: y + self.cell_h,
                        }
                    } else {
                        RECT {
                            left: x,
                            top: y,
                            right: x + stroke,
                            bottom: y + self.cell_h,
                        }
                    },
                    ACCENT,
                );
            }
        }
    }
    unsafe fn index(&self, lp: isize) -> usize {
        let (rows, cols) = self.parser.screen().size();
        let x = (lp as u16 as i16 as i32 - px(self.hwnd, 16)) / self.cell_w;
        let y = ((lp >> 16) as u16 as i16 as i32 - px(self.hwnd, 8)) / self.cell_h;
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
                self.write(&paste_bytes(&text, self.parser.screen().bracketed_paste()));
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
        crate::assets::copy_text(self.hwnd, &text);
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
            CreateRoundRectRgn(
                0,
                0,
                rc.right + 1,
                rc.bottom + 1,
                px(hwnd, 14),
                px(hwnd, 14),
            ),
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
        WM_SHOWWINDOW if wp == 0 => {
            s.renderer.release();
            KillTimer(hwnd, 2);
            KillTimer(hwnd, 3);
            s.frame_pending = false;
        }
        WM_SHOWWINDOW => {
            if s.parser.callbacks().sync_started.is_some() {
                s.frame_pending = SetTimer(hwnd, 2, 8, None) != 0;
            }
            invalidate(hwnd);
        }
        FONTS_CHANGED => {
            let replacement = font(px(hwnd, 17), 400, "Consolas");
            if !replacement.is_null() {
                let dc = GetDC(hwnd);
                let old = SelectObject(dc, replacement);
                let mut metrics = zeroed();
                GetTextMetricsW(dc, &mut metrics);
                SelectObject(dc, old);
                ReleaseDC(hwnd, dc);
                DeleteObject(s.font);
                s.font = replacement;
                s.cell_w = metrics.tmAveCharWidth.max(1);
                s.cell_h = metrics.tmHeight.max(1);
                s.resize();
            }
        }

        WM_TIMER if wp == 3 => {
            KillTimer(hwnd, 3);
            invalidate(hwnd);
        }
        WM_TIMER if wp == 2 => {
            let waiting = s
                .parser
                .callbacks()
                .sync_started
                .is_some_and(|start| start.elapsed().as_millis() < 250);
            if !waiting {
                KillTimer(hwnd, 2);
                s.parser.callbacks_mut().sync_started = None;
                s.frame_pending = false;
                s.position_ime();
                invalidate(hwnd);
            }
        }
        READY => s.output(),
        RESIZE => s.resize(),
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            // Keep the previous frame while a synchronized redraw is incomplete.
            if !s.frame_pending {
                s.paint(dc, &ps.rcPaint);
            }
            EndPaint(hwnd, &ps);
        }
        WM_SETFOCUS => {
            CreateCaret(hwnd, null_mut(), s.cell_w, s.cell_h);
            s.position_ime();
            invalidate(hwnd);
        }
        WM_KILLFOCUS => {
            DestroyCaret();
            s.surrogate = None;
            invalidate(hwnd);
        }
        WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION => {
            s.position_ime();
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
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
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            let shift = GetKeyState(VK_SHIFT as i32) < 0;
            let alt = GetKeyState(VK_MENU as i32) < 0;
            if alt && matches!(wp as u16, VK_F4 | VK_SPACE) {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            if !alt && ((ctrl && wp == 0x56) || (shift && wp == VK_INSERT as usize)) {
                s.paste();
            } else if !alt && ctrl && ((shift && wp == 0x43) || wp == VK_INSERT as usize) {
                s.copy();
            } else if let Some(bytes) = key_sequence(
                wp as u16,
                ctrl,
                shift,
                alt,
                s.parser.screen().application_cursor(),
            ) {
                s.write(&bytes);
            } else if msg == WM_SYSKEYDOWN {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            invalidate(hwnd);
        }
        WM_SYSCHAR => {
            if (lp >> 16) & 0xff == 0x0e {
                return 0;
            } // Backspace was sent by WM_SYSKEYDOWN.
            if wp == VK_SPACE as usize {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            if GetKeyState(VK_CONTROL as i32) >= 0 {
                if let Some(ch) = char::from_u32(wp as u32) {
                    s.write(format!("\x1b{ch}").as_bytes());
                }
            }
            invalidate(hwnd);
        }
        WM_CHAR => {
            // Physical Backspace is encoded on keydown; Ctrl+H has a different scan code.
            if (lp >> 16) & 0xff == 0x0e {
                return 0;
            }
            let ch = if wp == 8 && GetKeyState(VK_CONTROL as i32) >= 0 {
                127
            } else {
                wp as u16
            };
            if ch == 22
                || (ch == 3 && GetKeyState(VK_SHIFT as i32) < 0)
                || (ch == 9 && GetKeyState(VK_SHIFT as i32) < 0)
                || (ch == 127 && GetKeyState(VK_CONTROL as i32) < 0)
            {
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
        let (input, received) = channel();
        let original_writer = {
            let mut s = (*state).borrow_mut();
            std::mem::replace(&mut s.session.as_mut().unwrap().writer, input)
        };
        let mut original_keys = [0u8; 256];
        GetKeyboardState(original_keys.as_mut_ptr());
        let mut keys = [0u8; 256];
        keys[VK_CONTROL as usize] = 0x80;
        SetKeyboardState(keys.as_ptr());
        SendMessageW(terminal.0, WM_KEYDOWN, VK_LEFT as usize, 0);
        SendMessageW(terminal.0, WM_KEYDOWN, VK_BACK as usize, 0);
        SendMessageW(terminal.0, WM_CHAR, 127, 0);
        SendMessageW(terminal.0, WM_CHAR, 8, 0x23 << 16); // Ctrl+H stays distinct.
        keys[VK_CONTROL as usize] = 0;
        SetKeyboardState(keys.as_ptr());
        SendMessageW(terminal.0, WM_KEYDOWN, VK_BACK as usize, 0x0e << 16);
        SendMessageW(terminal.0, WM_CHAR, 8, 0x0e << 16);
        SendMessageW(terminal.0, WM_KEYDOWN, VK_DELETE as usize, 0);
        for unit in "中文😀".encode_utf16() {
            SendMessageW(terminal.0, WM_CHAR, unit as usize, 0);
        }
        SetKeyboardState(original_keys.as_ptr());
        let actual: Vec<u8> = received.try_iter().flatten().collect();
        assert_eq!(actual, "\x1b[1;5D\x17\x08\x7f\x1b[3~中文😀".as_bytes());
        (*state).borrow_mut().session.as_mut().unwrap().writer = original_writer;
        (*state)
            .borrow_mut()
            .write(b"Write-Output ('BACK'+'SPACE_OK')X\x7f\r");
        let deletion = Instant::now();
        loop {
            let mut msg: MSG = zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let contents = (*state).borrow().parser.screen().contents();
            if contents.contains("BACKSPACE_OK") {
                break;
            }
            assert!(
                deletion.elapsed() < Duration::from_secs(10),
                "Backspace must remove only X and leave the command intact: {contents}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        ShowWindow(parent, SW_SHOWNOACTIVATE);
        ShowWindow(terminal.0, SW_SHOWNA);
        SetFocus(terminal.0);
        let mut caret: POINT = zeroed();
        assert_ne!(GetCaretPos(&mut caret), 0);
        assert!(caret.x >= 0 && caret.y >= 0);
        let (cursor_x, cursor_y) = {
            let mut s = (*state).borrow_mut();
            s.parser.process(b"\x1b[2J\x1b[2;5H\x1b[?25h\x1b[6 q");
            s.frame_pending = false;
            s.cursor
                .update((1, 4), false, Instant::now() - Duration::from_millis(40));
            (
                px(terminal.0, 16) + 4 * s.cell_w,
                px(terminal.0, 8) + s.cell_h + 4,
            )
        };
        invalidate(terminal.0);
        UpdateWindow(terminal.0);
        let pixel = || {
            let dc = GetDC(terminal.0);
            let color = GetPixel(dc, cursor_x, cursor_y);
            ReleaseDC(terminal.0, dc);
            color
        };
        assert_eq!(
            pixel(),
            ACCENT,
            "Cursor must be visible before working redraw"
        );
        (*state)
            .borrow_mut()
            .parser
            .process(b"\x1b[?2026h\x1b[?25l\x1b[H\x1b[?25h\x1b[?2026l");
        invalidate(terminal.0);
        UpdateWindow(terminal.0);
        assert_eq!(
            pixel(),
            ACCENT,
            "A visible temporary move must not erase the input cursor"
        );
        (*state).borrow_mut().parser.process(b"\x1b[2;5H");
        invalidate(terminal.0);
        UpdateWindow(terminal.0);
        (*state).borrow_mut().parser.process(b"\x1b[?25l\x1b[H");
        invalidate(terminal.0);
        UpdateWindow(terminal.0);
        assert_eq!(
            pixel(),
            ACCENT,
            "A partial working redraw must keep the last cursor in place"
        );
        (*state).borrow_mut().cursor.pending =
            Some((None, Instant::now() - Duration::from_millis(40)));
        SendMessageW(terminal.0, WM_TIMER, 3, 0);
        UpdateWindow(terminal.0);
        assert_eq!(
            pixel(),
            SURFACE,
            "A sustained hide must still remove the cursor"
        );
        {
            let mut s = (*state).borrow_mut();
            s.parser.process(b"\x1b[?2026h\x1b[?25l");
            s.frame_pending = true;
        }
        SendMessageW(terminal.0, WM_TIMER, 2, 0);
        assert!(
            (*state).borrow().frame_pending,
            "Partial synchronized frame must remain hidden"
        );
        (*state)
            .borrow_mut()
            .parser
            .process(b"\x1b[?25h\x1b[?2026l");
        SendMessageW(terminal.0, WM_TIMER, 2, 0);
        assert!(
            !(*state).borrow().frame_pending,
            "Completed frame must be presented"
        );
        {
            let mut s = (*state).borrow_mut();
            s.parser.process(b"\x1b[?2026h");
            s.parser.callbacks_mut().sync_started =
                Some(Instant::now() - Duration::from_millis(300));
            s.frame_pending = true;
        }
        SendMessageW(terminal.0, WM_TIMER, 2, 0);
        assert!(
            !(*state).borrow().frame_pending,
            "Missing end marker must not freeze the terminal"
        );
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

#[test]
fn conpty_working_cursor_does_not_follow_intermediate_positions() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let mut parser = vt100::Parser::new_with_callbacks(30, 80, 0, Replies::default());
    let mut cursor = Cursor::default();
    cursor.update((23, 2), false, start);
    cursor.update((23, 2), false, start + Duration::from_millis(32));
    // Actual Codex 0.153.2 working output captured through Windows ConPTY:
    // 15428 ms: sync ends with visible cursor at (19, 0).
    // 15443 ms: ConPTY finally restores the input cursor to (23, 2).
    for frame in 0..60 {
        let now = start + Duration::from_millis(100 + frame * 93);
        for (delay, bytes) in [
            (
                0,
                &b"\x1b[?2026h\x1b[?25l\x1b[22m\x1b[20;1H\x1b[?25h\x1b[0 q\x1b[?2026l"[..],
            ),
            (15, &b"\x1b[?25l \x1b[24;3H\x1b[?25h"[..]),
        ] {
            parser.process(bytes);
            cursor.update(
                parser.screen().cursor_position(),
                parser.screen().hide_cursor(),
                now + Duration::from_millis(delay),
            );
            assert_eq!(cursor.position, Some((23, 2)));
        }
        cursor.update((23, 2), false, now + Duration::from_millis(60));
        assert_eq!(cursor.position, Some((23, 2)));
    }
    let now = start + Duration::from_secs(7);
    assert!(cursor.update((23, 3), false, now));
    assert!(!cursor.update((23, 3), false, now + Duration::from_millis(32)));
    assert_eq!(
        cursor.position,
        Some((23, 3)),
        "Real typing/movement must settle"
    );
    assert!(cursor.update((0, 0), true, now + Duration::from_millis(40)));
    assert!(!cursor.update((0, 0), true, now + Duration::from_millis(72)));
    assert_eq!(cursor.position, None, "Explicit hiding must still work");
    // Held arrow keys must keep advancing even without an idle paint between repeats.
    for col in 0..20 {
        cursor.update(
            (23, col),
            false,
            now + Duration::from_millis(100 + u64::from(col) * 33),
        );
        if col > 0 {
            assert_eq!(cursor.position, Some((23, col - 1)));
        }
    }
}

#[test]
fn terminal_keys_and_paste_preserve_input_semantics() {
    let mut parser = vt100::Parser::new_with_callbacks(12, 90, 0, Replies::default());
    parser.process(b"abc\x1b[");
    parser.process(b"6nxyz\x1b[5n");
    assert_eq!(parser.callbacks().bytes, b"\x1b[1;4R\x1b[0n");
    parser.process(b"\x1b[?2026$p\x1b[?2026h\x1b[?25l\x1b[6 q");
    assert!(parser.callbacks().sync_started.is_some());
    assert!(parser.screen().hide_cursor());
    assert_eq!(parser.callbacks().cursor_style, 6);
    assert!(parser.callbacks().bytes.ends_with(b"\x1b[?2026;2$y"));
    parser.process(b"frame\x1b[?25h\x1b[?202");
    assert!(parser.callbacks().sync_started.is_some());
    parser.process(b"6l");
    assert!(parser.callbacks().sync_started.is_none());
    assert!(!parser.screen().hide_cursor());

    assert_eq!(
        key_sequence(VK_LEFT, true, false, false, false).unwrap(),
        b"\x1b[1;5D"
    );
    assert_eq!(
        key_sequence(VK_UP, false, false, false, true).unwrap(),
        b"\x1bOA"
    );
    assert_eq!(
        key_sequence(VK_F5, false, true, true, false).unwrap(),
        b"\x1b[15;4~"
    );
    assert_eq!(
        key_sequence(VK_TAB, false, true, false, false).unwrap(),
        b"\x1b[Z"
    );
    assert_eq!(
        key_sequence(VK_BACK, true, false, false, false).unwrap(),
        [23]
    );
    assert_eq!(
        key_sequence(VK_BACK, false, false, false, false).unwrap(),
        [127]
    );
    assert_eq!(
        key_sequence(VK_BACK, false, true, false, false).unwrap(),
        [127]
    );
    assert_eq!(
        key_sequence(VK_BACK, false, false, true, false).unwrap(),
        [27, 127]
    );
    assert!(key_sequence(0x41, false, false, false, false).is_none());
    assert_eq!(
        paste_bytes("中文\r\nnext\nlast", false),
        "中文\rnext\rlast".as_bytes()
    );
    assert_eq!(
        paste_bytes("a\r\nb\x1b[201~", true),
        b"\x1b[200~a\nb[201~\x1b[201~"
    );
}
