//! PDFium is loaded on demand for search and selection. Rendering remains in the existing Windows worker.
use std::{
    ffi::c_void,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    ptr::null,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use windows_sys::Win32::{Foundation::*, System::LibraryLoader::*};
type Handle = *mut c_void;
#[derive(Clone, Debug)]
pub struct Match {
    pub page: usize,
    /// Normalized displayed-page coordinates, including crop and rotation.
    pub boxes: Vec<[f32; 4]>,
}
#[repr(C)]
struct Access {
    length: u32,
    read: unsafe extern "C" fn(Handle, u32, *mut u8, u32) -> i32,
    file: Handle,
}
unsafe extern "C" fn read(file: Handle, offset: u32, buffer: *mut u8, size: u32) -> i32 {
    let file = &mut *file.cast::<File>();
    i32::from(
        file.seek(SeekFrom::Start(offset as u64))
            .and_then(|_| file.read_exact(std::slice::from_raw_parts_mut(buffer, size as usize)))
            .is_ok(),
    )
}
// ponytail: PDFium's global state is not thread-safe; serialize text extraction, never the UI.
static LOCK: Mutex<()> = Mutex::new(());
macro_rules! api {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)*) => {
        #[allow(non_snake_case)]
        struct Api { library: HMODULE, $( $name: unsafe extern "system" fn($($arg),*) -> $ret, )* }
        impl Api {
            unsafe fn load() -> Result<Self, String> {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let mut folder = exe.parent().ok_or("Missing application directory")?;
                if folder.file_name().is_some_and(|n| n == "deps") { folder = folder.parent().ok_or("Missing runtime directory")?; }
                let path = crate::ui::path_wide(&folder.join("runtime/pdfium/pdfium.dll"));
                let library = LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
                if library.is_null() { return Err("PDF search runtime missing · Keep the runtime folder beside PlumeTxt.exe".into()); }
                let loaded = (|| Ok(Self { library, $( $name: std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn($($arg),*) -> $ret>(GetProcAddress(library, concat!(stringify!($name), "\0").as_ptr()).ok_or("Incompatible PDF search runtime")?), )* }))();
                if loaded.is_err() { FreeLibrary(library); }
                loaded
            }
        }
    }
}
api! {
    FPDF_InitLibrary() -> ();
    FPDF_DestroyLibrary() -> ();
    FPDF_LoadCustomDocument(*mut Access, *const u8) -> Handle;
    FPDF_CloseDocument(Handle) -> ();
    FPDF_GetPageCount(Handle) -> i32;
    FPDF_LoadPage(Handle, i32) -> Handle;
    FPDF_ClosePage(Handle) -> ();
    FPDFText_LoadPage(Handle) -> Handle;
    FPDFText_ClosePage(Handle) -> ();
    FPDFText_CountChars(Handle) -> i32;
    FPDFText_GetText(Handle, i32, i32, *mut u16) -> i32;
    FPDFText_GetCharIndexAtPos(Handle, f64, f64, f64, f64) -> i32;
    FPDF_DeviceToPage(Handle, i32, i32, i32, i32, i32, i32, i32, *mut f64, *mut f64) -> i32;
    FPDFText_FindStart(Handle, *const u16, u32, i32) -> Handle;
    FPDFText_FindNext(Handle) -> i32;
    FPDFText_GetSchResultIndex(Handle) -> i32;
    FPDFText_GetSchCount(Handle) -> i32;
    FPDFText_FindClose(Handle) -> ();
    FPDFText_CountRects(Handle, i32, i32) -> i32;
    FPDFText_GetRect(Handle, i32, *mut f64, *mut f64, *mut f64, *mut f64) -> i32;
    FPDF_PageToDevice(Handle, i32, i32, i32, i32, i32, f64, f64, *mut i32, *mut i32) -> i32;
}
impl Drop for Api {
    fn drop(&mut self) {
        unsafe {
            (self.FPDF_DestroyLibrary)();
            FreeLibrary(self.library);
        }
    }
}
struct Owned(Handle, unsafe extern "system" fn(Handle));
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            (self.1)(self.0);
        }
    }
}

