use crate::theme::wide;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{
        Gdi::*,
        GdiPlus::{
            GdipCreateBitmapFromHBITMAP, GdipDisposeImage, GdipSaveImageToFile, GdiplusShutdown,
            GdiplusStartup, GdiplusStartupInput,
        },
    },
    System::{DataExchange::*, Memory::*},
    UI::{Shell::DragQueryFileW, WindowsAndMessaging::*},
};
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const PNG: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x557cf406,
    data2: 0x1a04,
    data3: 0x11d3,
    data4: [0x9a, 0x73, 0x00, 0x00, 0xf8, 0x1e, 0xf3, 0x2e],
};
struct Imaging(usize);
impl Imaging {
    unsafe fn new() -> Result<Self, String> {
        let mut token = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            SuppressExternalCodecs: 1,
            ..zeroed()
        };
        if GdiplusStartup(&mut token, &input, null_mut()) != 0 {
            return Err("Could not initialize image support".into());
        }
        Ok(Self(token))
    }
}
impl Drop for Imaging {
    fn drop(&mut self) {
        unsafe {
            GdiplusShutdown(self.0);
        }
    }
}
pub struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}
pub enum Paste {
    Bitmap(Bitmap),
    Png(Vec<u8>),
    File(PathBuf),
}
pub const EXTENSIONS: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "gif", "bmp", "dib", "tif", "tiff", "ico", "jxr",
    "wdp", "hdp", "webp", "heic", "heif", "avif",
];
pub fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| EXTENSIONS.contains(&s.to_ascii_lowercase().as_str()))
}
pub fn dialog_filter() -> String {
    format!(
        "Images\0{}\0\0",
        EXTENSIONS
            .iter()
            .map(|ext| format!("*.{ext}"))
            .collect::<Vec<_>>()
            .join(";")
    )
}
pub unsafe fn available() -> bool {
    IsClipboardFormatAvailable(2) != 0
        || IsClipboardFormatAvailable(15) != 0
        || IsClipboardFormatAvailable(RegisterClipboardFormatW(wide("PNG").as_ptr())) != 0
}
pub unsafe fn clipboard(hwnd: HWND) -> Result<Option<Paste>, String> {
    if OpenClipboard(hwnd) == 0 {
        return Err("Clipboard is busy. Try pasting again.".into());
    }
    let result = (|| {
        let png = GetClipboardData(RegisterClipboardFormatW(wide("PNG").as_ptr()));
        if !png.is_null() {
            let size = GlobalSize(png);
            if size > MAX_BYTES as usize {
                return Err("Image exceeds 16 MiB".into());
            }
            let ptr = GlobalLock(png) as *const u8;
            if !ptr.is_null() {
                let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
                GlobalUnlock(png);
                if png_size(&bytes).is_some() {
                    return Ok(Some(Paste::Png(bytes)));
                }
            }
        }
        let files = GetClipboardData(15);
        if !files.is_null() && DragQueryFileW(files, 0xffff_ffff, null_mut(), 0) == 1 {
            let len = DragQueryFileW(files, 0, null_mut(), 0);
            let mut name = vec![0u16; len as usize + 1];
            DragQueryFileW(files, 0, name.as_mut_ptr(), name.len() as u32);
            use std::os::windows::ffi::OsStringExt;
            let path = PathBuf::from(std::ffi::OsString::from_wide(&name[..len as usize]));
            if supported(&path) {
                return Ok(Some(Paste::File(path)));
            }
        }
        let bitmap = GetClipboardData(2);
        if !bitmap.is_null() {
            let mut info: BITMAP = zeroed();
            if GetObjectW(bitmap, size_of::<BITMAP>() as i32, &mut info as *mut _ as _) == 0
                || !dimensions(info.bmWidth as u32, info.bmHeight.unsigned_abs())
            {
                return Err("Image dimensions exceed the 16 megapixel limit".into());
            }
            let copy = CopyImage(bitmap, IMAGE_BITMAP, 0, 0, LR_CREATEDIBSECTION);
            if copy.is_null() {
                return Err("Could not copy the image".into());
            }
            return Ok(Some(Paste::Bitmap(Bitmap(copy))));
        }
        Ok(None)
    })();
    CloseClipboard();
    result
}
fn dimensions(w: u32, h: u32) -> bool {
    w > 0 && h > 0 && w as u64 * h as u64 <= MAX_PIXELS
}
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    dimensions(w, h).then_some((w, h))
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Image exceeds 16 MiB".into());
    }
    Ok(bytes)
}
fn url(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}
pub fn insert(document: &Path, paste: Paste) -> Result<String, String> {
    let document = std::path::absolute(document).map_err(|e| e.to_string())?;
    let parent = document.parent().ok_or("Save the Markdown file first")?;
    let dir_name = format!(
        "{}.assets",
        document.file_stem().unwrap_or_default().to_string_lossy()
    );
    let dir = parent.join(&dir_name);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Validate before creating an attachment; preserve the original format and frames.
    let original = if let Paste::File(path) = &paste {
        if !supported(path) {
            return Err("Unsupported image format".into());
        }
        let bytes = read(path)?;
        decode(&bytes, MAX_PIXELS)?;
        Some(bytes)
    } else {
        None
    };
    let ext = match &paste {
        Paste::File(path) => path
            .extension()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase(),
        _ => "png".into(),
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let filename = format!("image-{stamp}.{ext}");
    let target = dir.join(&filename);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        match paste {
            Paste::Png(bytes) => {
                if png_size(&bytes).is_none() {
                    return Err("Invalid or oversized PNG".into());
                }
                file.write_all(&bytes).map_err(|e| e.to_string())?;
            }
            Paste::File(_) => {
                file.write_all(original.as_ref().unwrap())
                    .map_err(|e| e.to_string())?;
            }
            Paste::Bitmap(bitmap) => unsafe {
                drop(file);
                let _imaging = Imaging::new()?;
                let mut image = null_mut();
                if GdipCreateBitmapFromHBITMAP(bitmap.0, null_mut(), &mut image) != 0 {
                    return Err("Could not read clipboard image".into());
                }
                let saved = GdipSaveImageToFile(
                    image as _,
                    wide(&target.to_string_lossy()).as_ptr(),
                    &PNG,
                    null(),
                );
                GdipDisposeImage(image as _);
                if saved != 0 {
                    return Err("Could not save clipboard image".into());
                }
            },
        }
        Ok(format!(
            "![Image]({})",
            url(&format!("{dir_name}/{filename}"))
        ))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&target);
    }
    result
}

