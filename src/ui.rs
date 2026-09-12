#[cfg(test)]
use crate::pdf;
use crate::{
    document::{self, Encoding},
    markdown, palette, reader, scroll, terminal, theme,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        mpsc::{self, Receiver, TryRecvError},
        Arc, Mutex,
    },
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
const OVERVIEW: usize = 126;
const OPEN_FOLDER: usize = 127;
const CLOSE_FOLDER: usize = 128;
const TOGGLE_FILES: usize = 130;
const PRINT: usize = 131;
const CANCEL_PRINT: usize = 132;
const FIT_PAGE: usize = 133;
const TEXT_RESET: usize = 134;
const SEARCH: usize = 135;
const FIND: usize = 136;
const REFRESH_FILES: usize = 129;
pub const PASTE: u32 = WM_APP + 8;
const FOLD_HEADING: u32 = WM_APP + 9;
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
pub(crate) fn path_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
unsafe fn error(hwnd: HWND, text: &str) {
    MessageBoxW(
        hwnd,
        wide(text).as_ptr(),
        wide("PlumeTxt").as_ptr(),
        MB_OK | MB_ICONERROR,
    );
}
unsafe fn text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd).max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let read = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
    String::from_utf16_lossy(&buf[..read])
}

struct LoadedText {
    units: Vec<u16>,
    encoding: Encoding,
    crlf: bool,
    original: Option<u64>,
    chunk: Option<document::Chunk>,
}
struct Loading {
    receiver: Receiver<Result<LoadedText, String>>,
    data: Option<LoadedText>,
    inserted: usize,
    batch: usize,
}

unsafe fn append_load_batch(
    edit: HWND,
    units: &[u16],
    start: usize,
    batch: usize,
) -> Result<usize, String> {
    // Insert through TOM without moving the caret to EOF (which forces full-document layout).
    let mut end = (start + batch).min(units.len());
    if end < units.len()
        && ((0xd800..=0xdbff).contains(&units[end - 1])
            || (units[end - 1] == 13 && units[end] == 10))
    {
        end -= 1;
    }
    if start == end {
        return Ok(end);
    }
    let doc = crate::syntax::document(edit).ok_or("Text services unavailable")?;
    use windows::Win32::UI::Controls::RichEdit::{tomResume, tomSuspend};
    doc.Undo(tomSuspend.0).map_err(|e| e.to_string())?;
    SendMessageW(edit, EM_SETREADONLY, 0, 0);
    let result = (|| -> windows_core::Result<()> {
        let selection = doc.GetSelection()?;
        let a = selection.GetStart()?;
        let b = selection.GetEnd()?;
        let length = doc.Range(0, 0)?.GetStoryLength()?.saturating_sub(1);
        doc.Range(length, length)?
            .SetText(&windows_core::BSTR::from_wide(&units[start..end]))?;
        selection.SetRange(a, b)?;
        Ok(())
    })();
    SendMessageW(edit, EM_SETREADONLY, 1, 0);
    let _ = doc.Undo(tomResume.0);
    theme::invalidate(edit);
    result.map(|()| end).map_err(|e| e.to_string())
}

type PreviewResult = (u64, Receiver<Result<String, String>>);

struct App {
    search: Option<crate::search::Search>,
    search_visible: bool,
    search_hit: Option<crate::search::Hit>,
    workspace: Option<crate::workspace::Workspace>,
    workspace_width: i32,
    workspace_visible: bool,
    workspace_drag: Option<theme::Divider>,
    folded: RefCell<BTreeSet<usize>>,
    fold_revision: Cell<u64>,
    hwnd: HWND,
    dpi: u32,
    edit: HWND,
    highlighter: crate::syntax::Highlighter,
    preview: HWND,
    preview_only: bool,
    preview_job: RefCell<Option<PreviewResult>>,
    preview_version: Cell<u64>,
    preview_pending: Cell<bool>,
    status: HWND,
    commands_button: HWND,
    status_message: RefCell<String>,
    file_type: HWND,
    terminal_button: HWND,
    terminal: Option<terminal::Terminal>,
    terminal_open: bool,
    terminal_width: i32,
    terminal_started: std::time::Instant,
    terminal_from: f64,
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
    printing: Option<crate::printing::Job>,
    exporting: bool,
    export_result: Arc<Mutex<Option<Result<PathBuf, String>>>>,
    loading: Option<Loading>,
    chunk: Option<document::Chunk>,
    saving: Option<Receiver<Result<(PathBuf, u64), String>>>,
}