pub fn search(
    path: &Path,
    query: &str,
    cancel: &AtomicBool,
    emit: &mut impl FnMut(Match, String, String) -> bool,
) -> Result<bool, String> {
    let _lock = LOCK.lock().map_err(|_| "PDF search stopped")?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(true);
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let length = u32::try_from(file.metadata().map_err(|e| e.to_string())?.len())
        .map_err(|_| "PDF search supports files smaller than 4 GiB")?;
    unsafe {
        let api = Api::load()?;
        (api.FPDF_InitLibrary)();
        let mut access = Access {
            length,
            read,
            file: (&mut file as *mut File).cast(),
        };
        let doc = (api.FPDF_LoadCustomDocument)(&mut access, null());
        if doc.is_null() {
            return Err("Cannot search this PDF · It may be encrypted or damaged".into());
        }
        let doc = Owned(doc, api.FPDF_CloseDocument);
        let query = crate::theme::wide(query);
        let mut has_text = false;
        for index in 0..(api.FPDF_GetPageCount)(doc.0) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let page = (api.FPDF_LoadPage)(doc.0, index);
            if page.is_null() {
                return Err(format!("Cannot read PDF page {}", index + 1));
            }
            let page = Owned(page, api.FPDF_ClosePage);
            let text = (api.FPDFText_LoadPage)(page.0);
            if text.is_null() {
                return Err(format!("Cannot read text on page {}", index + 1));
            }
            let text = Owned(text, api.FPDFText_ClosePage);
            let count = (api.FPDFText_CountChars)(text.0).max(0);
            has_text |= count > 0;
            let find = (api.FPDFText_FindStart)(text.0, query.as_ptr(), 0, 0);
            if find.is_null() {
                continue;
            }
            let find = Owned(find, api.FPDFText_FindClose);
            while !cancel.load(Ordering::Relaxed) && (api.FPDFText_FindNext)(find.0) != 0 {
                let start = (api.FPDFText_GetSchResultIndex)(find.0);
                let len = (api.FPDFText_GetSchCount)(find.0);
                if start < 0 || start >= count || len <= 0 || len > count - start {
                    break;
                }
                let matched = api.extract(text.0, start, len.min(2048));
                let snippet = api
                    .extract(
                        text.0,
                        (start - 24).max(0),
                        (count - (start - 24).max(0)).min(180),
                    )
                    .replace(['\r', '\n', '\t'], " ");
                let boxes = api.boxes(page.0, text.0, start, len);
                if !emit(
                    Match {
                        page: index as usize,
                        boxes,
                    },
                    matched,
                    snippet,
                ) {
                    return Ok(has_text);
                }
            }
        }
        Ok(has_text)
    }
}

