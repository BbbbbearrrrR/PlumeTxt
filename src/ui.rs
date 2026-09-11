#[cfg(test)]
use crate::pdf;
use crate::{
    document::{self, Encoding},
    markdown, palette, reader, scroll, terminal, theme,
};
use std::{
    cell::RefCell,
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{Arc, Mutex},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    Storage::Xps::*,
    System::LibraryLoader::*,
    UI::{
        Controls::{Dialogs::*, *},
        Input::KeyboardAndMouse::*,
        Shell::*,
        WindowsAndMessaging::*,
    },
};

const NEW: usize = 101;
const OPEN: usize = 102;
const SAVE: usize = 103;
const SAVE_AS: usize = 104;
const PREVIEW: usize = 105;
const EXPORT: usize = 106;
const PREV: usize = 107;
const NEXT: usize = 108;
const ZOOM_IN: usize = 109;
const ZOOM_OUT: usize = 110;
const FIT: usize = 111;
const EDITOR: usize = 112;
const BOLD: usize = 113;
const ITALIC: usize = 114;
const CODE: usize = 115;
const EXIT: usize = 116;
const TOC: usize = 117;
const COMMANDS: usize = 118;
const FONT_MODE: usize = 119;
const TEXT_LARGER: usize = 120;
const TEXT_SMALLER: usize = 121;
const TERMINAL: usize = 122;
const IMAGE: usize = 123;
const KEEP_CURRENT: usize = 124;
const USE_DISK: usize = 125;
pub const PASTE: u32 = WM_APP + 8;
const DISPATCH: u32 = WM_APP + 2;
const LAYOUT: usize = 200;
const CHANGE: usize = 201;
const EXPORTED: u32 = WM_APP + 3;
const EM_STREAMIN: u32 = WM_USER + 73;
const EM_FORMATRANGE: u32 = WM_USER + 57;
const EM_EXLIMITTEXT: u32 = WM_USER + 53;
const EM_SETEVENTMASK: u32 = WM_USER + 69;
const EM_SETTEXTMODE: u32 = WM_USER + 89;
const EM_SETUNDOLIMIT: u32 = WM_USER + 82;

thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn path_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
unsafe fn error(hwnd: HWND, text: &str) {
    MessageBoxW(
        hwnd,
        wide(text).as_ptr(),
        wide("FeatherPad").as_ptr(),
        MB_OK | MB_ICONERROR,
    );
}
unsafe fn text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd).max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let read = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
    String::from_utf16_lossy(&buf[..read])
}

struct App {
    hwnd: HWND,
    edit: HWND,
    highlighter: crate::syntax::Highlighter,
    preview: HWND,
    status: HWND,
    file_type: HWND,
    terminal_button: HWND,
    terminal: Option<terminal::Terminal>,
    terminal_open: bool,
    terminal_width: i32,
    font: HFONT,
    path: Option<PathBuf>,
    encoding: Encoding,
    crlf: bool,
    original: Option<u64>,
    watch: Option<crate::external::Watch>,
    comparison: Option<crate::external::Snapshot>,
    compare_actions: [HWND; 2],
    preview_before_compare: bool,
    image_path: Option<PathBuf>,
    image: Option<crate::imageview::ImageView>,
    pdf_path: Option<PathBuf>,
    large_path: Option<PathBuf>,
    large: Option<crate::large::Large>,
    viewer: Option<reader::Reader>,
    fonts: theme::Fonts,
    palette: palette::Palette,
    monospace: bool,
    text_zoom: i32,
    buffer: theme::Buffer,
    exporting: bool,
    export_result: Arc<Mutex<Option<Result<PathBuf, String>>>>,
}

