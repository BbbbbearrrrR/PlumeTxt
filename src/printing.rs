use crate::theme::wide;
use std::{
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
};
use windows_sys::Win32::{
    Foundation::*, Graphics::Gdi::*, Storage::Xps::*, UI::Controls::Dialogs::*,
};

pub struct Job {
    pub result: Receiver<Result<(), String>>,
    cancel: Arc<AtomicBool>,
}
impl Job {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

// The DC is used only by the worker after the dialog returns.
pub unsafe fn choose(
    owner: HWND,
    path: PathBuf,
    pdf: bool,
    pages: u32,
) -> Result<Option<Job>, String> {
    if pages == 0 {
        return Err("Wait for the PDF to finish opening.".into());
    }
    let mut ranges = [PRINTPAGERANGE {
        nFromPage: 1,
        nToPage: pages,
    }; 16];
    let mut dialog = PRINTDLGEXW {
        lStructSize: size_of::<PRINTDLGEXW>() as u32,
        hwndOwner: owner,
        Flags: PD_RETURNDC
            | PD_NOSELECTION
            | PD_NOCURRENTPAGE
            | PD_HIDEPRINTTOFILE
            | PD_USEDEVMODECOPIESANDCOLLATE,
        nMinPage: 1,
        nMaxPage: pages,
        nCopies: 1,
        nMaxPageRanges: ranges.len() as u32,
        lpPageRanges: ranges.as_mut_ptr(),
        nStartPage: START_PAGE_GENERAL,
        ..zeroed()
    };
    let result = PrintDlgExW(&mut dialog);
    if !dialog.hDevMode.is_null() {
        GlobalFree(dialog.hDevMode);
    }
    if !dialog.hDevNames.is_null() {
        GlobalFree(dialog.hDevNames);
    }
    if result < 0 || dialog.dwResultAction != PD_RESULT_PRINT {
        if !dialog.hDC.is_null() {
            DeleteDC(dialog.hDC);
        }
        return if result < 0 {
            Err(format!(
                "Could not open the print dialog (0x{:08X}).",
                result as u32
            ))
        } else {
            Ok(None)
        };
    }
    let selected = if dialog.Flags & PD_PAGENUMS != 0 {
        ranges[..dialog.nPageRanges.min(ranges.len() as u32) as usize]
            .iter()
            .map(|r| (r.nFromPage, r.nToPage))
            .collect()
    } else {
        vec![(1, pages)]
    };
    let dc = dialog.hDC as usize;
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let (sender, result) = mpsc::channel();
    std::thread::spawn(move || {
        let result = unsafe { print(dc as _, &path, pdf, &selected, &signal, None) };
        unsafe {
            DeleteDC(dc as _);
        }
        let _ = sender.send(result);
    });
    Ok(Some(Job { result, cancel }))
}

fn fit(width: u32, height: u32, page_width: i32, page_height: i32) -> (i32, i32, i32, i32) {
    let scale =
        (page_width as f64 / width.max(1) as f64).min(page_height as f64 / height.max(1) as f64);
    let w = (width as f64 * scale).round().max(1.) as i32;
    let h = (height as f64 * scale).round().max(1.) as i32;
    ((page_width - w) / 2, (page_height - h) / 2, w, h)
}

unsafe fn print(
    dc: HDC,
    path: &Path,
    pdf: bool,
    ranges: &[(u32, u32)],
    cancel: &AtomicBool,
    output: Option<&Path>,
) -> Result<(), String> {
    use windows::{
        core::HSTRING,
        Data::Pdf::PdfDocument,
        Storage::StorageFile,
        Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    };
    RoInitialize(RO_INIT_MULTITHREADED).map_err(|e| e.to_string())?;
    let result = (|| {
        if dc.is_null() {
            return Err("The printer did not provide a drawing surface.".into());
        }
        let document = if pdf {
            let path = crate::ui::path_wide(path);
            let file =
                StorageFile::GetFileFromPathAsync(&HSTRING::from_wide(&path[..path.len() - 1]))
                    .and_then(|v| v.join())
                    .map_err(|e| e.to_string())?;
            Some(
                PdfDocument::LoadFromFileAsync(&file)
                    .and_then(|v| v.join())
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let count = match &document {
            Some(doc) => doc.PageCount().map_err(|e| e.to_string())?,
            None => 1,
        };
        if ranges.is_empty()
            || ranges
                .iter()
                .any(|&(from, to)| from == 0 || from > to || to > count)
        {
            return Err(
                "The selected page range is no longer valid. Reopen the document and try again."
                    .into(),
            );
        }
        let picture = if pdf {
            None
        } else {
            Some(crate::assets::print_picture(path)?)
        };
        let title = wide(&path.file_name().unwrap_or_default().to_string_lossy());
        let output = output.map(crate::ui::path_wide);
        let info = DOCINFOW {
            cbSize: size_of::<DOCINFOW>() as i32,
            lpszDocName: title.as_ptr(),
            lpszOutput: output.as_ref().map_or(null(), |v| v.as_ptr()),
            ..zeroed()
        };
        if cancel.load(Ordering::Relaxed) {
            return Err("Printing cancelled.".into());
        }
        if StartDocW(dc, &info) <= 0 {
            return Err("Could not start printing. Check the printer and print queue.".into());
        }
        let result = (|| {
            for &(from, to) in ranges {
                for index in from - 1..to {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Printing cancelled.".into());
                    }
                    let page_width = GetDeviceCaps(dc, HORZRES as i32).max(1);
                    let page_height = GetDeviceCaps(dc, VERTRES as i32).max(1);
                    // Render one page at a time; reuse the viewer's bounded raster pipeline.
                    let raster = if let Some(doc) = &document {
                        let width = (page_width as f64 * 300.
                            / GetDeviceCaps(dc, LOGPIXELSX as i32).max(1) as f64)
                            as u32;
                        Some(crate::pdf::render(doc, index, width).map_err(|e| e.to_string())?)
                    } else {
                        None
                    };
                    let mut bitmap: BITMAPINFO = zeroed();
                    let (bits, header, width, height) = if let Some(page) = &raster {
                        bitmap.bmiHeader = BITMAPINFOHEADER {
                            biSize: size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: page.width as i32,
                            biHeight: -(page.height as i32),
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB,
                            ..zeroed()
                        };
                        (
                            page.pixels.as_ptr(),
                            &bitmap as *const BITMAPINFO,
                            page.width,
                            page.height,
                        )
                    } else {
                        let (dib, w, h) = picture.as_ref().unwrap();
                        (
                            dib.as_ptr().add(40),
                            dib.as_ptr().cast::<BITMAPINFO>(),
                            *w,
                            *h,
                        )
                    };
                    let dx = GetDeviceCaps(dc, LOGPIXELSX as i32).max(1) as f64;
                    let dy = GetDeviceCaps(dc, LOGPIXELSY as i32).max(1) as f64;
                    let (x, y, w, h) = fit(
                        width,
                        height,
                        page_width,
                        (page_height as f64 * dx / dy).round() as i32,
                    );
                    let (y, h) = (
                        (y as f64 * dy / dx).round() as i32,
                        (h as f64 * dy / dx).round() as i32,
                    );
                    if StartPage(dc) <= 0 {
                        return Err("Could not start the printed page.".into());
                    }
                    SetStretchBltMode(dc, HALFTONE);
                    SetBrushOrgEx(dc, 0, 0, null_mut());
                    let drawn = StretchDIBits(
                        dc,
                        x,
                        y,
                        w,
                        h,
                        0,
                        0,
                        width as i32,
                        height as i32,
                        bits.cast(),
                        header,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                    if drawn == 0 || drawn == -1 || EndPage(dc) <= 0 {
                        return Err("Could not send the page to the printer.".into());
                    }
                }
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("Printing cancelled.".into());
            }
            if EndDoc(dc) <= 0 {
                return Err("The printer could not finish the job.".into());
            }
            Ok(())
        })();
        if result.is_err() {
            AbortDoc(dc);
        }
        result
    })();
    RoUninitialize();
    result
}

#[test]
fn fit_preserves_aspect_and_centers() {
    assert_eq!(fit(400, 200, 1000, 1000), (0, 250, 1000, 500));
    assert_eq!(fit(200, 400, 1000, 1000), (250, 0, 500, 1000));
}

#[test]
#[ignore = "Requires Microsoft Print to PDF; prints only to temporary files"]
fn native_image_and_pdf_print_to_virtual_printer() {
    use std::time::{Duration, Instant};
    let root = std::env::temp_dir().join(format!("plumetxt-print-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let picture = root.join("input.bmp");
    let source = root.join("image-output.pdf");
    let output = root.join("selected-page.pdf");
    let mut bmp = vec![0u8; 62];
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&62u32.to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&2u32.to_le_bytes());
    bmp[22..26].copy_from_slice(&1u32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    bmp[54..60].copy_from_slice(&[0, 0, 255, 255, 0, 0]);
    std::fs::write(&picture, bmp).unwrap();
    let wait = |path: &Path, pages| {
        let start = Instant::now();
        loop {
            if lopdf::Document::load(path).is_ok_and(|d| d.get_pages().len() == pages) {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "Virtual print did not finish"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    unsafe {
        let dc = CreateDCW(
            wide("WINSPOOL").as_ptr(),
            wide("Microsoft Print to PDF").as_ptr(),
            null(),
            null(),
        );
        assert!(!dc.is_null());
        let cancel = AtomicBool::new(false);
        let result = print(
            dc,
            &picture,
            false,
            &[(1, 1), (1, 1)],
            &cancel,
            Some(&source),
        );
        DeleteDC(dc);
        result.unwrap();
        wait(&source, 2);
        let dc = CreateDCW(
            wide("WINSPOOL").as_ptr(),
            wide("Microsoft Print to PDF").as_ptr(),
            null(),
            null(),
        );
        assert!(!dc.is_null());
        let result = print(dc, &source, true, &[(2, 2)], &cancel, Some(&output));
        DeleteDC(dc);
        result.unwrap();
        wait(&output, 1);
    }
    for path in [picture, source, output] {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}
