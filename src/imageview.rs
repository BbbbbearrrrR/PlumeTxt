use crate::{assets, theme::*};
use std::{
    mem::zeroed,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub struct ImageView(pub HWND);
struct State {
    dib: Vec<u8>,
    width: u32,
    height: u32,
    zoom: f64,
    x: i32,
    y: i32,
    drag: Option<(i32, i32)>,
    renderer: crate::render::Renderer,
}
impl ImageView {
    pub unsafe fn open(parent: HWND, path: &Path) -> Result<Self, String> {
        let (dib, width, height) = assets::load_picture(path)?;
        let class = wide("PlumeTxtImage");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(null()),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..zeroed()
        });
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Image").as_ptr(),
            WS_CHILD | WS_CLIPSIBLINGS | WS_TABSTOP,
            0,
            0,
            1,
            1,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if hwnd.is_null() {
            return Err("Could not create image viewer".into());
        }
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::into_raw(Box::new(State {
                dib,
                width,
                height,
                zoom: 1.,
                x: 0,
                y: 0,
                drag: None,
                renderer: crate::render::Renderer::default(),
            })) as isize,
        );
        Ok(Self(hwnd))
    }
    pub unsafe fn zoom(&self, factor: f64) {
        SendMessageW(
            self.0,
            WM_APP + 1,
            if factor == 0. {
                0
            } else if factor > 1. {
                1
            } else {
                2
            },
            0,
        );
    }
}
impl Drop for ImageView {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if msg == WM_NCDESTROY && !ptr.is_null() {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(ptr));
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let s = &mut *ptr;
    match msg {
        WM_ERASEBKGND => return 1,
        WM_SHOWWINDOW if wp == 0 => {
            s.renderer.release();
        }
        WM_SIZE => {
            invalidate(hwnd);
            return 0;
        }
        WM_LBUTTONDOWN => {
            s.drag = Some((lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32));
            SetFocus(hwnd);
            SetCapture(hwnd);
            return 0;
        }
        WM_LBUTTONUP => {
            s.drag = None;
            ReleaseCapture();
            return 0;
        }
        WM_CAPTURECHANGED => {
            s.drag = None;
            return 0;
        }
        WM_MOUSEMOVE => {
            if let Some((x, y)) = s.drag {
                let next = (lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32);
                s.x += next.0 - x;
                s.y += next.1 - y;
                s.drag = Some(next);
                invalidate(hwnd);
            }
            return 0;
        }
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as u16 as i16 as f64 / 120.;
            s.zoom = (s.zoom * 1.15f64.powf(delta)).clamp(0.1, 16.);
            invalidate(hwnd);
            return 0;
        }
        m if m == WM_APP + 1 => {
            if wp == 0 {
                s.zoom = 1.;
                s.x = 0;
                s.y = 0;
            } else {
                s.zoom = (s.zoom * if wp == 1 { 1.15 } else { 1. / 1.15 }).clamp(0.1, 16.);
            }
            invalidate(hwnd);
            return 0;
        }
        WM_PAINT => {
            let mut ps = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let rc = client(hwnd);
            s.renderer.paint(hwnd, dc, &ps.rcPaint, |canvas| {
                canvas.fill(rc, CANVAS);
                let fit = ((rc.right - px(hwnd, 40)).max(1) as f64 / s.width as f64)
                    .min((rc.bottom - px(hwnd, 40)).max(1) as f64 / s.height as f64)
                    .min(1.);
                let w = (s.width as f64 * fit * s.zoom).round().max(1.) as i32;
                let h = (s.height as f64 * fit * s.zoom).round().max(1.) as i32;
                let left = (rc.right - w) / 2 + s.x;
                let top = (rc.bottom - h) / 2 + s.y;
                let bounds = RECT {
                    left,
                    top,
                    right: left + w,
                    bottom: top + h,
                };
                canvas.shadow(bounds, px(hwnd, 1));
                canvas.bitmap(0, &s.dib[40..], s.width, s.height, true, bounds);
            });
            EndPaint(hwnd, &ps);
            return 0;
        }
        _ => (),
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

#[test]
#[ignore = "Requires Windows desktop and image codecs"]
fn native_image_open_zoom_resize_and_close() {
    unsafe {
        let _ole = assets::Ole::new().unwrap();
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            0,
            0,
            600,
            400,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        assert!(!parent.is_null());
        for ext in ["png", "jpg", "gif", "bmp", "tiff", "ico"] {
            let path = format!("tests/fixtures/images/sample.{ext}");
            let view = ImageView::open(parent, Path::new(&path)).unwrap();
            MoveWindow(view.0, 0, 0, 600, 400, 0);
            ShowWindow(parent, SW_SHOW);
            ShowWindow(view.0, SW_SHOW);
            UpdateWindow(view.0);
            let state = GetWindowLongPtrW(view.0, GWLP_USERDATA) as *const State;
            let dc = GetDC(view.0);
            assert_ne!(GetPixel(dc, 300, 200), CANVAS);
            ReleaseDC(view.0, dc);
            view.zoom(1.15);
            assert!((*state).zoom > 1.);
            MoveWindow(view.0, 0, 0, 320, 400, 0);
            UpdateWindow(view.0);
            view.zoom(0.);
            assert_eq!((*state).zoom, 1.);
            assert_eq!(((*state).x, (*state).y), (0, 0));
            let hwnd = view.0;
            drop(view);
            assert_eq!(IsWindow(hwnd), 0);
        }
        assert!(ImageView::open(parent, Path::new("missing.png")).is_err());
        DestroyWindow(parent);
    }
}
