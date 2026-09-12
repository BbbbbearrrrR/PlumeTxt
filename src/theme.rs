use std::{
    mem::zeroed,
    ptr::{null, null_mut},
};
use windows_sys::Win32::UI::Controls::{EM_GETMODIFY, EM_SETMODIFY};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};

pub const INK: u32 = rgb(222, 232, 233);
pub const MUTED: u32 = rgb(156, 173, 184);
pub const CANVAS: u32 = rgb(12, 17, 23);
pub const LINE: u32 = rgb(48, 62, 74);
pub const SURFACE: u32 = rgb(23, 31, 40);
pub const FIELD: u32 = rgb(16, 23, 31);
pub const HOVER: u32 = rgb(39, 54, 66);
pub const ACCENT: u32 = rgb(63, 221, 207);
pub const SELECTED: u32 = rgb(28, 70, 77);
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
    #[cfg(test)]
    pub unsafe fn new() -> Self {
        Self::at_dpi(96)
    }
    pub unsafe fn at_dpi(dpi: u32) -> Self {
        Self {
            ui: font(scale(16, dpi), 400, "Segoe UI"),
            small: font(scale(13, dpi), 400, "Segoe UI"),
            code: font(scale(20, dpi), 400, "Consolas"),
            body: font(scale(20, dpi), 400, "Segoe UI"),
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
    SetDCBrushColor(dc, color);
    let old_brush = SelectObject(dc, GetStockObject(DC_BRUSH));
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
}

// Static material: edge, upper highlight and inset body, without blur surfaces.
pub unsafe fn panel(dc: HDC, mut rect: RECT, body: u32, edge: u32, radius: i32, unit: i32) {
    rounded(dc, rect, edge, radius);
    InflateRect(&mut rect, -unit, -unit);
    rounded(dc, rect, body, (radius - unit).max(1));
    let top = RECT {
        left: rect.left + radius / 2,
        top: rect.top,
        right: rect.right - radius / 2,
        bottom: rect.top + unit,
    };
    fill(dc, top, rgb(61, 77, 90));
}
pub unsafe fn shadow(dc: HDC, rect: RECT, unit: i32) {
    for (spread, shade) in [(5, rgb(8, 12, 17)), (3, rgb(6, 9, 13)), (1, rgb(3, 5, 8))] {
        rounded(
            dc,
            RECT {
                left: rect.left - spread * unit,
                top: rect.top + unit,
                right: rect.right + spread * unit,
                bottom: rect.bottom + (spread + 2) * unit,
            },
            shade,
            16 * unit,
        );
    }
}
pub unsafe fn input_frame(parent: HWND, dc: HDC, edit: HWND) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
    let mut rect: RECT = zeroed();
    GetWindowRect(edit, &mut rect);
    MapWindowPoints(null_mut(), parent, &mut rect as *mut RECT as *mut POINT, 2);
    InflateRect(&mut rect, px(parent, 7), px(parent, 6));
    panel(
        dc,
        rect,
        FIELD,
        if GetFocus() == edit { ACCENT } else { LINE },
        px(parent, 12),
        px(parent, 1),
    );
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

// Time-based easing keeps duration independent of delayed UI timer messages.
pub fn ease_out(progress: f64) -> f64 {
    1. - (1. - progress.clamp(0., 1.)).powi(3)
}

#[test]
fn animation_progress_is_bounded_and_monotonic() {
    assert_eq!(ease_out(-1.), 0.);
    assert_eq!(ease_out(1.), 1.);
    assert_eq!(ease_out(2.), 1.);
    assert!((ease_out(0.5) - 0.875).abs() < 1e-10);
    for i in 0..100 {
        assert!(ease_out(i as f64 / 100.) <= ease_out((i + 1) as f64 / 100.));
    }
}

pub unsafe fn animations_enabled() -> bool {
    let mut enabled: i32 = 1;
    SystemParametersInfoW(
        SPI_GETCLIENTAREAANIMATION,
        0,
        &mut enabled as *mut _ as _,
        0,
    );
    enabled != 0
}
pub unsafe fn animation_progress(start: std::time::Instant, duration: f64) -> f64 {
    if animations_enabled() {
        ease_out(start.elapsed().as_secs_f64() / duration)
    } else {
        1.
    }
}

pub const FONTS_CHANGED: u32 = WM_APP + 180;
pub fn scale(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi.max(96) as i64 + if value >= 0 { 48 } else { -48 }) / 96) as i32
}
pub unsafe fn dpi(hwnd: HWND) -> u32 {
    let root = GetAncestor(hwnd, GA_ROOT);
    let stored = GetPropW(root, wide("FeatherPadDpi").as_ptr()) as usize;
    if stored > 0 {
        stored as u32
    } else {
        windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96)
    }
}
pub unsafe fn px(hwnd: HWND, value: i32) -> i32 {
    scale(value, dpi(hwnd))
}