impl App {
    unsafe fn close_comparison(&mut self) {
        if self.comparison.take().is_none() {
            return;
        }
        for action in self.compare_actions {
            ShowWindow(action, SW_HIDE);
        }
        if !self.preview_before_compare && !self.preview.is_null() {
            DestroyWindow(self.preview);
            self.preview = null_mut();
        }
        scroll::pair(self.edit, self.preview);
        if !self.preview.is_null() {
            scroll::pair(self.preview, self.edit);
        }
    }
    unsafe fn poll_external(&mut self) {
        if self.image.is_some() || self.pdf_path.is_some() || self.large.is_some() {
            return;
        }
        let Some(result) = self.watch.as_ref().and_then(|watch| watch.take()) else {
            return;
        };
        match result {
            Ok(snapshot) => self.review_external(snapshot),
            Err(e) => self.status(&format!("External update: {e}")),
        }
    }
    unsafe fn review_external(&mut self, snapshot: crate::external::Snapshot) {
        if self.original == Some(snapshot.fingerprint) {
            if self.comparison.is_some() {
                self.close_comparison();
                self.layout();
                self.refresh_preview();
            }
            return;
        }
        if self.comparison.is_none() {
            self.preview_before_compare = !self.preview.is_null();
            if self.preview.is_null() {
                self.preview = rich_edit(self.hwnd, true, self.fonts.code);
            }
            scroll::pair(self.edit, null_mut());
            scroll::pair(self.preview, null_mut());
        }
        self.comparison = Some(snapshot);
        SendMessageW(self.preview, WM_USER + 225, self.text_zoom as usize, 100);
        self.layout();
        self.refresh_preview();
        self.status("External changes · review the disk version on the right");
    }
    unsafe fn resolve_external(&mut self, use_disk: bool) {
        let (Some(path), Some(snapshot)) = (&self.path, &self.comparison) else {
            return;
        };
        let (source, encoding, fingerprint) = match document::read(path) {
            Ok(value) => value,
            Err(e) => {
                self.status(&format!("Cannot resolve: {e}"));
                return;
            }
        };
        if fingerprint != snapshot.fingerprint {
            self.review_external(crate::external::Snapshot {
                source,
                encoding,
                fingerprint,
            });
            self.status("Disk changed again · review the latest version");
            return;
        }
        if use_disk {
            let mut position: POINT = zeroed();
            let (mut start, mut end) = (0u32, 0u32);
            SendMessageW(
                self.edit,
                EM_GETSEL,
                &mut start as *mut _ as usize,
                &mut end as *mut _ as isize,
            );
            SendMessageW(
                self.edit,
                WM_USER + 221,
                0,
                &mut position as *mut _ as isize,
            );
            SendMessageW(self.edit, EM_SETSEL, 0, -1);
            SendMessageW(self.edit, EM_REPLACESEL, 1, wide(&source).as_ptr() as isize);
            if document::encode(&text(self.edit), Encoding::Utf8, false)
                != document::encode(&source, Encoding::Utf8, false)
            {
                SendMessageW(self.edit, WM_UNDO, 0, 0);
                self.status("Could not adopt the complete disk version; adoption cancelled");
                return;
            }
            SendMessageW(self.edit, EM_SETSEL, start as usize, end as isize);
            self.encoding = snapshot.encoding;
            self.crlf = source.contains("\r\n");
            SendMessageW(self.edit, WM_USER + 222, 0, &position as *const _ as isize);
        }
        self.original = Some(fingerprint);
        SendMessageW(self.edit, EM_SETMODIFY, usize::from(!use_disk), 0);
        self.close_comparison();
        self.highlighter.clear();
        self.layout();
        self.refresh_preview();
        self.title();
        SetFocus(self.edit);
        self.status(if use_disk {
            "Disk version adopted · Ctrl+Z restores previous text"
        } else {
            "Current version kept · Ctrl+S to save"
        });
    }
    unsafe fn title(&self) {
        let dirty = self.large.is_none()
            && self.image.is_none()
            && self.pdf_path.is_none()
            && SendMessageW(self.edit, EM_GETMODIFY, 0, 0) != 0;
        let path = self
            .large_path
            .as_ref()
            .or(self.image_path.as_ref())
            .or(self.pdf_path.as_ref())
            .or(self.path.as_ref());
        let name = path
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        SetWindowTextW(
            self.file_type,
            wide(&file_type(path.map(PathBuf::as_path))).as_ptr(),
        );
        SetWindowTextW(
            self.hwnd,
            wide(&format!(
                "{}{}{} — FeatherPad",
                if dirty { "* " } else { "" },
                name,
                if self.large.is_some() {
                    " · Read only"
                } else {
                    ""
                }
            ))
            .as_ptr(),
        );
    }
    fn hints(&self) -> &'static str {
        if self.image.is_some() {
            "Wheel Zoom    Drag Pan    Ctrl+0 Fit    Ctrl+E Editor    Ctrl+J Terminal"
        } else if self.pdf_path.is_some() {
            "Ctrl+O Open    Ctrl+Shift+L Outline    Ctrl+J Terminal    Ctrl+Shift+P Commands"
        } else if self.large.is_some() {
            "Ctrl+O Open    Ctrl+E Editor    Ctrl+J Terminal    Ctrl+Shift+P Commands"
        } else {
            "Ctrl+S Save    Ctrl+Shift+M Preview    Ctrl+Shift+I Image    Ctrl+J Terminal    Ctrl+Shift+P Commands"
        }
    }
    unsafe fn status(&self, message: &str) {
        SetWindowTextW(
            self.status,
            wide(if message.is_empty() {
                self.hints()
            } else {
                message
            })
            .as_ptr(),
        );
        if !message.is_empty() {
            SetTimer(self.hwnd, 8, 3000, None);
        }
    }
    unsafe fn layout(&mut self) {
        let rc = theme::client(self.hwnd);
        MoveWindow(
            self.status,
            24,
            rc.bottom - 27,
            (rc.right - 230).max(1),
            24,
            0,
        );
        MoveWindow(self.file_type, rc.right - 188, rc.bottom - 27, 120, 24, 0);
        MoveWindow(
            self.terminal_button,
            rc.right - 52,
            rc.bottom - 29,
            36,
            24,
            0,
        );
        let bottom = rc.bottom;
        let right = (rc.right - self.terminal_width).max(1);
        if let Some(view) = &self.image {
            MoveWindow(view.0, 0, 0, right, (bottom - 34).max(1), 0);
            theme::invalidate(view.0);
        }
        if let Some(view) = &self.large {
            MoveWindow(view.0, 0, 0, right, (bottom - 34).max(1), 0);
        }
        if let Some(terminal) = &self.terminal {
            MoveWindow(
                terminal.0,
                right + 8,
                12,
                (self.terminal_width - 20).max(1),
                (bottom - 58).max(1),
                0,
            );
        }
        if let Some(viewer) = &self.viewer {
            MoveWindow(viewer.0, 0, 0, right, (bottom - 34).max(1), 0);
        }
        let inset = if self.preview.is_null() {
            ((right - 960) / 2).max(16)
        } else {
            16
        };
        let split = if self.preview.is_null() {
            right - inset
        } else {
            right / 2
        };
        scroll::resize(
            self.edit,
            inset,
            28,
            (split - inset).max(1),
            (bottom - 64).max(1),
        );
        if !self.preview.is_null() {
            scroll::resize(
                self.preview,
                split + 1,
                if self.comparison.is_some() { 40 } else { 24 },
                (right - split - 17).max(1),
                (bottom - if self.comparison.is_some() { 80 } else { 64 }).max(1),
            );
        }
        let comparing = self.comparison.is_some()
            && self.image.is_none()
            && self.pdf_path.is_none()
            && self.large.is_none();
        for (i, action) in self.compare_actions.iter().enumerate() {
            MoveWindow(*action, split + 12 + i as i32 * 134, 6, 128, 26, 0);
            ShowWindow(*action, if comparing { SW_SHOWNA } else { SW_HIDE });
        }
        scroll::show(self.edit, self.preview.is_null() || comparing);
        InvalidateRect(self.edit, null(), 1);
        if !self.preview.is_null() {
            InvalidateRect(self.preview, null(), 1);
        }
        theme::invalidate(self.hwnd);
        SetTimer(self.hwnd, 9, 80, None);
    }
    unsafe fn sync_scroll(&self, source: HWND) {
        if self.comparison.is_some() {
            return;
        }
        if self.preview.is_null() || (source != self.edit && source != self.preview) {
            return;
        }
        let target = if source == self.edit {
            self.preview
        } else {
            self.edit
        };
        scroll::measure(source);
        scroll::measure(target);
        let from = scroll::info(source, true);
        let to = scroll::info(target, true);
        let pos = (from.nPos as f64 / scroll::limit(&from).max(1) as f64
            * scroll::limit(&to) as f64)
            .round() as i32;
        if (to.nPos - pos).abs() > 1 {
            scroll::set_position(target, true, pos);
        }
    }
    unsafe fn show_editor(&mut self) {
        self.image.take();
        self.image_path = None;
        self.large.take();
        self.large_path = None;
        self.pdf_path = None;
        if let Some(viewer) = &self.viewer {
            viewer.close();
        }
        ShowWindow(self.edit, SW_SHOW);
        if !self.preview.is_null() {
            ShowWindow(self.preview, SW_SHOW);
        }
        self.layout();
        self.title();
        SetFocus(self.edit);
        self.status("");
        self.highlighter.clear();
        SetTimer(self.hwnd, 9, 80, None);
    }
    unsafe fn confirm_save(&mut self) -> bool {
        if self.large.is_some() {
            return true;
        }
        if SendMessageW(self.edit, EM_GETMODIFY, 0, 0) == 0 {
            return true;
        }
        match MessageBoxW(
            self.hwnd,
            wide("Save changes?").as_ptr(),
            wide("FeatherPad").as_ptr(),
            MB_YESNOCANCEL | MB_ICONQUESTION,
        ) {
            IDYES => {
                if self.image.is_some() {
                    self.show_editor();
                }
                self.save(false)
            }
            IDNO => true,
            _ => false,
        }
    }
    unsafe fn save(&mut self, save_as: bool) -> bool {
        if self.comparison.is_some() {
            self.status("Review external changes first: Keep current or Use disk");
            return false;
        }
        if self.image.is_some() {
            self.status("Image · Read only");
            return false;
        }
        if self.large.is_some() {
            self.status("Large file · Read only");
            return false;
        }
        let destination = if save_as || self.path.is_none() {
            dialog(self.hwnd, true, false)
        } else {
            self.path.clone()
        };
        let Some(path) = destination else {
            return false;
        };
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            error(self.hwnd, "Use Export PDF to create a PDF.");
            return false;
        }
        if self.path.as_ref() == Some(&path) {
            if let Some(original) = self.original {
                match document::read_bytes(&path) {
                    Ok(bytes) if document::fingerprint(&bytes) == original => (),
                    _ => {
                        error(
                            self.hwnd,
                            "The file changed on disk. Save as a different file.",
                        );
                        return false;
                    }
                }
            }
        }
        let bytes = document::encode(&text(self.edit), self.encoding, self.crlf);
        if bytes.len() as u64 > document::MAX_TEXT_BYTES {
            error(self.hwnd, "Text exceeds the 32 MiB limit.");
            return false;
        }
        match document::atomic_write(&path, &bytes) {
            Ok(()) => {
                self.path = Some(path);
                self.original = Some(document::fingerprint(&bytes));
                self.watch = self
                    .path
                    .clone()
                    .map(|p| crate::external::Watch::new(p, self.original.unwrap()));
                SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
                self.title();
                self.status("Saved");
                self.highlighter.clear();
                SetTimer(self.hwnd, 9, 80, None);
                true
            }
            Err(e) => {
                error(self.hwnd, &format!("Save failed: {e}"));
                false
            }
        }
    }
    unsafe fn open(&mut self, path: PathBuf) {
        if crate::assets::supported(&path) {
            let path = match std::path::absolute(&path) {
                Ok(path) => path,
                Err(e) => {
                    error(self.hwnd, &e.to_string());
                    return;
                }
            };
            let view = match crate::imageview::ImageView::open(self.hwnd, &path) {
                Ok(view) => view,
                Err(e) => {
                    error(self.hwnd, &e);
                    return;
                }
            };
            self.large.take();
            self.large_path = None;
            self.pdf_path = None;
            if let Some(viewer) = &self.viewer {
                viewer.close();
            }
            ShowWindow(self.edit, SW_HIDE);
            if !self.preview.is_null() {
                ShowWindow(self.preview, SW_HIDE);
            }
            self.image = Some(view);
            self.image_path = Some(path);
            self.layout();
            let hwnd = self.image.as_ref().unwrap().0;
            ShowWindow(hwnd, SW_SHOW);
            SetFocus(hwnd);
            self.title();
            self.status("");
            return;
        }

        let is_pdf = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        if !is_pdf && std::fs::metadata(&path).is_ok_and(|m| m.len() > crate::large::THRESHOLD) {
            if !self.confirm_save() {
                return;
            }
            self.close_comparison();
            self.watch = None;
            self.image.take();
            self.image_path = None;
            self.large.take();
            SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
            if let Some(viewer) = &self.viewer {
                viewer.close();
            }
            self.pdf_path = None;
            self.large_path = Some(path.clone());
            self.large = Some(crate::large::Large::create(
                self.hwnd,
                path,
                self.fonts.code,
            ));
            ShowWindow(self.edit, SW_HIDE);
            if !self.preview.is_null() {
                ShowWindow(self.preview, SW_HIDE);
            }
            self.layout();
            self.title();
            self.status("");
            SetFocus(self.large.as_ref().unwrap().0);
            return;
        }
        if is_pdf {
            self.image.take();
            self.image_path = None;
            self.large.take();
            self.large_path = None;
        }

        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            let path = match std::path::absolute(&path) {
                Ok(p) => p,
                Err(e) => {
                    error(self.hwnd, &e.to_string());
                    return;
                }
            };
            self.pdf_path = Some(path.clone());
            ShowWindow(self.edit, SW_HIDE);
            if !self.preview.is_null() {
                ShowWindow(self.preview, SW_HIDE);
            }
            if self.viewer.is_none() {
                self.viewer = Some(reader::Reader::create(
                    self.hwnd,
                    self.fonts.ui,
                    self.fonts.small,
                ));
            }
            self.layout();
            self.viewer.as_ref().unwrap().open(path);
            self.title();
            self.status("");
        } else {
            if !self.confirm_save() {
                return;
            }
            match document::read(&path) {
                Ok((source, encoding, original)) => {
                    if SetWindowTextW(self.edit, wide(&source).as_ptr()) == 0 {
                        error(self.hwnd, "Could not load text");
                        return;
                    }
                    self.close_comparison();
                    self.path = Some(path);
                    self.watch = self
                        .path
                        .clone()
                        .map(|p| crate::external::Watch::new(p, original));
                    self.encoding = encoding;
                    self.crlf = source.contains("\r\n");
                    self.original = Some(original);
                    SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
                    SendMessageW(self.edit, EM_EMPTYUNDOBUFFER, 0, 0);
                    self.show_editor();
                    self.refresh_preview();
                }
                Err(e) => error(self.hwnd, &e),
            }
        }
    }
    unsafe fn refresh_preview(&self) {
        if let Some(snapshot) = &self.comparison {
            if self.image.is_none() && self.pdf_path.is_none() && self.large.is_none() {
                let mut position: POINT = zeroed();
                SendMessageW(
                    self.preview,
                    WM_USER + 221,
                    0,
                    &mut position as *mut _ as isize,
                );
                SendMessageW(self.preview, WM_SETREDRAW, 0, 0);
                let result = set_rtf(
                    self.preview,
                    &crate::external::rtf(&text(self.edit), &snapshot.source),
                );
                SendMessageW(
                    self.preview,
                    WM_USER + 222,
                    0,
                    &position as *const _ as isize,
                );
                SendMessageW(self.preview, WM_SETREDRAW, 1, 0);
                theme::invalidate(self.preview);
                if let Err(e) = result {
                    self.status(&e);
                }
            }
            return;
        }
        if self.image.is_some() || self.large.is_some() || self.pdf_path.is_some() {
            return;
        }
        if !self.preview.is_null() {
            let mut rc: RECT = zeroed();
            GetClientRect(self.preview, &mut rc);
            let dc = GetDC(self.preview);
            let dpi = GetDeviceCaps(dc, LOGPIXELSX as i32).max(1);
            ReleaseDC(self.preview, dc);
            let width = ((rc.right - 48).max(16) * 1440 / dpi) as usize;
            SendMessageW(self.preview, WM_SETREDRAW, 0, 0);
            let result = set_rtf(
                self.preview,
                &markdown::with_images(
                    &text(self.edit),
                    width,
                    true,
                    self.path.as_deref().and_then(Path::parent),
                ),
            );
            self.sync_scroll(self.edit);
            SendMessageW(self.preview, WM_SETREDRAW, 1, 0);
            theme::invalidate(self.preview);
            if let Err(e) = result {
                error(self.hwnd, &e);
            }
        }
    }
    fn is_markdown(&self) -> bool {
        self.path.as_ref().is_none_or(|p| {
            p.extension().and_then(|s| s.to_str()).is_some_and(|s| {
                matches!(s.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown")
            })
        })
    }
    unsafe fn insert_image(&mut self, paste: crate::assets::Paste) {
        if !self.is_markdown() {
            self.status("Images are available in Markdown files");
            return;
        }
        if self.path.is_none() && !self.save(true) {
            return;
        }
        if !self.is_markdown() {
            self.status("Save with a .md extension to insert images");
            return;
        }
        match crate::assets::insert(self.path.as_deref().unwrap(), paste) {
            Ok(link) => {
                SendMessageW(
                    self.edit,
                    EM_REPLACESEL,
                    1,
                    wide(&format!("\r\n\r\n{link}\r\n\r\n")).as_ptr() as isize,
                );
                self.refresh_preview();
                self.status("Image saved beside this document");
                SetFocus(self.edit);
            }
            Err(e) => error(self.hwnd, &e),
        }
    }
    unsafe fn wrap(&self, marker: &str) {
        if self.pdf_path.is_some() {
            return;
        }
        let mut a: u32 = 0;
        let mut b: u32 = 0;
        SendMessageW(
            self.edit,
            EM_GETSEL,
            &mut a as *mut _ as usize,
            &mut b as *mut _ as isize,
        );
        let mut selected = vec![0u16; b.saturating_sub(a) as usize + 1];
        SendMessageW(self.edit, WM_USER + 62, 0, selected.as_mut_ptr() as isize);
        let selected = String::from_utf16_lossy(
            &selected[..selected
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(selected.len())],
        );
        SendMessageW(
            self.edit,
            EM_REPLACESEL,
            1,
            wide(&format!("{marker}{selected}{marker}")).as_ptr() as isize,
        );
        SetFocus(self.edit);
    }
    unsafe fn command(&mut self, id: usize) {
        if self.image.is_some()
            && matches!(
                id,
                EXPORT | BOLD | ITALIC | CODE | FONT_MODE | TEXT_LARGER | TEXT_SMALLER | 202
            )
        {
            self.status("Image · Read only");
            return;
        }

        if self.large.is_some()
            && matches!(
                id,
                PREVIEW
                    | EXPORT
                    | BOLD
                    | ITALIC
                    | CODE
                    | FONT_MODE
                    | TEXT_LARGER
                    | TEXT_SMALLER
                    | 202
            )
        {
            self.status("Large file · Read only");
            return;
        }

        match id {
            KEEP_CURRENT | USE_DISK => self.resolve_external(id == USE_DISK),
            OPEN => {
                if let Some(path) = dialog(self.hwnd, false, false) {
                    self.open(path);
                }
            }
            NEW => {
                if self.confirm_save() {
                    SetWindowTextW(self.edit, wide("").as_ptr());
                    SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
                    self.close_comparison();
                    self.watch = None;
                    self.path = None;
                    self.original = None;
                    self.encoding = Encoding::Utf8;
                    self.crlf = true;
                    self.show_editor();
                    self.refresh_preview();
                }
            }
            SAVE => {
                self.save(false);
            }
            SAVE_AS => {
                self.save(true);
            }
            EDITOR => self.show_editor(),
            PREVIEW if self.comparison.is_some() => {
                self.status("Review external changes first");
            }
            PREVIEW => {
                self.show_editor();
                if self.preview.is_null() {
                    self.preview = rich_edit(self.hwnd, true, self.font);
                    scroll::pair(self.preview, self.edit);
                    scroll::pair(self.edit, self.preview);
                    SendMessageW(self.preview, WM_USER + 225, self.text_zoom as usize, 100);
                } else {
                    scroll::pair(self.edit, null_mut());
                    DestroyWindow(self.preview);
                    self.preview = null_mut();
                }
                self.layout();
                self.refresh_preview();
            }
            EXPORT => {
                if self.exporting {
                    self.status("Exporting…");
                    return;
                }
                if let Some(path) = dialog(self.hwnd, true, true) {
                    if self.path.as_ref() == Some(&path) || self.pdf_path.as_ref() == Some(&path) {
                        error(self.hwnd, "Choose a different output file.");
                        return;
                    }
                    self.exporting = true;
                    self.status("Exporting…");
                    let source = text(self.edit);
                    let base = self
                        .path
                        .as_deref()
                        .and_then(Path::parent)
                        .map(Path::to_path_buf);
                    let output = self.export_result.clone();
                    let hwnd = self.hwnd as usize;
                    std::thread::spawn(move || {
                        let result =
                            export_complete(&source, &path, base.as_deref()).map(|()| path);
                        *output.lock().unwrap() = Some(result);
                        PostMessageW(hwnd as HWND, EXPORTED, 0, 0);
                    });
                }
            }
            ZOOM_IN | ZOOM_OUT | FIT if self.image.is_some() => {
                self.image.as_ref().unwrap().zoom(match id {
                    ZOOM_IN => 1.15,
                    ZOOM_OUT => 1. / 1.15,
                    _ => 0.,
                });
            }
            PREV | NEXT | ZOOM_IN | ZOOM_OUT | FIT if self.pdf_path.is_some() => {
                if let Some(viewer) = &self.viewer {
                    viewer.action(match id {
                        PREV => reader::UP,
                        NEXT => reader::DOWN,
                        ZOOM_IN => reader::LARGER,
                        ZOOM_OUT => reader::SMALLER,
                        _ => reader::FIT,
                    });
                }
            }
            BOLD => self.wrap("**"),
            ITALIC => self.wrap("*"),
            CODE => self.wrap("`"),
            EXIT => {
                if self.exporting {
                    error(self.hwnd, "Wait for the export to finish.");
                } else if self.confirm_save() {
                    DestroyWindow(self.hwnd);
                }
            }
            LAYOUT => {
                if self.terminal_open {
                    SetTimer(self.hwnd, 7, 15, None);
                }
                self.layout();
                if !self.preview.is_null() && self.pdf_path.is_none() && self.image.is_none() {
                    SetTimer(self.hwnd, 1, 350, None);
                }
            }
            CHANGE => {
                self.title();
                self.highlighter.clear();
                SetTimer(self.hwnd, 9, 120, None);
                if !self.preview.is_null() && self.pdf_path.is_none() && self.image.is_none() {
                    if self.comparison.is_some() || GetWindowTextLengthW(self.edit) <= 512 * 1024 {
                        SetTimer(self.hwnd, 1, 350, None);
                    } else {
                        self.status("Automatic preview paused. Press Ctrl+Shift+R to refresh.");
                    }
                }
            }
            TERMINAL => {
                self.terminal_open = !self.terminal_open;
                if self.terminal_open
                    && (self.terminal.is_none()
                        || self.terminal.as_ref().is_some_and(|t| t.exited()))
                {
                    let cwd = self
                        .large_path
                        .as_ref()
                        .or(self.image_path.as_ref())
                        .or(self.pdf_path.as_ref())
                        .or(self.path.as_ref())
                        .and_then(|p| p.parent())
                        .and_then(|p| std::path::absolute(p).ok())
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    self.terminal = Some(terminal::Terminal::create(self.hwnd, &cwd));
                }
                if self.terminal_open {
                    self.layout();
                    if let Some(t) = &self.terminal {
                        ShowWindow(t.0, SW_SHOW);
                        SetFocus(t.0);
                    }
                }
                SetTimer(self.hwnd, 7, 15, None);
            }
            IMAGE => {
                if self.image.is_some() || self.pdf_path.is_some() || self.large.is_some() {
                    self.status("Open a Markdown file to insert images");
                    return;
                }
                if let Some(path) = image_dialog(self.hwnd) {
                    self.insert_image(crate::assets::Paste::File(path));
                }
            }
            COMMANDS => self.palette.show(),
            FONT_MODE => {
                self.monospace = !self.monospace;
                SendMessageW(
                    self.edit,
                    WM_SETFONT,
                    if self.monospace {
                        self.fonts.code
                    } else {
                        self.fonts.body
                    } as usize,
                    1,
                );
                theme::editor_colors(self.edit);
                self.highlighter.clear();
                SetTimer(self.hwnd, 9, 80, None);
                self.status(if self.monospace {
                    "Consolas"
                } else {
                    "Segoe UI"
                });
            }
            TEXT_LARGER | TEXT_SMALLER => {
                self.text_zoom =
                    (self.text_zoom + if id == TEXT_LARGER { 10 } else { -10 }).clamp(70, 200);
                SendMessageW(self.edit, WM_USER + 225, self.text_zoom as usize, 100);
                if !self.preview.is_null() {
                    SendMessageW(self.preview, WM_USER + 225, self.text_zoom as usize, 100);
                }
                self.status(&format!("{}%", self.text_zoom));
            }
            TOC => {
                if let Some(viewer) = &self.viewer {
                    viewer.action(reader::TOGGLE_TOC);
                }
            }
            202 => self.refresh_preview(),
            _ => (),
        }
    }
    unsafe fn paint(&mut self, target: HDC, dirty: &RECT) {
        use theme::*;
        let rc = client(self.hwnd);
        if !self.buffer.ensure(target, rc.right, rc.bottom) {
            return;
        }
        let dc = self.buffer.dc;
        fill(dc, rc, CANVAS);
        if self.large.is_none()
            && self.pdf_path.is_none()
            && self.image.is_none()
            && !self.preview.is_null()
        {
            let middle = (rc.right - self.terminal_width).max(1) / 2;
            fill(
                dc,
                RECT {
                    left: middle,
                    top: 20,
                    right: middle + 1,
                    bottom: rc.bottom - 50,
                },
                LINE,
            );
        }
        fill(
            dc,
            RECT {
                left: 0,
                top: rc.bottom - 34,
                right: rc.right,
                bottom: rc.bottom - 33,
            },
            LINE,
        );
        self.buffer.blit(target, dirty);
    }
}

