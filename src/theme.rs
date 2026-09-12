use std::{
    mem::zeroed,
    ptr::{null, null_mut},
};
use windows_sys::Win32::UI::Controls::{EM_GETMODIFY, EM_SETMODIFY};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};

pub const INK: u32 = rgb(222, 232, 233);
pub const MUTED: u32 = rgb(133, 153, 156);
pub const CANVAS: u32 = rgb(9, 13, 15);
pub const LINE: u32 = rgb(33, 47, 50);
pub const SURFACE: u32 = rgb(16, 23, 26);
pub const ACCENT: u32 = rgb(63, 221, 207);
pub const SELECTED: u32 = rgb(23, 57, 58);
pub const WHITE: u32 = rgb(255, 255, 255);
pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub unsafe fn fill(dc: HDC, rect: RECT, color: u32) {
    SetDCBrushColor(dc, color);
    FillRect(dc, &rect, GetStockObject(DC_BRUSH) as _);
}
pub unsafe fn label(dc: HDC, text: &str, mut rect: RECT, font: HFONT, color: u32, flags: u32) {
    let old = SelectObject(dc, font);
    SetTextColor(dc, color);
    SetBkMode(dc, TRANSPARENT as i32);
    let text = wide(text);
    DrawTextW(
        dc,
        text.as_ptr(),
        (text.len() - 1) as i32,
        &mut rect,
        flags | DT_NOPREFIX,
    );
    SelectObject(dc, old);
}
pub unsafe fn font(size: i32, weight: i32, name: &str) -> HFONT {
    CreateFontW(
        -size,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        wide(name).as_ptr(),
    )
}
pub struct Fonts {
    pub ui: HFONT,
    pub small: HFONT,
    pub code: HFONT,
    pub body: HFONT,
}
impl Fonts {
    pub unsafe fn new() -> Self {
        Self {
            ui: font(16, 350, "Segoe UI Semilight"),
            small: font(13, 400, "Segoe UI"),
            code: font(20, 400, "Consolas"),
            body: font(20, 350, "Segoe UI Semilight"),
        }
    }
}
impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for f in [self.ui, self.small, self.code, self.body] {
                DeleteObject(f);
            }
        }
    }
}

// Persistent, viewport-sized GDI back buffer. No visible erase step between frames.
pub struct Buffer {
    pub dc: HDC,
    bitmap: HBITMAP,
    original: HGDIOBJ,
    width: i32,
    height: i32,
}
impl Default for Buffer {
    fn default() -> Self {
        Self {
            dc: null_mut(),
            bitmap: null_mut(),
            original: null_mut(),
            width: 0,
            height: 0,
        }
    }
}
impl Buffer {
    pub unsafe fn ensure(&mut self, target: HDC, width: i32, height: i32) -> bool {
        if width <= 0 || height <= 0 {
            return false;
        }
        if !self.dc.is_null() && self.width == width && self.height == height {
            return true;
        }
        self.clear();
        self.dc = CreateCompatibleDC(target);
        if self.dc.is_null() {
            return false;
        }
        self.bitmap = CreateCompatibleBitmap(target, width, height);
        if self.bitmap.is_null() {
            self.clear();
            return false;
        }
        self.original = SelectObject(self.dc, self.bitmap);
        self.width = width;
        self.height = height;
        true
    }
    pub unsafe fn blit(&self, target: HDC, rect: &RECT) {
        BitBlt(
            target,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            self.dc,
            rect.left,
            rect.top,
            SRCCOPY,
        );
    }
    unsafe fn clear(&mut self) {
        if !self.dc.is_null() {
            if !self.original.is_null() {
                SelectObject(self.dc, self.original);
            }
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
        self.dc = null_mut();
        self.bitmap = null_mut();
        self.original = null_mut();
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            self.clear();
        }
    }
}
pub unsafe fn client(hwnd: HWND) -> RECT {
    let mut rect = zeroed();
    GetClientRect(hwnd, &mut rect);
    rect
}
pub unsafe fn invalidate(hwnd: HWND) {
    InvalidateRect(hwnd, null(), 0);
}

