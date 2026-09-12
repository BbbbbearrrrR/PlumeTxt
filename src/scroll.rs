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
const TREE_IDLE: usize = 905;
pub const ES_DISABLENOSCROLL: u32 = 0x2000;
const MEASURE: u32 = WM_APP + 95;
struct Host {
    v: HWND,
    h: HWND,
    kind: u8,
    visible: bool,
    wheel: i32,
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
    settled: Option<SCROLLINFO>,
    buffer: Buffer,
}
pub unsafe fn info(hwnd: HWND, vertical: bool) -> SCROLLINFO {
    // RichEdit exposes a provisional range until wrapping has been measured.
    measure(hwnd);
    let value = native_info(hwnd, vertical);
    if vertical && limit(&value) == 0 {
        let peer = GetPropW(hwnd, wide("PlumeTxtScrollPeer").as_ptr());
        if !peer.is_null() {
            measure(peer);
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
        RemovePropW(hwnd, wide("PlumeTxtScrollPeer").as_ptr());
    } else {
        SetPropW(hwnd, wide("PlumeTxtScrollPeer").as_ptr(), peer);
    }
}
pub unsafe fn hold_range(hwnd: HWND, hold: bool) {
    let key = wide("PlumeTxtHoldRange");
    if hold {
        SetPropW(hwnd, key.as_ptr(), 1usize as _);
    } else {
        RemovePropW(hwnd, key.as_ptr());
        measure(hwnd);
        PostMessageW(hwnd, UPDATE, 0, 0);
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
    } else if class.starts_with("PlumeTxtReader") {
        2
    } else if class.starts_with("PlumeTxtLarge") {
        4
    } else {
        3
    };
    if kind == 0 || kind == 1 {
        SetPropW(
            hwnd,
            wide("PlumeTxtClippedScroll").as_ptr(),
            (bg as usize + 1) as _,
        );
    }
    if kind == 3 {
        SetPropW(
            hwnd,
            wide("PlumeTxtTreeScroll").as_ptr(),
            (bg as usize + 1) as _,
        );
    }
    if kind == 1 || kind == 3 {
        SetPropW(hwnd, wide("PlumeTxtQuietScroll").as_ptr(), 1usize as _);
    }
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    SetWindowLongW(
        hwnd,
        GWL_STYLE,
        if kind == 0 {
            ((style & !WS_HSCROLL) | WS_VSCROLL | ES_DISABLENOSCROLL | WS_CLIPSIBLINGS) as i32
        } else if kind == 1 {
            ((style & !WS_HSCROLL) | WS_VSCROLL | LBS_DISABLENOSCROLL as u32 | WS_CLIPCHILDREN)
                as i32
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
    let name = wide("PlumeTxtScroll");
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
                settled: None,
                buffer: Buffer::default(),
            }))) as isize,
        );
        child
    };
    let host = Box::new(RefCell::new(Host {
        v: make(true),
        h: make(false),
        kind,
        visible: kind != 3 && kind != 1,
        wheel: 0,
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
    let extra_height = if !GetPropW(hwnd, wide("PlumeTxtTreeScroll").as_ptr()).is_null()
        && GetWindowLongW(hwnd, GWL_STYLE) as u32 & TVS_NOHSCROLL == 0
    {
        windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CYHSCROLL, dpi(hwnd))
    } else {
        0
    };
    let mut outer: RECT = zeroed();
    GetWindowRect(hwnd, &mut outer);
    let mut origin = POINT {
        x: outer.left,
        y: outer.top,
    };
    ScreenToClient(GetParent(hwnd), &mut origin);
    let mut region: RECT = zeroed();
    if origin.x == x
        && origin.y == y
        && outer.right - outer.left
            == width
                + windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXVSCROLL, dpi(hwnd))
        && outer.bottom - outer.top == height + extra_height
        && GetWindowRgnBox(hwnd, &mut region) != 0
        && region.right == width
        && region.bottom == height
    {
        return;
    }
    // Clip before resizing: shrinking can otherwise expose the old native track for one paint.
    SetWindowRgn(hwnd, CreateRectRgn(0, 0, width.max(1), height.max(1)), 0);
    crate::theme::move_window(
        hwnd,
        x,
        y,
        width + windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXVSCROLL, dpi(hwnd)),
        height
            + if GetPropW(hwnd, wide("PlumeTxtTreeScroll").as_ptr()).is_null()
                || GetWindowLongW(hwnd, GWL_STYLE) as u32 & TVS_NOHSCROLL != 0
            {
                0
            } else {
                windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CYHSCROLL, dpi(hwnd))
            },
        0,
    );
    let rc = editor_viewport(hwnd);
    SetWindowRgn(hwnd, CreateRectRgn(0, 0, rc.right, rc.bottom), 0);
}
unsafe fn editor_viewport(hwnd: HWND) -> RECT {
    let mut rc = client(hwnd);
    let mut outer = zeroed();
    GetWindowRect(hwnd, &mut outer);
    // The native track is outside our viewport even while RichEdit hides/recreates it.
    // Using the transient client width here exposes its light gutter for one frame.
    rc.right = rc.right.min(
        (outer.right
            - outer.left
            - windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXVSCROLL, dpi(hwnd)))
        .max(1),
    );
    if !GetPropW(hwnd, wide("PlumeTxtTreeScroll").as_ptr()).is_null()
        && GetWindowLongW(hwnd, GWL_STYLE) as u32 & TVS_NOHSCROLL == 0
    {
        rc.bottom = rc.bottom.min(
            (outer.bottom
                - outer.top
                - windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CYHSCROLL, dpi(hwnd)))
            .max(1),
        );
    }
    rc
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
        let peer = GetPropW(hwnd, wide("PlumeTxtScrollPeer").as_ptr());
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
        3 if vertical => {
            // TreeView ignores synthetic thumb positions; select its first visible row directly.
            let current = info(hwnd, true).nPos;
            let mut item = SendMessageW(hwnd, TVM_GETNEXTITEM, TVGN_FIRSTVISIBLE as usize, 0);
            let direction = if pos > current {
                TVGN_NEXTVISIBLE
            } else {
                TVGN_PREVIOUSVISIBLE
            };
            for _ in 0..(pos - current).unsigned_abs() {
                let next = SendMessageW(hwnd, TVM_GETNEXTITEM, direction as usize, item);
                if next == 0 {
                    break;
                }
                item = next;
            }
            SendMessageW(hwnd, TVM_SELECTITEM, TVGN_FIRSTVISIBLE as usize, item);
        }
        _ => {
            SendMessageW(
                hwnd,
                if vertical { WM_VSCROLL } else { WM_HSCROLL },
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
            let rc = editor_viewport(hwnd);
            SetWindowRgn(hwnd, CreateRectRgn(0, 0, rc.right, rc.bottom), 0);
        }
    } else if s.kind != 3 && s.kind != 1 {
        ShowScrollBar(hwnd, SB_BOTH, 0);
    }
    let r = if s.kind == 0 || s.kind == 1 || s.kind == 3 {
        editor_viewport(hwnd)
    } else {
        client(hwnd)
    };
    let mut origin: POINT = zeroed();
    if GetParent(s.v) != hwnd {
        MapWindowPoints(hwnd, GetParent(s.v), &mut origin, 1);
    }
    for (bar, vertical) in [(s.v, true), (s.h, false)] {
        let visible = s.visible
            && (s.kind != 0 || IsWindowVisible(hwnd) != 0)
            && limit(&info(hwnd, vertical)) > 0
            && (vertical || s.kind == 2);
        crate::theme::move_window(
            bar,
            origin.x + if vertical { r.right - px(hwnd, 12) } else { 0 },
            origin.y + if vertical { 0 } else { r.bottom - px(hwnd, 12) },
            if vertical {
                px(hwnd, 12)
            } else {
                r.right - px(hwnd, 12)
            },
            if vertical {
                if s.kind == 2 || s.kind == 3 {
                    r.bottom - px(hwnd, 12)
                } else {
                    r.bottom
                }
            } else {
                px(hwnd, 12)
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
    // This default handler sends WM_SIZE recursively; Host must remain borrowable.
    if msg == WM_WINDOWPOSCHANGED {
        return DefSubclassProc(hwnd, msg, wp, lp);
    }
    let ptr = data as *mut RefCell<Host>;
    // Native scroll painting can reenter while Host is borrowed. Its geometry stays
    // enabled for RichEdit; only our dark sibling controls should ever draw the tracks.
    let clipped = GetPropW(hwnd, wide("PlumeTxtClippedScroll").as_ptr());
    if !clipped.is_null() {
        if msg == WM_NCPAINT {
            return 0;
        }
        if msg == WM_ERASEBKGND {
            // Preserve existing glyphs until the buffered paint is ready. Only guard the edge.
            let mut edge = editor_viewport(hwnd);
            edge.left = (edge.right - 1).max(0);
            fill(wp as HDC, edge, (clipped as usize - 1) as u32);
            return 1;
        }
        if msg == WM_SIZE {
            let rc = editor_viewport(hwnd);
            SetWindowRgn(hwnd, CreateRectRgn(0, 0, rc.right, rc.bottom), 0);
        }
    }
    let tree_background = GetPropW(hwnd, wide("PlumeTxtTreeScroll").as_ptr());
    if !tree_background.is_null() {
        if msg == WM_ERASEBKGND {
            return 1;
        }
        // Keep native ranges, but clip both native tracks outside the tree viewport.
        if msg == WM_NCPAINT {
            return 0;
        }
        if msg == WM_SIZE {
            let rc = editor_viewport(hwnd);
            SetWindowRgn(hwnd, CreateRectRgn(0, 0, rc.right, rc.bottom), 0);
        }
    }
    if msg == WM_NCDESTROY {
        RemovePropW(hwnd, wide("PlumeTxtClippedScroll").as_ptr());
        RemovePropW(hwnd, wide("PlumeTxtTreeScroll").as_ptr());
        RemovePropW(hwnd, wide("PlumeTxtQuietScroll").as_ptr());
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
    if s.kind == 3 || s.kind == 1 {
        if matches!(msg, WM_MOUSEWHEEL | WM_KEYDOWN | WM_LBUTTONDOWN) || (msg == UPDATE && wp == 2)
        {
            s.visible = true;
            SetTimer(hwnd, TREE_IDLE, 900, None);
            PostMessageW(hwnd, UPDATE, 0, 0);
        }
        if msg == WM_TIMER && wp == TREE_IDLE {
            if GetCapture() != s.v && GetCapture() != s.h {
                KillTimer(hwnd, TREE_IDLE);
                s.visible = false;
                refresh(hwnd, &mut s);
            }
            return 0;
        }
    }
    if msg == MEASURE {
        if s.kind == 0 && s.layout_dirty {
            refresh(hwnd, &mut s);
        }
        return 0;
    }
    if msg == WM_ERASEBKGND && (s.kind == 1 || s.kind == 3) {
        return 1;
    }
    if msg == WM_PAINT && (s.kind == 0 || s.kind == 1 || s.kind == 3) {
        let mut ps = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let rc = editor_viewport(hwnd);
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
            let peer = GetPropW(hwnd, wide("PlumeTxtScrollPeer").as_ptr());
            if !peer.is_null() && limit(&native_info(peer, true)) > 0 {
                SendMessageW(peer, msg, wp, lp);
                return 0;
            }
        }
        let delta = (wp >> 16) as u16 as i16 as i32;
        let i = info(hwnd, true);
        s.wheel += delta * 4;
        let next = (i.nPos - s.wheel / 5).clamp(0, limit(&i));
        s.wheel %= 5;
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
    if s.kind == 0 && msg == EM_SCROLLCARET {
        if s.layout_dirty {
            refresh(hwnd, &mut s);
        }
        s.wheel = 0;
    }
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
            WM_MOUSEWHEEL | WM_VSCROLL | WM_KEYDOWN | WM_CHAR | WM_LBUTTONUP | EM_SCROLLCARET
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
fn thumb(length: i32, i: &SCROLLINFO, dpi: u32) -> (i32, i32) {
    let length = (length - scale(8, dpi)).max(1);
    let size = ((length as f64 * i.nPage as f64 / (i.nMax + 1).max(1) as f64) as i32)
        .clamp(scale(24, dpi).min(length), length);
    (
        scale(4, dpi) + ((length - size) as f64 * i.nPos as f64 / limit(i).max(1) as f64) as i32,
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
    let tree = !GetPropW(s.owner, wide("PlumeTxtQuietScroll").as_ptr()).is_null();
    if tree && matches!(msg, WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP) {
        PostMessageW(s.owner, UPDATE, 2, 0);
    }
    let rc = client(hwnd);
    let length = if s.vertical { rc.bottom } else { rc.right };
    let mut i = info(s.owner, s.vertical);
    let held = !GetPropW(s.owner, wide("PlumeTxtHoldRange").as_ptr()).is_null();
    if held {
        if let Some(settled) = s.settled {
            i.nMax = settled.nMax;
            i.nPage = settled.nPage;
        }
    } else {
        s.settled = Some(i);
    }
    let draw_thumb = !held || s.settled.is_some();
    let (start, size) = thumb(length, &i, dpi(hwnd));
    match msg {
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            if s.buffer.ensure(dc, rc.right, rc.bottom) {
                fill(s.buffer.dc, rc, s.bg);
                let r = if s.vertical {
                    RECT {
                        left: px(hwnd, 4),
                        right: px(hwnd, 8),
                        top: start,
                        bottom: start + size,
                    }
                } else {
                    RECT {
                        left: start,
                        right: start + size,
                        top: px(hwnd, 4),
                        bottom: px(hwnd, 8),
                    }
                };
                if draw_thumb {
                    rounded(
                        s.buffer.dc,
                        r,
                        if s.drag {
                            ACCENT
                        } else if tree {
                            rgb(105, 130, 146)
                        } else {
                            MUTED
                        },
                        px(hwnd, 4),
                    );
                }
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
            let pos = ((p - s.offset - px(hwnd, 4)) as f64
                / (length - size - px(hwnd, 8)).max(1) as f64
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
            let pos = ((p - s.offset - px(hwnd, 4)) as f64
                / (length - size - px(hwnd, 8)).max(1) as f64
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
    assert_eq!(thumb(408, &i, 96), (4, 40));
    i.nPos = 900;
    assert_eq!(thumb(408, &i, 96), (364, 40));
    assert_eq!(thumb(816, &i, 192), (728, 80));
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
        resize(hwnd, 0, 0, 500, 240);
        // RichEdit can hide its native track while text/layout is being changed.
        ShowScrollBar(hwnd, SB_VERT, 0);
        SetWindowLongW(
            hwnd,
            GWL_STYLE,
            GetWindowLongW(hwnd, GWL_STYLE) & !(WS_VSCROLL as i32),
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
        SendMessageW(hwnd, WM_SIZE, 0, 0);
        let region = CreateRectRgn(0, 0, 0, 0);
        GetWindowRgn(hwnd, region);
        let mut bounds = zeroed();
        GetRgnBox(region, &mut bounds);
        assert!(
            bounds.right <= 500,
            "A hidden native track must not expand the visible editor into its gutter: {}",
            bounds.right
        );
        DeleteObject(region);
        measure(hwnd);
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
            let dc = GetDC(hwnd);
            let mut buffer = Buffer::default();
            assert!(buffer.ensure(dc, 500, 400));
            fill(
                buffer.dc,
                RECT {
                    left: 0,
                    top: 0,
                    right: 500,
                    bottom: 400,
                },
                rgb(255, 255, 255),
            );
            assert_eq!(SendMessageW(hwnd, WM_ERASEBKGND, buffer.dc as usize, 0), 1);
            let rc = editor_viewport(hwnd);
            assert_eq!(
                GetPixel(buffer.dc, 20, 20),
                rgb(255, 255, 255),
                "Erase must preserve the text area until buffered paint"
            );
            assert_eq!(
                GetPixel(buffer.dc, rc.right - 1, 20),
                CANVAS,
                "Reentrant background erase must cover the editor edge with its dark color"
            );
            ReleaseDC(hwnd, dc);
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
        for width in [180, 700, 240, 500] {
            resize(hwnd, 0, 0, width, 400);
            assert!(
                (*(host_data as *const RefCell<Host>)).borrow().layout_dirty,
                "Nested WM_SIZE must invalidate the measured range"
            );
            let first = info(hwnd, true);
            measure(hwnd);
            let settled = info(hwnd, true);
            assert_eq!(
                (first.nMax, first.nPage),
                (settled.nMax, settled.nPage),
                "The first thumb must use the final wrapped range"
            );
        }
        let max = limit(&info(hwnd, true));
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
        let mut data = 0;
        GetWindowSubclass(hwnd, Some(host_proc), 902, &mut data);
        let state = &*(data as *const RefCell<Host>);
        assert!(
            info(hwnd, true).nPos > 0,
            "Wheel over a short preview must scroll the source"
        );
        SendMessageW(hwnd, WM_MOUSEWHEEL, ((-120i16) as u16 as usize) << 16, 0);
        SetWindowTextW(hwnd, wide("Short source").as_ptr());
        measure(hwnd);
        assert_eq!(
            info(hwnd, true).nPos,
            0,
            "Shrinking text clamps immediately"
        );
        pair(hwnd, null_mut());
        DestroyWindow(short);
        let bar = state.borrow().v;
        SetWindowTextW(hwnd, wide(&"Stable range\r\n".repeat(3000)).as_ptr());
        SendMessageW(bar, WM_MOUSEMOVE, 0, 0);
        let bar_state = GetWindowLongPtrW(bar, GWLP_USERDATA) as *const RefCell<Bar>;
        let original_range = (*bar_state).borrow().settled.unwrap().nMax;
        hold_range(hwnd, true);
        SetWindowTextW(hwnd, wide(&"Replacement\r\n".repeat(100)).as_ptr());
        SendMessageW(bar, WM_MOUSEMOVE, 0, 0);
        assert_eq!((*bar_state).borrow().settled.unwrap().nMax, original_range);
        hold_range(hwnd, false);
        SendMessageW(bar, WM_MOUSEMOVE, 0, 0);
        assert_eq!(
            (*bar_state).borrow().settled.unwrap().nMax,
            info(hwnd, true).nMax
        );
        DestroyWindow(hwnd);
        FreeLibrary(library);
    }
}

#[test]
#[ignore = "Requires Windows native controls"]
fn native_tree_tracks_are_clipped_and_hide_when_idle() {
    unsafe {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TREEVIEW_CLASSES,
        });
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            0,
            0,
            400,
            400,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let tree = CreateWindowExW(
            0,
            wide("SysTreeView32").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            220,
            200,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        attach(tree, SURFACE);
        for i in 0..80 {
            let mut label = wide(&format!("{i} {}", "long filename ".repeat(15)));
            let item = TVINSERTSTRUCTW {
                hParent: TVI_ROOT,
                hInsertAfter: TVI_LAST,
                Anonymous: TVINSERTSTRUCTW_0 {
                    item: TVITEMW {
                        mask: TVIF_TEXT,
                        pszText: label.as_mut_ptr(),
                        ..zeroed()
                    },
                },
            };
            SendMessageW(tree, TVM_INSERTITEMW, 0, &item as *const _ as isize);
        }
        resize(tree, 0, 0, 220, 200);
        ShowWindow(parent, SW_SHOWNOACTIVATE);
        SendMessageW(tree, UPDATE, 0, 0);
        ValidateRect(tree, null());
        resize(tree, 0, 0, 220, 200);
        assert_eq!(
            GetUpdateRect(tree, null_mut(), 0),
            0,
            "Unchanged tree geometry must not repaint during layout"
        );
        let region = CreateRectRgn(0, 0, 0, 0);
        assert_ne!(GetWindowRgn(tree, region), 0);
        assert_ne!(PtInRegion(region, 219, 199), 0);
        assert_eq!(PtInRegion(region, 220, 100), 0);
        assert_eq!(PtInRegion(region, 100, 200), 0);
        DeleteObject(region);
        let mut data = 0;
        windows_sys::Win32::UI::Shell::GetWindowSubclass(tree, Some(host_proc), 902, &mut data);
        let (v, h) = {
            let state = (*(data as *const RefCell<Host>)).borrow();
            (state.v, state.h)
        };
        assert_eq!(IsWindowVisible(v), 0);
        SendMessageW(tree, UPDATE, 2, 0);
        assert_ne!(IsWindowVisible(v), 0);
        assert_eq!(IsWindowVisible(h), 0);
        set_position(tree, true, 30);
        assert!(
            info(tree, true).nPos > 0,
            "vertical range {} page {}",
            info(tree, true).nMax,
            info(tree, true).nPage
        );
        set_position(tree, false, 150);
        assert!(info(tree, false).nPos > 0);
        SendMessageW(tree, WM_TIMER, TREE_IDLE, 0);
        assert_eq!(IsWindowVisible(v), 0);
        assert_eq!(IsWindowVisible(h), 0);
        DestroyWindow(parent);
    }
}