// Only local relative links are read; no URL or network share is fetched.
pub fn picture(
    base: &Path,
    link: &str,
    width: usize,
    budget: &mut (usize, u64),
    dark: bool,
) -> Option<String> {
    let mut decoded = Vec::new();
    let b = link.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            decoded.push(u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16).ok()?);
            i += 3;
        } else {
            decoded.push(b[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    if decoded.contains([':', '\\', '\0']) || decoded.starts_with('/') {
        return None;
    }
    let path = base.join(&decoded);
    if !supported(&path) {
        return None;
    }
    let bytes = read(&path).ok()?;
    if bytes.len() > budget.0 {
        return None;
    }
    let image = decode(&bytes, budget.1).ok()?;
    let (w, h) = (image.original_width, image.original_height);
    let pixels = w as u64 * h as u64;
    let goal = (w as usize * 15).min(width.max(1));
    let height = goal * h as usize / w as usize;
    let (dw, dh) = (image.width, image.height);
    let dib = image.dib(dark);
    if dib.len() > budget.0 {
        return None;
    }
    use std::fmt::Write;
    let mut out =
        format!("{{\\pict\\dibitmap0\\picw{dw}\\pich{dh}\\picwgoal{goal}\\pichgoal{height} ");
    for byte in &dib {
        let _ = write!(out, "{byte:02x}");
    }
    out.push('}');
    budget.0 -= bytes.len().max(dib.len());
    budget.1 -= pixels;
    Some(out)
}

#[test]
fn image_paths_and_bounds() {
    assert_eq!(
        url("中文 notes.assets/a (1).png"),
        "%E4%B8%AD%E6%96%87%20notes.assets/a%20%281%29.png"
    );
    assert!(dimensions(1920, 1080));
    assert!(!dimensions(u32::MAX, 100));
    assert!(png_size(b"not an image").is_none());
    assert!(picture(
        Path::new("."),
        "https://example.com/a.png",
        9000,
        &mut (100, 100),
        false
    )
    .is_none());
}

#[test]
#[ignore = "Requires Windows GDI+, RichEdit and Print to PDF"]
fn native_image_paste_and_render() {
    let _ole = Ole::new().unwrap();
    use windows::{core::Interface, Win32::UI::Controls::RichEdit::IRichEditOle};
    use windows_sys::Win32::System::LibraryLoader::*;
    unsafe {
        let dir = std::env::current_dir()
            .unwrap()
            .join("tmp/image-regression");
        fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("notes 中文.md");
        let bits = vec![0xff40d0e0u32; 64 * 32];
        let bitmap = CreateBitmap(64, 32, 1, 32, bits.as_ptr() as _);
        assert!(!bitmap.is_null());
        let link = insert(&doc, Paste::Bitmap(Bitmap(bitmap))).unwrap();
        let link2 = insert(
            &doc,
            Paste::File(
                fs::read_dir(dir.join("notes 中文.assets"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path(),
            ),
        )
        .unwrap();
        assert_ne!(
            link, link2,
            "Images must never overwrite earlier attachments"
        );
        let rtf = crate::markdown::with_images(&link, 9000, true, Some(&dir));
        assert!(rtf.contains("\\dibitmap0"));
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let edit = CreateWindowExW(
            0,
            wide("RICHEDIT50W").as_ptr(),
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
        crate::ui::set_rtf(edit, &rtf).unwrap();
        let ole: IRichEditOle = crate::syntax::document(edit).unwrap().cast().unwrap();
        assert_eq!(
            ole.GetObjectCount(),
            1,
            "Preview must contain a real image object"
        );
        drop(ole);
        DestroyWindow(edit);
        let pdf = dir.join("image-export.pdf");
        crate::ui::export_complete(&link, &pdf, Some(&dir)).unwrap();
        let pdf = lopdf::Document::load(pdf).unwrap();
        assert!(
            pdf.objects
                .values()
                .any(|o| o.as_stream().ok().is_some_and(|s| s
                    .dict
                    .get(b"Subtype")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    == Some(b"Image"))),
            "Export must retain the embedded image"
        );
        FreeLibrary(library);
        fs::write(
            dir.join("preview.md"),
            format!(
                "# Image insertion\n\n{link}\n\nA local image, stored beside this Markdown file.\n"
            ),
        )
        .unwrap();
    }
}

// Keep OLE alive for the lifetime of RichEdit image objects on this thread.
pub struct Ole;
impl Ole {
    pub fn new() -> Result<Self, String> {
        if unsafe { windows_sys::Win32::System::Ole::OleInitialize(std::ptr::null()) } < 0 {
            return Err("Could not initialize image embedding".into());
        }
        Ok(Self)
    }
}
impl Drop for Ole {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Ole::OleUninitialize();
        }
    }
}

// RichEdit requires a storage callback even for static RTF pictures.
mod embedding {
    use windows::{
        core::*,
        Win32::{
            Foundation::{E_NOTIMPL, HGLOBAL},
            System::{
                Com::{
                    IDataObject, StructuredStorage::*, STGM_CREATE, STGM_READWRITE,
                    STGM_SHARE_EXCLUSIVE,
                },
                Ole::*,
                SystemServices::*,
            },
            UI::{Controls::RichEdit::*, WindowsAndMessaging::HMENU},
        },
    };
    #[implement(IRichEditOleCallback)]
    pub struct Pictures;
    impl IRichEditOleCallback_Impl for Pictures_Impl {
        fn GetNewStorage(&self) -> Result<IStorage> {
            unsafe {
                let memory = CreateILockBytesOnHGlobal(None, true)?;
                StgCreateDocfileOnILockBytes(
                    &memory,
                    STGM_CREATE | STGM_READWRITE | STGM_SHARE_EXCLUSIVE,
                    0,
                )
            }
        }
        fn GetInPlaceContext(
            &self,
            _: OutRef<IOleInPlaceFrame>,
            _: OutRef<IOleInPlaceUIWindow>,
            _: *mut OLEINPLACEFRAMEINFO,
        ) -> Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn ShowContainerUI(&self, _: BOOL) -> Result<()> {
            Ok(())
        }
        fn QueryInsertObject(&self, _: *mut GUID, _: Ref<IStorage>, _: i32) -> Result<()> {
            Ok(())
        }
        fn DeleteObject(&self, _: Ref<IOleObject>) -> Result<()> {
            Ok(())
        }
        fn QueryAcceptData(
            &self,
            _: Ref<IDataObject>,
            _: *mut u16,
            _: RECO_FLAGS,
            _: BOOL,
            _: HGLOBAL,
        ) -> Result<()> {
            Ok(())
        }
        fn ContextSensitiveHelp(&self, _: BOOL) -> Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn GetClipboardData(
            &self,
            _: *mut CHARRANGE,
            _: u32,
            _: OutRef<IDataObject>,
        ) -> Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn GetDragDropEffect(
            &self,
            _: BOOL,
            _: MODIFIERKEYS_FLAGS,
            _: *mut DROPEFFECT,
        ) -> Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn GetContextMenu(
            &self,
            _: RICH_EDIT_GET_CONTEXT_MENU_SEL_TYPE,
            _: Ref<IOleObject>,
            _: *mut CHARRANGE,
            _: *mut HMENU,
        ) -> Result<()> {
            Err(E_NOTIMPL.into())
        }
    }
}
pub unsafe fn enable_images(hwnd: HWND) {
    use windows::{core::Interface, Win32::UI::Controls::RichEdit::IRichEditOleCallback};
    let callback: IRichEditOleCallback = embedding::Pictures.into();
    SendMessageW(hwnd, WM_USER + 70, 0, callback.as_raw() as isize);
}

struct Decoded {
    original_width: u32,
    original_height: u32,
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}
impl Decoded {
    fn dib(&self, dark: bool) -> Vec<u8> {
        let stride = (self.width as usize * 3 + 3) & !3;
        let mut out = vec![0u8; 40 + stride * self.height as usize];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&self.width.to_le_bytes());
        out[8..12].copy_from_slice(&self.height.to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&24u16.to_le_bytes());
        let background = if dark { [15u32, 13, 9] } else { [255; 3] };
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let source = (y * self.width as usize + x) * 4;
                let target = 40 + (self.height as usize - 1 - y) * stride + x * 3;
                let alpha = self.bgra[source + 3] as u32;
                for c in 0..3 {
                    out[target + c] = ((self.bgra[source + c] as u32 * alpha
                        + background[c] * (255 - alpha)
                        + 127)
                        / 255) as u8;
                }
            }
        }
        out
    }
}
fn decode(bytes: &[u8], pixel_budget: u64) -> Result<Decoded, String> {
    decode_scaled(bytes, pixel_budget, 1600)
}
pub fn load_picture(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let image = decode_scaled(&read(path)?, MAX_PIXELS, u32::MAX)?;
    Ok((image.dib(true), image.width, image.height))
}
fn decode_scaled(bytes: &[u8], pixel_budget: u64, edge: u32) -> Result<Decoded, String> {
    use windows::Win32::{
        Graphics::Imaging::*,
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
    };
    let result = (|| -> windows::core::Result<Decoded> {
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let stream = factory.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let decoder =
                factory.CreateDecoderFromStream(&stream, null(), WICDecodeMetadataCacheOnDemand)?;
            // ponytail: static preview uses the first frame; retain originals for future animation support.
            let frame = decoder.GetFrame(0)?;
            use windows::{
                core::{Interface, PCWSTR},
                Win32::System::Com::StructuredStorage::{
                    PropVariantClear, PropVariantToUInt32, PROPVARIANT,
                },
            };
            let mut orientation = 1;
            if let Ok(metadata) = frame.GetMetadataQueryReader() {
                for query in ["/app1/ifd/{ushort=274}", "/ifd/{ushort=274}"] {
                    let mut value: PROPVARIANT = std::mem::zeroed();
                    if metadata
                        .GetMetadataByName(PCWSTR(wide(query).as_ptr()), &mut value)
                        .is_ok()
                    {
                        orientation = PropVariantToUInt32(&value).unwrap_or(1);
                        let _ = PropVariantClear(&mut value);
                        break;
                    }
                    let _ = PropVariantClear(&mut value);
                }
            }
            let transform = match orientation {
                2 => WICBitmapTransformFlipHorizontal,
                3 => WICBitmapTransformRotate180,
                4 => WICBitmapTransformFlipVertical,
                5 => WICBitmapTransformOptions(
                    WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0,
                ),
                6 => WICBitmapTransformRotate90,
                7 => WICBitmapTransformOptions(
                    WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0,
                ),
                8 => WICBitmapTransformRotate270,
                _ => WICBitmapTransformRotate0,
            };
            let source: IWICBitmapSource = if orientation > 1 && orientation <= 8 {
                let rotated = factory.CreateBitmapFlipRotator()?;
                rotated.Initialize(&frame, transform)?;
                rotated.cast()?
            } else {
                frame.cast()?
            };
            let (mut w, mut h) = (0, 0);
            source.GetSize(&mut w, &mut h)?;
            if !dimensions(w, h) || w as u64 * h as u64 > pixel_budget {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_OUTOFMEMORY,
                    "Image exceeds the 16 megapixel or document image limit",
                ));
            }
            let scale = (edge as f64 / w.max(h) as f64).min(1.0);
            let dw = (w as f64 * scale).round().max(1.0) as u32;
            let dh = (h as f64 * scale).round().max(1.0) as u32;
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(&source, dw, dh, WICBitmapInterpolationModeFant)?;
            let source = WICConvertBitmapSource(&GUID_WICPixelFormat32bppBGRA, &scaler)?;
            let mut bgra = vec![0u8; dw as usize * dh as usize * 4];
            source.CopyPixels(null(), dw * 4, &mut bgra)?;
            Ok(Decoded {
                original_width: w,
                original_height: h,
                width: dw,
                height: dh,
                bgra,
            })
        }
    })();
    result.map_err(|e|format!("Could not decode image. It may be damaged, exceed the 16 megapixel limit, or require a Windows image codec (WebP / HEIF / AV1). {e}"))
}