unsafe fn rich_edit(parent: HWND, readonly: bool, font: HFONT) -> HWND {
    let control = CreateWindowExW(
        0,
        wide("RICHEDIT50W").as_ptr(),
        wide("").as_ptr(),
        (if parent.is_null() { WS_POPUP } else { WS_CHILD })
            | WS_VSCROLL
            | scroll::ES_DISABLENOSCROLL
            | WS_TABSTOP
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | ES_WANTRETURN as u32
            | if readonly { ES_READONLY as u32 } else { 0 },
        0,
        0,
        1,
        1,
        parent,
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    if control.is_null() {
        return control;
    }
    if !readonly {
        SendMessageW(control, EM_SETTEXTMODE, 2, 0); // Rich text colours; files remain plain text.
        crate::syntax::attach(control);
    }
    SendMessageW(
        control,
        EM_EXLIMITTEXT,
        0,
        document::MAX_TEXT_BYTES as isize,
    );
    SendMessageW(control, EM_SETUNDOLIMIT, if readonly { 0 } else { 100 }, 0);
    SendMessageW(control, WM_SETFONT, font as usize, 0);
    if !parent.is_null() {
        theme::editor_colors(control);
        theme::dark_scrollbars(control, theme::CANVAS);
    }
    SendMessageW(control, EM_SETEVENTMASK, 0, 1); // ENM_CHANGE
    let margin = RECT {
        left: 32,
        top: 20,
        right: 0,
        bottom: 0,
    };
    // RichEdit uses native keyboard navigation, IME and accessibility.
    SendMessageW(
        control,
        EM_SETMARGINS,
        (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
        ((margin.left << 16) | margin.left) as isize,
    );
    control
}

#[repr(C, packed(4))]
struct EditStream {
    cookie: usize,
    error: u32,
    callback: Option<unsafe extern "system" fn(usize, *mut u8, i32, *mut i32) -> u32>,
}
unsafe extern "system" fn stream_read(
    cookie: usize,
    dest: *mut u8,
    count: i32,
    written: *mut i32,
) -> u32 {
    let source = &mut *(cookie as *mut &[u8]);
    let len = source.len().min(count.max(0) as usize);
    std::ptr::copy_nonoverlapping(source.as_ptr(), dest, len);
    *written = len as i32;
    *source = &source[len..];
    0
}
pub(crate) unsafe fn set_rtf(hwnd: HWND, rtf: &str) -> Result<(), String> {
    if rtf.contains("\\pict") {
        crate::assets::enable_images(hwnd);
    }
    let mut bytes = rtf.as_bytes();
    let mut stream = EditStream {
        cookie: &mut bytes as *mut _ as usize,
        error: 0,
        callback: Some(stream_read),
    };
    SendMessageW(hwnd, EM_STREAMIN, 2, &mut stream as *mut _ as isize);
    if stream.error != 0 {
        Err("Markdown rendering failed".into())
    } else {
        Ok(())
    }
}

unsafe fn dialog(hwnd: HWND, save: bool, pdf: bool) -> Option<PathBuf> {
    let mut name = vec![0u16; 32768];
    let open_filter = format!(
        "Documents and images\0*.md;*.markdown;*.txt;*.pdf;{}\0All files\0*.*\0\0",
        crate::assets::EXTENSIONS
            .iter()
            .map(|ext| format!("*.{ext}"))
            .collect::<Vec<_>>()
            .join(";")
    );
    let filter = wide(if pdf {
        "PDF\0*.pdf\0\0"
    } else if save {
        "Markdown\0*.md\0Text\0*.txt\0All files\0*.*\0\0"
    } else {
        &open_filter
    });
    let ext = wide(if pdf { "pdf" } else { "md" });
    let mut ofn: OPENFILENAMEW = zeroed();
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = hwnd;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = name.as_mut_ptr();
    ofn.nMaxFile = name.len() as u32;
    ofn.lpstrDefExt = ext.as_ptr();
    ofn.Flags = OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };
    let ok = if save {
        GetSaveFileNameW(&mut ofn)
    } else {
        GetOpenFileNameW(&mut ofn)
    };
    if ok == 0 {
        let code = CommDlgExtendedError();
        if code != 0 {
            error(hwnd, &format!("File dialog failed: {code}"));
        }
        return None;
    }
    use std::os::windows::ffi::OsStringExt;
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Some(PathBuf::from(std::ffi::OsString::from_wide(&name[..end])))
}

#[repr(C, packed(4))]
struct FormatRange {
    dc: HDC,
    target: HDC,
    area: RECT,
    page: RECT,
    start: i32,
    end: i32,
}

pub(crate) fn export_complete(
    source: &str,
    output: &Path,
    base: Option<&Path>,
) -> Result<(), String> {
    let _ole = crate::assets::Ole::new()?;
    use std::{
        io::{Read, Seek, SeekFrom},
        os::windows::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };
    let temporary = output.with_file_name(format!(
        ".featherpad-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        unsafe {
            export_pdf(
                null_mut(),
                &markdown::with_images(source, 9000, false, base),
                &temporary,
                GetStockObject(DEFAULT_GUI_FONT),
            )?;
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            // The spooler may finish after EndDoc. Never replace the destination with a partial PDF.
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&temporary)
            {
                let size = file.metadata().map_err(|e| e.to_string())?.len();
                file.seek(SeekFrom::Start(size.saturating_sub(1024)))
                    .map_err(|e| e.to_string())?;
                let mut tail = Vec::new();
                file.read_to_end(&mut tail).map_err(|e| e.to_string())?;
                if tail.windows(5).any(|b| b == b"%%EOF") {
                    drop(file);
                    return document::replace_file(&temporary, output).map_err(|e| e.to_string());
                }
            }
            if Instant::now() >= deadline {
                return Err("PDF export timed out. Check the print queue.".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

unsafe fn export_pdf(parent: HWND, rtf: &str, output: &Path, font: HFONT) -> Result<(), String> {
    let control = rich_edit(parent, true, font);
    if control.is_null() {
        return Err("Could not create the layout control".into());
    }
    ShowWindow(control, SW_HIDE);
    let result = (|| {
        set_rtf(control, rtf)?;
        let dc = CreateDCW(
            wide("WINSPOOL").as_ptr(),
            wide("Microsoft Print to PDF").as_ptr(),
            null(),
            null(),
        );
        if dc.is_null() {
            return Err("Enable Microsoft Print to PDF in Windows and try again.".into());
        }
        let result = (|| {
            let destination = path_wide(output);
            let title = wide("FeatherPad Markdown");
            let info = DOCINFOW {
                cbSize: size_of::<DOCINFOW>() as i32,
                lpszDocName: title.as_ptr(),
                lpszOutput: destination.as_ptr(),
                lpszDatatype: null(),
                fwType: 0,
            };
            if StartDocW(dc, &info) <= 0 {
                return Err("Could not start PDF export. Check the path and print service.".into());
            }
            let dx = GetDeviceCaps(dc, LOGPIXELSX as i32).max(1);
            let dy = GetDeviceCaps(dc, LOGPIXELSY as i32).max(1);
            let w = GetDeviceCaps(dc, HORZRES as i32) * 1440 / dx;
            let h = GetDeviceCaps(dc, VERTRES as i32) * 1440 / dy;
            let length_options = [10u32, 1200u32]; // GTL_PRECISE | GTL_NUMCHARS, UTF-16; no CRLF expansion.
            let length =
                SendMessageW(control, WM_USER + 95, length_options.as_ptr() as usize, 0) as i32;
            let mut range = FormatRange {
                dc,
                target: dc,
                area: RECT {
                    left: 720,
                    top: 720,
                    right: w - 720,
                    bottom: h - 720,
                },
                page: RECT {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: h,
                },
                start: 0,
                end: length,
            };
            let mut success = true;
            loop {
                if StartPage(dc) <= 0 {
                    success = false;
                    break;
                }
                let next =
                    SendMessageW(control, EM_FORMATRANGE, 1, &range as *const _ as isize) as i32;
                if EndPage(dc) <= 0 || (next <= range.start && range.start < range.end) {
                    success = false;
                    break;
                }
                range.start = next;
                if range.start >= range.end {
                    break;
                }
            }
            SendMessageW(control, EM_FORMATRANGE, 0, 0);
            if success {
                if EndDoc(dc) <= 0 {
                    return Err("PDF printing did not finish. Check the print queue.".into());
                }
                Ok(())
            } else {
                AbortDoc(dc);
                Err("PDF export failed; output may be incomplete.".into())
            }
        })();
        DeleteDC(dc);
        result
    })();
    DestroyWindow(control);
    result
}

fn file_type(path: Option<&Path>) -> String {
    let ext = path
        .and_then(Path::extension)
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => "Markdown".into(),
        "" | "txt" | "log" => "Plain text".into(),
        "rs" => "Rust".into(),
        "py" | "pyw" => "Python".into(),
        "js" | "mjs" | "cjs" => "JavaScript".into(),
        "ts" | "tsx" => "TypeScript".into(),
        _ => ext.chars().take(16).collect::<String>().to_uppercase(),
    }
}
unsafe fn draw_terminal_button(draw: &DRAWITEMSTRUCT) {
    // Owner-draw is synchronous and can reenter while App is borrowed during layout.
    // Keep this path independent of App so every paint supplies the dark background.
    theme::fill(draw.hDC, draw.rcItem, theme::CANVAS);
    theme::label(
        draw.hDC,
        &if draw.CtlID == TERMINAL as u32 {
            ">_".into()
        } else {
            text(draw.hwndItem)
        },
        draw.rcItem,
        SendMessageW(draw.hwndItem, WM_GETFONT, 0, 0) as HFONT,
        theme::ACCENT,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    if draw.itemState & (ODS_FOCUS | ODS_SELECTED) != 0 {
        let middle = (draw.rcItem.left + draw.rcItem.right) / 2;
        theme::fill(
            draw.hDC,
            RECT {
                left: middle - 4,
                right: middle + 4,
                top: draw.rcItem.bottom - 2,
                bottom: draw.rcItem.bottom - 1,
            },
            theme::ACCENT,
        );
    }
}

#[test]
#[ignore = "Requires Windows GDI"]
fn native_terminal_button_paints_during_reentrant_layout() {
    unsafe {
        let button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("Terminal").as_ptr(),
            WS_POPUP | BS_OWNERDRAW as u32,
            0,
            0,
            36,
            24,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let screen = GetDC(button);
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, 36, 24);
        let old = SelectObject(dc, bitmap);
        SendMessageW(
            button,
            WM_SETFONT,
            GetStockObject(DEFAULT_GUI_FONT) as usize,
            0,
        );
        for state in [0, ODS_FOCUS, ODS_SELECTED] {
            let draw = DRAWITEMSTRUCT {
                CtlID: TERMINAL as u32,
                hwndItem: button,
                hDC: dc,
                rcItem: RECT {
                    left: 0,
                    top: 0,
                    right: 36,
                    bottom: 24,
                },
                itemState: state,
                ..zeroed()
            };
            theme::fill(dc, draw.rcItem, theme::WHITE);
            APP.with(|slot| {
                let _borrow = slot.borrow_mut();
                assert_eq!(
                    wndproc(
                        null_mut(),
                        WM_DRAWITEM,
                        TERMINAL,
                        &draw as *const _ as isize
                    ),
                    1
                );
            });
            for (x, y) in [(0, 0), (35, 0), (0, 23), (35, 23), (2, 12)] {
                assert_eq!(GetPixel(dc, x, y), theme::CANVAS);
            }
            assert!((8..28).any(|x| (4..20).any(|y| GetPixel(dc, x, y) != theme::CANVAS)));
        }
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        ReleaseDC(button, screen);
        DestroyWindow(button);
    }
    for (name, expected) in [
        ("notes.MD", "Markdown"),
        ("paper.pdf", "PDF"),
        ("photo.webp", "WEBP"),
        ("main.rs", "Rust"),
        ("data.json", "JSON"),
    ] {
        assert_eq!(file_type(Some(Path::new(name))), expected);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_DRAWITEM if !((lp as *const DRAWITEMSTRUCT).is_null()) => {
            let draw = &*(lp as *const DRAWITEMSTRUCT);
            if [TERMINAL as u32, KEEP_CURRENT as u32, USE_DISK as u32].contains(&draw.CtlID) {
                draw_terminal_button(draw);
                return 1;
            }
        }
        WM_SYSKEYUP if wp as u16 == VK_MENU => {
            PostMessageW(hwnd, DISPATCH, COMMANDS, 0);
            return 0;
        }
        WM_ERASEBKGND => {
            theme::fill(wp as HDC, theme::client(hwnd), theme::CANVAS);
            return 1;
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lp as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT { x: 820, y: 560 };
            return 0;
        }
        WM_COMMAND => {
            PostMessageW(
                hwnd,
                DISPATCH,
                if (wp >> 16) == EN_CHANGE as usize {
                    CHANGE
                } else {
                    wp & 0xffff
                },
                lp,
            );
            return 0;
        }
        WM_SIZE => {
            PostMessageW(hwnd, DISPATCH, LAYOUT, 0);
            return 0;
        }
        WM_CLOSE => {
            PostMessageW(hwnd, DISPATCH, EXIT, 0);
            return 0;
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return 0;
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let dc = wp as HDC;
            SetTextColor(
                dc,
                if GetDlgCtrlID(lp as HWND) == 904 {
                    theme::ACCENT
                } else {
                    theme::MUTED
                },
            );
            SetBkColor(dc, theme::CANVAS);
            SetDCBrushColor(dc, theme::CANVAS);
            return GetStockObject(DC_BRUSH) as isize;
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            // Creation and reentrant paints can precede an available App.
            theme::fill(dc, ps.rcPaint, theme::CANVAS);
            APP.with(|slot| {
                if let Ok(mut app) = slot.try_borrow_mut() {
                    if let Some(app) = app.as_mut() {
                        app.paint(dc, &ps.rcPaint);
                    }
                } else {
                    SetTimer(hwnd, 6, 30, None);
                }
            });
            EndPaint(hwnd, &ps);
            return 0;
        }
        _ => (),
    }
    let handled = APP.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            let timer = match msg {
                EXPORTED => 4,
                DISPATCH if wp == LAYOUT => 5,
                _ => 0,
            };
            if timer != 0 {
                SetTimer(hwnd, timer, 100, None);
            }
            return false;
        };
        let Some(app) = slot.as_mut() else {
            return false;
        };
        match msg {
            PASTE => {
                if app.pdf_path.is_none() && app.image.is_none() && app.large.is_none() {
                    if app.is_markdown() && crate::assets::available() {
                        match crate::assets::clipboard(hwnd) {
                            Ok(Some(paste)) => app.insert_image(paste),
                            Ok(None) => {
                                SendMessageW(app.edit, WM_USER + 64, 13, 0);
                            }
                            Err(e) => error(hwnd, &e),
                        }
                    } else {
                        SendMessageW(app.edit, WM_USER + 64, 13, 0);
                    }
                }
            }
            DISPATCH => {
                if wp != CHANGE || lp == app.edit as isize {
                    app.command(wp);
                }
            }
            EXPORTED => {
                app.exporting = false;
                let result = app.export_result.lock().unwrap().take();
                if let Some(result) = result {
                    match result {
                        Ok(path) => app.open(path),
                        Err(e) => {
                            app.status("Export failed");
                            error(hwnd, &e);
                        }
                    }
                }
            }
            WM_TIMER => {
                KillTimer(hwnd, wp);
                match wp {
                    10 => {
                        self::App::poll_external(app);
                        SetTimer(hwnd, 10, 400, None);
                    }
                    1 => app.refresh_preview(),
                    9 => {
                        if app.pdf_path.is_none() && app.image.is_none() && app.large.is_none() {
                            app.highlighter.update(app.edit, app.path.as_deref());
                        }
                    }
                    4 => {
                        PostMessageW(hwnd, EXPORTED, 0, 0);
                    }
                    5 => app.layout(),
                    6 => theme::invalidate(hwnd),
                    8 => {
                        app.status("");
                    }
                    7 => {
                        let target = if app.terminal_open {
                            (theme::client(hwnd).right / 3).clamp(320, 520)
                        } else {
                            0
                        };
                        let delta = target - app.terminal_width;
                        if delta.abs() <= 3 {
                            app.terminal_width = target;
                            if !app.preview.is_null()
                                && app.pdf_path.is_none()
                                && app.image.is_none()
                            {
                                SetTimer(hwnd, 1, 350, None);
                            }
                            if !app.terminal_open {
                                if let Some(t) = &app.terminal {
                                    ShowWindow(t.0, SW_HIDE);
                                    SetFocus(app.large.as_ref().map_or_else(
                                        || {
                                            app.viewer
                                                .as_ref()
                                                .filter(|_| app.pdf_path.is_some())
                                                .map_or_else(
                                                    || {
                                                        app.image
                                                            .as_ref()
                                                            .map_or(app.edit, |view| view.0)
                                                    },
                                                    |reader| reader.0,
                                                )
                                        },
                                        |view| view.0,
                                    ));
                                }
                            }
                        } else {
                            app.terminal_width += (delta as f32 * 0.4).round() as i32;
                            SetTimer(hwnd, 7, 15, None);
                        }
                        app.layout();
                    }
                    _ => (),
                }
            }
            WM_DROPFILES => {
                let drop = wp as HDROP;
                let len = DragQueryFileW(drop, 0, null_mut(), 0);
                let mut name = vec![0u16; len as usize + 1];
                DragQueryFileW(drop, 0, name.as_mut_ptr(), name.len() as u32);
                DragFinish(drop);
                use std::os::windows::ffi::OsStringExt;
                let path = PathBuf::from(std::ffi::OsString::from_wide(&name[..len as usize]));
                if app.is_markdown()
                    && app.path.is_some()
                    && GetKeyState(VK_SHIFT as i32) >= 0
                    && app.pdf_path.is_none()
                    && app.image.is_none()
                    && app.large.is_none()
                    && crate::assets::supported(&path)
                {
                    app.insert_image(crate::assets::Paste::File(path));
                } else {
                    app.open(path);
                }
            }
            scroll::SYNC => {
                // Only a user-driven source owns the paired scroll; programmatic updates do not echo.
                app.sync_scroll(wp as HWND);
                SetTimer(hwnd, 9, 80, None);
            }
            _ => return false,
        }
        true
    });
    if handled {
        if msg == WM_DRAWITEM {
            1
        } else {
            0
        }
    } else {
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

pub fn run() {
    let _ole = match crate::assets::Ole::new() {
        Ok(ole) => ole,
        Err(e) => {
            unsafe {
                error(null_mut(), &e);
            }
            return;
        }
    };
    unsafe {
        SetProcessDPIAware();
        let instance = GetModuleHandleW(null());
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        if library.is_null() {
            error(null_mut(), "Could not load RichEdit");
            return;
        }
        let common = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TREEVIEW_CLASSES,
        };
        InitCommonControlsEx(&common);
        let icon = LoadImageW(
            instance,
            std::ptr::without_provenance::<u16>(1),
            IMAGE_ICON,
            64,
            64,
            LR_SHARED,
        ) as HICON;
        let class = wide("FeatherPadWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: icon,
            hbrBackground: CreateSolidBrush(theme::CANVAS),
            ..zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            error(null_mut(), "Could not register the window");
            return;
        }
        let hwnd = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            class.as_ptr(),
            wide("FeatherPad").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            780,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            error(null_mut(), "Could not create the window");
            return;
        }
        let fonts = theme::Fonts::new();
        let font = fonts.body;
        let caption = theme::CANVAS;
        let caption_text = theme::INK;
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            35,
            &caption as *const _ as _,
            4,
        );
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            36,
            &caption_text as *const _ as _,
            4,
        );
        let edit = rich_edit(hwnd, false, font);
        if edit.is_null() {
            error(hwnd, "Could not create the editor");
            DestroyWindow(hwnd);
            return;
        }
        let status = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | 0x100 | 0xc000,
            0,
            0,
            1,
            1,
            hwnd,
            COMMANDS as _,
            instance,
            null(),
        );
        SendMessageW(status, WM_SETFONT, fonts.small as usize, 0);
        let file_type = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("Plain text").as_ptr(),
            WS_CHILD | WS_VISIBLE | 2, // SS_RIGHT
            0,
            0,
            120,
            24,
            hwnd,
            904usize as _,
            instance,
            null(),
        );
        SendMessageW(file_type, WM_SETFONT, fonts.small as usize, 0);
        let terminal_button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("Terminal").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
            0,
            0,
            36,
            24,
            hwnd,
            TERMINAL as _,
            instance,
            null(),
        );
        SendMessageW(terminal_button, WM_SETFONT, fonts.small as usize, 0);
        let compare_actions =
            [(KEEP_CURRENT, "Keep current"), (USE_DISK, "Use disk")].map(|(id, title)| {
                let button = CreateWindowExW(
                    0,
                    wide("BUTTON").as_ptr(),
                    wide(title).as_ptr(),
                    WS_CHILD | WS_TABSTOP | BS_OWNERDRAW as u32,
                    0,
                    0,
                    128,
                    26,
                    hwnd,
                    id as _,
                    instance,
                    null(),
                );
                SendMessageW(button, WM_SETFONT, fonts.small as usize, 0);
                button
            });
        let palette = palette::Palette::create(
            hwnd,
            fonts.ui,
            fonts.small,
            vec![
                (OPEN, "Open", "Ctrl+O", "open file pdf markdown image photo"),
                (NEW, "New", "Ctrl+N", "new"),
                (SAVE, "Save", "Ctrl+S", "save"),
                (SAVE_AS, "Save as", "Ctrl+Shift+S", "save as"),
                (PREVIEW, "Preview", "Ctrl+Shift+M", "preview"),
                (EXPORT, "Export PDF", "Ctrl+P", "export print"),
                (TOC, "Outline", "Ctrl+Shift+L", "outline sidebar"),
                (FIT, "Fit width", "Ctrl+0", "fit"),
                (ZOOM_IN, "Zoom in", "Ctrl +", "zoom in"),
                (ZOOM_OUT, "Zoom out", "Ctrl -", "zoom out"),
                (FONT_MODE, "Font", "", "font segoe consolas"),
                (TEXT_LARGER, "Larger text", "", "font larger size"),
                (TEXT_SMALLER, "Smaller text", "", "font smaller size"),
                (EDITOR, "Editor", "Ctrl+E", "editor"),
                (202, "Refresh", "Ctrl+Shift+R", "refresh"),
                (BOLD, "Bold", "Ctrl+B", "bold"),
                (ITALIC, "Italic", "Ctrl+I", "italic"),
                (CODE, "Inline code", "Ctrl+K", "code"),
                (TERMINAL, "Terminal", "Ctrl+J", "shell powershell"),
                (
                    IMAGE,
                    "Insert image",
                    "Ctrl+Shift+I",
                    "picture photo png jpeg",
                ),
                (EXIT, "Quit", "Ctrl+Q", "exit"),
            ],
            DISPATCH,
        );
        APP.with(|slot| {
            *slot.borrow_mut() = Some(App {
                hwnd,
                edit,
                highlighter: crate::syntax::Highlighter::default(),
                preview: null_mut(),
                status,
                file_type,
                terminal_button,
                terminal: None,
                terminal_open: false,
                terminal_width: 0,
                font,
                path: None,
                encoding: Encoding::Utf8,
                crlf: true,
                original: None,
                watch: None,
                comparison: None,
                compare_actions,
                preview_before_compare: false,
                image_path: None,
                image: None,
                pdf_path: None,
                large_path: None,
                large: None,
                viewer: None,
                fonts,
                palette,
                monospace: false,
                text_zoom: 100,
                buffer: theme::Buffer::default(),
                exporting: false,
                export_result: Arc::new(Mutex::new(None)),
            })
        });
        APP.with(|slot| slot.borrow_mut().as_mut().unwrap().show_editor());
        ShowWindow(hwnd, SW_SHOW);
        // Submit a complete dark frame before synchronous file loading starts.
        RedrawWindow(
            hwnd,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
        SetTimer(hwnd, 10, 400, None);
        if let Some(path) = std::env::args_os().nth(1) {
            APP.with(|slot| {
                slot.borrow_mut()
                    .as_mut()
                    .unwrap()
                    .open(PathBuf::from(path))
            });
        }
        let mut msg: MSG = zeroed();
        loop {
            let status = GetMessageW(&mut msg, null_mut(), 0, 0);
            if status <= 0 {
                break;
            }
            if msg.message == WM_SYSKEYUP && msg.wParam == VK_MENU as usize {
                SendMessageW(hwnd, DISPATCH, COMMANDS, 0);
                continue;
            }
            if APP.with(|s| s.borrow().as_ref().is_some_and(|a| a.palette.key(&msg))) {
                continue;
            }
            let in_terminal = APP.with(|s| {
                s.borrow()
                    .as_ref()
                    .is_some_and(|a| a.terminal.as_ref().is_some_and(|t| t.contains(msg.hwnd)))
            });
            if msg.message == WM_KEYDOWN {
                let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
                let shift = GetKeyState(VK_SHIFT as i32) < 0;
                let pdf_mode = APP.with(|s| {
                    s.borrow()
                        .as_ref()
                        .is_some_and(|a| a.pdf_path.is_some() || a.image.is_some())
                });
                let command = shortcut(msg.wParam as u16, ctrl, shift, pdf_mode, in_terminal);
                if command != 0 {
                    SendMessageW(hwnd, DISPATCH, command, 0);
                    continue;
                }
                let in_editor =
                    APP.with(|s| s.borrow().as_ref().is_some_and(|a| msg.hwnd == a.edit));
                if in_editor
                    && ((ctrl && msg.wParam == 0x56) || (shift && msg.wParam == VK_INSERT as usize))
                {
                    PostMessageW(hwnd, PASTE, 0, 0);
                    continue;
                }
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|s| {
            s.borrow_mut().take();
        });
        FreeLibrary(library);
    }
}

