use crate::theme::*;
use std::{
    cell::RefCell,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::*,
        Input::KeyboardAndMouse::*,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};
pub const SYNC: u32 = WM_APP + 91;
pub const POSITION: u32 = WM_APP + 92;
const UPDATE: u32 = WM_APP + 93;
const TIMER: usize = 904;
pub const ES_DISABLENOSCROLL: u32 = 0x2000;
const MEASURE: u32 = WM_APP + 95;
struct Host {
    v: HWND,
    h: HWND,
    kind: u8,
    visible: bool,
    target: f64,
    wheel: i32,
    animating: bool,
    layout_dirty: bool,
    saved_position: POINT,
    background: u32,
    buffer: Buffer,
    tint: Buffer,
}
struct Bar {
    owner: HWND,
    vertical: bool,
    bg: u32,
    drag: bool,
    offset: i32,
    buffer: Buffer,
}
pub unsafe fn info(hwnd: HWND, vertical: bool) -> SCROLLINFO {
    let value = native_info(hwnd, vertical);
    if vertical && limit(&value) == 0 {
        let peer = GetPropW(hwnd, wide("FeatherPadScrollPeer").as_ptr());
        if !peer.is_null() {
            return native_info(peer, true);
        }
    }
    value
}
unsafe fn native_info(hwnd: HWND, vertical: bool) -> SCROLLINFO {
    let mut i = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        ..zeroed()
    };
    GetScrollInfo(hwnd, if vertical { SB_VERT } else { SB_HORZ }, &mut i);
    i
}
pub unsafe fn pair(hwnd: HWND, peer: HWND) {
    if peer.is_null() {
        RemovePropW(hwnd, wide("FeatherPadScrollPeer").as_ptr());
    } else {
        SetPropW(hwnd, wide("FeatherPadScrollPeer").as_ptr(), peer);
    }
}
pub unsafe fn measure(hwnd: HWND) {
    SendMessageW(hwnd, MEASURE, 0, 0);
}
pub unsafe fn saved_position(hwnd: HWND) -> Option<POINT> {
    let mut data = 0;
    if windows_sys::Win32::UI::Shell::GetWindowSubclass(hwnd, Some(host_proc), 902, &mut data) == 0
    {
        return None;
    }
    let state = (*(data as *const RefCell<Host>)).try_borrow().ok()?;
    (state.kind == 0).then(|| {
        if limit(&native_info(hwnd, true)) == 0 {
            POINT {
                x: state.saved_position.x,
                y: info(hwnd, true).nPos,
            }
        } else {
            state.saved_position
        }
    })
}
pub fn limit(i: &SCROLLINFO) -> i32 {
    (i.nMax - i.nPage.saturating_sub(1) as i32).max(0)
}
pub unsafe fn attach(hwnd: HWND, bg: u32) {
    let mut class = [0u16; 64];
    GetClassNameW(hwnd, class.as_mut_ptr(), 64);
    let class = String::from_utf16_lossy(&class);
    let kind = if class.starts_with("RICHEDIT") {
        0
    } else if class.starts_with("LISTBOX") {
        1
    } else if class.starts_with("FeatherPadReader") {
        2
    } else if class.starts_with("FeatherPadLarge") {
        4
    } else {
        3
    };
    if kind == 0 {
        SetPropW(hwnd, wide("FeatherPadClippedScroll").as_ptr(), 1usize as _);
    }
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    SetWindowLongW(
        hwnd,
        GWL_STYLE,
        if kind == 0 {
            ((style & !WS_HSCROLL) | WS_VSCROLL | ES_DISABLENOSCROLL | WS_CLIPSIBLINGS) as i32
        } else {
            (style & !(WS_VSCROLL | WS_HSCROLL) | WS_CLIPCHILDREN) as i32
        },
    );
    SetWindowPos(
        hwnd,
        null_mut(),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
    );
    let name = wide("FeatherPadScroll");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(bar_proc),
        hInstance: GetModuleHandleW(null()),
        lpszClassName: name.as_ptr(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        ..zeroed()
    };
    RegisterClassW(&wc);
    let parent = if kind == 0 && !GetParent(hwnd).is_null() {
        GetParent(hwnd)
    } else {
        hwnd
    };
    let make = |vertical| {
        let child = CreateWindowExW(
            0,
            name.as_ptr(),
            wide(if vertical {
                "Vertical scroll"
            } else {
                "Horizontal scroll"
            })
            .as_ptr(),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            12,
            12,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        SetWindowLongPtrW(
            child,
            GWLP_USERDATA,
            Box::into_raw(Box::new(RefCell::new(Bar {
                owner: hwnd,
                vertical,
                bg,
                drag: false,
                offset: 0,
                buffer: Buffer::default(),
            }))) as isize,
        );
        child
    };
    let host = Box::new(RefCell::new(Host {
        v: make(true),
        h: make(false),
        kind,
        visible: true,
        target: 0.,
        wheel: 0,
        animating: false,
        layout_dirty: true,
        saved_position: POINT { x: 0, y: 0 },
        background: bg,
        buffer: Buffer::default(),
        tint: Buffer::default(),
    }));
    SetWindowSubclass(hwnd, Some(host_proc), 902, Box::into_raw(host) as usize);
    if kind == 0 {
        SendMessageW(hwnd, WM_USER + 63, 1, 0);
    }
    PostMessageW(hwnd, UPDATE, 0, 0);
}
pub unsafe fn resize(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
    // Clip before resizing: shrinking can otherwise expose the old native track for one paint.
    SetWindowRgn(hwnd, CreateRectRgn(0, 0, width.max(1), height.max(1)), 0);
    MoveWindow(
        hwnd,
        x,
        y,
        width + GetSystemMetrics(SM_CXVSCROLL),
        height,
        0,
    );
    let rc = client(hwnd);
    SetWindowRgn(hwnd, CreateRectRgn(0, 0, rc.right, rc.bottom), 0);
}
pub unsafe fn show(hwnd: HWND, visible: bool) {
    SendMessageW(hwnd, UPDATE, 1, visible as isize);
}
pub unsafe fn set_position(hwnd: HWND, vertical: bool, pos: i32) {
    SendMessageW(hwnd, POSITION, vertical as usize, pos as isize);
}
unsafe fn position(hwnd: HWND, kind: u8, vertical: bool, pos: i32) {
    let pos = pos.clamp(0, limit(&info(hwnd, vertical)));
    if kind == 0 && vertical && limit(&native_info(hwnd, true)) == 0 {
        let peer = GetPropW(hwnd, wide("FeatherPadScrollPeer").as_ptr());
        if !peer.is_null() && limit(&native_info(peer, true)) > 0 {
            set_position(peer, vertical, pos);
            return;
        }
    }
    match kind {
        0 => {
            let mut p = POINT {
                x: native_info(hwnd, false).nPos,
                y: native_info(hwnd, true).nPos,
            };
            if vertical {
                p.y = pos
            } else {
                p.x = pos
            };
            SendMessageW(hwnd, WM_USER + 222, 0, &p as *const _ as isize);
        }
        1 => {
            SendMessageW(hwnd, LB_SETTOPINDEX, pos as usize, 0);
        }
        2 | 4 => {
            SendMessageW(
                hwnd,
                crate::reader::SCROLL_TO,
                vertical as usize,
                pos as isize,
            );
        }
        _ => {
            SendMessageW(
                hwnd,
                WM_VSCROLL,
                SB_THUMBPOSITION as usize | ((pos as usize) << 16),
                0,
            );
        }
    }
}
unsafe fn refresh(hwnd: HWND, s: &mut Host) {
    if s.kind == 0 {
        if s.layout_dirty {
            SendMessageW(hwnd, WM_USER + 96, SB_VERT as usize, 1);
            SendMessageW(hwnd, EM_GETLINECOUNT, 0, 0);
            s.layout_dirty = false;
            let rc = client(hwnd);
            SetWindowRgn(hwnd, CreateRectRgn(0, 0, rc.right, rc.bottom), 0);
        }
    } else {
        ShowScrollBar(hwnd, SB_BOTH, 0);
    }
    let r = client(hwnd);
    let mut origin: POINT = zeroed();
    if GetParent(s.v) != hwnd {
        MapWindowPoints(hwnd, GetParent(s.v), &mut origin, 1);
    }
    for (bar, vertical) in [(s.v, true), (s.h, false)] {
        let visible = s.visible
            && (s.kind != 0 || IsWindowVisible(hwnd) != 0)
            && limit(&info(hwnd, vertical)) > 0
            && (vertical || s.kind == 2);
        MoveWindow(
            bar,
            origin.x + if vertical { r.right - 12 } else { 0 },
            origin.y + if vertical { 0 } else { r.bottom - 12 },
            if vertical { 12 } else { r.right - 12 },
            if vertical {
                if s.kind == 2 {
                    r.bottom - 12
                } else {
                    r.bottom
                }
            } else {
                12
            },
            0,
        );
        ShowWindow(bar, if visible { SW_SHOWNA } else { SW_HIDE });
        SetWindowPos(
            bar,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        invalidate(bar);
    }
}
unsafe extern "system" fn host_proc(
    hwnd: HWND,
    msg: u32,
    wp: usize,
    lp: isize,
    _id: usize,
    data: usize,
) -> isize {
    let ptr = data as *mut RefCell<Host>;
    // Native scroll painting can reenter while Host is borrowed. Its geometry stays
    // enabled for RichEdit; only our dark sibling controls should ever draw the tracks.
    if msg == WM_NCPAINT && !GetPropW(hwnd, wide("FeatherPadClippedScroll").as_ptr()).is_null() {
        return 0;
    }
    if msg == WM_NCDESTROY {
        RemovePropW(hwnd, wide("FeatherPadClippedScroll").as_ptr());
        RemoveWindowSubclass(hwnd, Some(host_proc), 902);
        let state = (*ptr).borrow();
        if state.kind == 0 {
            DestroyWindow(state.v);
            DestroyWindow(state.h);
        }
        drop(state);
        let result = DefSubclassProc(hwnd, msg, wp, lp);
        drop(Box::from_raw(ptr));
        return result;
    }
    let Ok(mut s) = (*ptr).try_borrow_mut() else {
        return DefSubclassProc(hwnd, msg, wp, lp);
    };
    if s.kind == 0 && msg == WM_SIZE {
        let rc = client(hwnd);
        SetWindowRgn(hwnd, CreateRectRgn(0, 0, rc.right, rc.bottom), 0);
    }
    if msg == MEASURE {
        if s.kind == 0 && s.layout_dirty {
            refresh(hwnd, &mut s);
        }
        return 0;
    }
    if msg == WM_ERASEBKGND && s.kind == 1 {
        fill(wp as HDC, client(hwnd), s.background);
        return 1;
    }
    if msg == WM_PAINT && (s.kind == 0 || s.kind == 1) {
        let mut ps = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let rc = client(hwnd);
        if s.buffer.ensure(dc, rc.right, rc.bottom) {
            fill(s.buffer.dc, rc, s.background);
            DefSubclassProc(
                hwnd,
                WM_PRINTCLIENT,
                s.buffer.dc as usize,
                PRF_CLIENT as isize,
            );
            if s.kind == 0 {
                let dc = s.buffer.dc;
                crate::selection::paint(hwnd, dc, &mut s.tint);
            }
            s.buffer.blit(dc, &ps.rcPaint);
        }
        EndPaint(hwnd, &ps);
        PostMessageW(hwnd, UPDATE, 0, 0);
        return 0;
    }
    if msg == UPDATE {
        if wp == 1 {
            s.visible = lp != 0;
        }
        refresh(hwnd, &mut s);
        return 0;
    }
    if msg == POSITION {
        if s.kind == 0 && s.layout_dirty {
            refresh(hwnd, &mut s);
        }
        s.animating = false;
        KillTimer(hwnd, TIMER);
        position(hwnd, s.kind, wp != 0, lp as i32);
        if s.kind == 0 {
            s.saved_position = POINT {
                x: info(hwnd, false).nPos,
                y: info(hwnd, true).nPos,
            };
        }
        refresh(hwnd, &mut s);
        return 0;
    }
    if msg == WM_MOUSEWHEEL && s.kind == 1 {
        let delta = (wp >> 16) as u16 as i16 as i32;
        s.wheel += delta;
        let lines = s.wheel / 40;
        s.wheel %= 40;
        let current = info(hwnd, true).nPos;
        position(hwnd, s.kind, true, current - lines);
        refresh(hwnd, &mut s);
        return 0;
    }
    if msg == WM_MOUSEWHEEL && wp & 8 == 0 && (s.kind == 0 || s.kind == 2) {
        if s.kind == 0 && limit(&native_info(hwnd, true)) == 0 {
            let peer = GetPropW(hwnd, wide("FeatherPadScrollPeer").as_ptr());
            if !peer.is_null() && limit(&native_info(peer, true)) > 0 {
                SendMessageW(peer, msg, wp, lp);
                return 0;
            }
        }
        let delta = (wp >> 16) as u16 as i16 as f64;
        let i = info(hwnd, true);
        if !s.animating {
            s.target = i.nPos as f64;
        }
        s.target = (s.target - delta * 0.8).clamp(0., limit(&i) as f64);
        s.animating = true;
        SetTimer(hwnd, TIMER, 15, None);
        return 0;
    }
    if msg == WM_TIMER && wp == TIMER {
        if s.kind == 0 {
            s.target = s.target.clamp(0., limit(&info(hwnd, true)) as f64);
        }
        let current = info(hwnd, true).nPos;
        let remaining = s.target - current as f64;
        let next = if remaining.abs() < 2. {
            s.animating = false;
            KillTimer(hwnd, TIMER);
            s.target as i32
        } else {
            current + (remaining * 0.35).round() as i32
        };
        position(hwnd, s.kind, true, next);
        if s.kind == 0 {
            s.saved_position = POINT {
                x: info(hwnd, false).nPos,
                y: info(hwnd, true).nPos,
            };
        }
        PostMessageW(GetAncestor(hwnd, GA_ROOT), SYNC, hwnd as usize, 0);
        refresh(hwnd, &mut s);
        return 0;
    }
    // IMR_QUERYCHARPOSITION can scroll RichEdit to an off-screen caret.
    // A position query must preserve the reading viewport, just like focus restoration.
    let focus_position =
        if s.kind == 0 && (msg == WM_SETFOCUS || (msg == WM_IME_REQUEST && wp == 6)) {
            Some((
                s.saved_position.x,
                if limit(&native_info(hwnd, true)) == 0 {
                    info(hwnd, true).nPos
                } else {
                    s.saved_position.y
                },
            ))
        } else {
            None
        };
    let result = DefSubclassProc(hwnd, msg, wp, lp);
    if s.kind == 0 {
        if msg == WM_SETFOCUS {
            DefSubclassProc(hwnd, WM_USER + 63, 1, 0);
        }
        if matches!(
            msg,
            WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_KEYDOWN
                | WM_CHAR
                | WM_SETFOCUS
                | WM_KILLFOCUS
                | EM_SETSEL
        ) || msg == WM_USER + 55
            || (msg == WM_MOUSEMOVE
                && wp & windows_sys::Win32::System::SystemServices::MK_LBUTTON as usize != 0)
        {
            invalidate(hwnd);
        }
    }
    if s.kind == 0 && (msg == WM_SETTEXT || msg == WM_USER + 73) {
        s.saved_position = POINT { x: 0, y: 0 };
    }
    if s.kind == 0
        && matches!(
            msg,
            WM_MOUSEWHEEL | WM_VSCROLL | WM_KEYDOWN | WM_CHAR | WM_LBUTTONUP
        )
    {
        s.saved_position = POINT {
            x: info(hwnd, false).nPos,
            y: info(hwnd, true).nPos,
        };
    }
    if s.kind == 0
        && (matches!(
            msg,
            WM_SIZE
                | WM_SETTEXT
                | WM_SETFONT
                | WM_CHAR
                | WM_PASTE
                | WM_CUT
                | WM_CLEAR
                | WM_UNDO
                | EM_REPLACESEL
        ) || msg == WM_USER + 73
            || msg == WM_USER + 225)
    {
        s.layout_dirty = true;
        PostMessageW(hwnd, UPDATE, 0, 0);
    }
    if msg == WM_SHOWWINDOW && s.kind == 0 {
        refresh(hwnd, &mut s);
    }
    if msg == WM_SETFOCUS || focus_position.is_some() {
        refresh(hwnd, &mut s);
        if let Some((x, y)) = focus_position {
            position(hwnd, 0, false, x);
            position(hwnd, 0, true, y);
            refresh(hwnd, &mut s);
            PostMessageW(hwnd, POSITION, 0, x as isize);
            PostMessageW(hwnd, POSITION, 1, y as isize);
        }
    }
    if matches!(
        msg,
        WM_SIZE
            | WM_SETFOCUS
            | WM_PAINT
            | WM_SETTEXT
            | WM_MOUSEWHEEL
            | WM_VSCROLL
            | WM_HSCROLL
            | WM_KEYDOWN
            | WM_SHOWWINDOW
    ) {
        PostMessageW(hwnd, UPDATE, 0, 0);
        if matches!(msg, WM_MOUSEWHEEL | WM_VSCROLL | WM_KEYDOWN)
            || (msg == WM_SETFOCUS && s.kind != 0)
        {
            PostMessageW(GetAncestor(hwnd, GA_ROOT), SYNC, hwnd as usize, 0);
        }
    }
    result
}
fn thumb(length: i32, i: &SCROLLINFO) -> (i32, i32) {
    let length = (length - 8).max(1);
    let size = ((length as f64 * i.nPage as f64 / (i.nMax + 1).max(1) as f64) as i32)
        .clamp(24.min(length), length);
    (
        4 + ((length - size) as f64 * i.nPos as f64 / limit(i).max(1) as f64) as i32,
        size,
    )
}
unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<Bar>;
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let Ok(mut s) = (*ptr).try_borrow_mut() else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    let rc = client(hwnd);
    let length = if s.vertical { rc.bottom } else { rc.right };
    let i = info(s.owner, s.vertical);
    let (start, size) = thumb(length, &i);
    match msg {
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            if s.buffer.ensure(dc, rc.right, rc.bottom) {
                fill(s.buffer.dc, rc, s.bg);
                let r = if s.vertical {
                    RECT {
                        left: 4,
                        right: 8,
                        top: start,
                        bottom: start + size,
                    }
                } else {
                    RECT {
                        left: start,
                        right: start + size,
                        top: 4,
                        bottom: 8,
                    }
                };
                rounded(s.buffer.dc, r, if s.drag { ACCENT } else { MUTED }, 4);
                s.buffer.blit(dc, &ps.rcPaint);
            }
            EndPaint(hwnd, &ps);
        }
        WM_LBUTTONDOWN => {
            let p = if s.vertical {
                (lp >> 16) as u16 as i16 as i32
            } else {
                lp as u16 as i16 as i32
            };
            s.drag = true;
            s.offset = if p >= start && p < start + size {
                p - start
            } else {
                size / 2
            };
            SetCapture(hwnd);
            let pos = ((p - s.offset - 4) as f64 / (length - size - 8).max(1) as f64
                * limit(&i) as f64) as i32;
            set_position(s.owner, s.vertical, pos);
            PostMessageW(GetAncestor(s.owner, GA_ROOT), SYNC, s.owner as usize, 0);
            invalidate(hwnd);
        }
        WM_MOUSEMOVE if s.drag => {
            let p = if s.vertical {
                (lp >> 16) as u16 as i16 as i32
            } else {
                lp as u16 as i16 as i32
            };
            let pos = ((p - s.offset - 4) as f64 / (length - size - 8).max(1) as f64
                * limit(&i) as f64) as i32;
            set_position(s.owner, s.vertical, pos);
            PostMessageW(GetAncestor(s.owner, GA_ROOT), SYNC, s.owner as usize, 0);
            invalidate(hwnd);
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            s.drag = false;
            if msg == WM_LBUTTONUP {
                ReleaseCapture();
            }
            invalidate(hwnd);
        }
        WM_MOUSEWHEEL => {
            SendMessageW(s.owner, msg, wp, lp);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}
#[test]
fn thumb_reaches_both_ends() {
    let mut i = SCROLLINFO {
        nMax: 999,
        nPage: 100,
        ..unsafe { zeroed() }
    };
    assert_eq!(thumb(408, &i), (4, 40));
    i.nPos = 900;
    assert_eq!(thumb(408, &i), (364, 40));
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_scroll_remains_available_without_native_tracks() {
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        assert!(!library.is_null());
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            0,
            0,
            800,
            600,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let hwnd = CreateWindowExW(
            0,
            wide("RICHEDIT50W").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD
                | WS_VSCROLL
                | ES_DISABLENOSCROLL
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32,
            0,
            0,
            500,
            240,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        editor_colors(hwnd);
        attach(hwnd, CANVAS);
        SetWindowTextW(hwnd, wide(&"scroll test line\r\n".repeat(200)).as_ptr());
        measure(hwnd);
        ValidateRect(hwnd, null());
        for _ in 0..30 {
            SendMessageW(hwnd, WM_MOUSEMOVE, 0, (40 << 16) | 40);
        }
        assert_eq!(
            GetUpdateRect(hwnd, null_mut(), 0),
            0,
            "Hovering must not repaint the editor and interrupt caret blinking"
        );
        let max = limit(&info(hwnd, true));
        assert!(max > 0, "Missing hidden scroll range");
        set_position(hwnd, true, max / 2);
        assert!(info(hwnd, true).nPos > 0);
        set_position(hwnd, true, max);
        assert!((info(hwnd, true).nPos - max).abs() <= 2);
        SendMessageW(hwnd, WM_SETFOCUS, 0, 0);
        let region = CreateRectRgn(0, 0, 0, 0);
        assert_ne!(GetWindowRgn(hwnd, region), 0);
        let mut bounds = zeroed();
        GetRgnBox(region, &mut bounds);
        assert_eq!(
            bounds.right,
            client(hwnd).right,
            "Native scrollbar must stay outside the visible region"
        );
        DeleteObject(region);
        let mut data = 0;
        windows_sys::Win32::UI::Shell::GetWindowSubclass(hwnd, Some(host_proc), 902, &mut data);
        let bar = (*(data as *const RefCell<Host>)).borrow().v;
        assert_eq!(
            GetParent(bar),
            parent,
            "Custom track must not scroll with RichEdit's children"
        );
        DestroyWindow(hwnd);
        DestroyWindow(parent);
        FreeLibrary(library);
    }
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_markdown_long_scroll_settles() {
    use windows_sys::Win32::{System::LibraryLoader::LoadLibraryW, UI::Shell::GetWindowSubclass};
    unsafe extern "system" fn native_paint_probe(
        hwnd: HWND,
        msg: u32,
        wp: usize,
        lp: isize,
        _: usize,
        data: usize,
    ) -> isize {
        if msg == WM_NCPAINT {
            let count = &*(data as *const std::cell::Cell<u32>);
            count.set(count.get() + 1);
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let make = || {
            CreateWindowExW(
                0,
                wide("RICHEDIT50W").as_ptr(),
                wide("").as_ptr(),
                WS_POPUP | WS_VSCROLL | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32,
                0,
                0,
                500,
                400,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        let hwnd = make();
        let native_paints = std::cell::Cell::new(0u32);
        SetWindowSubclass(
            hwnd,
            Some(native_paint_probe),
            905,
            &native_paints as *const _ as usize,
        );
        editor_colors(hwnd);
        attach(hwnd, CANVAS);
        let mut host_data = 0;
        GetWindowSubclass(hwnd, Some(host_proc), 902, &mut host_data);
        native_paints.set(0);
        {
            let _layout = (*(host_data as *const RefCell<Host>)).borrow_mut();
            SendMessageW(hwnd, WM_NCPAINT, 1, 0);
        }
        assert_eq!(
            native_paints.get(),
            0,
            "Reentrant paint must not reach the native light track"
        );
        SetWindowTextW(
            hwnd,
            wide(&"Markdown scroll test\r\n".repeat(12000)).as_ptr(),
        );
        measure(hwnd);
        let max = limit(&info(hwnd, true));
        assert!(
            max > 100000,
            "The full document must be measured before drawing its thumb"
        );
        for wanted in [100, 1000, max / 2, max - 100, max] {
            set_position(hwnd, true, wanted);
            assert!((info(hwnd, true).nPos - wanted).abs() <= 2);
            assert_eq!(
                limit(&info(hwnd, true)),
                max,
                "Dragging must not grow the scroll range"
            );
        }
        set_position(hwnd, true, max / 2);
        let reset = POINT { x: 0, y: 0 };
        SendMessageW(hwnd, WM_USER + 222, 0, &reset as *const _ as isize);
        SendMessageW(hwnd, WM_SETFOCUS, 0, 0);
        assert!(
            (info(hwnd, true).nPos - max / 2).abs() <= 2,
            "Focus must preserve the viewport"
        );
        let short = make();
        editor_colors(short);
        attach(short, CANVAS);
        SetWindowTextW(short, wide("Short preview").as_ptr());
        measure(short);
        pair(hwnd, short);
        pair(short, hwnd);
        assert_eq!(
            limit(&info(short, true)),
            max,
            "The right thumb must exist even when preview fits"
        );
        set_position(short, true, max / 3);
        assert!((info(hwnd, true).nPos - max / 3).abs() <= 2);
        set_position(hwnd, true, 0);
        SendMessageW(short, WM_MOUSEWHEEL, ((-120i16) as u16 as usize) << 16, 0);
        for _ in 0..80 {
            SendMessageW(hwnd, WM_TIMER, TIMER, 0);
        }
        let mut data = 0;
        GetWindowSubclass(hwnd, Some(host_proc), 902, &mut data);
        let state = &*(data as *const RefCell<Host>);
        assert!(!state.borrow().animating, "Wheel easing must settle");
        assert!(
            info(hwnd, true).nPos > 0,
            "Wheel over a short preview must scroll the source"
        );
        SendMessageW(hwnd, WM_MOUSEWHEEL, ((-120i16) as u16 as usize) << 16, 0);
        SetWindowTextW(hwnd, wide("Short source").as_ptr());
        measure(hwnd);
        for _ in 0..80 {
            SendMessageW(hwnd, WM_TIMER, TIMER, 0);
        }
        assert!(
            !state.borrow().animating,
            "Shrinking text must not leave the timer running"
        );
        pair(hwnd, null_mut());
        DestroyWindow(short);
        DestroyWindow(hwnd);
        FreeLibrary(library);
    }
}