impl Api {
    unsafe fn extract(&self, text: Handle, start: i32, count: i32) -> String {
        let mut units = vec![0u16; count as usize * 2 + 1];
        let n = (self.FPDFText_GetText)(text, start, count, units.as_mut_ptr()).max(1) as usize;
        String::from_utf16_lossy(&units[..n.saturating_sub(1).min(units.len())])
    }
    unsafe fn boxes(&self, page: Handle, text: Handle, start: i32, len: i32) -> Vec<[f32; 4]> {
        let mut boxes = Vec::new();
        for i in 0..(self.FPDFText_CountRects)(text, start, len).clamp(0, 20000) {
            let (mut l, mut t, mut r, mut b) = (0., 0., 0., 0.);
            if (self.FPDFText_GetRect)(text, i, &mut l, &mut t, &mut r, &mut b) == 0 {
                continue;
            }
            let (mut x1, mut y1, mut x2, mut y2) = (0, 0, 0, 0);
            if (self.FPDF_PageToDevice)(page, 0, 0, 1_000_000, 1_000_000, 0, l, t, &mut x1, &mut y1)
                == 0
                || (self.FPDF_PageToDevice)(
                    page, 0, 0, 1_000_000, 1_000_000, 0, r, b, &mut x2, &mut y2,
                ) == 0
            {
                continue;
            }
            boxes.push([
                x1.min(x2) as f32 / 1e6,
                y1.min(y2) as f32 / 1e6,
                x1.max(x2) as f32 / 1e6,
                y1.max(y2) as f32 / 1e6,
            ]);
        }

        boxes
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub page: usize,
    pub x: f32,
    pub y: f32,
}
pub struct Selection {
    pub text: String,
    pub pages: Vec<Match>,
}

pub fn select(
    path: &Path,
    mut range: [Point; 2],
    cancel: &AtomicBool,
) -> Result<Selection, String> {
    let _lock = LOCK.lock().map_err(|_| "PDF text extraction stopped")?;
    let mut out = Selection {
        text: String::new(),
        pages: Vec::new(),
    };
    if cancel.load(Ordering::Relaxed) || range[0] == range[1] {
        return Ok(out);
    }
    if range.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return Err("Invalid PDF selection".into());
    }
    if range[0].page > range[1].page {
        range.swap(0, 1);
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let length = u32::try_from(file.metadata().map_err(|e| e.to_string())?.len())
        .map_err(|_| "PDF text extraction supports files smaller than 4 GiB")?;
    unsafe {
        let api = Api::load()?;
        (api.FPDF_InitLibrary)();
        let mut access = Access {
            length,
            read,
            file: (&mut file as *mut File).cast(),
        };
        let doc = (api.FPDF_LoadCustomDocument)(&mut access, null());
        if doc.is_null() {
            return Err("Cannot read text in this PDF".into());
        }
        let doc = Owned(doc, api.FPDF_CloseDocument);
        if range[1].page >= (api.FPDF_GetPageCount)(doc.0).max(0) as usize {
            return Err("PDF page changed; select again".into());
        }
        let mut chars = 0i64;
        let mut rectangles = 0i64;
        for index in range[0].page..=range[1].page {
            if cancel.load(Ordering::Relaxed) {
                return Ok(out);
            }
            let page = (api.FPDF_LoadPage)(doc.0, index as i32);
            if page.is_null() {
                return Err("Cannot read PDF page".into());
            }
            let page = Owned(page, api.FPDF_ClosePage);
            let text = (api.FPDFText_LoadPage)(page.0);
            if text.is_null() {
                return Err("Cannot read PDF text".into());
            }
            let text = Owned(text, api.FPDFText_ClosePage);
            let count = (api.FPDFText_CountChars)(text.0);
            if count < 0 {
                return Err("Cannot read PDF characters".into());
            }
            if count == 0 {
                continue;
            }
            let hit = |point: Point| -> Result<i32, String> {
                let (mut x, mut y) = (0., 0.);
                if (api.FPDF_DeviceToPage)(
                    page.0,
                    0,
                    0,
                    1_000_000,
                    1_000_000,
                    0,
                    (point.x.clamp(0., 1.) * 1e6) as i32,
                    (point.y.clamp(0., 1.) * 1e6) as i32,
                    &mut x,
                    &mut y,
                ) == 0
                {
                    return Err("Cannot map PDF selection".into());
                }
                let i = (api.FPDFText_GetCharIndexAtPos)(text.0, x, y, 1e9, 1e9);
                if i < 0 || i >= count {
                    Err("No selectable text here".into())
                } else {
                    Ok(i)
                }
            };
            let mut start = if index == range[0].page {
                hit(range[0])?
            } else {
                0
            };
            let mut end = if index == range[1].page {
                hit(range[1])?
            } else {
                count - 1
            };
            if start > end {
                std::mem::swap(&mut start, &mut end);
            }
            chars += i64::from(end - start + 1);
            rectangles +=
                i64::from((api.FPDFText_CountRects)(text.0, start, end - start + 1).max(0));
            if chars > 1_000_000 || rectangles > 20000 {
                return Err("Selection too large; copy a smaller range".into());
            }
            if !out.text.is_empty() && !out.text.ends_with('\n') {
                out.text.push_str("\r\n");
            }
            out.text
                .push_str(&api.extract(text.0, start, end - start + 1));
            out.pages.push(Match {
                page: index,
                boxes: api.boxes(page.0, text.0, start, end - start + 1),
            });
        }
    }
    if out.text.is_empty() {
        return Err("No text layer in this selection · Scanned pages need OCR".into());
    }
    Ok(out)
}

#[test]
fn pdf_search_finds_compressed_text_and_rotated_cropped_boxes() {
    use lopdf::{dictionary, Document, Object, Stream};
    let mut doc = Document::with_version("1.7");
    let pages = doc.new_object_id();
    let font = doc.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
    let mut kids = Vec::new();
    for rotation in [0, 90] {
        let mut content = Stream::new(
            dictionary! {},
            b"BT /F1 20 Tf 100 300 Td [(Need) 0 (le) 0 ( text)] TJ ET".to_vec(),
        );
        content.compress().unwrap();
        let content = doc.add_object(content);
        kids.push(Object::Reference(doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(),0.into(),600.into(),800.into()],
            "CropBox" => vec![50.into(),100.into(),550.into(),700.into()], "Rotate" => rotation,
            "Resources" => resources, "Contents" => content,
        })));
    }
    kids.push(Object::Reference(doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages,
        "MediaBox" => vec![0.into(),0.into(),600.into(),800.into()],
    })));
    doc.set_object(
        pages,
        dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 3 },
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    let root = std::env::current_dir().unwrap().join("tmp/pdf-search-test");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("搜索.pdf");
    doc.save(&path).unwrap();
    let mut hits = Vec::new();
    assert!(search(
        &path,
        "needle",
        &AtomicBool::new(false),
        &mut |hit, matched, snippet| {
            assert_eq!(matched, "Needle");
            assert!(snippet.contains("Needle"));
            hits.push(hit);
            true
        }
    )
    .unwrap());
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].page, 0);
    assert_eq!(hits[1].page, 1);
    let a = hits[0].boxes[0];
    let b = hits[1].boxes[0];
    assert!(
        a[0] > 0.09 && a[0] < 0.12 && a[1] > 0.63 && a[3] < 0.68,
        "{a:?}"
    );
    assert!(
        (b[0] - (1. - a[3])).abs() < 0.002 && (b[1] - a[0]).abs() < 0.002,
        "{a:?} {b:?}"
    );
    let range = [
        Point {
            page: 0,
            x: a[0] + 0.001,
            y: (a[1] + a[3]) / 2.,
        },
        Point {
            page: 0,
            x: a[2] - 0.001,
            y: (a[1] + a[3]) / 2.,
        },
    ];
    assert!(select(
        &path,
        [
            Point {
                page: 2,
                ..range[0]
            },
            Point {
                page: 2,
                ..range[1]
            }
        ],
        &AtomicBool::new(false)
    )
    .err()
    .unwrap()
    .contains("No text layer"));
    let selected = select(&path, range, &AtomicBool::new(false)).unwrap();
    assert_eq!(selected.text, "Needle");
    assert_eq!(
        select(&path, [range[1], range[0]], &AtomicBool::new(false))
            .unwrap()
            .text,
        "Needle"
    );
    let rotated = [
        Point {
            page: 1,
            x: (b[0] + b[2]) / 2.,
            y: b[1] + 0.001,
        },
        Point {
            page: 1,
            x: (b[0] + b[2]) / 2.,
            y: b[3] - 0.001,
        },
    ];
    assert_eq!(
        select(&path, rotated, &AtomicBool::new(false))
            .unwrap()
            .text,
        "Needle"
    );
    let across = select(&path, [range[0], rotated[1]], &AtomicBool::new(false)).unwrap();
    assert_eq!(across.text, "Needle text\r\nNeedle");
    assert_eq!(across.pages.len(), 2);
    assert!(select(&path, range, &AtomicBool::new(true))
        .unwrap()
        .text
        .is_empty());
    assert!(select(
        &path,
        [
            range[0],
            Point {
                page: 100,
                ..range[1]
            }
        ],
        &AtomicBool::new(false)
    )
    .is_err());
    assert!(search(
        &path,
        "absent",
        &AtomicBool::new(false),
        &mut |_, _, _| panic!("Unexpected match")
    )
    .unwrap());
    assert!(search(
        &path,
        "needle",
        &AtomicBool::new(true),
        &mut |_, _, _| panic!("Cancelled search emitted")
    )
    .unwrap());
}