pub unsafe fn move_window(hwnd: HWND, x: i32, y: i32, width: i32, height: i32, _repaint: i32) {
    let mut previous: RECT = zeroed();
    GetWindowRect(hwnd, &mut previous);
    let mut origin = POINT {
        x: previous.left,
        y: previous.top,
    };
    ScreenToClient(GetParent(hwnd), &mut origin);
    if origin.x == x
        && origin.y == y
        && previous.right - previous.left == width
        && previous.bottom - previous.top == height
    {
        return;
    }
    // Do not stretch/copy old glyphs while the new layout is pending.
    SetWindowPos(
        hwnd,
        null_mut(),
        x,
        y,
        width,
        height,
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOREDRAW | SWP_NOCOPYBITS,
    );
    invalidate(hwnd);
    invalidate(GetParent(hwnd));
}

// CHARFORMATW has the same layout on Win32 and Win64.
#[repr(C)]
struct CharacterFormat {
    size: u32,
    mask: u32,
    effects: u32,
    height: i32,
    offset: i32,
    color: u32,
    charset: u8,
    pitch: u8,
    face: [u16; 32],
}
pub unsafe fn editor_colors(hwnd: HWND) {
    let modified = SendMessageW(hwnd, EM_GETMODIFY, 0, 0);
    SendMessageW(hwnd, WM_USER + 67, 0, CANVAS as isize);
    let format = CharacterFormat {
        size: std::mem::size_of::<CharacterFormat>() as u32,
        mask: 0x40000000,
        color: INK,
        ..zeroed()
    };
    SendMessageW(hwnd, WM_USER + 68, 0, &format as *const _ as isize);
    SendMessageW(hwnd, WM_USER + 68, 4, &format as *const _ as isize);
    SendMessageW(hwnd, EM_SETMODIFY, modified as usize, 0);
}

pub unsafe fn dark_scrollbars(hwnd: HWND, background: u32) {
    crate::scroll::attach(hwnd, background);
}

// A compositor-backed divider guide keeps expensive document reflow out of mouse motion.
pub struct Divider {
    hwnd: HWND,
    pub x: i32,
}
impl Divider {
    pub unsafe fn new(parent: HWND, x: i32) -> Self {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        let name = wide("FeatherPadDivider");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(divider_proc),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: name.as_ptr(),
            ..zeroed()
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
            name.as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            0,
            0,
            2,
            1,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        SetLayeredWindowAttributes(hwnd, 0, 200, LWA_ALPHA);
        let mut guide = Self { hwnd, x };
        guide.move_to(parent, x);
        guide
    }
    pub unsafe fn move_to(&mut self, parent: HWND, x: i32) {
        self.x = x;
        let mut origin = POINT { x, y: 0 };
        ClientToScreen(parent, &mut origin);
        SetWindowPos(
            self.hwnd,
            HWND_TOP,
            origin.x,
            origin.y,
            2,
            client(parent).bottom.max(1),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}
impl Drop for Divider {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}
unsafe extern "system" fn divider_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_PAINT {
        let mut paint = zeroed();
        let dc = BeginPaint(hwnd, &mut paint);
        fill(dc, client(hwnd), ACCENT);
        EndPaint(hwnd, &paint);
        return 0;
    }
    if msg == WM_ERASEBKGND {
        return 1;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
pub unsafe fn rounded(dc: HDC, rect: RECT, color: u32, radius: i32) {
    let brush = CreateSolidBrush(color);
    let old_brush = SelectObject(dc, brush);
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    RoundRect(
        dc,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        radius,
        radius,
    );
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
    DeleteObject(brush);
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn changing_font_preserves_readable_color_and_clean_state() {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        assert!(!library.is_null());
        let hwnd = CreateWindowExW(
            0,
            wide("RICHEDIT50W").as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            0,
            0,
            500,
            300,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(hwnd, WM_USER + 89, 1, 0);
        SetWindowTextW(hwnd, wide("中文 example").as_ptr());
        SendMessageW(hwnd, EM_SETMODIFY, 0, 0);
        let fonts = Fonts::new();
        for font in [fonts.body, fonts.code] {
            SendMessageW(hwnd, WM_SETFONT, font as usize, 0);
            editor_colors(hwnd);
            let mut format = CharacterFormat {
                size: std::mem::size_of::<CharacterFormat>() as u32,
                ..zeroed()
            };
            SendMessageW(hwnd, WM_USER + 58, 0, &mut format as *mut _ as isize);
            assert_eq!(format.color, INK);
            assert_eq!(format.effects & 0x40000000, 0);
            assert_eq!(SendMessageW(hwnd, EM_GETMODIFY, 0, 0), 0);
        }
        DestroyWindow(hwnd);
        windows_sys::Win32::Foundation::FreeLibrary(library);
    }
}