// Replace borrowed handles in every native descendant before freeing the old font set.
pub unsafe fn replace_fonts(hwnd: HWND, old: &Fonts, new: &Fonts) {
    let pairs = [
        (old.ui, new.ui),
        (old.small, new.small),
        (old.code, new.code),
        (old.body, new.body),
    ];
    unsafe extern "system" fn update(hwnd: HWND, data: isize) -> i32 {
        let pairs = &*(data as *const [(HFONT, HFONT); 4]);
        let current = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
        if let Some((_, replacement)) = pairs.iter().find(|(old, _)| *old == current) {
            SendMessageW(hwnd, WM_SETFONT, *replacement as usize, 0);
        }
        let mut class = [0u16; 32];
        GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
        if String::from_utf16_lossy(&class).starts_with("SysTreeView32") {
            SendMessageW(
                hwnd,
                windows_sys::Win32::UI::Controls::TVM_SETITEMHEIGHT,
                px(hwnd, 26) as usize,
                0,
            );
        }
        invalidate(hwnd);
        1
    }
    EnumChildWindows(hwnd, Some(update), &pairs as *const _ as isize);
}

pub unsafe fn attach_button(hwnd: HWND) {
    windows_sys::Win32::UI::Shell::SetWindowSubclass(hwnd, Some(button_proc), 110, 0);
}
unsafe extern "system" fn button_proc(
    hwnd: HWND,
    msg: u32,
    wp: usize,
    lp: isize,
    _: usize,
    _: usize,
) -> isize {
    use windows_sys::Win32::UI::{Input::KeyboardAndMouse::*, Shell::*};
    match msg {
        WM_SETFOCUS => {
            invalidate(hwnd);
            invalidate(GetParent(hwnd));
        }
        WM_MOUSEMOVE => {
            if GetPropW(hwnd, wide("FeatherPadHover").as_ptr()).is_null() {
                SetPropW(hwnd, wide("FeatherPadHover").as_ptr(), 1usize as _);
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut track);
                invalidate(hwnd);
            }
        }
        windows_sys::Win32::UI::Controls::WM_MOUSELEAVE | WM_KILLFOCUS => {
            RemovePropW(hwnd, wide("FeatherPadHover").as_ptr());
            invalidate(hwnd);
            invalidate(GetParent(hwnd));
        }
        WM_NCDESTROY => {
            RemovePropW(hwnd, wide("FeatherPadHover").as_ptr());
            RemoveWindowSubclass(hwnd, Some(button_proc), 110);
        }
        _ => (),
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

#[test]
fn dpi_dimensions_round_and_return_without_drift() {
    for dpi in [96, 120, 144, 192, 240] {
        assert_eq!(scale(96, dpi), dpi as i32);
        assert_eq!(scale(-96, dpi), -(dpi as i32));
        assert!(scale(24, dpi) > scale(16, dpi));
    }
    assert_eq!(scale(13, 120), 16);
    assert_eq!(scale(24, 144), 36);
}

// Let DWM own title-bar material, window shadow and supported-system fallback.
pub unsafe fn window_material(hwnd: HWND) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    for (attribute, value) in [(20u32, 1u32), (33, 2), (34, LINE), (35, CANVAS), (36, INK)] {
        DwmSetWindowAttribute(hwnd, attribute, &value as *const _ as _, 4);
    }
    let backdrop = 2u32;
    if DwmSetWindowAttribute(hwnd, 38, &backdrop as *const _ as _, 4) >= 0 {
        let default = 0xffffffffu32;
        DwmSetWindowAttribute(hwnd, 35, &default as *const _ as _, 4);
    }
}
