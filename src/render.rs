//! On-demand hardware drawing for custom views; native input controls stay native.
use crate::theme::{self, Buffer};
use std::{
    cell::RefCell,
    collections::VecDeque,
    mem::zeroed,
    ptr::null_mut,
    rc::{Rc, Weak},
    time::Instant,
};
use windows::{
    core::{Result, PCWSTR},
    Win32::Graphics::{
        Direct2D::{Common::*, *},
        DirectWrite::*,
        Dxgi::Common::*,
    },
};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*};

const TEXTURE_BYTES: usize = 16 * 1024 * 1024;
type Factories = (ID2D1Factory, IDWriteFactory);
thread_local! { static FACTORIES: RefCell<Weak<Factories>> = const { RefCell::new(Weak::new()) }; }

#[derive(Default)]
pub struct Renderer {
    gpu: Option<Gpu>,
    fallback: Buffer,
    retry: Option<Instant>,
    over_budget: bool,
}
struct Gpu {
    _factories: Rc<Factories>,
    target: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    write: IDWriteFactory,
    fonts: Vec<(usize, IDWriteTextFormat)>,
    textures: VecDeque<(u64, ID2D1Bitmap, usize)>,
    dpi: u32,
    failed: bool,
    frame_bytes: usize,
    over_budget: bool,
}
pub struct Canvas<'a> {
    gpu: Option<&'a mut Gpu>,
    dc: HDC,
}

