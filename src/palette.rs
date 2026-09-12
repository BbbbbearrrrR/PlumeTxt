use crate::theme::*;
use std::{
    cell::RefCell,
    mem::zeroed,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub type Command = (usize, &'static str, &'static str, &'static str);
pub const KEY: u32 = WM_APP + 31;
const REFRESH: u32 = WM_APP + 32;
struct State {
    hwnd: HWND,
    parent: HWND,
    previous: HWND,
    previous_scroll: Option<POINT>,
    edit: HWND,
    list: HWND,
    font: HFONT,
    small: HFONT,
    items: Vec<Command>,
    filtered: Vec<usize>,
    query: String,
    dispatch: u32,
    buffer: Buffer,
    reveal: i32,
}
pub struct Palette(pub HWND);
impl Palette {
    pub unsafe fn create(
        parent: HWND,
        font: HFONT,
        small: HFONT,
        items: Vec<Command>,
        dispatch: u32,
    ) -> Self {
        let name = wide("FeatherPadCommands");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            name.as_ptr(),
            wide("Commands").as_ptr(),
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            600,
            388,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, 600, 388, 24, 24), 0);
        let edit = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            24,
            20,
            490,
            32,
            hwnd,
            1usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(edit, WM_SETFONT, font as usize, 0);
        SendMessageW(
            edit,
            EM_SETCUEBANNER,
            1,
            wide("Search commands…").as_ptr() as isize,
        );
        let list = CreateWindowExW(
            0,
            wide("LISTBOX").as_ptr(),
            wide("Commands").as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | LBS_NOTIFY as u32
                | LBS_OWNERDRAWFIXED as u32
                | LBS_HASSTRINGS as u32
                | LBS_NOINTEGRALHEIGHT as u32,
            12,
            76,
            574,
            288,
            hwnd,
            2usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        dark_scrollbars(list, SURFACE);
        crate::scroll::resize(list, 12, 76, 574, 288);
        SendMessageW(list, WM_SETFONT, font as usize, 0);
        SendMessageW(list, LB_SETITEMHEIGHT, 0, 48);
        let state = State {
            hwnd,
            parent,
            previous: null_mut(),
            previous_scroll: None,
            edit,
            list,
            font,
            small,
            items,
            filtered: Vec::new(),
            query: String::new(),
            dispatch,
            buffer: Buffer::default(),
            reveal: 388,
        };
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::into_raw(Box::new(RefCell::new(state))) as isize,
        );
        Self(hwnd)
    }
    pub unsafe fn set_commands(&self, items: Vec<Command>) {
        with(self.0, |s| s.items = items);
    }
    pub unsafe fn show(&self) {
        with(self.0, |s| {
            if GetWindowLongW(s.hwnd, GWL_STYLE) as u32 & WS_VISIBLE != 0 {
                s.hide();
                return;
            }
            s.previous = GetFocus();
            s.previous_scroll = crate::scroll::saved_position(s.previous);
            let mut rc = zeroed();
            GetClientRect(s.parent, &mut rc);
            SetWindowPos(
                s.hwnd,
                HWND_TOP,
                (rc.right - 600) / 2,
                48,
                600,
                388,
                SWP_NOACTIVATE,
            );
            SetWindowTextW(s.edit, wide("").as_ptr());
            s.refresh();
            s.reveal = 60;
            SetWindowRgn(s.hwnd, CreateRoundRectRgn(0, 0, 600, s.reveal, 24, 24), 0);
            ShowWindow(s.hwnd, SW_SHOW);
            SetTimer(s.hwnd, 903, 15, None);
            SetFocus(s.edit);
        });
    }
    pub unsafe fn key(&self, msg: &MSG) -> bool {
        if IsWindowVisible(self.0) == 0 {
            return false;
        }
        if msg.message == WM_MOUSEWHEEL {
            with(self.0, |s| {
                SendMessageW(s.list, WM_MOUSEWHEEL, msg.wParam, msg.lParam);
            });
            return true;
        }
        if msg.message == WM_KEYDOWN
            && matches!(
                msg.wParam as u16,
                VK_ESCAPE | VK_RETURN | VK_UP | VK_DOWN | VK_TAB
            )
        {
            SendMessageW(self.0, KEY, msg.wParam, 0);
            return true;
        }
        false
    }
}
impl Drop for Palette {
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
fn matches(command: &Command, query: &str) -> bool {
    let haystack = command.1.to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(word))
}
impl State {
    unsafe fn current_query(&self) -> String {
        let mut value = vec![0u16; GetWindowTextLengthW(self.edit) as usize + 1];
        GetWindowTextW(self.edit, value.as_mut_ptr(), value.len() as i32);
        String::from_utf16_lossy(&value[..value.len() - 1]).to_lowercase()
    }
    unsafe fn refresh(&mut self) {
        let query = self.current_query();
        self.query = query.clone();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, c)| matches(c, &query))
            .map(|(i, _)| i)
            .collect();
        SendMessageW(self.list, WM_SETREDRAW, 0, 0);
        SendMessageW(self.list, LB_RESETCONTENT, 0, 0);
        for &index in &self.filtered {
            SendMessageW(
                self.list,
                LB_ADDSTRING,
                0,
                wide(self.items[index].1).as_ptr() as isize,
            );
        }
        SendMessageW(self.list, LB_SETCURSEL, 0, 0);
        SendMessageW(self.list, WM_SETREDRAW, 1, 0);
        ShowWindow(
            self.list,
            if self.filtered.is_empty() {
                SW_HIDE
            } else {
                SW_SHOWNA
            },
        );
        invalidate(self.list);
        invalidate(self.hwnd);
    }
    unsafe fn hide(&self) {
        KillTimer(self.hwnd, 903);
        ShowWindow(self.hwnd, SW_HIDE);
        if IsWindow(self.previous) != 0 {
            SetFocus(self.previous);
            if let Some(point) = self.previous_scroll {
                PostMessageW(self.previous, crate::scroll::POSITION, 0, point.x as isize);
                PostMessageW(self.previous, crate::scroll::POSITION, 1, point.y as isize);
                PostMessageW(self.parent, crate::scroll::SYNC, self.previous as usize, 0);
            }
        }
    }
    unsafe fn execute(&self) {
        let selected = SendMessageW(self.list, LB_GETCURSEL, 0, 0);
        if let Some(&index) = self.filtered.get(selected as usize) {
            let id = self.items[index].0;
            self.hide();
            PostMessageW(self.parent, self.dispatch, id, 0);
        }
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_NCDESTROY {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<State>;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !p.is_null() {
            drop(Box::from_raw(p));
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_MEASUREITEM {
        (*(lp as *mut MEASUREITEMSTRUCT)).itemHeight = 48;
        return 1;
    }
    if msg == WM_ERASEBKGND {
        return 1;
    }
    if msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORLISTBOX {
        let dc = wp as HDC;
        SetTextColor(dc, INK);
        SetBkColor(dc, SURFACE);
        SetDCBrushColor(dc, SURFACE);
        return GetStockObject(DC_BRUSH) as isize;
    }
    if msg == WM_COMMAND && wp >> 16 == EN_CHANGE as usize {
        PostMessageW(hwnd, REFRESH, 0, 0);
        return 0;
    }
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>;
    if p.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let Ok(mut s) = (*p).try_borrow_mut() else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        WM_TIMER if wp == 903 => {
            s.reveal += (388 - s.reveal + 2) / 3;
            if s.reveal >= 386 {
                s.reveal = 388;
                KillTimer(hwnd, 903);
            }
            SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, 600, s.reveal, 24, 24), 1);
        }
        WM_MOUSEWHEEL => {
            SendMessageW(s.list, WM_MOUSEWHEEL, wp, lp);
        }
        REFRESH => {
            if s.current_query() != s.query {
                s.refresh();
            }
        }
        WM_CLOSE => s.hide(),
        WM_ACTIVATE if wp & 0xffff == WA_INACTIVE as usize => {
            ShowWindow(hwnd, SW_HIDE);
        }
        WM_COMMAND if (wp >> 16) as u32 == LBN_DBLCLK => s.execute(),
        KEY => {
            if s.current_query() != s.query {
                s.refresh();
            }
            match wp as u16 {
                VK_ESCAPE => s.hide(),
                VK_RETURN => s.execute(),
                VK_UP | VK_DOWN => {
                    let current = SendMessageW(s.list, LB_GETCURSEL, 0, 0);
                    let step = if wp as u16 == VK_UP { -1 } else { 1 };
                    let next =
                        (current + step).clamp(0, s.filtered.len().saturating_sub(1) as isize);
                    SendMessageW(s.list, LB_SETCURSEL, next as usize, 0);
                    invalidate(s.list);
                    SetFocus(s.edit);
                }
                VK_TAB => {
                    SetFocus(if GetFocus() == s.edit { s.list } else { s.edit });
                }
                _ => (),
            }
        }
        WM_DRAWITEM => {
            let d = &*(lp as *const DRAWITEMSTRUCT);
            if let Some(&index) = s.filtered.get(d.itemID as usize) {
                let selected = d.itemState & ODS_SELECTED != 0;
                fill(d.hDC, d.rcItem, SURFACE);
                if selected {
                    let mut r = d.rcItem;
                    r.right -= 12;
                    rounded(d.hDC, r, SELECTED, 12);
                }
                if selected {
                    fill(
                        d.hDC,
                        RECT {
                            left: 0,
                            top: d.rcItem.top + 12,
                            right: 3,
                            bottom: d.rcItem.bottom - 12,
                        },
                        ACCENT,
                    );
                }
                let item = s.items[index];
                let mut r = d.rcItem;
                r.left += 18;
                r.right -= 130;
                label(
                    d.hDC,
                    item.1,
                    r,
                    s.font,
                    if selected { ACCENT } else { INK },
                    DT_SINGLELINE | DT_VCENTER,
                );
                r.left = r.right;
                r.right = d.rcItem.right - 16;
                label(
                    d.hDC,
                    item.2,
                    r,
                    s.small,
                    MUTED,
                    DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
                );
            }
            return 1;
        }
        WM_PAINT => {
            let mut ps = zeroed();
            let target = BeginPaint(hwnd, &mut ps);
            let rc = client(hwnd);
            if !s.buffer.ensure(target, rc.right, rc.bottom) {
                EndPaint(hwnd, &ps);
                return 0;
            }
            let dc = s.buffer.dc;
            fill(dc, rc, SURFACE);
            fill(
                dc,
                RECT {
                    left: 0,
                    top: 0,
                    right: rc.right,
                    bottom: 2,
                },
                ACCENT,
            );
            label(
                dc,
                "ESC",
                RECT {
                    left: 510,
                    top: 16,
                    right: 572,
                    bottom: 42,
                },
                s.small,
                MUTED,
                DT_SINGLELINE | DT_RIGHT,
            );
            fill(
                dc,
                RECT {
                    left: 24,
                    top: 63,
                    right: 574,
                    bottom: 64,
                },
                LINE,
            );
            if s.filtered.is_empty() {
                label(
                    dc,
                    "No results",
                    RECT {
                        left: 24,
                        top: 80,
                        right: 570,
                        bottom: 130,
                    },
                    s.font,
                    MUTED,
                    DT_SINGLELINE,
                );
            }
            s.buffer.blit(target, &ps.rcPaint);
            EndPaint(hwnd, &ps);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}
#[test]
fn command_search_matches_all_words() {
    let c = (1, "Markdown 导出 PDF", "Ctrl+P", "export print");
    assert!(matches(&c, "导出 pdf"));
    assert!(matches(&c, "markdown"));
    assert!(!matches(&c, "export"));
    assert!(!matches(&c, "ctrl+p"));
    assert!(!matches(&c, "导出 保存"));
}

#[test]
#[ignore = "Requires Windows native controls"]
fn native_search_executes_without_editing_document() {
    unsafe {
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("Palette test").as_ptr(),
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
        let palette = Palette::create(
            parent,
            fonts.ui,
            fonts.small,
            vec![
                (42, "导出 PDF", "Ctrl+P", "export"),
                (43, "保存", "Ctrl+S", "save"),
            ],
            WM_APP + 100,
        );
        palette.show();
        with(palette.0, |s| {
            SetWindowTextW(s.edit, wide("PDF").as_ptr());
            s.refresh();
            assert_eq!(s.filtered, vec![0]);
            assert_eq!(SendMessageW(s.list, LB_GETCOUNT, 0, 0), 1);
            s.execute();
        });
        assert_eq!(GetWindowLongW(palette.0, GWL_STYLE) as u32 & WS_VISIBLE, 0);
        let mut msg: MSG = zeroed();
        assert_ne!(
            PeekMessageW(&mut msg, parent, WM_APP + 100, WM_APP + 100, PM_REMOVE),
            0
        );
        assert_eq!(msg.wParam, 42);
        with(palette.0, |s| {
            SetWindowTextW(s.edit, wide("no-such-command").as_ptr());
            s.refresh();
            assert!(s.filtered.is_empty());
            s.execute();
        });
        assert_eq!(
            PeekMessageW(&mut msg, parent, WM_APP + 100, WM_APP + 100, PM_REMOVE),
            0
        );
        with(palette.0, |s| {
            s.items = vec![(1, "Command", "", ""); 20];
            SetWindowTextW(s.edit, wide("").as_ptr());
            s.refresh();
        });
        for _ in 0..120 {
            SendMessageW(palette.0, WM_MOUSEWHEEL, ((-1i16) as u16 as usize) << 16, 0);
        }
        with(palette.0, |s| {
            assert!(SendMessageW(s.list, LB_GETTOPINDEX, 0, 0) > 0)
        });
        let library =
            windows_sys::Win32::System::LibraryLoader::LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let editor = CreateWindowExW(
            0,
            wide("RICHEDIT50W").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD
                | WS_VSCROLL
                | crate::scroll::ES_DISABLENOSCROLL
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32,
            0,
            0,
            500,
            300,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(editor, WM_USER + 89, 1, 0);
        crate::scroll::attach(editor, CANVAS);
        SetWindowTextW(
            editor,
            wide(&"Scroll restoration\r\n".repeat(12000)).as_ptr(),
        );
        crate::scroll::measure(editor);
        with(palette.0, |s| {
            s.previous = editor;
            s.previous_scroll = Some(POINT { x: 0, y: 1000 });
        });
        SendMessageW(palette.0, KEY, VK_ESCAPE as usize, 0);
        while PeekMessageW(
            &mut msg,
            editor,
            crate::scroll::POSITION,
            crate::scroll::POSITION,
            PM_REMOVE,
        ) != 0
        {
            DispatchMessageW(&msg);
        }
        assert!(
            (crate::scroll::info(editor, true).nPos - 1000).abs() <= 2,
            "Closing commands must restore the reading position"
        );
        ShowWindow(parent, SW_SHOWNOACTIVATE);
        ShowWindow(editor, SW_SHOW);
        SetFocus(editor);
        crate::scroll::set_position(editor, true, 150000);
        palette.show();
        SendMessageW(palette.0, KEY, VK_ESCAPE as usize, 0);
        for _ in 0..30 {
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        #[repr(C)]
        struct ImePosition {
            size: u32,
            character: u32,
            point: POINT,
            line_height: u32,
            document: RECT,
        }
        let mut query: ImePosition = zeroed();
        query.size = size_of::<ImePosition>() as u32;
        SendMessageW(editor, WM_IME_REQUEST, 6, &mut query as *mut _ as isize);
        let mut point: POINT = zeroed();
        SendMessageW(editor, WM_USER + 221, 0, &mut point as *mut _ as isize);
        assert!(
            (point.y - 150000).abs() <= 2,
            "Visible focus restoration must preserve actual pixels"
        );
        DestroyWindow(editor);
        FreeLibrary(library);
        drop(palette);
        DestroyWindow(parent);
    }
}

#[test]
#[ignore = "Requires Windows native controls"]
fn native_palette_arrows_select_and_enter_executes_once() {
    unsafe {
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("Keys test").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            0,
            0,
            800,
            600,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let fonts = Fonts::new();
        let palette = Palette::create(
            parent,
            fonts.ui,
            fonts.small,
            vec![(41, "First", "", ""), (42, "Second", "", "")],
            WM_APP + 100,
        );
        palette.show();
        let key = |key| MSG {
            hwnd: parent,
            message: WM_KEYDOWN,
            wParam: key as usize,
            ..zeroed()
        };
        // A delayed focus transfer from the terminal must not bypass palette navigation.
        SetFocus(parent);
        assert!(palette.key(&key(VK_DOWN)));
        SendMessageW(palette.0, REFRESH, 0, 0);
        with(palette.0, |s| {
            assert_eq!(SendMessageW(s.list, LB_GETCURSEL, 0, 0), 1);
            assert_eq!(GetFocus(), s.edit);
            assert!(s.current_query().is_empty());
        });
        let mut msg: MSG = zeroed();
        assert_eq!(
            PeekMessageW(&mut msg, parent, WM_APP + 100, WM_APP + 100, PM_REMOVE),
            0
        );
        assert!(palette.key(&key(VK_UP)));
        assert!(palette.key(&key(VK_DOWN)));
        assert!(palette.key(&key(VK_RETURN)));
        assert_ne!(
            PeekMessageW(&mut msg, parent, WM_APP + 100, WM_APP + 100, PM_REMOVE),
            0
        );
        assert_eq!(msg.wParam, 42);
        assert_eq!(
            PeekMessageW(&mut msg, parent, WM_APP + 100, WM_APP + 100, PM_REMOVE),
            0
        );
        assert_eq!(IsWindowVisible(palette.0), 0);
        drop(palette);
        DestroyWindow(parent);
    }
}
