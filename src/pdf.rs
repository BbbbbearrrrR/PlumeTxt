use crate::outline;
use std::{
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    sync::{
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc, Condvar, Mutex,
    },
    thread,
};
use windows::{
    core::{Result, HSTRING},
    Data::Pdf::{PdfDocument, PdfPageRenderOptions},
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
        ExifOrientationMode,
    },
    Storage::{StorageFile, Streams::InMemoryRandomAccessStream},
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

pub const READY: u32 = 0x8001;
pub struct Request {
    pub generation: u64,
    pub document_id: u64,
    pub path: PathBuf,
    pub jobs: Vec<(u32, u32)>,
}
pub struct Page {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    #[cfg(test)]
    pub count: u32,
    pub index: u32,
}
pub enum Reply {
    Info(u64, Vec<(f32, f32)>),
    Page(u64, u64, Page),
    Outline(u64, std::result::Result<Vec<outline::Bookmark>, String>),
    Error(u64, String),
}
enum Work {
    Render(Request),
    Close,
    Stop,
}
pub struct Worker {
    pending: Arc<(Mutex<Option<Work>>, Condvar)>,
    pub results: Receiver<Reply>,
}
fn emit(sender: &SyncSender<Reply>, hwnd: usize, reply: Reply) -> bool {
    if sender.send(reply).is_err() {
        return false;
    }
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd as _, READY, 0, 0);
    }
    true
}
impl Worker {
    pub fn new(hwnd: usize) -> Self {
        let pending = Arc::new((Mutex::new(None), Condvar::new()));
        // Backpressure bounds completed raster memory even during a native modal dialog.
        let (sender, results) = sync_channel(1);
        let queue = pending.clone();
        thread::spawn(move || {
            let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
            let mut document: Option<(PathBuf, u64, PdfDocument, bool)> = None;
            loop {
                let work = {
                    let (lock, wake) = &*queue;
                    let mut slot = lock.lock().unwrap();
                    while slot.is_none() {
                        slot = wake.wait(slot).unwrap();
                    }
                    slot.take().unwrap()
                };
                match work {
                    Work::Stop => break,
                    Work::Close => document = None,
                    Work::Render(request) => {
                        let result = (|| -> Result<()> {
                            initialized.clone()?;
                            if document.as_ref().map(|d| (&d.0, d.1))
                                != Some((&request.path, request.document_id))
                            {
                                document = None;
                                // StorageFile requires Windows separators, unlike std::fs.
                                let path: Vec<u16> = request
                                    .path
                                    .as_os_str()
                                    .encode_wide()
                                    .map(|c| if c == b'/' as u16 { b'\\' as u16 } else { c })
                                    .collect();
                                let file =
                                    StorageFile::GetFileFromPathAsync(&HSTRING::from_wide(&path))?
                                        .join()?;
                                let doc = PdfDocument::LoadFromFileAsync(&file)?.join()?;
                                let count = doc.PageCount()?;
                                if count == 0 {
                                    return Err(windows::core::Error::from_hresult(
                                        windows::core::HRESULT(0x80004005u32 as i32),
                                    ));
                                }
                                let mut sizes = Vec::with_capacity(count as usize);
                                for n in 0..count {
                                    let page = doc.GetPage(n)?;
                                    let size = page.Size();
                                    let _ = page.Close();
                                    let size = size?;
                                    sizes.push((size.Width.max(1.), size.Height.max(1.)));
                                }
                                document =
                                    Some((request.path.clone(), request.document_id, doc, false));
                                if !emit(&sender, hwnd, Reply::Info(request.document_id, sizes)) {
                                    return Ok(());
                                }
                            }
                            let doc = &mut document.as_mut().unwrap();
                            for (index, width) in request.jobs {
                                if queue.0.lock().unwrap().is_some() {
                                    break;
                                }
                                let page = render(&doc.2, index, width)?;
                                if !emit(
                                    &sender,
                                    hwnd,
                                    Reply::Page(request.document_id, request.generation, page),
                                ) {
                                    return Ok(());
                                }
                            }
                            // Get the first visible images on screen before extracting bookmarks.
                            if !doc.3 && queue.0.lock().unwrap().is_none() {
                                doc.3 = true;
                                emit(
                                    &sender,
                                    hwnd,
                                    Reply::Outline(
                                        request.document_id,
                                        outline::read(&request.path),
                                    ),
                                );
                            }
                            Ok(())
                        })();
                        if let Err(e) = result {
                            if !emit(
                                &sender,
                                hwnd,
                                Reply::Error(
                                    request.document_id,
                                    format!("Could not open or render PDF: {e}"),
                                ),
                            ) {
                                break;
                            }
                        }
                    }
                }
            }
            drop(document);
            if initialized.is_ok() {
                unsafe {
                    RoUninitialize();
                }
            }
        });
        Self { pending, results }
    }
    fn submit(&self, work: Work) {
        *self.pending.0.lock().unwrap() = Some(work);
        self.pending.1.notify_one();
    }
    pub fn request(&self, request: Request) {
        self.submit(Work::Render(request));
    }
    pub fn close(&self) {
        self.submit(Work::Close);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.submit(Work::Stop);
    }
}

pub fn dimensions(width: f32, height: f32, requested: u32) -> (u32, u32) {
    let ratio = (height / width).clamp(0.01, 100.0) as f64;
    // ponytail: 单页最多 8M 像素；更高倍率的清晰缩放需要分块渲染。
    let w = (requested.clamp(64, 4096) as f64)
        .min((8_000_000.0 / ratio).sqrt())
        .min(8192.0 / ratio)
        .max(1.0);
    (w as u32, (w * ratio).max(1.0) as u32)
}

pub(crate) fn render(doc: &PdfDocument, index: u32, requested: u32) -> Result<Page> {
    let count = doc.PageCount()?;
    let index = index.min(count.saturating_sub(1));
    let page = doc.GetPage(index)?;
    let result = (|| {
        let size = page.Size()?;
        let (width, height) = dimensions(size.Width, size.Height, requested);
        let options = PdfPageRenderOptions::new()?;
        options.SetDestinationWidth(width)?;
        options.SetDestinationHeight(height)?;
        options.SetBackgroundColor(windows::UI::Color {
            A: 255,
            R: 255,
            G: 255,
            B: 255,
        })?;
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)?
            .join()?;
        stream.Seek(0)?;
        let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
        let pixels = decoder
            .GetPixelDataTransformedAsync(
                BitmapPixelFormat::Bgra8,
                BitmapAlphaMode::Ignore,
                &BitmapTransform::new()?,
                ExifOrientationMode::IgnoreExifOrientation,
                ColorManagementMode::DoNotColorManage,
            )?
            .join()?
            .DetachPixelData()?;
        let width = decoder.PixelWidth()?;
        let height = decoder.PixelHeight()?;
        if pixels.len() != width as usize * height as usize * 4 {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x80004005u32 as i32,
            )));
        }
        Ok(Page {
            pixels: pixels.to_vec(),
            width,
            height,
            #[cfg(test)]
            count,
            index,
        })
    })();
    let _ = page.Close();
    result
}

#[test]
fn raster_memory_is_bounded() {
    for (w, h) in [(595., 842.), (100., 10000.), (10000., 100.)] {
        for requested in [1, 800, 100_000] {
            let (rw, rh) = dimensions(w, h, requested);
            assert!(rw > 0 && rh > 0 && rw <= 4096 && rh <= 8192);
            assert!(rw as u64 * rh as u64 <= 8_000_000);
        }
    }
}