#[test]
#[ignore = "Requires Windows image codecs"]
fn native_image_formats_and_validation() {
    let _ole = Ole::new().unwrap();
    let dir = Path::new("tests/fixtures/images");
    for ext in ["png", "jpg", "gif", "bmp", "tiff", "ico", "webp", "avif"] {
        let path = dir.join(format!("sample.{ext}"));
        let bytes = fs::read(&path).unwrap();
        let decoded = decode(&bytes, MAX_PIXELS);
        if matches!(ext, "webp" | "avif") && decoded.is_err() {
            println!(
                "{ext}: Windows codec unavailable: {}",
                decoded.err().unwrap()
            );
            continue;
        }
        let image = decoded.unwrap_or_else(|e| panic!("{ext}: {e}"));
        assert!(image.width > 0 && image.height > 0);
        let dark = image.dib(true);
        let light = image.dib(false);
        assert_eq!(dark.len(), light.len());
        if ext == "png" {
            assert_ne!(dark, light, "Transparency must composite onto each theme");
        }
        let mut budget = (16 * 1024 * 1024, MAX_PIXELS);
        assert!(
            picture(dir, &format!("sample.{ext}"), 9000, &mut budget, false).is_some(),
            "{ext} direct Markdown link"
        );
        let original = insert(Path::new("tmp/formats-test.md"), Paste::File(path)).unwrap();
        let relative = original
            .strip_prefix("![Image](")
            .unwrap()
            .strip_suffix(')')
            .unwrap();
        assert_eq!(
            fs::read(Path::new("tmp").join(relative)).unwrap(),
            bytes,
            "{ext}: preserve original format"
        );
        println!("{ext}: import and direct preview passed");
    }
    let image = decode(&fs::read(dir.join("rotated.jpg")).unwrap(), MAX_PIXELS).unwrap();
    assert_eq!((image.width, image.height), (20, 32), "EXIF orientation");
    assert!(decode(b"not an image", MAX_PIXELS).is_err());
    assert!(decode(&fs::read(dir.join("sample.png")).unwrap(), 10).is_err());
    assert!(supported(Path::new("photo.TIFF")));
    assert!(!supported(Path::new("file.pdf")));
    let mut budget = (1, 1);
    assert!(picture(dir, "sample.png", 9000, &mut budget, false).is_none());
    assert_eq!(budget, (1, 1));
}