#[test]
#[ignore = "Requires Windows desktop, Print Spooler and Microsoft Print to PDF"]
fn native_markdown_pdf_roundtrip() {
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        assert!(!library.is_null());
        let control = rich_edit(null_mut(), false, GetStockObject(DEFAULT_GUI_FONT));
        assert!(!control.is_null());
        assert_ne!(
            SetWindowTextW(control, wide("中文 😀\r\nline 2").as_ptr()),
            0
        );
        assert_eq!(
            text(control).replace("\r\n", "\n").replace('\r', "\n"),
            "中文 😀\nline 2"
        );
        DestroyWindow(control);
        let control = rich_edit(null_mut(), true, GetStockObject(DEFAULT_GUI_FONT));
        let source = include_str!("../examples/welcome.md");
        set_rtf(control, &markdown::rtf(source, 9000)).unwrap();
        let rendered = text(control);
        assert!(rendered.contains("FeatherPad"), "{rendered}");
        assert!(rendered.contains("Hello, world"));
        assert!(!rendered.contains("**加粗**"));
        DestroyWindow(control);
        let dir = std::env::current_dir().unwrap().join("tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("smoke.pdf");
        let long_source = format!(
            "{source}\n\n{}",
            "分页验证：这是一段中文和 English 的混合文本，用于验证长文档分页。\n\n".repeat(65)
        );
        export_complete(&long_source, &path, None).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
        let worker = pdf::Worker::new(0);
        let mut count = 1;
        for index in 0..10 {
            if index >= count {
                break;
            }
            worker.request(pdf::Request {
                generation: index as u64 + 1,
                document_id: 1,
                path: path.clone(),
                jobs: vec![(index, 900)],
            });
            let start = std::time::Instant::now();
            let page = loop {
                if let Ok(reply) = worker.results.try_recv() {
                    match reply {
                        pdf::Reply::Page(_, generation, page) => {
                            assert_eq!(generation, index as u64 + 1);
                            break page;
                        }
                        pdf::Reply::Error(_, e) => panic!("{e}"),
                        _ => (),
                    }
                }
                assert!(start.elapsed().as_secs() < 30, "PDF render timed out");
                std::thread::sleep(std::time::Duration::from_millis(30));
            };
            count = page.count;
            assert_eq!(page.index, index);
            assert!(page
                .pixels
                .chunks_exact(4)
                .any(|p| p[0] < 200 && p[1] < 200 && p[2] < 200));
            let mut bmp = Vec::new();
            bmp.extend(b"BM");
            bmp.extend((54 + page.pixels.len() as u32).to_le_bytes());
            bmp.extend([0u8; 4]);
            bmp.extend(54u32.to_le_bytes());
            bmp.extend(40u32.to_le_bytes());
            bmp.extend((page.width as i32).to_le_bytes());
            bmp.extend((-(page.height as i32)).to_le_bytes());
            bmp.extend(1u16.to_le_bytes());
            bmp.extend(32u16.to_le_bytes());
            bmp.extend([0u8; 24]);
            bmp.extend(&page.pixels);
            std::fs::write(dir.join(format!("smoke-page-{}.bmp", index + 1)), bmp).unwrap();
        }
        assert!(
            (2..=10).contains(&count),
            "Unexpected PDF page count: {count}"
        );
        println!("Exported and rendered {count} pages: {}", path.display());
        FreeLibrary(library);
    }
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_startup_background_is_dark_before_app_is_ready() {
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let instance = GetModuleHandleW(null());
        let name = wide("FeatherPadStartupPaintTest");
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..zeroed()
        };
        assert_ne!(RegisterClassW(&class), 0);
        let parent = CreateWindowExW(
            0,
            name.as_ptr(),
            name.as_ptr(),
            WS_POPUP,
            0,
            0,
            320,
            200,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        assert!(!parent.is_null());
        let dc = GetDC(parent);
        let mut buffer = theme::Buffer::default();
        assert!(buffer.ensure(dc, 320, 200));
        let rect = RECT {
            left: 0,
            top: 0,
            right: 320,
            bottom: 200,
        };
        // No App exists yet; a native erase must still replace a white surface.
        theme::fill(buffer.dc, rect, theme::WHITE);
        assert_eq!(
            SendMessageW(parent, WM_ERASEBKGND, buffer.dc as usize, 0),
            1
        );
        assert_eq!(GetPixel(buffer.dc, 160, 100), theme::CANVAS);
        let edit = rich_edit(parent, false, GetStockObject(DEFAULT_GUI_FONT));
        assert!(!edit.is_null());
        assert_eq!(GetWindowLongW(edit, GWL_STYLE) as u32 & WS_VISIBLE, 0);
        MoveWindow(edit, 0, 0, 320, 200, 0);
        theme::fill(buffer.dc, rect, theme::WHITE);
        SendMessageW(
            edit,
            WM_PRINTCLIENT,
            buffer.dc as usize,
            PRF_CLIENT as isize,
        );
        assert_eq!(GetPixel(buffer.dc, 160, 100), theme::CANVAS);
        ReleaseDC(parent, dc);
        DestroyWindow(parent);
        UnregisterClassW(name.as_ptr(), instance);
        FreeLibrary(library);
    }
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_selection_overlay_preserves_document() {
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let fonts = theme::Fonts::new();
        let edit = rich_edit(null_mut(), false, fonts.code);
        MoveWindow(edit, 0, 0, 600, 400, 0);
        let rect = RECT {
            left: 16,
            top: 16,
            right: 560,
            bottom: 380,
        };
        SendMessageW(edit, EM_SETRECT, 0, &rect as *const _ as isize);
        theme::editor_colors(edit);
        theme::dark_scrollbars(edit, theme::CANVAS);
        ShowWindow(edit, SW_SHOW);
        SetFocus(edit);
        SetWindowTextW(edit, wide("Selection 中文 123\r\nSecond line").as_ptr());
        SendMessageW(edit, EM_EMPTYUNDOBUFFER, 0, 0);
        SendMessageW(edit, EM_SETMODIFY, 0, 0);
        SendMessageW(edit, EM_SETSEL, 0, 8);
        let before = text(edit);
        let screen = GetDC(edit);
        let mut image = theme::Buffer::default();
        let mut tint = theme::Buffer::default();
        assert!(image.ensure(screen, 600, 400));
        theme::fill(image.dc, rect, theme::CANVAS);
        SendMessageW(edit, WM_PRINTCLIENT, image.dc as usize, PRF_CLIENT as isize);
        crate::selection::paint(edit, image.dc, &mut tint);
        let mut pos: POINT = zeroed();
        SendMessageW(edit, EM_POSFROMCHAR, &mut pos as *mut _ as usize, 1);
        assert_ne!(GetPixel(image.dc, pos.x, pos.y + 4), theme::CANVAS);
        assert_eq!(text(edit), before);
        assert_eq!(SendMessageW(edit, EM_GETMODIFY, 0, 0), 0);
        assert_eq!(SendMessageW(edit, EM_CANUNDO, 0, 0), 0);
        let (mut a, mut b) = (0u32, 0u32);
        SendMessageW(
            edit,
            EM_GETSEL,
            &mut a as *mut _ as usize,
            &mut b as *mut _ as isize,
        );
        assert_eq!((a, b), (0, 8));
        ReleaseDC(edit, screen);
        DestroyWindow(edit);
        FreeLibrary(library);
    }
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_block_caret_tracks_input_and_font() {
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let fonts = theme::Fonts::new();
        let large_font = theme::font(32, 400, "Consolas");
        let edit = rich_edit(null_mut(), false, fonts.code);
        MoveWindow(edit, 0, 0, 600, 400, 0);
        let rect = RECT {
            left: 8,
            top: 8,
            right: 580,
            bottom: 380,
        };
        SendMessageW(edit, EM_SETRECT, 0, &rect as *const _ as isize);
        ShowWindow(edit, SW_SHOW);
        SetFocus(edit);
        let source = "iW中1😀e\u{301} ";
        SetWindowTextW(edit, wide(source).as_ptr());
        SendMessageW(edit, EM_SETMODIFY, 0, 0);
        for font in [fonts.code, fonts.body, large_font] {
            SendMessageW(edit, WM_SETFONT, font as usize, 1);
            SendMessageW(edit, EM_SETMODIFY, 0, 0);
            for zoom in [100, 150] {
                SendMessageW(edit, WM_USER + 225, zoom, 100);
                for (cp, next) in [(0, 1), (1, 2), (2, 3), (3, 4), (4, 6), (6, 8)] {
                    SendMessageW(edit, EM_SETSEL, cp, cp as isize);
                    SetFocus(null_mut());
                    SetFocus(edit);
                    let (mut start, mut end): (POINT, POINT) = (zeroed(), zeroed());
                    SendMessageW(
                        edit,
                        EM_POSFROMCHAR,
                        &mut start as *mut _ as usize,
                        cp as isize,
                    );
                    SendMessageW(edit, EM_POSFROMCHAR, &mut end as *mut _ as usize, next);
                    let mut info: GUITHREADINFO = zeroed();
                    info.cbSize = size_of::<GUITHREADINFO>() as u32;
                    assert_ne!(
                        GetGUIThreadInfo(GetWindowThreadProcessId(edit, null_mut()), &mut info),
                        0
                    );
                    assert_eq!(info.hwndCaret, edit);
                    assert_eq!(info.rcCaret.left, start.x, "caret start at {cp}");
                    assert_eq!(
                        info.rcCaret.right - info.rcCaret.left,
                        end.x - start.x,
                        "character width at {cp}, zoom {zoom}"
                    );
                }
            }
        }
        assert_eq!(text(edit), source);
        assert_eq!(SendMessageW(edit, EM_GETMODIFY, 0, 0), 0);
        for (start, end) in [(0, 6), (6, 0), (2, 4)] {
            SendMessageW(edit, EM_SETSEL, start, end);
            let (mut a, mut b) = (0u32, 0u32);
            SendMessageW(
                edit,
                EM_GETSEL,
                &mut a as *mut _ as usize,
                &mut b as *mut _ as isize,
            );
            assert_eq!(
                (a, b),
                (
                    (start as u32).min(end as u32),
                    (start as u32).max(end as u32)
                )
            );
            let mut caret: GUITHREADINFO = zeroed();
            caret.cbSize = size_of::<GUITHREADINFO>() as u32;
            GetGUIThreadInfo(GetWindowThreadProcessId(edit, null_mut()), &mut caret);
            assert!(
                caret.flags & GUI_CARETBLINKING == 0
                    || caret.rcCaret.right - caret.rcCaret.left <= 2,
                "Selected text must not have an overlapping block caret"
            );
        }
        SendMessageW(edit, EM_SETSEL, 2, 2);
        SendMessageW(edit, WM_IME_STARTCOMPOSITION, 0, 0);
        let mut info: GUITHREADINFO = zeroed();
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        GetGUIThreadInfo(GetWindowThreadProcessId(edit, null_mut()), &mut info);
        if info.flags & GUI_CARETBLINKING != 0 {
            assert_eq!(info.rcCaret.right - info.rcCaret.left, 2);
        }
        SendMessageW(edit, WM_IME_ENDCOMPOSITION, 0, 0);
        SetWindowTextW(edit, wide("abc").as_ptr());
        SendMessageW(edit, EM_SETSEL, 1, 1);
        SendMessageW(edit, WM_CHAR, 'x' as usize, 0);
        assert_eq!(text(edit), "axbc");
        SendMessageW(edit, WM_UNDO, 0, 0);
        assert_eq!(text(edit), "abc");
        // Passive traffic must leave the current caret lifetime and geometry
        // alone; recreating it restarts the system blink cycle.
        let blink_time = GetCaretBlinkTime();
        RedrawWindow(edit, null(), null_mut(), RDW_INVALIDATE | RDW_UPDATENOW);
        assert_ne!(CreateCaret(edit, null_mut(), 3, 20), 0);
        SetCaretPos(20, 20);
        ShowCaret(edit);
        let dc = GetDC(edit);
        let mut buffer = theme::Buffer::default();
        assert!(buffer.ensure(dc, 600, 400));
        for _ in 0..8 {
            for (message, w, l) in [
                (WM_GETTEXTLENGTH, 0, 0),
                (WM_GETFONT, 0, 0),
                (WM_PRINTCLIENT, buffer.dc as usize, PRF_CLIENT as isize),
                (WM_MOUSEMOVE, 0, 0),
            ] {
                SendMessageW(edit, message, w, l);
                let mut idle: GUITHREADINFO = zeroed();
                idle.cbSize = size_of::<GUITHREADINFO>() as u32;
                GetGUIThreadInfo(GetWindowThreadProcessId(edit, null_mut()), &mut idle);
                assert_eq!(
                    idle.rcCaret.right - idle.rcCaret.left,
                    3,
                    "Idle message {message:x} must not replace the caret"
                );
            }
        }
        assert_eq!(GetCaretBlinkTime(), blink_time);
        ReleaseDC(edit, dc);
        DestroyWindow(edit);
        DeleteObject(large_font);
        FreeLibrary(library);
    }
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_highlighting_preserves_text_selection_scroll_and_undo() {
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let fonts = theme::Fonts::new();
        let edit = rich_edit(null_mut(), false, fonts.code);
        MoveWindow(edit, 0, 0, 600, 400, 0);
        let source = "# 中文 😀\r\n\r\n```rust\r\nlet value = 42; // comment\r\n```\r\n";
        SetWindowTextW(edit, wide(source).as_ptr());
        SendMessageW(edit, EM_EMPTYUNDOBUFFER, 0, 0);
        SendMessageW(edit, EM_SETMODIFY, 0, 0);
        SendMessageW(edit, EM_SETSEL, 2, 4);
        let mut h = crate::syntax::Highlighter::default();
        let before = text(edit);
        h.update(edit, Some(Path::new("test.md")));
        assert_eq!(text(edit), before);
        assert_eq!(SendMessageW(edit, EM_GETMODIFY, 0, 0), 0);
        assert_eq!(SendMessageW(edit, EM_CANUNDO, 0, 0), 0);
        let mut a = 0u32;
        let mut b = 0u32;
        SendMessageW(
            edit,
            EM_GETSEL,
            &mut a as *mut _ as usize,
            &mut b as *mut _ as isize,
        );
        assert_eq!((a, b), (2, 4));
        let doc = crate::syntax::document(edit).unwrap();
        assert_eq!(
            doc.Range(2, 4)
                .unwrap()
                .GetFont()
                .unwrap()
                .GetForeColor()
                .unwrap(),
            theme::ACCENT as i32
        );
        SendMessageW(edit, EM_SETSEL, 0, 0);
        SendMessageW(edit, EM_REPLACESEL, 1, wide("X").as_ptr() as isize);
        let edited = text(edit);
        h.clear();
        h.update(edit, Some(Path::new("test.md")));
        assert_ne!(SendMessageW(edit, EM_CANUNDO, 0, 0), 0);
        SendMessageW(edit, WM_UNDO, 0, 0);
        assert_eq!(
            text(edit),
            before,
            "Highlighting must not consume the user's undo"
        );
        SendMessageW(edit, WM_USER + 84, 0, 0); // EM_REDO
        assert_eq!(text(edit), edited);
        // A viewport update on a long source must neither move the caret nor scroll.
        SetWindowTextW(
            edit,
            wide(&"# heading 中文 😀\r\nlet x = 42;\r\n".repeat(4000)).as_ptr(),
        );
        SendMessageW(edit, EM_GETLINECOUNT, 0, 0);
        let mut point = POINT { x: 0, y: 20000 };
        SendMessageW(edit, WM_USER + 222, 0, &point as *const _ as isize);
        SendMessageW(edit, WM_USER + 221, 0, &mut point as *mut _ as isize);
        h.clear();
        h.update(edit, Some(Path::new("test.md")));
        let mut after: POINT = zeroed();
        SendMessageW(edit, WM_USER + 221, 0, &mut after as *mut _ as isize);
        assert_eq!(point.y, after.y);
        drop(doc);
        DestroyWindow(edit);
        FreeLibrary(library);
    }
}