impl Renderer {
    pub fn release(&mut self) {
        self.gpu = None;
        self.over_budget = false;
        self.retry = None;
        self.fallback = Buffer::default();
    }
    pub fn forget_bitmap(&mut self, key: u64) {
        self.over_budget = false;
        if let Some(gpu) = &mut self.gpu {
            gpu.textures.retain(|t| t.0 != key);
        }
    }
    pub unsafe fn paint(
        &mut self,
        hwnd: HWND,
        dc: HDC,
        dirty: &RECT,
        mut draw: impl FnMut(&mut Canvas),
    ) {
        let rc = theme::client(hwnd);
        if rc.right <= 0
            || rc.bottom <= 0
            || windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(
                windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor(
                    hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::GA_ROOT,
                ),
            ) != 0
        {
            self.release();
            return;
        }
        if self.gpu.is_none()
            && !self.over_budget
            && self.retry.is_none_or(|t| t.elapsed().as_secs() >= 5)
            && std::env::var_os("PLUMETXT_RENDERER")
                .or_else(|| std::env::var_os("FEATHERPAD_RENDERER"))
                .is_none_or(|v| v != "gdi")
        {
            match Gpu::new(hwnd, rc) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(_) => self.retry = Some(Instant::now()),
            }
        }
        if let Some(gpu) = &mut self.gpu {
            let size = D2D_SIZE_U {
                width: rc.right as u32,
                height: rc.bottom as u32,
            };
            let old = gpu.target.GetPixelSize();
            let resized = (old.width == size.width && old.height == size.height)
                || gpu.target.Resize(&size).is_ok();
            if gpu.dpi != theme::dpi(hwnd) {
                gpu.fonts.clear();
                gpu.dpi = theme::dpi(hwnd);
            }
            if resized {
                gpu.failed = false;
                gpu.frame_bytes = 0;
                gpu.over_budget = false;
                gpu.target.BeginDraw();
                draw(&mut Canvas { gpu: Some(gpu), dc });
                let ended = gpu.target.EndDraw(None, None);
                if ended.is_ok() && !gpu.failed {
                    // Never retain both a viewport-sized GDI buffer and GPU back buffer.
                    self.fallback = Buffer::default();
                    return;
                }
            }
            self.over_budget = gpu.over_budget;
            self.gpu = None;
            self.retry = Some(Instant::now());
        }
        if self.fallback.ensure(dc, rc.right, rc.bottom) {
            draw(&mut Canvas {
                gpu: None,
                dc: self.fallback.dc,
            });
            self.fallback.blit(dc, dirty);
        } else {
            draw(&mut Canvas { gpu: None, dc });
        }
    }
}
impl Gpu {
    unsafe fn new(hwnd: HWND, rc: RECT) -> Result<Self> {
        let factories = FACTORIES.with(|slot| -> Result<Rc<Factories>> {
            let mut slot = slot.borrow_mut();
            if let Some(factories) = slot.upgrade() {
                return Ok(factories);
            }
            let factories = Rc::new((
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
            ));
            *slot = Rc::downgrade(&factories);
            Ok(factories)
        })?;
        let (factory, write) = &*factories;
        let target = factory.CreateHwndRenderTarget(
            &D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_HARDWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.,
                dpiY: 96.,
                ..Default::default()
            },
            &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: windows::Win32::Foundation::HWND(hwnd),
                pixelSize: D2D_SIZE_U {
                    width: rc.right as u32,
                    height: rc.bottom as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            },
        )?;
        let brush = target.CreateSolidColorBrush(&color(theme::INK, 1.), None)?;
        Ok(Self {
            target,
            brush,
            write: write.clone(),
            _factories: factories,
            fonts: Vec::new(),
            textures: VecDeque::new(),
            dpi: theme::dpi(hwnd),
            failed: false,
            frame_bytes: 0,
            over_budget: false,
        })
    }
    unsafe fn font(&mut self, font: HFONT) -> Result<IDWriteTextFormat> {
        if let Some((_, format)) = self.fonts.iter().find(|f| f.0 == font as usize) {
            return Ok(format.clone());
        }
        let mut info: LOGFONTW = zeroed();
        GetObjectW(font, size_of::<LOGFONTW>() as i32, &mut info as *mut _ as _);
        let format = self.write.CreateTextFormat(
            PCWSTR(info.lfFaceName.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT(info.lfWeight),
            if info.lfItalic != 0 {
                DWRITE_FONT_STYLE_ITALIC
            } else {
                DWRITE_FONT_STYLE_NORMAL
            },
            DWRITE_FONT_STRETCH_NORMAL,
            info.lfHeight.unsigned_abs().max(1) as f32,
            windows::core::w!(""),
        )?;
        if self.fonts.len() >= 8 {
            self.fonts.clear();
        }
        self.fonts.push((font as usize, format.clone()));
        Ok(format)
    }
}
fn color(c: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (c & 255) as f32 / 255.,
        g: ((c >> 8) & 255) as f32 / 255.,
        b: ((c >> 16) & 255) as f32 / 255.,
        a,
    }
}
fn rect(r: RECT) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left as f32,
        top: r.top as f32,
        right: r.right as f32,
        bottom: r.bottom as f32,
    }
}
impl Canvas<'_> {
    pub unsafe fn fill(&mut self, r: RECT, c: u32) {
        if let Some(g) = &self.gpu {
            g.brush.SetColor(&color(c, 1.));
            g.target.FillRectangle(&rect(r), &g.brush);
        } else {
            theme::fill(self.dc, r, c);
        }
    }
    pub unsafe fn rounded(&mut self, r: RECT, c: u32, radius: i32) {
        if let Some(g) = &self.gpu {
            g.brush.SetColor(&color(c, 1.));
            g.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(r),
                    radiusX: radius as f32 / 2.,
                    radiusY: radius as f32 / 2.,
                },
                &g.brush,
            );
        } else {
            theme::rounded(self.dc, r, c, radius);
        }
    }
    pub unsafe fn shadow(&mut self, r: RECT, scale: i32) {
        // Three bounded layers, no blur texture or continuous animation.
        for (spread, shade) in [
            (5, theme::rgb(7, 10, 13)),
            (3, theme::rgb(5, 7, 10)),
            (1, 0),
        ] {
            self.rounded(
                RECT {
                    left: r.left - spread * scale,
                    top: r.top + scale,
                    right: r.right + spread * scale,
                    bottom: r.bottom + (spread + 2) * scale,
                },
                shade,
                8 * scale,
            );
        }
    }
    pub unsafe fn label(&mut self, text: &str, r: RECT, font: HFONT, c: u32, flags: u32) {
        if let Some(g) = &mut self.gpu {
            let result = (|| -> Result<()> {
                let format = g.font(font)?;
                format.SetTextAlignment(if flags & DT_CENTER != 0 {
                    DWRITE_TEXT_ALIGNMENT_CENTER
                } else if flags & DT_RIGHT != 0 {
                    DWRITE_TEXT_ALIGNMENT_TRAILING
                } else {
                    DWRITE_TEXT_ALIGNMENT_LEADING
                })?;
                format.SetParagraphAlignment(if flags & DT_VCENTER != 0 {
                    DWRITE_PARAGRAPH_ALIGNMENT_CENTER
                } else {
                    DWRITE_PARAGRAPH_ALIGNMENT_NEAR
                })?;
                format.SetWordWrapping(if flags & DT_WORDBREAK != 0 {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                })?;
                if flags & DT_END_ELLIPSIS != 0 {
                    let sign = g.write.CreateEllipsisTrimmingSign(&format)?;
                    format.SetTrimming(
                        &DWRITE_TRIMMING {
                            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                            ..Default::default()
                        },
                        &sign,
                    )?;
                } else {
                    format.SetTrimming(&DWRITE_TRIMMING::default(), None)?;
                }
                g.brush.SetColor(&color(c, 1.));
                let text: Vec<u16> = text.encode_utf16().collect();
                g.target.DrawText(
                    &text,
                    &format,
                    &rect(r),
                    &g.brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_GDI_CLASSIC,
                );
                Ok(())
            })();
            g.failed |= result.is_err();
        } else {
            theme::label(self.dc, text, r, font, c, flags);
        }
    }
    pub unsafe fn text_width(&self, text: &str, font: HFONT) -> i32 {
        let old = SelectObject(self.dc, font);
        let text = theme::wide(text);
        let mut size: SIZE = zeroed();
        GetTextExtentPoint32W(self.dc, text.as_ptr(), text.len() as i32 - 1, &mut size);
        SelectObject(self.dc, old);
        size.cx
    }
    pub unsafe fn clip(&mut self, r: RECT) {
        if let Some(g) = &self.gpu {
            g.target
                .PushAxisAlignedClip(&rect(r), D2D1_ANTIALIAS_MODE_ALIASED);
        } else {
            SaveDC(self.dc);
            IntersectClipRect(self.dc, r.left, r.top, r.right, r.bottom);
        }
    }
    pub unsafe fn unclip(&mut self) {
        if let Some(g) = &self.gpu {
            g.target.PopAxisAlignedClip();
        } else {
            RestoreDC(self.dc, -1);
        }
    }
    pub unsafe fn tint(&mut self, r: RECT, c: u32, alpha: u8) {
        if let Some(g) = &self.gpu {
            g.brush.SetColor(&color(c, alpha as f32 / 255.));
            g.target.FillRectangle(&rect(r), &g.brush);
        } else {
            let mut pixel = Buffer::default();
            if pixel.ensure(self.dc, 1, 1) {
                theme::fill(
                    pixel.dc,
                    RECT {
                        left: 0,
                        top: 0,
                        right: 1,
                        bottom: 1,
                    },
                    c,
                );
                GdiAlphaBlend(
                    self.dc,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    pixel.dc,
                    0,
                    0,
                    1,
                    1,
                    BLENDFUNCTION {
                        BlendOp: AC_SRC_OVER as u8,
                        BlendFlags: 0,
                        SourceConstantAlpha: alpha,
                        AlphaFormat: 0,
                    },
                );
            }
        }
    }
    pub unsafe fn bitmap(
        &mut self,
        key: u64,
        pixels: &[u8],
        width: u32,
        height: u32,
        bgr24: bool,
        r: RECT,
    ) {
        let stride = if bgr24 {
            (width as usize * 3 + 3) & !3
        } else {
            width as usize * 4
        };
        if width == 0
            || height == 0
            || stride
                .checked_mul(height as usize)
                .is_none_or(|n| n > pixels.len())
        {
            return;
        }
        if let Some(g) = &mut self.gpu {
            let result = (|| -> Result<()> {
                let bytes = width as usize * height as usize * 4;
                g.frame_bytes += bytes;
                if g.frame_bytes > TEXTURE_BYTES
                    || width > g.target.GetMaximumBitmapSize()
                    || height > g.target.GetMaximumBitmapSize()
                {
                    g.over_budget = true;
                    return Err(windows::core::Error::from_hresult(
                        windows::Win32::Foundation::E_OUTOFMEMORY,
                    ));
                }
                let index = g.textures.iter().position(|t| t.0 == key);
                let texture = if let Some(index) = index {
                    g.textures.remove(index).unwrap()
                } else {
                    while g.textures.iter().map(|t| t.2).sum::<usize>() + bytes > TEXTURE_BYTES {
                        g.textures.pop_front();
                    }
                    let converted;
                    let pixels = if bgr24 {
                        let stride = (width as usize * 3 + 3) & !3;
                        converted = (0..height as usize)
                            .rev()
                            .flat_map(|y| {
                                pixels[y * stride..y * stride + width as usize * 3]
                                    .chunks_exact(3)
                                    .flat_map(|p| [p[0], p[1], p[2], 255])
                            })
                            .collect::<Vec<_>>();
                        &converted[..]
                    } else {
                        pixels
                    };
                    let bitmap = g.target.CreateBitmap(
                        D2D_SIZE_U { width, height },
                        Some(pixels.as_ptr().cast()),
                        width * 4,
                        &D2D1_BITMAP_PROPERTIES {
                            pixelFormat: D2D1_PIXEL_FORMAT {
                                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                alphaMode: D2D1_ALPHA_MODE_IGNORE,
                            },
                            dpiX: 96.,
                            dpiY: 96.,
                        },
                    )?;
                    (key, bitmap, bytes)
                };
                g.target.DrawBitmap(
                    &texture.1,
                    Some(&rect(r)),
                    1.,
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
                g.textures.push_back(texture);
                Ok(())
            })();
            g.failed |= result.is_err();
        } else {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: 40,
                    biWidth: width as i32,
                    biHeight: if bgr24 {
                        height as i32
                    } else {
                        -(height as i32)
                    },
                    biPlanes: 1,
                    biBitCount: if bgr24 { 24 } else { 32 },
                    ..zeroed()
                },
                ..zeroed()
            };
            SetStretchBltMode(self.dc, HALFTONE);
            SetBrushOrgEx(self.dc, 0, 0, null_mut());
            StretchDIBits(
                self.dc,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                0,
                0,
                width as i32,
                height as i32,
                pixels.as_ptr().cast(),
                &info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
    }
}