impl App {
    unsafe fn focus_content(&self) {
        let target = if self.pdf_path.is_some() {
            self.viewer.as_ref().map_or(self.hwnd, |v| v.0)
        } else if let Some(image) = &self.image {
            image.0
        } else if let Some(large) = &self.large {
            large.0
        } else if self.preview_only && !self.preview.is_null() {
            self.preview
        } else {
            self.edit
        };
        SetFocus(target);
    }
    fn sidebar_visible(&self) -> bool {
        self.workspace_visible
            && if self.search_visible {
                self.search.is_some()
            } else {
                self.workspace.is_some()
            }
    }
    unsafe fn search_in(&mut self, path: PathBuf) {
        if !self.search.as_ref().is_some_and(|s| s.is_scope(&path)) {
            self.search.take();
            self.workspace_width = self.workspace_width.max(theme::px(self.hwnd, 340));
            self.search = Some(crate::search::Search::create(
                self.hwnd,
                path,
                self.fonts.small,
            ));
        }
        self.search_visible = true;
        self.workspace_visible = true;
        self.layout();
        self.search.as_ref().unwrap().focus();
    }
    unsafe fn select_search_result(&mut self, selection: Option<(i32, i32)>) {
        self.preview_only = false;
        self.show_editor();
        if let Some((start, end)) = selection {
            SendMessageW(self.edit, EM_SETSEL, start as usize, end as isize);
            SendMessageW(self.edit, EM_SCROLLCARET, 0, 0);
            theme::invalidate(self.edit);
        } else {
            self.status("File changed · Search again");
        }
    }
    unsafe fn open_search_result(&mut self, hit: crate::search::Hit) {
        if let Some(pdf) = hit.pdf {
            if self.pdf_path.as_ref() != Some(&hit.path) {
                self.open(hit.path.clone());
            }
            if self.pdf_path.as_ref() == Some(&hit.path) {
                if let Some(viewer) = &self.viewer {
                    viewer.highlight(pdf);
                }
            }
            return;
        }
        if self.path.as_ref() == Some(&hit.path)
            && self.chunk.is_none()
            && self.loading.is_none()
            && self.pdf_path.is_none()
            && self.image.is_none()
            && self.large.is_none()
        {
            self.select_search_result(hit.current_selection(&text(self.edit)));
            return;
        }
        if !self.confirm_save() {
            return;
        }
        self.cancel_loading();
        let offset = std::fs::metadata(&hit.path)
            .ok()
            .filter(|m| m.len() > crate::large::THRESHOLD)
            .map(|_| hit.offset.saturating_sub(4096));
        self.begin_load(hit.path.clone(), offset);
        self.search_hit = Some(hit);
    }
    unsafe fn open_folder(&mut self, path: PathBuf) {
        match crate::workspace::Workspace::create(self.hwnd, path, self.fonts.small) {
            Ok(workspace) => {
                self.search.take();
                self.search_visible = false;
                self.workspace = Some(workspace);
                self.workspace_visible = true;
                self.layout();
                self.status("");
            }
            Err(e) => error(self.hwnd, &e),
        }
    }
    unsafe fn toggle_heading(&self, start: i32, end: i32) -> bool {
        use windows::Win32::UI::Controls::RichEdit::ITextRange2;
        use windows_core::Interface;
        let key = (|| {
            let doc = crate::syntax::document(self.preview)?;
            let range: ITextRange2 = doc.Range(start, end).ok()?.cast().ok()?;
            let url = range.GetURL().ok()?.to_string();
            url.trim_matches('"')
                .strip_prefix("plumetxt-fold:")?
                .parse::<usize>()
                .ok()
        })();
        let Some(key) = key else {
            return false;
        };
        let mut position: POINT = zeroed();
        SendMessageW(
            self.preview,
            WM_USER + 221,
            0,
            &mut position as *mut _ as isize,
        );
        {
            let mut folded = self.folded.borrow_mut();
            if !folded.remove(&key) {
                folded.insert(key);
            }
        }
        self.refresh_preview();
        SendMessageW(
            self.preview,
            WM_USER + 222,
            0,
            &position as *const _ as isize,
        );
        true
    }
    unsafe fn cancel_loading(&mut self) {
        self.preview_version
            .set(self.preview_version.get().wrapping_add(1));
        let loading = self.loading.take().is_some();
        if loading || self.chunk.take().is_some() {
            KillTimer(self.hwnd, 11);
            SendMessageW(self.edit, EM_SETREADONLY, 0, 0);
            SetWindowTextW(self.edit, wide("").as_ptr());
            SendMessageW(self.edit, EM_SETEVENTMASK, 0, 1);
            SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
            self.path = None;
            self.original = None;
            self.watch = None;
        }
    }
    unsafe fn begin_load(&mut self, path: PathBuf, offset: Option<u64>) {
        self.preview_version
            .set(self.preview_version.get().wrapping_add(1));
        self.search_hit = None;
        self.preview_only = false;
        self.folded.borrow_mut().clear();
        self.loading = None;
        self.chunk = None;
        self.close_comparison();
        self.watch = None;
        self.original = None;
        self.path = Some(path.clone());
        SendMessageW(self.edit, EM_SETEVENTMASK, 0, 0);
        SetWindowTextW(self.edit, wide("").as_ptr());
        SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
        self.show_editor();
        if !self.preview.is_null() {
            ShowWindow(self.preview, SW_HIDE);
        }
        SendMessageW(self.edit, EM_SETREADONLY, 1, 0);
        let (tx, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = if let Some(offset) = offset {
                document::Chunk::read(&path, offset).map(|(chunk, source)| LoadedText {
                    units: source.encode_utf16().collect(),
                    encoding: chunk.encoding,
                    crlf: chunk.crlf,
                    original: None,
                    chunk: Some(chunk),
                })
            } else {
                document::read(&path).map(|(source, encoding, original)| LoadedText {
                    units: source.encode_utf16().collect(),
                    encoding,
                    crlf: source.contains("\r\n"),
                    original: Some(original),
                    chunk: None,
                })
            };
            let _ = tx.send(result);
        });
        self.loading = Some(Loading {
            receiver,
            data: None,
            inserted: 0,
            batch: 8192,
        });
        self.status("Loading…");
        SetTimer(self.hwnd, 11, 15, None);
    }

    unsafe fn load_tick(&mut self) {
        let Some(mut loading) = self.loading.take() else {
            return;
        };
        if loading.data.is_none() {
            match loading.receiver.try_recv() {
                Ok(Ok(data)) => loading.data = Some(data),
                Ok(Err(e)) => {
                    SendMessageW(self.edit, EM_SETREADONLY, 0, 0);
                    SendMessageW(self.edit, EM_SETEVENTMASK, 0, 1);
                    self.path = None;
                    self.title();
                    self.status(&format!("Open failed: {e}"));
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    SendMessageW(self.edit, EM_SETREADONLY, 0, 0);
                    SendMessageW(self.edit, EM_SETEVENTMASK, 0, 1);
                    self.path = None;
                    self.title();
                    self.status("Open failed: loader stopped");
                    return;
                }
                Err(TryRecvError::Empty) => (),
            }
        }
        if let Some(data) = &loading.data {
            let began = std::time::Instant::now();
            loading.inserted =
                match append_load_batch(self.edit, &data.units, loading.inserted, loading.batch) {
                    Ok(end) => end,
                    Err(e) => {
                        SendMessageW(self.edit, EM_SETREADONLY, 0, 0);
                        SendMessageW(self.edit, EM_SETEVENTMASK, 0, 1);
                        SetWindowTextW(self.edit, wide("").as_ptr());
                        SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
                        self.path = None;
                        self.title();
                        self.status(&format!("Open failed: {e}"));
                        return;
                    }
                };
            // Aim for 8 ms of insertion work, with bounded growth to avoid a long surprise batch.
            let factor = (0.008 / began.elapsed().as_secs_f64().max(0.0001)).clamp(0.5, 2.0);
            loading.batch = ((loading.batch as f64 * factor) as usize).clamp(8192, 256 * 1024);
            if loading.inserted == data.units.len() {
                let search_selection = self.search_hit.take().map(|hit| {
                    let start = data.chunk.as_ref().map_or(
                        match data.encoding {
                            Encoding::Utf8 => 0,
                            Encoding::Utf8Bom => 3,
                            _ => 2,
                        },
                        |chunk| chunk.start,
                    );
                    hit.selection(&String::from_utf16_lossy(&data.units), data.encoding, start)
                });
                self.encoding = data.encoding;
                self.crlf = data.crlf;
                self.original = data.original;
                self.chunk = loading.data.take().unwrap().chunk;
                SendMessageW(self.edit, EM_EMPTYUNDOBUFFER, 0, 0);
                SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
                SendMessageW(self.edit, EM_SETREADONLY, 0, 0);
                SendMessageW(self.edit, EM_SETEVENTMASK, 0, 1);
                if let (Some(path), Some(original)) = (&self.path, self.original) {
                    self.watch = Some(crate::external::Watch::new(path.clone(), original));
                }
                self.highlighter.clear();
                self.preview_only = self.is_markdown() && search_selection.is_none();
                scroll::pair(self.edit, null_mut());
                if !self.preview.is_null() {
                    scroll::pair(self.preview, null_mut());
                }
                if self.preview_only && self.preview.is_null() {
                    self.preview = rich_edit(self.hwnd, true, self.font);
                    SendMessageW(self.preview, WM_USER + 225, self.text_zoom as usize, 100);
                }
                if !self.preview_only && !self.preview.is_null() {
                    scroll::pair(self.edit, null_mut());
                    DestroyWindow(self.preview);
                    self.preview = null_mut();
                }
                self.layout();
                if !self.preview.is_null() {
                    ShowWindow(self.preview, SW_SHOW);
                    self.layout();
                }
                SetTimer(self.hwnd, 9, 80, None);
                self.refresh_preview();
                self.title();
                self.status("");
                if let Some(selection) = search_selection {
                    self.select_search_result(selection);
                }
                return;
            }
        }
        self.loading = Some(loading);
        SetTimer(self.hwnd, 11, 15, None);
    }

    unsafe fn save_tick(&mut self) {
        let Some(receiver) = self.saving.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok((path, offset))) => {
                SendMessageW(self.edit, EM_SETREADONLY, 0, 0);
                SendMessageW(self.edit, EM_SETMODIFY, 0, 0);
                self.cancel_loading();
                if std::fs::metadata(&path).is_ok_and(|m| m.len() > crate::large::THRESHOLD) {
                    self.begin_load(path, Some(offset));
                } else {
                    self.open(path);
                }
            }
            Err(TryRecvError::Empty) => {
                self.saving = Some(receiver);
                SetTimer(self.hwnd, 12, 30, None);
            }
            result => {
                SendMessageW(self.edit, EM_SETREADONLY, 0, 0);
                self.status("Save failed; your edits are still here");
                let message = match result {
                    Ok(Err(e)) => e,
                    _ => "Save worker stopped".into(),
                };
                error(self.hwnd, &message);
            }
        }
    }

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
        if self.loading.is_some() || self.chunk.is_some() || self.saving.is_some() {
            return;
        }
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
            wide(&format!(
                "{}{}",
                file_type(path.map(PathBuf::as_path)),
                if dirty { " *" } else { "" }
            ))
            .as_ptr(),
        );
        SetWindowTextW(
            self.hwnd,
            wide(&format!(
                "{}{}{} — PlumeTxt",
                if dirty { "* " } else { "" },
                name,
                if self.large.is_some() {
                    " · Large file"
                } else if self.chunk.is_some() {
                    " · Region"
                } else {
                    ""
                }
            ))
            .as_ptr(),
        );
    }
    unsafe fn hints(&self) -> String {
        if self.printing.is_some() {
            return "Printing…".into();
        }
        if self.exporting {
            return "Exporting PDF…".into();
        }
        if self.saving.is_some() {
            return "Saving…".into();
        }
        if self.loading.is_some() {
            return "Loading…".into();
        }
        let wide = theme::client(self.hwnd).right >= theme::px(self.hwnd, 1050);
        if self.image.is_some() {
            if wide {
                "Image  ·  Print Ctrl+P  ·  Fit Ctrl+0"
            } else {
                "Image  ·  Print Ctrl+P"
            }
            .into()
        } else if self.pdf_path.is_some() {
            let position = self
                .viewer
                .as_ref()
                .map(|v| v.page_status())
                .unwrap_or_else(|| "PDF".into());
            format!(
                "{position}  ·  Print Ctrl+P{}",
                if wide {
                    "  ·  Outline Ctrl+Shift+L"
                } else {
                    ""
                }
            )
        } else if self.large.is_some() {
            "Overview  ·  Edit region Ctrl+E".into()
        } else if self.comparison.is_some() {
            "Review external changes".into()
        } else if self.preview_only {
            if wide {
                "Reading  ·  Edit Ctrl+E  ·  Export Ctrl+Shift+E"
            } else {
                "Reading  ·  Edit Ctrl+E"
            }
            .into()
        } else if self.is_markdown() {
            if wide {
                "Editing  ·  Save Ctrl+S  ·  Read Ctrl+E"
            } else {
                "Editing  ·  Save Ctrl+S"
            }
            .into()
        } else {
            "Editing  ·  Save Ctrl+S".into()
        }
    }
    unsafe fn refresh_status(&self) {
        let message = self.status_message.borrow();
        let value = if message.is_empty() {
            self.hints()
        } else {
            message.clone()
        };
        if text(self.status) != value {
            SetWindowTextW(self.status, wide(&value).as_ptr());
        }
    }
    unsafe fn status(&self, message: &str) {
        *self.status_message.borrow_mut() = message.into();
        self.refresh_status();
        if !message.is_empty() {
            SetTimer(self.hwnd, 8, 3000, None);
        }
    }
    unsafe fn layout(&mut self) {
        let rc = theme::client(self.hwnd);
        let d = |v| theme::px(self.hwnd, v);
        theme::move_window(
            self.status,
            d(16),
            rc.bottom - d(27),
            (rc.right - d(398)).max(1),
            d(24),
            0,
        );
        theme::move_window(
            self.file_type,
            rc.right - d(366),
            rc.bottom - d(27),
            d(120),
            d(24),
            0,
        );
        theme::move_window(
            self.commands_button,
            rc.right - d(236),
            rc.bottom - d(29),
            d(176),
            d(24),
            0,
        );
        self.refresh_status();
        theme::move_window(
            self.terminal_button,
            rc.right - d(52),
            rc.bottom - d(29),
            d(36),
            d(24),
            0,
        );
        let bottom = rc.bottom;
        let left = if self.sidebar_visible() {
            self.workspace_width.min((rc.right - d(360)).max(d(160)))
        } else {
            0
        };
        let right = (rc.right - self.terminal_width).max(left + 1);
        if let Some(workspace) = &self.workspace {
            ShowWindow(
                workspace.hwnd,
                if self.workspace_visible && !self.search_visible {
                    SW_SHOWNA
                } else {
                    SW_HIDE
                },
            );
            if self.workspace_visible && !self.search_visible {
                scroll::resize(
                    workspace.hwnd,
                    d(8),
                    d(12),
                    (left - d(16)).max(1),
                    (bottom - d(54)).max(1),
                );
            }
        }
        if let Some(search) = &self.search {
            ShowWindow(
                search.0,
                if self.workspace_visible && self.search_visible {
                    SW_SHOWNA
                } else {
                    SW_HIDE
                },
            );
            if self.workspace_visible && self.search_visible {
                theme::move_window(search.0, 0, 0, left - d(4), (bottom - d(34)).max(1), 0);
            }
        }
        if let Some(view) = &self.image {
            theme::move_window(view.0, left, 0, right - left, (bottom - d(34)).max(1), 0);
            theme::invalidate(view.0);
        }
        if let Some(view) = &self.large {
            theme::move_window(view.0, left, 0, right - left, (bottom - d(34)).max(1), 0);
        }
        if let Some(terminal) = &self.terminal {
            theme::move_window(
                terminal.0,
                right + d(8),
                d(12),
                (self.terminal_width - d(20)).max(1),
                (bottom - d(58)).max(1),
                0,
            );
        }
        if let Some(viewer) = &self.viewer {
            theme::move_window(viewer.0, left, 0, right - left, (bottom - d(34)).max(1), 0);
        }
        let inset = if self.preview.is_null() {
            left + ((right - left - d(960)) / 2).max(d(16))
        } else {
            left + d(16)
        };
        let split = if self.preview.is_null() {
            right - (inset - left)
        } else {
            left + (right - left) / 2
        };
        scroll::resize(
            self.edit,
            inset,
            d(28),
            (split - inset).max(1),
            (bottom - d(64)).max(1),
        );
        let reading = self.preview_only && self.comparison.is_none();
        if self.image.is_none() && self.pdf_path.is_none() && self.large.is_none() {
            ShowWindow(self.edit, if reading { SW_HIDE } else { SW_SHOWNA });
        }
        if !self.preview.is_null() {
            let reading_inset = ((right - left - d(880)) / 2).max(d(24));
            scroll::resize(
                self.preview,
                if reading {
                    left + reading_inset
                } else {
                    split + 1
                },
                if self.comparison.is_some() {
                    d(40)
                } else {
                    d(24)
                },
                (right
                    - if reading {
                        left + 2 * reading_inset
                    } else {
                        split + d(17)
                    })
                .max(1),
                (bottom
                    - if self.comparison.is_some() {
                        d(80)
                    } else {
                        d(64)
                    })
                .max(1),
            );
        }
        let comparing = self.comparison.is_some()
            && self.image.is_none()
            && self.pdf_path.is_none()
            && self.large.is_none();
        for (i, action) in self.compare_actions.iter().enumerate() {
            theme::move_window(
                *action,
                split + d(12) + i as i32 * d(134),
                d(6),
                d(128),
                d(26),
                0,
            );
            ShowWindow(*action, if comparing { SW_SHOWNA } else { SW_HIDE });
        }
        scroll::show(self.edit, !reading && (self.preview.is_null() || comparing));
        theme::invalidate(self.hwnd);
        SetTimer(self.hwnd, 9, 80, None);
    }
    unsafe fn change_dpi(&mut self, dpi: u32) {
        let dpi = dpi.max(96);
        if self.dpi == dpi {
            return;
        }
        self.workspace_width = (self.workspace_width as i64 * dpi as i64 / self.dpi as i64) as i32;
        self.terminal_width = (self.terminal_width as i64 * dpi as i64 / self.dpi as i64) as i32;
        self.dpi = dpi;
        SetPropW(self.hwnd, wide("PlumeTxtDpi").as_ptr(), dpi as usize as _);
        let fonts = theme::Fonts::at_dpi(dpi);
        theme::replace_fonts(self.hwnd, &self.fonts, &fonts);
        if let Some(workspace) = &mut self.workspace {
            let _ = workspace.update_icons();
        }
        self.font = if self.monospace {
            fonts.code
        } else {
            fonts.body
        };
        // RichEdit copies font attributes and can return NULL for WM_GETFONT.
        let modified = SendMessageW(self.edit, EM_GETMODIFY, 0, 0);
        SendMessageW(self.edit, WM_SETFONT, self.font as usize, 0);
        SendMessageW(self.edit, EM_SETMODIFY, modified as usize, 0);
        for control in [
            self.palette.0,
            self.viewer.as_ref().map_or(null_mut(), |v| v.0),
        ] {
            SendMessageW(
                control,
                theme::FONTS_CHANGED,
                fonts.ui as usize,
                fonts.small as isize,
            );
        }
        if let Some(terminal) = &self.terminal {
            SendMessageW(terminal.0, theme::FONTS_CHANGED, 0, 0);
        }
        if let Some(large) = &self.large {
            SendMessageW(large.0, theme::FONTS_CHANGED, fonts.code as usize, 0);
        }
        if let Some(search) = &self.search {
            SendMessageW(search.0, WM_SIZE, 0, 0);
        }
        let margin = theme::scale(32, dpi);
        for control in [self.edit, self.preview] {
            if !control.is_null() {
                SendMessageW(
                    control,
                    EM_SETMARGINS,
                    (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
                    ((margin << 16) | margin) as isize,
                );
            }
        }
        self.fonts = fonts;
        self.highlighter.clear();
    }
    unsafe fn sync_scroll(&self, source: HWND) {
        if self.comparison.is_some() || self.preview_only {
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
        if self.saving.is_some() {
            self.status("Saving… Please wait before closing or opening another file");
            return false;
        }
        if self.loading.is_some() {
            return true;
        }
        if self.large.is_some() {
            return true;
        }
        if SendMessageW(self.edit, EM_GETMODIFY, 0, 0) == 0 {
            return true;
        }
        match MessageBoxW(
            self.hwnd,
            wide("Save changes?").as_ptr(),
            wide("PlumeTxt").as_ptr(),
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
        if self.loading.is_some() || self.saving.is_some() {
            self.status("Please wait for loading or saving to finish");
            return false;
        }
        if self.comparison.is_some() {
            self.status("Review external changes first: Keep current or Use disk");
            return false;
        }
        if self.image.is_some() {
            self.status("Image · Read only");
            return false;
        }
        if self.large.is_some() {
            self.status("Double-click or Ctrl+E to edit this region");
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
        if let Some(chunk) = self.chunk.clone() {
            let source = text(self.edit);
            let (tx, receiver) = mpsc::channel();
            self.saving = Some(receiver);
            SendMessageW(self.edit, EM_SETREADONLY, 1, 0);
            self.status("Saving…");
            std::thread::spawn(move || {
                let result = chunk.save(&path, &source).map(|()| (path, chunk.start));
                let _ = tx.send(result);
            });
            SetTimer(self.hwnd, 12, 30, None);
            // Closing/opening waits for the asynchronous save; the current document stays open.
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
    unsafe fn browse_large(&mut self, path: PathBuf, offset: u64) {
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
        if let Some(view) = &self.large {
            view.seek(offset);
        }
    }
    unsafe fn open(&mut self, path: PathBuf) {
        self.search_hit = None;
        if path.is_dir() {
            self.open_folder(path);
            return;
        }
        if !self.confirm_save() {
            return;
        }
        self.cancel_loading();
        self.chunk = None;
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
            self.begin_load(path, Some(0));
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
            self.begin_load(path, None);
        }
    }
    unsafe fn refresh_preview(&self) {
        self.preview_version
            .set(self.preview_version.get().wrapping_add(1));
        if self.loading.is_some() {
            return;
        }
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
            // ponytail: finish one render before starting the latest request; no queued snapshots.
            if self.preview_job.borrow().is_some() {
                self.preview_pending.set(true);
                return;
            }
            self.preview_pending.set(false);
            let mut rc: RECT = zeroed();
            GetClientRect(self.preview, &mut rc);
            let dpi = theme::dpi(self.preview) as i32;
            let width = ((rc.right - theme::px(self.preview, 48)).max(16) * 1440 / dpi) as usize;
            let source = text(self.edit);
            let revision = document::fingerprint(source.as_bytes());
            if self.fold_revision.replace(revision) != revision {
                self.folded.borrow_mut().clear();
            }
            let version = self.preview_version.get();
            let base = self
                .path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf);
            let folded = self.folded.borrow().clone();
            let (sender, receiver) = mpsc::channel();
            let worker = std::thread::Builder::new()
                .name("markdown-preview".into())
                .spawn(move || {
                    use windows::Win32::System::Com::{
                        CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED,
                    };
                    let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok();
                    let result = result.map_err(|e| e.to_string()).map(|()| {
                        let rtf =
                            markdown::folding_preview(&source, width, base.as_deref(), &folded);
                        unsafe { CoUninitialize() };
                        rtf
                    });
                    let _ = sender.send(result);
                });
            match worker {
                Ok(_) => {
                    *self.preview_job.borrow_mut() = Some((version, receiver));
                    SetTimer(self.hwnd, 14, 30, None);
                }
                Err(e) => self.status(&format!("Preview worker: {e}")),
            }
        }
    }
    unsafe fn preview_tick(&self) {
        let result = self
            .preview_job
            .borrow()
            .as_ref()
            .map(|(version, rx)| (*version, rx.try_recv()));
        let Some((version, result)) = result else {
            return;
        };
        if matches!(result, Err(TryRecvError::Empty)) {
            SetTimer(self.hwnd, 14, 30, None);
            return;
        }
        self.preview_job.borrow_mut().take();
        if version == self.preview_version.get()
            && !self.preview.is_null()
            && self.loading.is_none()
            && self.comparison.is_none()
            && self.pdf_path.is_none()
            && self.image.is_none()
            && self.large.is_none()
        {
            match result {
                Ok(Ok(rtf)) => {
                    let mut position: POINT = zeroed();
                    SendMessageW(
                        self.preview,
                        WM_USER + 221,
                        0,
                        &mut position as *mut _ as isize,
                    );
                    SendMessageW(self.preview, WM_SETREDRAW, 0, 0);
                    let rendered = set_rtf(self.preview, &rtf);
                    if self.preview_only {
                        SendMessageW(
                            self.preview,
                            WM_USER + 222,
                            0,
                            &position as *const _ as isize,
                        );
                    } else {
                        self.sync_scroll(self.edit);
                    }
                    SendMessageW(self.preview, WM_SETREDRAW, 1, 0);
                    theme::invalidate(self.preview);
                    if let Err(e) = rendered {
                        self.status(&e);
                    }
                }
                Ok(Err(e)) => self.status(&e),
                Err(_) => self.status("Preview worker stopped"),
            }
        } else {
            drop(result);
        }
        if self.preview_pending.replace(false) {
            self.refresh_preview();
        }
    }
    fn is_markdown(&self) -> bool {
        self.loading.is_none()
            && self.path.as_ref().is_none_or(|p| {
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
        if !self.is_markdown()
            || self.pdf_path.is_some()
            || self.image.is_some()
            || self.large.is_some()
            || self.preview_only
            || self.comparison.is_some()
        {
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
    fn essential_commands(&self) -> Vec<palette::Command> {
        let mut items = vec![(OPEN, "Open file", "Ctrl+O", "open")];
        if self.pdf_path.is_some() || self.large_path.is_some() || self.path.is_some() {
            items.push((FIND, "Find in current file", "Ctrl+F", ""));
        }
        if self.workspace.is_some() {
            items.extend([
                (SEARCH, "Search workspace", "Ctrl+Shift+F", ""),
                (
                    TOGGLE_FILES,
                    "Toggle file tree",
                    "Ctrl+Shift+B",
                    "workspace sidebar hide show",
                ),
                (REFRESH_FILES, "Refresh files", "", "workspace reload"),
                (CLOSE_FOLDER, "Close folder", "", "workspace single file"),
            ]);
        } else {
            items.push((OPEN_FOLDER, "Open folder", "Ctrl+Shift+O", "workspace"));
        }
        if self.loading.is_none() && self.saving.is_none() {
            if self.pdf_path.is_some() || self.image.is_some() {
                if self.printing.is_none() {
                    items.push((PRINT, "Print", "Ctrl+P", "printer pages copies"));
                }
                items.push((FIT, "Fit width", "Ctrl+0", "fit image"));
                if self.pdf_path.is_some() {
                    items.extend([
                        (
                            TOC,
                            "Toggle outline",
                            "Ctrl+Shift+L",
                            "pdf directory hide show",
                        ),
                        (FIT_PAGE, "Fit page", "Ctrl+Shift+0", "pdf whole page"),
                    ]);
                }
            } else if self.large.is_some() {
                items.push((PREVIEW, "Edit region", "Ctrl+E", "large file"));
            } else if self.comparison.is_some() {
                items.extend([
                    (KEEP_CURRENT, "Keep current", "", "external changes"),
                    (USE_DISK, "Use disk", "", "external changes"),
                ]);
            } else {
                items.extend([
                    (SAVE, "Save", "Ctrl+S", "save"),
                    (SAVE_AS, "Save as", "Ctrl+Shift+S", "save copy"),
                ]);
                if self.is_markdown() {
                    items.extend([
                        (
                            PREVIEW,
                            if self.preview_only {
                                "Edit Markdown"
                            } else {
                                "Read Markdown"
                            },
                            "Ctrl+E",
                            "preview reading editing",
                        ),
                        (EXPORT, "Export PDF", "Ctrl+Shift+E", "export"),
                        (IMAGE, "Insert image", "Ctrl+Shift+I", "picture"),
                    ]);
                }
                if self.chunk.is_some() {
                    items.push((OVERVIEW, "Document overview", "", "large file navigate"));
                }
            }
        }
        items.push((TERMINAL, "Toggle terminal", "Ctrl+Shift+J", "shell"));
        if self.printing.is_some() {
            items.push((CANCEL_PRINT, "Cancel print", "", "stop printer"));
        }
        items
    }
    unsafe fn command(&mut self, id: usize) {
        if id == PRINT {
            if self.printing.is_some() {
                self.status("Printing…");
                return;
            }
            let Some(path) = self.pdf_path.as_ref().or(self.image_path.as_ref()).cloned() else {
                self.status("Open an image or PDF to print.");
                return;
            };
            let pdf = self.pdf_path.is_some();
            let pages = if pdf {
                self.viewer.as_ref().map_or(0, |v| v.page_count())
            } else {
                1
            };
            match crate::printing::choose(self.hwnd, path, pdf, pages) {
                Ok(Some(job)) => {
                    self.printing = Some(job);
                    self.status("Printing…");
                    SetTimer(self.hwnd, 13, 200, None);
                }
                Ok(None) => (),
                Err(e) => error(self.hwnd, &e),
            }
            return;
        }
        if id == CANCEL_PRINT {
            if let Some(job) = &self.printing {
                job.cancel();
                self.status("Cancelling print…");
            }
            return;
        }
        if (self.loading.is_some() || self.saving.is_some())
            && !matches!(
                id,
                OPEN | NEW
                    | EXIT
                    | TERMINAL
                    | COMMANDS
                    | LAYOUT
                    | OPEN_FOLDER
                    | CLOSE_FOLDER
                    | TOGGLE_FILES
                    | REFRESH_FILES
                    | SEARCH
                    | FIND
            )
        {
            return;
        }
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
                EXPORT | BOLD | ITALIC | CODE | FONT_MODE | TEXT_LARGER | TEXT_SMALLER | 202
            )
        {
            self.status("Double-click or Ctrl+E to edit this region");
            return;
        }

        match id {
            FIND => {
                let path = self
                    .pdf_path
                    .as_ref()
                    .or(self.large_path.as_ref())
                    .or(self.path.as_ref())
                    .cloned();
                if let Some(path) = path {
                    self.search_in(path);
                } else {
                    self.status(
                        "Open or save a file to find. Ctrl+Shift+F searches the workspace.",
                    );
                }
            }
            SEARCH => {
                if self.workspace.is_none() {
                    if let Some(path) = crate::workspace::choose_folder(self.hwnd) {
                        self.open_folder(path);
                    }
                }
                if let Some(workspace) = &self.workspace {
                    self.search_in(workspace.root.clone());
                }
            }
            OPEN_FOLDER => {
                if let Some(path) = crate::workspace::choose_folder(self.hwnd) {
                    self.open_folder(path);
                }
            }
            TOGGLE_FILES => {
                if self.workspace.is_some() || self.search.is_some() {
                    self.workspace_visible = self.search_visible || !self.workspace_visible;
                    self.search_visible = false;
                    self.workspace_drag.take();
                    if GetCapture() == self.hwnd {
                        ReleaseCapture();
                    }
                    if !self.sidebar_visible() {
                        self.focus_content();
                    }
                    self.layout();
                }
            }
            CLOSE_FOLDER => {
                self.search.take();
                self.search_visible = false;
                self.workspace.take();
                self.layout();
                self.focus_content();
            }
            REFRESH_FILES => {
                if let Some(workspace) = &mut self.workspace {
                    if let Err(e) = workspace.refresh() {
                        error(self.hwnd, &e);
                    }
                }
            }
            KEEP_CURRENT | USE_DISK => self.resolve_external(id == USE_DISK),
            OPEN => {
                if let Some(path) = dialog(self.hwnd, false, false) {
                    self.open(path);
                }
            }
            NEW => {
                if self.confirm_save() {
                    self.cancel_loading();
                    self.chunk = None;
                    self.preview_only = false;
                    self.folded.borrow_mut().clear();
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
            EDITOR => {
                self.preview_only = false;
                if let (Some(view), Some(path)) = (&self.large, &self.large_path) {
                    let offset = view.offset();
                    self.begin_load(path.clone(), Some(offset));
                } else {
                    self.show_editor();
                }
            }
            OVERVIEW => {
                if let Some(chunk) = self.chunk.clone() {
                    if self.confirm_save() {
                        self.cancel_loading();
                        self.browse_large(chunk.path, chunk.start);
                    }
                } else if self.large.is_none() {
                    self.status("Overview is available for large files");
                }
            }
            PREVIEW
                if self.pdf_path.is_some()
                    || self.image.is_some()
                    || self.large.is_some()
                    || !self.is_markdown() =>
            {
                self.command(EDITOR);
            }
            PREVIEW if self.comparison.is_some() => {
                self.status("Review external changes first");
            }
            PREVIEW => {
                if self.preview.is_null() {
                    self.preview = rich_edit(self.hwnd, true, self.font);
                    SendMessageW(self.preview, WM_USER + 225, self.text_zoom as usize, 100);
                }
                self.preview_only = !self.preview_only;
                scroll::pair(
                    self.edit,
                    if self.preview_only {
                        null_mut()
                    } else {
                        self.preview
                    },
                );
                scroll::pair(
                    self.preview,
                    if self.preview_only {
                        null_mut()
                    } else {
                        self.edit
                    },
                );
                self.show_editor();
                self.refresh_preview();
                self.focus_content();
            }
            EXPORT => {
                if self.pdf_path.is_some() || self.image.is_some() {
                    self.status("Open Markdown to export. Use Ctrl+P to print this file.");
                    return;
                }
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
                    let chunk = self.chunk.clone();
                    let base = self
                        .path
                        .as_deref()
                        .and_then(Path::parent)
                        .map(Path::to_path_buf);
                    let output = self.export_result.clone();
                    let hwnd = self.hwnd as usize;
                    std::thread::spawn(move || {
                        let result = if let Some(chunk) = chunk {
                            export_chunk_document(&chunk, &source, &path, base.as_deref())
                        } else {
                            export_complete(&source, &path, base.as_deref())
                        }
                        .map(|()| path);
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
                    self.terminal_width = (theme::client(self.hwnd).right / 3)
                        .clamp(theme::px(self.hwnd, 320), theme::px(self.hwnd, 520));
                    SetTimer(self.hwnd, 7, 15, None);
                }
                self.layout();
                if !self.preview.is_null() && self.pdf_path.is_none() && self.image.is_none() {
                    SetTimer(self.hwnd, 1, 350, None);
                }
            }
            CHANGE => {
                self.preview_version
                    .set(self.preview_version.get().wrapping_add(1));
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
                let previous_target = if self.terminal_open { 1. } else { 0. };
                self.terminal_from += (previous_target - self.terminal_from)
                    * theme::animation_progress(self.terminal_started, 0.18);
                self.terminal_open = !self.terminal_open;
                self.terminal_started = std::time::Instant::now();
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
                        .or_else(|| {
                            self.workspace
                                .as_ref()
                                .map(|workspace| workspace.root.clone())
                        })
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    self.terminal = Some(terminal::Terminal::create(self.hwnd, &cwd));
                }
                if self.terminal_open {
                    self.terminal_width = (theme::client(self.hwnd).right / 3)
                        .clamp(theme::px(self.hwnd, 320), theme::px(self.hwnd, 520));
                    self.layout();
                    if let Some(t) = &self.terminal {
                        let rc = theme::client(t.0);
                        let visible = (rc.right as f64 * self.terminal_from).round() as i32;
                        SetWindowRgn(
                            t.0,
                            CreateRectRgn(rc.right - visible, 0, rc.right, rc.bottom),
                            0,
                        );
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
            COMMANDS => {
                self.palette.set_commands(self.essential_commands());
                self.palette.show();
            }
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
                    "Segoe UI Semilight"
                });
            }
            TEXT_LARGER | TEXT_SMALLER | TEXT_RESET => {
                self.text_zoom = if id == TEXT_RESET {
                    100
                } else {
                    (self.text_zoom + if id == TEXT_LARGER { 10 } else { -10 }).clamp(70, 200)
                };
                SendMessageW(self.edit, WM_USER + 225, self.text_zoom as usize, 100);
                if !self.preview.is_null() {
                    SendMessageW(self.preview, WM_USER + 225, self.text_zoom as usize, 100);
                }
                self.status(&format!("{}%", self.text_zoom));
            }
            FIT_PAGE => {
                if self.pdf_path.is_some() {
                    if let Some(viewer) = &self.viewer {
                        viewer.action(reader::FIT_PAGE);
                    }
                }
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
        let d = |v| theme::px(self.hwnd, v);
        let dc = target;
        fill(dc, *dirty, CANVAS);
        fill(
            dc,
            RECT {
                left: 0,
                top: rc.bottom - d(34),
                right: rc.right,
                bottom: rc.bottom,
            },
            SURFACE,
        );
        if self.sidebar_visible() {
            let width = self.workspace_width.min((rc.right - d(360)).max(d(160)));
            fill(
                dc,
                RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: rc.bottom - d(34),
                },
                SURFACE,
            );
            fill(
                dc,
                RECT {
                    left: width - 1,
                    top: 0,
                    right: width,
                    bottom: rc.bottom - d(34),
                },
                LINE,
            );
        }
        if self.large.is_none()
            && self.pdf_path.is_none()
            && self.image.is_none()
            && !self.preview.is_null()
            && (!self.preview_only || self.comparison.is_some())
        {
            let left = if self.sidebar_visible() {
                self.workspace_width.min((rc.right - d(360)).max(d(160)))
            } else {
                0
            };
            let middle = left + (rc.right - self.terminal_width - left).max(1) / 2;
            fill(
                dc,
                RECT {
                    left: middle,
                    top: d(20),
                    right: middle + 1,
                    bottom: rc.bottom - d(50),
                },
                LINE,
            );
        }
        fill(
            dc,
            RECT {
                left: 0,
                top: rc.bottom - d(34),
                right: rc.right,
                bottom: rc.bottom - d(33),
            },
            LINE,
        );
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
    SendMessageW(
        control,
        WM_USER + 204,
        0,
        windows::Win32::UI::Controls::RichEdit::SES_HYPERLINKTOOLTIPS as isize,
    );
    SendMessageW(
        control,
        EM_SETEVENTMASK,
        0,
        if readonly { 1 | 0x04000000 } else { 1 },
    ); // ENM_CHANGE | ENM_LINK
    let margin = RECT {
        left: theme::px(parent, 32),
        top: theme::px(parent, 20),
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
    export_sections(std::iter::once(Ok(source.to_owned())), output, base)
}

fn export_chunk_document(
    chunk: &document::Chunk,
    source: &str,
    output: &Path,
    base: Option<&Path>,
) -> Result<(), String> {
    let snapshot = std::env::temp_dir().join(format!(
        "plumetxt-export-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        chunk.save(&snapshot, source)?;
        export_sections(document::Chunk::sections(snapshot.clone())?, output, base)
    })();
    let _ = std::fs::remove_file(snapshot);
    result
}

fn export_sections(
    sections: impl Iterator<Item = Result<String, String>>,
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
        ".plumetxt-{}-{}.pdf",
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
                sections,
                &temporary,
                GetStockObject(DEFAULT_GUI_FONT),
                base,
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

unsafe fn export_pdf(
    parent: HWND,
    sections: impl Iterator<Item = Result<String, String>>,
    output: &Path,
    font: HFONT,
    base: Option<&Path>,
) -> Result<(), String> {
    let control = rich_edit(parent, true, font);
    if control.is_null() {
        return Err("Could not create the layout control".into());
    }
    ShowWindow(control, SW_HIDE);
    let result = (|| {
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
            let title = wide("PlumeTxt Markdown");
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
            let mut success = true;
            let mut section_error = None;
            // ponytail: bounded sections restart Markdown context and start a new page;
            // a streaming block parser is needed for exact cross-section constructs.
            for section in sections {
                let rendered = section.and_then(|source| {
                    set_rtf(control, &markdown::with_images(&source, 9000, false, base))
                });
                if let Err(e) = rendered {
                    section_error = Some(e);
                    success = false;
                    break;
                }
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
                loop {
                    if StartPage(dc) <= 0 {
                        success = false;
                        break;
                    }
                    let next = SendMessageW(control, EM_FORMATRANGE, 1, &range as *const _ as isize)
                        as i32;
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
                if !success {
                    break;
                }
            }
            if success {
                if EndDoc(dc) <= 0 {
                    return Err("PDF printing did not finish. Check the print queue.".into());
                }
                Ok(())
            } else {
                AbortDoc(dc);
                Err(section_error
                    .unwrap_or_else(|| "PDF export failed; output may be incomplete.".into()))
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
    theme::fill(draw.hDC, draw.rcItem, theme::SURFACE);
    let d = |v| theme::px(draw.hwndItem, v);
    let hovered = !GetPropW(draw.hwndItem, wide("PlumeTxtHover").as_ptr()).is_null();
    let pressed = draw.itemState & ODS_SELECTED != 0;
    let focused = draw.itemState & ODS_FOCUS != 0;
    let mut surface = draw.rcItem;
    InflateRect(&mut surface, -d(2), -d(2));
    if focused {
        theme::rounded(draw.hDC, surface, theme::ACCENT, d(8));
        InflateRect(&mut surface, -1, -1);
    }
    theme::panel(
        draw.hDC,
        surface,
        if pressed {
            theme::SELECTED
        } else if hovered {
            theme::HOVER
        } else {
            theme::SURFACE
        },
        if focused { theme::ACCENT } else { theme::LINE },
        d(10),
        d(1),
    );
    theme::label(
        draw.hDC,
        &if draw.CtlID == TERMINAL as u32 {
            ">_".into()
        } else {
            text(draw.hwndItem)
        },
        draw.rcItem,
        SendMessageW(draw.hwndItem, WM_GETFONT, 0, 0) as HFONT,
        if draw.itemState & ODS_DISABLED != 0 {
            theme::MUTED
        } else {
            theme::INK
        },
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
            for (x, y) in [(0, 0), (35, 0), (0, 23), (35, 23)] {
                assert_eq!(GetPixel(dc, x, y), theme::SURFACE);
            }
            assert_eq!(
                GetPixel(dc, 4, 12),
                if state & ODS_SELECTED != 0 {
                    theme::SELECTED
                } else {
                    theme::SURFACE
                }
            );
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
        WM_NOTIFY if lp != 0 => {
            return APP.with(|slot| {
                let Ok(mut slot) = slot.try_borrow_mut() else {
                    return 0;
                };
                let Some(app) = slot.as_mut() else {
                    return 0;
                };
                let hdr = &*(lp as *const NMHDR);
                if app
                    .workspace
                    .as_ref()
                    .is_some_and(|tree| tree.hwnd == hdr.hwndFrom)
                {
                    if hdr.code == NM_CUSTOMDRAW {
                        let draw = &mut *(lp as *mut NMTVCUSTOMDRAW);
                        if draw.nmcd.dwDrawStage == CDDS_PREPAINT {
                            return CDRF_NOTIFYITEMDRAW as isize;
                        }
                        if draw.nmcd.dwDrawStage == CDDS_ITEMPREPAINT {
                            let selected = draw.nmcd.uItemState & (CDIS_SELECTED | CDIS_FOCUS) != 0;
                            draw.nmcd.uItemState &= !(CDIS_HOT | CDIS_FOCUS);
                            draw.clrText = if selected { theme::ACCENT } else { theme::INK };
                            draw.clrTextBk = if selected {
                                theme::SELECTED
                            } else {
                                theme::SURFACE
                            };
                            return CDRF_NEWFONT as isize;
                        }
                    }
                    match app.workspace.as_mut().unwrap().notify(lp) {
                        Ok(Some(path)) => app.open(path),
                        Err(e) => {
                            app.status(&e);
                            return 1;
                        }
                        _ => (),
                    }
                }
                use windows::Win32::UI::Controls::RichEdit::{ENLINK, EN_LINK};
                if hdr.hwndFrom == app.preview && hdr.code == EN_LINK && app.comparison.is_none() {
                    let link = &*(lp as *const ENLINK);
                    if link.msg == WM_LBUTTONUP
                        || (link.msg == WM_KEYDOWN && link.wParam.0 == VK_RETURN as usize)
                    {
                        // Rebuild after RichEdit finishes dispatching its link event.
                        PostMessageW(
                            hwnd,
                            FOLD_HEADING,
                            link.chrg.cpMin as usize,
                            link.chrg.cpMax as isize,
                        );
                        return 1;
                    }
                }
                0
            });
        }
        WM_DRAWITEM if !((lp as *const DRAWITEMSTRUCT).is_null()) => {
            let draw = &*(lp as *const DRAWITEMSTRUCT);
            if [
                TERMINAL as u32,
                COMMANDS as u32,
                KEEP_CURRENT as u32,
                USE_DISK as u32,
            ]
            .contains(&draw.CtlID)
            {
                draw_terminal_button(draw);
                return 1;
            }
        }
        WM_ERASEBKGND => {
            theme::fill(wp as HDC, theme::client(hwnd), theme::CANVAS);
            return 1;
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lp as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT {
                x: theme::px(hwnd, 820),
                y: theme::px(hwnd, 560),
            };
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
            SetBkColor(dc, theme::SURFACE);
            SetDCBrushColor(dc, theme::SURFACE);
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
            WM_DPICHANGED => {
                let rect = *(lp as *const RECT);
                app.change_dpi((wp & 0xffff) as u32);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                app.layout();
                app.refresh_preview();
            }

            crate::search::CLOSE => {
                app.search_visible = false;
                app.layout();
                if let Some(workspace) = &app.workspace {
                    SetFocus(workspace.hwnd);
                } else {
                    app.focus_content();
                }
            }
            crate::search::OPEN_RESULT => {
                if let Some(hit) = app
                    .search
                    .as_ref()
                    .filter(|search| search.0 as usize == wp)
                    .and_then(|search| search.take_hit())
                {
                    app.open_search_result(hit);
                }
            }
            WM_LBUTTONDOWN if app.sidebar_visible() => {
                let x = lp as u16 as i16 as i32;
                let width = app.workspace_width.min(
                    (theme::client(hwnd).right - theme::px(hwnd, 360)).max(theme::px(hwnd, 160)),
                );
                if (x - width).abs() <= theme::px(hwnd, 7) {
                    app.workspace_drag = Some(theme::Divider::new(hwnd, width));
                    SetCapture(hwnd);
                }
            }
            WM_MOUSEMOVE if app.sidebar_visible() => {
                let x = lp as u16 as i16 as i32;
                let rc = theme::client(hwnd);
                if let Some(guide) = &mut app.workspace_drag {
                    guide.move_to(
                        hwnd,
                        x.clamp(
                            theme::px(hwnd, 160),
                            (rc.right - theme::px(hwnd, 360)).max(theme::px(hwnd, 160)),
                        ),
                    );
                }
                if app.workspace_drag.is_some()
                    || (x - app
                        .workspace_width
                        .min((rc.right - theme::px(hwnd, 360)).max(theme::px(hwnd, 160))))
                    .abs()
                        <= theme::px(hwnd, 7)
                {
                    SetCursor(LoadCursorW(null_mut(), IDC_SIZEWE));
                }
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED if app.workspace_drag.is_some() => {
                let guide = app.workspace_drag.take().unwrap();
                let width = guide.x;
                drop(guide);
                if msg == WM_LBUTTONUP {
                    app.workspace_width = width;
                    ReleaseCapture();
                    app.layout();
                    SetTimer(hwnd, 1, 100, None);
                }
            }
            FOLD_HEADING => {
                if !app.preview.is_null() && app.comparison.is_none() {
                    app.toggle_heading(wp as i32, lp as i32);
                }
            }
            crate::large::EDIT_CURRENT => {
                if app.large.is_some() {
                    if let Some(path) = app.large_path.clone() {
                        app.begin_load(path, Some(lp as u64));
                    }
                }
            }
            PASTE => {
                if app.loading.is_none()
                    && app.saving.is_none()
                    && app.pdf_path.is_none()
                    && app.image.is_none()
                    && app.large.is_none()
                {
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
                    14 => app.preview_tick(),
                    11 => app.load_tick(),
                    12 => app.save_tick(),
                    13 => {
                        let result = app.printing.as_ref().map(|job| job.result.try_recv());
                        match result {
                            Some(Ok(result)) => {
                                app.printing.take();
                                match result {
                                    Ok(()) => app.status("Sent to printer"),
                                    Err(e) => {
                                        app.status("Print stopped");
                                        error(hwnd, &e);
                                    }
                                }
                            }
                            Some(Err(TryRecvError::Empty)) => {
                                SetTimer(hwnd, 13, 200, None);
                            }
                            Some(Err(TryRecvError::Disconnected)) => {
                                app.printing.take();
                                app.status("Print worker stopped");
                            }
                            None => (),
                        }
                    }
                    10 => {
                        self::App::poll_external(app);
                        app.refresh_status();
                        SetTimer(hwnd, 10, 400, None);
                    }
                    1 => app.refresh_preview(),
                    9 => {
                        if app.loading.is_none()
                            && app.saving.is_none()
                            && app.pdf_path.is_none()
                            && app.image.is_none()
                            && app.large.is_none()
                        {
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
                        let progress = theme::animation_progress(app.terminal_started, 0.18);
                        if let Some(t) = &app.terminal {
                            let rc = theme::client(t.0);
                            let target = if app.terminal_open { 1. } else { 0. };
                            let fraction =
                                app.terminal_from + (target - app.terminal_from) * progress;
                            let visible = (rc.right as f64 * fraction).round() as i32;
                            SetWindowRgn(
                                t.0,
                                CreateRectRgn(rc.right - visible, 0, rc.right, rc.bottom),
                                1,
                            );
                            if progress >= 1. {
                                if !app.terminal_open {
                                    ShowWindow(t.0, SW_HIDE);
                                }
                                SetWindowRgn(t.0, null_mut(), 0);
                            }
                        }
                        if progress >= 1. {
                            if !app.terminal_open {
                                app.terminal_width = 0;
                                app.layout();
                                app.focus_content();
                            }
                            SetTimer(hwnd, 1, 100, None);
                        } else {
                            SetTimer(hwnd, 7, 15, None);
                        }
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
                    && path.is_file()
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
        let class = wide("PlumeTxtWindow");
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
            wide("PlumeTxt").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            theme::scale(1100, windows_sys::Win32::UI::HiDpi::GetDpiForSystem()),
            theme::scale(780, windows_sys::Win32::UI::HiDpi::GetDpiForSystem()),
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            error(null_mut(), "Could not create the window");
            return;
        }
        let fonts = theme::Fonts::at_dpi(theme::dpi(hwnd));
        let font = fonts.body;
        theme::window_material(hwnd);
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
            WS_CHILD | WS_VISIBLE | 0xc000,
            0,
            0,
            1,
            1,
            hwnd,
            null_mut(),
            instance,
            null(),
        );
        SendMessageW(status, WM_SETFONT, fonts.small as usize, 0);
        let commands_button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("Commands  Ctrl+Shift+P").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
            0,
            0,
            176,
            24,
            hwnd,
            COMMANDS as _,
            instance,
            null(),
        );
        SendMessageW(commands_button, WM_SETFONT, fonts.small as usize, 0);
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
        for button in [
            commands_button,
            terminal_button,
            compare_actions[0],
            compare_actions[1],
        ] {
            theme::attach_button(button);
        }
        let palette = palette::Palette::create(hwnd, fonts.ui, fonts.small, Vec::new(), DISPATCH);
        APP.with(|slot| {
            *slot.borrow_mut() = Some(App {
                search: None,
                search_visible: false,
                search_hit: None,
                workspace: None,
                workspace_width: theme::px(hwnd, 240),
                workspace_visible: true,
                workspace_drag: None,
                folded: RefCell::new(BTreeSet::new()),
                fold_revision: Cell::new(0),
                hwnd,
                dpi: theme::dpi(hwnd),
                edit,
                highlighter: crate::syntax::Highlighter::default(),
                preview: null_mut(),
                preview_only: false,
                preview_job: RefCell::new(None),
                preview_version: Cell::new(0),
                preview_pending: Cell::new(false),
                status,
                commands_button,
                status_message: RefCell::new(String::new()),
                file_type,
                terminal_button,
                terminal: None,
                terminal_open: false,
                terminal_width: 0,
                terminal_started: std::time::Instant::now(),
                terminal_from: 0.,
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
                printing: None,
                exporting: false,
                export_result: Arc::new(Mutex::new(None)),
                loading: None,
                chunk: None,
                saving: None,
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
            if APP.with(|s| s.borrow().as_ref().is_some_and(|a| a.palette.key(&msg))) {
                continue;
            }
            if APP.with(|s| {
                s.borrow()
                    .as_ref()
                    .is_some_and(|a| a.search.as_ref().is_some_and(|search| search.key(&msg)))
            }) {
                continue;
            }
            let in_terminal = APP.with(|s| {
                s.borrow()
                    .as_ref()
                    .is_some_and(|a| a.terminal.as_ref().is_some_and(|t| t.contains(msg.hwnd)))
            });
            let in_search = APP.with(|s| {
                s.borrow().as_ref().is_some_and(|a| {
                    a.search
                        .as_ref()
                        .is_some_and(|search| search.contains(msg.hwnd))
                })
            });
            if msg.message == WM_KEYDOWN {
                let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
                let shift = GetKeyState(VK_SHIFT as i32) < 0;
                let alt = GetKeyState(VK_MENU as i32) < 0;
                let view = APP.with(|s| {
                    s.borrow().as_ref().map_or(ShortcutView::Text, |a| {
                        if a.pdf_path.is_some() {
                            ShortcutView::Pdf
                        } else if a.image.is_some() {
                            ShortcutView::Image
                        } else if a.is_markdown() {
                            ShortcutView::Markdown
                        } else {
                            ShortcutView::Text
                        }
                    })
                });
                let mut command = if alt {
                    0
                } else {
                    shortcut(msg.wParam as u16, ctrl, shift, view, in_terminal)
                };
                if in_search
                    && !matches!(
                        command,
                        SEARCH
                            | FIND
                            | PREVIEW
                            | COMMANDS
                            | TOGGLE_FILES
                            | TERMINAL
                            | OPEN
                            | OPEN_FOLDER
                            | CLOSE_FOLDER
                            | SAVE
                            | SAVE_AS
                            | EXIT
                            | NEW
                    )
                {
                    command = 0;
                }
                if command != 0 {
                    SendMessageW(hwnd, DISPATCH, command, 0);
                    continue;
                }
                let in_editor =
                    APP.with(|s| s.borrow().as_ref().is_some_and(|a| msg.hwnd == a.edit));
                if in_editor && paste_shortcut(msg.wParam as u16, ctrl, shift, alt) {
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
        assert!(rendered.contains("PlumeTxt"), "{rendered}");
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
#[ignore = "Requires Windows RichEdit and Microsoft Print to PDF"]
fn native_region_export_includes_whole_document_and_unsaved_edits() {
    let root = std::env::temp_dir().join(format!("plumetxt-region-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let input = root.join("input.md");
    let output = root.join("output.pdf");
    let source = format!(
        "# HEAD_SENTINEL\n\n{}# MIDDLE_SENTINEL\n\n{}# TAIL_SENTINEL\n",
        "\n".repeat(70000),
        "\n".repeat(70000)
    );
    std::fs::write(&input, &source).unwrap();
    let (chunk, region) = document::Chunk::read(&input, 70000).unwrap();
    unsafe {
        LoadLibraryW(wide("Msftedit.dll").as_ptr());
    }
    export_chunk_document(
        &chunk,
        &format!("# UNSAVED_SENTINEL\n\n{region}"),
        &output,
        Some(&root),
    )
    .unwrap();
    let pdf = lopdf::Document::load(&output).unwrap();
    let pages: Vec<_> = pdf.get_pages().keys().copied().collect();
    let content = pdf.extract_text(&pages).unwrap();
    for marker in [
        "HEAD_SENTINEL",
        "MIDDLE_SENTINEL",
        "TAIL_SENTINEL",
        "UNSAVED_SENTINEL",
    ] {
        assert!(
            content.contains(marker),
            "Missing {marker} in PDF: {content}"
        );
    }
    assert_eq!(std::fs::read_to_string(&input).unwrap(), source);
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_preview_heading_arrow_has_actionable_link() {
    use windows::Win32::UI::Controls::RichEdit::ITextRange2;
    use windows_core::Interface;
    let _ole = crate::assets::Ole::new().unwrap();
    unsafe {
        LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let control = rich_edit(null_mut(), true, GetStockObject(DEFAULT_GUI_FONT));
        let source = "# Heading\n\nBODY_MARKER\n\n# Next\n\nNEXT_MARKER";
        set_rtf(
            control,
            &markdown::folding_preview(source, 9000, None, &BTreeSet::new()),
        )
        .unwrap();
        let doc = crate::syntax::document(control).unwrap();
        let range = doc.Range(0, 0).unwrap();
        assert!(
            range
                .FindText(
                    &windows_core::BSTR::from("▾"),
                    1000,
                    windows::Win32::UI::Controls::RichEdit::tomConstants(0)
                )
                .unwrap()
                > 0
        );
        let link: ITextRange2 = range.cast().unwrap();
        assert_eq!(
            link.GetURL().unwrap().to_string().trim_matches('"'),
            "plumetxt-fold:0"
        );
        set_rtf(
            control,
            &markdown::folding_preview(source, 9000, None, &BTreeSet::from([0])),
        )
        .unwrap();
        assert!(!text(control).contains("BODY_MARKER"));
        assert!(text(control).contains("NEXT_MARKER"));
        DestroyWindow(control);
    }
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_startup_background_is_dark_before_app_is_ready() {
    unsafe {
        let library = LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let instance = GetModuleHandleW(null());
        let name = wide("PlumeTxtStartupPaintTest");
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
        for (path, content, keyword_len) in [
            ("query.sql", "SELECT name FROM items", 6),
            ("script.lua", "local name = '中文'", 5),
            ("script.ps1", "param($name)", 5),
            ("Dockerfile", "FROM alpine", 4),
            ("main.kt", "fun main() {}", 3),
            ("table.csv", "name,city\r\nAlice,Paris", 4),
        ] {
            SetWindowTextW(edit, wide(content).as_ptr());
            SendMessageW(edit, EM_EMPTYUNDOBUFFER, 0, 0);
            SendMessageW(edit, EM_SETMODIFY, 0, 0);
            h.update(edit, Some(Path::new(path)));
            assert_eq!(
                doc.Range(0, keyword_len)
                    .unwrap()
                    .GetFont()
                    .unwrap()
                    .GetForeColor()
                    .unwrap(),
                theme::ACCENT as i32,
                "{path}"
            );
            assert_eq!(text(edit), content);
            assert_eq!(SendMessageW(edit, EM_GETMODIFY, 0, 0), 0);
            assert_eq!(SendMessageW(edit, EM_CANUNDO, 0, 0), 0);
        }
        SetWindowTextW(edit, wide(&"name,city,note\r\n".repeat(4000)).as_ptr());
        SendMessageW(edit, EM_SETSEL, 0, 0);
        h.update(edit, Some(Path::new("dense.csv")));
        assert_eq!(
            doc.Range(0, 4)
                .unwrap()
                .GetFont()
                .unwrap()
                .GetForeColor()
                .unwrap(),
            theme::ACCENT as i32,
            "Dense CSV must retain visible field colours"
        );
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

fn paste_shortcut(key: u16, ctrl: bool, shift: bool, alt: bool) -> bool {
    !alt && matches!(
        (ctrl, shift, key),
        (true, false, 0x56) | (false, true, VK_INSERT)
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShortcutView {
    Text,
    Markdown,
    Pdf,
    Image,
}

fn shortcut(key: u16, ctrl: bool, shift: bool, view: ShortcutView, terminal: bool) -> usize {
    use ShortcutView::*;
    // Plain Ctrl combinations belong to the shell, including Ctrl+E.
    if terminal && !shift {
        return 0;
    }
    let visual = matches!(view, Pdf | Image);
    match (ctrl, shift, key) {
        (true, true, 0x4a) => TERMINAL,
        (true, false, 0x4e) => NEW,
        (true, true, 0x4f) => OPEN_FOLDER,
        (true, false, 0x4f) => OPEN,
        (true, true, 0x53) => SAVE_AS,
        (true, false, 0x53) => SAVE,
        (true, true, 0x50) => COMMANDS,
        (true, true, 0x46) => SEARCH,
        (true, false, 0x46) if view != Image => FIND,
        (true, false, 0x51) => EXIT,
        (true, false, 0x50) => PRINT,
        (true, true, 0x45) if view == Markdown => EXPORT,
        (true, false, 0x45) => PREVIEW,
        (true, true, 0x49) if view == Markdown => IMAGE,
        (true, true, 0x42) => TOGGLE_FILES,
        (true, false, 0x42) if !terminal && view == Markdown => BOLD,
        (true, false, 0x49) if !terminal && view == Markdown => ITALIC,
        (true, false, 0x4b) if !terminal && view == Markdown => CODE,
        (true, true, 0x52) if view == Markdown => 202,
        (true, true, 0x4c) if view == Pdf => TOC,
        (false, _, VK_PRIOR) if view == Pdf && !terminal => PREV,
        (false, _, VK_NEXT) if view == Pdf && !terminal => NEXT,
        (true, _, VK_OEM_PLUS | VK_ADD) if visual && !terminal => ZOOM_IN,
        (true, _, VK_OEM_MINUS | VK_SUBTRACT) if visual && !terminal => ZOOM_OUT,
        (true, true, 0x30) if view == Pdf && !terminal => FIT_PAGE,
        (true, false, 0x30) if visual && !terminal => FIT,
        (true, _, VK_OEM_PLUS | VK_ADD) if !terminal => TEXT_LARGER,
        (true, _, VK_OEM_MINUS | VK_SUBTRACT) if !terminal => TEXT_SMALLER,
        (true, false, 0x30) if !terminal => TEXT_RESET,
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
fn shortcuts_have_one_mode_switch_and_respect_document_and_terminal() {
    for ctrl in [false, true] {
        for shift in [false, true] {
            for alt in [false, true] {
                for key in [0x56, VK_INSERT] {
                    assert_eq!(
                        paste_shortcut(key, ctrl, shift, alt),
                        !alt && ((key == 0x56 && ctrl && !shift)
                            || (key == VK_INSERT && !ctrl && shift))
                    );
                }
            }
        }
    }
    use ShortcutView::*;
    for view in [Text, Markdown, Pdf, Image] {
        for terminal in [false, true] {
            for (key, command) in [
                (0x4a, TERMINAL),
                (0x4f, OPEN_FOLDER),
                (0x53, SAVE_AS),
                (0x50, COMMANDS),
                (0x46, SEARCH),
                (0x42, TOGGLE_FILES),
            ] {
                assert_eq!(shortcut(key, true, true, view, terminal), command);
            }
            for (key, command) in [
                (0x45, PREVIEW),
                (0x4e, NEW),
                (0x4f, OPEN),
                (0x53, SAVE),
                (0x50, PRINT),
                (0x51, EXIT),
            ] {
                assert_eq!(
                    shortcut(key, true, false, view, terminal),
                    if terminal { 0 } else { command }
                );
            }
            assert_eq!(
                shortcut(0x46, true, false, view, terminal),
                if terminal || view == Image { 0 } else { FIND }
            );
            for (key, command) in [(0x45, EXPORT), (0x49, IMAGE), (0x52, 202)] {
                assert_eq!(
                    shortcut(key, true, true, view, terminal),
                    if view == Markdown { command } else { 0 }
                );
            }
            for (key, command) in [(0x42, BOLD), (0x49, ITALIC), (0x4b, CODE)] {
                assert_eq!(
                    shortcut(key, true, false, view, terminal),
                    if view == Markdown && !terminal {
                        command
                    } else {
                        0
                    }
                );
            }
            for (key, shift) in [
                (0x31, false),
                (0x4d, true),
                (0x4a, false),
                (VK_OEM_3, false),
                (0x43, false),
                (0x56, false),
            ] {
                assert_eq!(shortcut(key, true, shift, view, terminal), 0);
            }
            assert_eq!(
                shortcut(0x4c, true, true, view, terminal),
                if view == Pdf { TOC } else { 0 }
            );
            assert_eq!(
                shortcut(0x30, true, true, view, terminal),
                if view == Pdf && !terminal {
                    FIT_PAGE
                } else {
                    0
                }
            );
            for key in VK_F1..=VK_F24 {
                for (ctrl, shift) in [(false, false), (false, true), (true, false), (true, true)] {
                    assert_eq!(shortcut(key, ctrl, shift, view, terminal), 0);
                }
            }
            for (key, zoom, text) in [
                (VK_OEM_PLUS, ZOOM_IN, TEXT_LARGER),
                (VK_ADD, ZOOM_IN, TEXT_LARGER),
                (VK_OEM_MINUS, ZOOM_OUT, TEXT_SMALLER),
                (VK_SUBTRACT, ZOOM_OUT, TEXT_SMALLER),
            ] {
                for shift in [false, true] {
                    assert_eq!(
                        shortcut(key, true, shift, view, terminal),
                        if terminal {
                            0
                        } else if matches!(view, Pdf | Image) {
                            zoom
                        } else {
                            text
                        }
                    );
                }
            }
            assert_eq!(
                shortcut(0x30, true, false, view, terminal),
                if terminal {
                    0
                } else if matches!(view, Pdf | Image) {
                    FIT
                } else {
                    TEXT_RESET
                }
            );
            for (key, command) in [(VK_PRIOR, PREV), (VK_NEXT, NEXT)] {
                assert_eq!(
                    shortcut(key, false, false, view, terminal),
                    if view == Pdf && !terminal { command } else { 0 }
                );
            }
        }
    }
}

#[test]
fn native_incremental_load_preserves_unicode_and_selection() {
    unsafe {
        LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let edit = rich_edit(null_mut(), false, GetStockObject(DEFAULT_GUI_FONT) as HFONT);
        assert!(!edit.is_null());
        MoveWindow(edit, 0, 0, 1000, 700, 0);
        SendMessageW(edit, EM_SETREADONLY, 1, 0);
        let source = format!("{}😀\r\n{}\r\n中文", "a".repeat(8191), "b".repeat(8190));
        let units: Vec<u16> = source.encode_utf16().collect();
        let mut offset = 0;
        let mut batches = 0;
        while offset < units.len() {
            let next = append_load_batch(edit, &units, offset, 8192).unwrap();
            assert!(next > offset && next - offset <= 8192);
            offset = next;
            batches += 1;
            let mut a = 1u32;
            let mut b = 1u32;
            SendMessageW(
                edit,
                EM_GETSEL,
                &mut a as *mut _ as usize,
                &mut b as *mut _ as isize,
            );
            assert_eq!((a, b), (0, 0));
        }
        assert!(batches >= 3);
        assert_eq!(
            document::encode(&text(edit), Encoding::Utf8, false),
            document::encode(&source, Encoding::Utf8, false)
        );
        SendMessageW(edit, EM_SETREADONLY, 0, 0);
        SendMessageW(edit, EM_SETSEL, 0, 1);
        SendMessageW(edit, EM_REPLACESEL, 1, wide("Z").as_ptr() as isize);
        assert!(text(edit).starts_with('Z'));
        DestroyWindow(edit);
    }
}

#[test]
fn native_preview_coalesces_requests_and_rejects_stale_results() {
    unsafe {
        LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let hwnd = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("Preview regression").as_ptr(),
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
        let fonts = theme::Fonts::new();
        let font = fonts.body;
        let edit = rich_edit(hwnd, false, font);
        let status = null_mut();
        let commands_button = null_mut();
        let file_type = null_mut();
        let terminal_button = null_mut();
        let palette = palette::Palette::create(hwnd, fonts.ui, fonts.small, Vec::new(), DISPATCH);
        let mut app = App {
            search: None,
            search_visible: false,
            search_hit: None,
            workspace: None,
            workspace_width: theme::px(hwnd, 240),
            workspace_visible: true,
            workspace_drag: None,
            folded: RefCell::new(BTreeSet::new()),
            fold_revision: Cell::new(0),
            hwnd,
            dpi: theme::dpi(hwnd),
            edit,
            highlighter: crate::syntax::Highlighter::default(),
            preview: null_mut(),
            preview_only: false,
            preview_job: RefCell::new(None),
            preview_version: Cell::new(0),
            preview_pending: Cell::new(false),
            status,
            commands_button,
            status_message: RefCell::new(String::new()),
            file_type,
            terminal_button,
            terminal: None,
            terminal_open: false,
            terminal_width: 0,
            terminal_started: std::time::Instant::now(),
            terminal_from: 0.,
            font,
            path: None,
            encoding: Encoding::Utf8,
            crlf: true,
            original: None,
            watch: None,
            comparison: None,
            compare_actions: [null_mut(); 2],
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
            printing: None,
            exporting: false,
            export_result: Arc::new(Mutex::new(None)),
            loading: None,
            chunk: None,
            saving: None,
        };
        app.preview = rich_edit(hwnd, true, font);
        MoveWindow(app.preview, 0, 0, 800, 600, 0);
        let drain = |app: &App| {
            let started = std::time::Instant::now();
            while app.preview_job.borrow().is_some() {
                assert!(started.elapsed().as_secs() < 10, "Preview did not finish");
                app.preview_tick();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        SetWindowTextW(edit, wide("# Old\r\n\r\nSTALE_MARKER").as_ptr());
        app.refresh_preview();
        let first_version = app.preview_job.borrow().as_ref().unwrap().0;
        SetWindowTextW(edit, wide("# Latest\r\n\r\nLATEST_MARKER").as_ptr());
        for _ in 0..20 {
            app.refresh_preview();
        }
        assert_eq!(
            app.preview_job.borrow().as_ref().unwrap().0,
            first_version,
            "Repeated refreshes must retain a single in-flight job"
        );
        assert!(app.preview_pending.get());
        drain(&app);
        assert!(text(app.preview).contains("LATEST_MARKER"));
        assert!(!text(app.preview).contains("STALE_MARKER"));
        app.folded.borrow_mut().insert(0);
        app.refresh_preview();
        drain(&app);
        assert!(!text(app.preview).contains("LATEST_MARKER"));
        app.folded.borrow_mut().clear();
        app.refresh_preview();
        app.preview_version
            .set(app.preview_version.get().wrapping_add(1));
        drain(&app);
        assert!(
            !text(app.preview).contains("LATEST_MARKER"),
            "Invalidated render must not update the control"
        );
        assert!(text(edit).contains("LATEST_MARKER"));
        let source = text(edit);
        SendMessageW(edit, EM_SETMODIFY, 0, 0);
        for dpi in [120, 144, 192, 96] {
            let previous_font = app.fonts.body;
            app.change_dpi(dpi);
            let mut released: LOGFONTW = zeroed();
            assert_eq!(
                GetObjectW(
                    previous_font,
                    size_of::<LOGFONTW>() as i32,
                    &mut released as *mut _ as _
                ),
                0,
                "Old DPI fonts must be released"
            );
            app.layout();
            assert_eq!(theme::dpi(hwnd), dpi);
            assert_eq!(text(edit), source);
            assert_eq!(SendMessageW(edit, EM_GETMODIFY, 0, 0), 0);
            let mut logical: LOGFONTW = zeroed();
            assert_ne!(
                GetObjectW(
                    app.fonts.body,
                    size_of::<LOGFONTW>() as i32,
                    &mut logical as *mut _ as _
                ),
                0
            );
            assert_eq!(logical.lfHeight, -theme::scale(20, dpi));
        }
        for reading in [true, false, true, false] {
            app.command(PREVIEW);
            drain(&app);
            assert_eq!(app.preview_only, reading);
            if reading {
                app.command(BOLD);
            }
            assert_eq!(text(edit), source);
            assert_eq!(SendMessageW(edit, EM_GETMODIFY, 0, 0), 0);
        }
        // A first toggle must enter reading even when no preview control exists yet.
        scroll::pair(edit, null_mut());
        DestroyWindow(app.preview);
        app.preview = null_mut();
        app.command(PREVIEW);
        drain(&app);
        assert!(app.preview_only);
        app.command(PREVIEW);
        drain(&app);
        assert!(!app.preview_only);
        app.path = Some(PathBuf::from("plain.txt"));
        for action in [BOLD, ITALIC, CODE, PREVIEW] {
            app.command(action);
        }
        assert_eq!(text(edit), source);
        assert!(!app.preview_only);
        app.pdf_path = Some(PathBuf::from("reader.pdf"));
        app.command(PREVIEW);
        assert!(app.pdf_path.is_none());
        assert_eq!(text(edit), source);
        drop(app);
        DestroyWindow(hwnd);
    }
}