fn shortcut(key: u16, ctrl: bool, shift: bool, pdf: bool, terminal: bool) -> usize {
    match (ctrl, shift, key) {
        (true, _, VK_OEM_3 | 0x4a) => TERMINAL,
        (true, _, 0x4e) => NEW,
        (true, _, 0x4f) => OPEN,
        (true, true, 0x53) => SAVE_AS,
        (true, false, 0x53) => SAVE,
        (true, true, 0x50) => COMMANDS,
        (true, false, 0x51) => EXIT,
        (true, false, 0x50) => EXPORT,
        (true, _, 0x45) => EDITOR,
        (true, true, 0x49) => IMAGE,
        (true, _, 0x42) if !terminal => BOLD,
        (true, false, 0x49) if !terminal => ITALIC,
        (true, _, 0x4b) if !terminal => CODE,
        (true, true, 0x4d) => PREVIEW,
        (true, true, 0x52) => 202,
        (true, true, 0x4c) if pdf => TOC,
        (false, _, VK_PRIOR) if pdf && !terminal => PREV,
        (false, _, VK_NEXT) if pdf && !terminal => NEXT,
        (true, _, VK_OEM_PLUS | VK_ADD) if pdf => ZOOM_IN,
        (true, _, VK_OEM_MINUS | VK_SUBTRACT) if pdf => ZOOM_OUT,
        (true, _, 0x30) if pdf => FIT,
        _ => 0,
    }
}
unsafe fn image_dialog(hwnd: HWND) -> Option<PathBuf> {
    let mut name = vec![0u16; 32768];
    let filter = wide(&crate::assets::dialog_filter());
    let mut ofn: OPENFILENAMEW = zeroed();
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = hwnd;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = name.as_mut_ptr();
    ofn.nMaxFile = name.len() as u32;
    ofn.Flags = OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_FILEMUSTEXIST;
    if GetOpenFileNameW(&mut ofn) == 0 {
        return None;
    }
    use std::os::windows::ffi::OsStringExt;
    let n = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Some(PathBuf::from(std::ffi::OsString::from_wide(&name[..n])))
}
#[test]
fn shortcuts_work_with_terminal_focus() {
    for terminal in [false, true] {
        assert_eq!(shortcut(0x50, true, true, false, terminal), COMMANDS);
        assert_eq!(shortcut(0x4a, true, false, false, terminal), TERMINAL);
        assert_eq!(shortcut(0x53, true, false, false, terminal), SAVE);
        assert_eq!(shortcut(0x4d, true, true, false, terminal), PREVIEW);
        assert_eq!(shortcut(0x52, true, true, false, terminal), 202);
        assert_eq!(shortcut(0x4c, true, true, true, terminal), TOC);
        assert_eq!(shortcut(0x4c, true, true, false, terminal), 0);
        assert_eq!(shortcut(0x51, true, false, false, terminal), EXIT);
        assert_eq!(shortcut(0x49, true, true, false, terminal), IMAGE);
        for key in VK_F1..=VK_F24 {
            for (ctrl, shift) in [(false, false), (false, true), (true, false), (true, true)] {
                assert_eq!(shortcut(key, ctrl, shift, true, terminal), 0);
            }
        }
    }
    assert_eq!(shortcut(0x43, true, false, false, true), 0); // Shell Ctrl+C stays native.
    assert_eq!(shortcut(0x56, true, false, false, true), 0); // Shell paste stays native.
    assert_eq!(shortcut(0x49, true, false, false, false), ITALIC);
}