#[test]
#[ignore = "Requires Windows graphics; exercises hardware and GDI fallback"]
fn native_renderer_draws_reuses_and_recovers() {
    use windows_sys::Win32::{System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*};
    unsafe {
        let hwnd = CreateWindowExW(
            0,
            theme::wide("STATIC").as_ptr(),
            theme::wide("Renderer check").as_ptr(),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            1280,
            800,
            null_mut(),
            null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        assert!(!hwnd.is_null());
        let dc = GetDC(hwnd);
        let rc = theme::client(hwnd);
        let fonts = theme::Fonts::new();
        let pixels = [30, 80, 220, 255].repeat(1600 * 1000);
        let scene = |canvas: &mut Canvas| {
            canvas.fill(rc, theme::CANVAS);
            canvas.shadow(
                RECT {
                    left: 20,
                    top: 20,
                    right: 920,
                    bottom: 760,
                },
                1,
            );
            canvas.bitmap(
                1,
                &pixels,
                1600,
                1000,
                false,
                RECT {
                    left: 20,
                    top: 20,
                    right: 920,
                    bottom: 760,
                },
            );
            canvas.clip(RECT {
                left: 930,
                top: 0,
                right: 1270,
                bottom: 800,
            });
            for row in 0..14 {
                canvas.label(
                    "中文 GPU / GDI text 123",
                    RECT {
                        left: 940,
                        top: row * 26,
                        right: 1260,
                        bottom: row * 26 + 24,
                    },
                    fonts.small,
                    theme::INK,
                    DT_SINGLELINE,
                );
            }
            canvas.unclip();
        };
        for hardware in [false, true] {
            let mut renderer = Renderer::default();
            if hardware {
                match Gpu::new(hwnd, rc) {
                    Ok(gpu) => renderer.gpu = Some(gpu),
                    Err(error) => {
                        eprintln!("Hardware unavailable: {error}; GDI verified");
                        continue;
                    }
                }
            } else {
                renderer.retry = Some(Instant::now());
            }
            renderer.paint(hwnd, dc, &rc, scene);
            assert_eq!(renderer.gpu.is_some(), hardware);
            assert_ne!(GetPixel(dc, 100, 100), theme::CANVAS);
            let start = Instant::now();
            for _ in 0..60 {
                renderer.paint(hwnd, dc, &rc, scene);
            }
            eprintln!(
                "{}: {:.2} ms/frame",
                if hardware { "GPU" } else { "GDI" },
                start.elapsed().as_secs_f64() * 1000. / 60.
            );
            if hardware {
                assert_eq!(renderer.gpu.as_ref().unwrap().textures.len(), 1);
                assert!(renderer.fallback.dc.is_null());
                renderer.forget_bitmap(1);
                assert!(renderer.gpu.as_ref().unwrap().textures.is_empty());
                MoveWindow(hwnd, 0, 0, 900, 600, 0);
                renderer.paint(hwnd, dc, &rc, scene);
                assert_eq!(
                    renderer.gpu.as_ref().unwrap().target.GetPixelSize().width,
                    900
                );
                renderer.paint(hwnd, dc, &rc, |canvas| {
                    scene(canvas);
                    for key in [2, 3] {
                        canvas.bitmap(
                            key,
                            &pixels,
                            1600,
                            1000,
                            false,
                            RECT {
                                left: 20,
                                top: 20,
                                right: 300,
                                bottom: 300,
                            },
                        );
                    }
                });
                assert!(renderer.over_budget && renderer.gpu.is_none());
                renderer.retry = None;
                renderer.paint(hwnd, dc, &rc, scene);
                assert!(
                    renderer.gpu.is_none(),
                    "Do not thrash over-budget textures on every paint"
                );
                renderer.release();
                renderer.paint(hwnd, dc, &rc, scene);
                assert!(renderer.gpu.is_some());
                // Exercise a failed GPU frame: repaint the same scene in GDI immediately.
                let mut attempts = 0;
                renderer.paint(hwnd, dc, &rc, |canvas| {
                    attempts += 1;
                    scene(canvas);
                    if let Some(gpu) = &mut canvas.gpu {
                        gpu.failed = true;
                    }
                });
                assert_eq!(attempts, 2);
                assert!(renderer.gpu.is_none());
                assert_ne!(GetPixel(dc, 100, 100), theme::CANVAS);
            }
            renderer.release();
            renderer.retry = Some(Instant::now());
            renderer.paint(hwnd, dc, &rc, scene);
            assert!(renderer.gpu.is_none());
            assert!(!renderer.fallback.dc.is_null());
        }
        ReleaseDC(hwnd, dc);
        DestroyWindow(hwnd);
    }
}
