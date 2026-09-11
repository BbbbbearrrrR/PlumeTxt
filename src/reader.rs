use crate::{outline::Bookmark, pdf, theme::*};
use std::{
    cell::RefCell,
    collections::VecDeque,
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, SystemServices::MK_CONTROL},
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub const SCROLL_TO: u32 = WM_APP + 94;
const RESIZE: u32 = WM_APP + 21;
pub const ACTION: u32 = WM_APP + 22;
pub const SMALLER: usize = 1;
pub const LARGER: usize = 2;
pub const FIT: usize = 3;
pub const UP: usize = 4;
pub const DOWN: usize = 5;
pub const TOGGLE_TOC: usize = 6;
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const HEADER: i32 = 0;
const GAP: i32 = 24;

#[derive(Clone, Copy, Debug)]
pub struct PageRect {
    pub top: i32,
    pub width: i32,
    pub height: i32,
}
#[derive(Default)]
pub struct Layout {
    pub pages: Vec<PageRect>,
    pub total: i32,
    pub widest: i32,
}
impl Layout {
    pub fn new(sizes: &[(f32, f32)], width: i32, zoom: f32) -> Self {
        let widest = sizes.iter().map(|p| p.0).fold(1.0f32, f32::max);
        let scale = (width - 2 * GAP).max(64) as f32 / widest * zoom;
        let mut result = Self::default();
        let mut top = GAP;
        for &(w, h) in sizes {
            let width = (w * scale).round().clamp(1., 100_000.) as i32;
            let height = (h * scale).round().clamp(1., 1_000_000.) as i32;
            result.pages.push(PageRect { top, width, height });
            result.widest = result.widest.max(width);
            top = top.saturating_add(height).saturating_add(GAP);
        }
        result.total = top;
        result
    }
    pub fn at(&self, y: i32) -> usize {
        self.pages.partition_point(|p| p.top <= y).saturating_sub(1)
    }
    pub fn visible(&self, y: i32, height: i32) -> std::ops::Range<usize> {
        if self.pages.is_empty() {
            return 0..0;
        }
        self.at(y)..(self.at(y.saturating_add(height)) + 1).min(self.pages.len())
    }
    fn anchor(&self, y: i32) -> (usize, f32) {
        let i = self.at(y);
        self.pages
            .get(i)
            .map(|p| (i, (y - p.top) as f32 / p.height as f32))
            .unwrap_or((0, 0.))
    }
    fn restore(&self, anchor: (usize, f32)) -> i32 {
        self.pages
            .get(anchor.0)
            .map(|p| p.top + (p.height as f32 * anchor.1) as i32)
            .unwrap_or(0)
    }
}

struct State {
    hwnd: HWND,
    tree: HWND,
    font: HFONT,
    small: HFONT,
    worker: pdf::Worker,
    path: PathBuf,
    document_id: u64,
    generation: u64,
    sizes: Vec<(f32, f32)>,
    layout: Layout,
    cache: VecDeque<pdf::Page>,
    bookmarks: Vec<Bookmark>,
    toc_visible: bool,
    toc_note: String,
    message: String,
    zoom: f32,
    x: i32,
    y: i32,
    viewport_width: i32,
    viewport_height: i32,
    wheel_remainder: i32,
    buffer: Buffer,
    paint_count: u64,
    render_count: u64,
    request_count: u64,
}

pub struct Reader(pub HWND);
impl Reader {
    pub unsafe fn create(parent: HWND, font: HFONT, small: HFONT) -> Self {
        let class = wide("FeatherPadReader");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("PDF").as_ptr(),
            WS_CHILD | WS_CLIPCHILDREN | WS_VSCROLL | WS_HSCROLL,
            0,
            0,
            1,
            1,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        dark_scrollbars(hwnd, CANVAS);
        let tree = CreateWindowExW(
            0,
            wide("SysTreeView32").as_ptr(),
            wide("Outline").as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | TVS_HASBUTTONS
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS,
            0,
            0,
            1,
            1,
            hwnd,
            900usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        dark_scrollbars(tree, SURFACE);
        SendMessageW(tree, WM_SETFONT, font as usize, 0);
        SendMessageW(tree, TVM_SETBKCOLOR, 0, SURFACE as isize);
        SendMessageW(tree, TVM_SETTEXTCOLOR, 0, INK as isize);
        SendMessageW(tree, TVM_SETITEMHEIGHT, 30, 0);
        SetWindowTheme(tree, wide("").as_ptr(), wide("").as_ptr());
        let state = State {
            hwnd,
            tree,
            font,
            small,
            worker: pdf::Worker::new(hwnd as usize),
            path: PathBuf::new(),
            document_id: 0,
            generation: 0,
            sizes: Vec::new(),
            layout: Layout::default(),
            cache: VecDeque::new(),
            bookmarks: Vec::new(),
            toc_visible: true,
            toc_note: "".into(),
            message: "Loading…".into(),
            zoom: 1.,
            x: 0,
            y: 0,
            viewport_width: 0,
            viewport_height: 0,
            wheel_remainder: 0,
            buffer: Buffer::default(),
            paint_count: 0,
            render_count: 0,
            request_count: 0,
        };
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::into_raw(Box::new(RefCell::new(state))) as isize,
        );
        Self(hwnd)
    }
    pub unsafe fn open(&self, path: PathBuf) {
        with(self.0, |s| {
            s.path = path;
            s.document_id += 1;
            s.generation += 1;
            s.cache.clear();
            s.sizes.clear();
            s.layout = Layout::default();
            s.bookmarks.clear();
            s.x = 0;
            s.y = 0;
            s.zoom = 1.;
            s.message = "Loading…".into();
            s.toc_note = "".into();
            SendMessageW(s.tree, TVM_DELETEITEM, 0, TVI_ROOT);
            s.resize();
            s.worker.request(pdf::Request {
                document_id: s.document_id,
                generation: s.generation,
                path: s.path.clone(),
                jobs: vec![(0, 800), (1, 800)],
            });
            invalidate(s.hwnd);
        });
        ShowWindow(self.0, SW_SHOW);
        SetFocus(self.0);
    }
    pub unsafe fn close(&self) {
        with(self.0, |s| {
            s.document_id += 1;
            s.worker.close();
            s.cache.clear();
            s.sizes.clear();
            s.layout = Layout::default();
            s.bookmarks.clear();
        });
        ShowWindow(self.0, SW_HIDE);
    }
    pub unsafe fn action(&self, action: usize) {
        SendMessageW(self.0, ACTION, action, 0);
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

unsafe fn with(hwnd: HWND, f: impl FnOnce(&mut State)) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>;
    if !ptr.is_null() {
        if let Ok(mut state) = (*ptr).try_borrow_mut() {
            f(&mut state);
        }
    }
}

impl State {
    fn sidebar(&self) -> i32 {
        if self.toc_visible {
            242
        } else {
            0
        }
    }
    unsafe fn resize(&mut self) {
        let rc = client(self.hwnd);
        let side = self.sidebar();
        MoveWindow(
            self.tree,
            12,
            HEADER + 64,
            (side - 24).max(1),
            (rc.bottom - HEADER - 104).max(1),
            0,
        );
        ShowWindow(self.tree, if self.toc_visible { SW_SHOW } else { SW_HIDE });
        // MoveWindow suppresses repaint; the parent's paint excludes this child.
        // Repaint newly exposed outline space when a docked panel closes.
        InvalidateRect(self.tree, null(), 1);
        let width = (rc.right - side).max(64);
        let height = (rc.bottom - HEADER).max(1);
        if width != self.viewport_width || height != self.viewport_height {
            let anchor = self.layout.anchor(self.y + self.viewport_height / 3);
            self.viewport_width = width;
            self.viewport_height = height;
            self.layout = Layout::new(&self.sizes, width, self.zoom);
            self.y = self.layout.restore(anchor) - height / 3;
            self.generation += 1;
            self.scrollbars();
            self.schedule();
        }
        invalidate(self.hwnd);
    }
    unsafe fn scrollbars(&mut self) {
        self.y = self
            .y
            .clamp(0, (self.layout.total - self.viewport_height).max(0));
        self.x = self.x.clamp(
            0,
            (self.layout.widest + 2 * GAP - self.viewport_width).max(0),
        );
        for (bar, total, visible, pos) in [
            (SB_VERT, self.layout.total, self.viewport_height, self.y),
            (
                SB_HORZ,
                self.layout.widest + 2 * GAP,
                self.viewport_width,
                self.x,
            ),
        ] {
            let info = SCROLLINFO {
                cbSize: size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
                nMin: 0,
                nMax: total.saturating_sub(1).max(0),
                nPage: visible.max(1) as u32,
                nPos: pos,
                nTrackPos: 0,
            };
            SetScrollInfo(self.hwnd, bar, &info, 0);
            ShowScrollBar(self.hwnd, bar, 0);
        }
    }
    fn target_width(&self, index: usize) -> u32 {
        let size = self.sizes[index];
        pdf::dimensions(size.0, size.1, self.layout.pages[index].width as u32).0
    }
    unsafe fn schedule(&mut self) {
        if self.sizes.is_empty() {
            return;
        }
        let visible = self.layout.visible(self.y, self.viewport_height);
        let mut order: Vec<usize> = visible.clone().collect();
        if visible.end < self.sizes.len() {
            order.push(visible.end);
        }
        if visible.start > 0 {
            order.push(visible.start - 1);
        }
        let jobs = order
            .into_iter()
            .filter(|&i| {
                !self
                    .cache
                    .iter()
                    .any(|p| p.index as usize == i && p.width == self.target_width(i))
            })
            .map(|i| (i as u32, self.layout.pages[i].width as u32))
            .collect::<Vec<_>>();
        // Empty requests still cancel obsolete work and allow deferred bookmark extraction.
        self.request_count += 1;
        self.worker.request(pdf::Request {
            document_id: self.document_id,
            generation: self.generation,
            path: self.path.clone(),
            jobs,
        });
    }
    fn keep(&mut self, page: pdf::Page) {
        self.cache.retain(|p| p.index != page.index);
        self.cache.push_back(page);
        let center = self.layout.at(self.y + self.viewport_height / 2) as u32;
        while self.cache.iter().map(|p| p.pixels.len()).sum::<usize>() > CACHE_BYTES {
            let farthest = self
                .cache
                .iter()
                .enumerate()
                .max_by_key(|(_, p)| p.index.abs_diff(center))
                .map(|(i, _)| i)
                .unwrap();
            self.cache.remove(farthest);
        }
    }
    unsafe fn replies(&mut self) {
        while let Ok(reply) = self.worker.results.try_recv() {
            match reply {
                pdf::Reply::Info(id, sizes) if id == self.document_id => {
                    self.sizes = sizes;
                    self.layout = Layout::new(&self.sizes, self.viewport_width, self.zoom);
                    self.y = 0;
                    self.message.clear();
                    self.scrollbars();
                    self.schedule();
                }
                pdf::Reply::Page(id, generation, page)
                    if id == self.document_id && generation == self.generation =>
                {
                    self.render_count += 1;
                    self.keep(page);
                }
                pdf::Reply::Outline(id, result) if id == self.document_id => {
                    match result {
                        Ok(bookmarks) => {
                            self.bookmarks = bookmarks;
                            self.toc_note = if self.bookmarks.is_empty() {
                                "".into()
                            } else {
                                String::new()
                            };
                        }
                        Err(_) => self.toc_note = "".into(),
                    }
                    self.populate_tree();
                }
                pdf::Reply::Error(id, message) if id == self.document_id => self.message = message,
                _ => (),
            }
        }
        invalidate(self.hwnd);
    }
    unsafe fn populate_tree(&mut self) {
        SendMessageW(self.tree, WM_SETREDRAW, 0, 0);
        SendMessageW(self.tree, TVM_DELETEITEM, 0, TVI_ROOT);
        if self.bookmarks.is_empty() {
            self.bookmarks = (0..self.sizes.len())
                .map(|i| Bookmark {
                    title: format!("Page {}", i + 1),
                    level: 0,
                    page: Some(i as u32),
                    top: None,
                })
                .collect();
        }
        let mut parents: Vec<HTREEITEM> = Vec::new();
        for (index, item) in self.bookmarks.iter().enumerate() {
            let mut title = wide(&item.title);
            let parent = if item.level == 0 {
                TVI_ROOT
            } else {
                parents.get(item.level - 1).copied().unwrap_or(TVI_ROOT)
            };
            let data = TVITEMW {
                mask: TVIF_TEXT | TVIF_PARAM,
                pszText: title.as_mut_ptr(),
                lParam: index as isize,
                ..zeroed()
            };
            let insert = TVINSERTSTRUCTW {
                hParent: parent,
                hInsertAfter: TVI_LAST,
                Anonymous: TVINSERTSTRUCTW_0 { item: data },
            };
            let node = SendMessageW(self.tree, TVM_INSERTITEMW, 0, &insert as *const _ as isize)
                as HTREEITEM;
            parents.truncate(item.level);
            parents.push(node);
            if parent != TVI_ROOT {
                SendMessageW(self.tree, TVM_EXPAND, TVE_EXPAND as usize, parent);
            }
        }
        SendMessageW(self.tree, WM_SETREDRAW, 1, 0);
        invalidate(self.tree);
    }
    unsafe fn jump(&mut self, index: usize) {
        if let Some(bookmark) = self.bookmarks.get(index) {
            if let Some(page) = bookmark
                .page
                .and_then(|p| self.layout.pages.get(p as usize).map(|r| (p, *r)))
            {
                self.y = page.1.top - GAP;
                if let Some(top) = bookmark.top {
                    let size = self.sizes[page.0 as usize];
                    self.y += (page.1.height as f32 * (1. - top / size.1).clamp(0., 1.)) as i32;
                }
                self.scrollbars();
                self.schedule();
                invalidate(self.hwnd);
            }
        }
    }
    unsafe fn zoom(&mut self, factor: f32, anchor_y: i32) {
        let anchor = self.layout.anchor(self.y + anchor_y);
        self.zoom = factor.clamp(0.25, 4.);
        self.layout = Layout::new(&self.sizes, self.viewport_width, self.zoom);
        self.y = self.layout.restore(anchor) - anchor_y;
        self.generation += 1;
        self.scrollbars();
        self.schedule();
        invalidate(self.hwnd);
    }
    unsafe fn scroll(&mut self, bar: i32, code: i32, delta: i32) {
        let mut info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL,
            ..zeroed()
        };
        GetScrollInfo(self.hwnd, bar, &mut info);
        let pos = if bar == SB_VERT {
            &mut self.y
        } else {
            &mut self.x
        };
        *pos = if delta != 0 {
            pos.saturating_sub(delta)
        } else {
            match code {
                SB_LINEUP => pos.saturating_sub(48),
                SB_LINEDOWN => pos.saturating_add(48),
                SB_PAGEUP => pos.saturating_sub(info.nPage as i32 - 32),
                SB_PAGEDOWN => pos.saturating_add(info.nPage as i32 - 32),
                SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
                SB_TOP => 0,
                SB_BOTTOM => info.nMax,
                _ => *pos,
            }
        };
        self.scrollbars();
        self.schedule();
        invalidate(self.hwnd);
    }
    unsafe fn action(&mut self, id: usize) {
        match id {
            SMALLER => self.zoom(self.zoom / 1.2, self.viewport_height / 3),
            LARGER => self.zoom(self.zoom * 1.2, self.viewport_height / 3),
            FIT => self.zoom(1., self.viewport_height / 3),
            UP => self.scroll(SB_VERT, SB_PAGEUP, 0),
            DOWN => self.scroll(SB_VERT, SB_PAGEDOWN, 0),
            TOGGLE_TOC => {
                self.toc_visible = !self.toc_visible;
                self.resize();
            }
            _ => (),
        }
    }
    unsafe fn paint(&mut self, target: HDC, dirty: &RECT) {
        let rc = client(self.hwnd);
        if !self.buffer.ensure(target, rc.right, rc.bottom) {
            return;
        }
        self.paint_count += 1;
        let dc = self.buffer.dc;
        fill(dc, rc, SURFACE);
        let side = self.sidebar();
        let body = RECT {
            left: side,
            top: HEADER,
            right: rc.right,
            bottom: rc.bottom,
        };
        fill(dc, body, CANVAS);
        let current = if self.sizes.is_empty() {
            0
        } else {
            self.layout.at(self.y + GAP) + 1
        };
        if self.toc_visible {
            label(
                dc,
                &format!(
                    "{} / {}   ·   {:.0}%",
                    current,
                    self.sizes.len(),
                    self.zoom * 100.
                ),
                RECT {
                    left: 18,
                    top: 36,
                    right: side - 12,
                    bottom: 60,
                },
                self.small,
                MUTED,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
        if self.toc_visible {
            label(
                dc,
                "Outline",
                RECT {
                    left: 18,
                    top: HEADER + 10,
                    right: side - 12,
                    bottom: HEADER + 34,
                },
                self.font,
                ACCENT,
                DT_SINGLELINE,
            );
            label(
                dc,
                &self.toc_note,
                RECT {
                    left: 18,
                    top: rc.bottom - 25,
                    right: side - 12,
                    bottom: rc.bottom - 3,
                },
                self.small,
                MUTED,
                DT_SINGLELINE | DT_END_ELLIPSIS,
            );
            fill(
                dc,
                RECT {
                    left: side - 1,
                    top: HEADER,
                    right: side,
                    bottom: rc.bottom,
                },
                LINE,
            );
        }
        let saved = SaveDC(dc);
        IntersectClipRect(dc, body.left, body.top, body.right, body.bottom);
        for i in self.layout.visible(self.y, self.viewport_height) {
            let rect = self.layout.pages[i];
            let x = side + ((self.viewport_width - rect.width) / 2).max(GAP) - self.x;
            let y = HEADER + rect.top - self.y;
            fill(
                dc,
                RECT {
                    left: x + 3,
                    top: y + 4,
                    right: x + rect.width + 3,
                    bottom: y + rect.height + 4,
                },
                rgb(0, 0, 0),
            );
            fill(
                dc,
                RECT {
                    left: x,
                    top: y,
                    right: x + rect.width,
                    bottom: y + rect.height,
                },
                WHITE,
            );
            if let Some(page) = self.cache.iter().find(|p| p.index as usize == i) {
                let bitmap = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: page.width as i32,
                        biHeight: -(page.height as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..zeroed()
                    },
                    ..zeroed()
                };
                SetStretchBltMode(dc, HALFTONE);
                SetBrushOrgEx(dc, 0, 0, null_mut());
                StretchDIBits(
                    dc,
                    x,
                    y,
                    rect.width,
                    rect.height,
                    0,
                    0,
                    page.width as i32,
                    page.height as i32,
                    page.pixels.as_ptr().cast(),
                    &bitmap,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
            } else {
                label(
                    dc,
                    &format!("Page {}", i + 1),
                    RECT {
                        left: x + 20,
                        top: y + 32,
                        right: x + rect.width - 20,
                        bottom: y + 72,
                    },
                    self.small,
                    MUTED,
                    DT_CENTER | DT_SINGLELINE,
                );
            }
        }
        if !self.message.is_empty() {
            label(
                dc,
                &self.message,
                RECT {
                    left: side + 35,
                    top: HEADER + 45,
                    right: rc.right - 35,
                    bottom: HEADER + 145,
                },
                self.font,
                MUTED,
                DT_WORDBREAK,
            );
        }
        RestoreDC(dc, saved);
        self.buffer.blit(target, dirty);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_ERASEBKGND {
        return 1;
    }
    if msg == WM_SIZE {
        PostMessageW(hwnd, RESIZE, 0, 0);
        return 0;
    }
    if msg == WM_NCDESTROY {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<State>;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_PAINT {
        let mut ps: PAINTSTRUCT = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        if let Ok(mut state) = (*ptr).try_borrow_mut() {
            state.paint(dc, &ps.rcPaint);
        } else {
            SetTimer(hwnd, 8, 30, None);
        }
        EndPaint(hwnd, &ps);
        return 0;
    }
    let Ok(mut s) = (*ptr).try_borrow_mut() else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        SCROLL_TO => {
            if wp != 0 {
                s.y = lp as i32
            } else {
                s.x = lp as i32
            };
            s.scrollbars();
            s.schedule();
            invalidate(hwnd);
        }
        RESIZE => s.resize(),
        pdf::READY => s.replies(),
        ACTION | WM_COMMAND => s.action(wp & 0xffff),
        WM_NOTIFY => {
            let hdr = &*(lp as *const NMHDR);
            if hdr.hwndFrom == s.tree && hdr.code == NM_CUSTOMDRAW {
                let draw = &mut *(lp as *mut NMTVCUSTOMDRAW);
                if draw.nmcd.dwDrawStage == CDDS_PREPAINT {
                    return CDRF_NOTIFYITEMDRAW as isize;
                }
                if draw.nmcd.dwDrawStage == CDDS_ITEMPREPAINT {
                    let selected = draw.nmcd.uItemState & (CDIS_SELECTED | CDIS_FOCUS) != 0;
                    draw.clrText = if selected { ACCENT } else { INK };
                    draw.clrTextBk = if selected { SELECTED } else { SURFACE };
                    return CDRF_NEWFONT as isize;
                }
            }
            if hdr.hwndFrom == s.tree && hdr.code == TVN_SELCHANGEDW {
                let tv = &*(lp as *const NMTREEVIEWW);
                s.jump(tv.itemNew.lParam as usize);
            }
        }
        WM_VSCROLL | WM_HSCROLL => s.scroll(
            if msg == WM_VSCROLL { SB_VERT } else { SB_HORZ },
            (wp & 0xffff) as i32,
            0,
        ),
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as u16 as i16 as i32;
            if wp & MK_CONTROL as usize != 0 {
                let mut point = POINT {
                    x: lp as u16 as i16 as i32,
                    y: (lp >> 16) as u16 as i16 as i32,
                };
                ScreenToClient(hwnd, &mut point);
                let zoom = s.zoom * 1.2f32.powf(delta as f32 / 120.);
                let y = (point.y - HEADER).clamp(0, s.viewport_height);
                s.zoom(zoom, y);
            } else {
                let mut lines = 3u32;
                SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut _ as _, 0);
                let units = if lines == u32::MAX {
                    s.viewport_height
                } else {
                    (lines.min(100) * 20) as i32
                };
                s.wheel_remainder += delta * units;
                let amount = s.wheel_remainder / 120;
                s.wheel_remainder %= 120;
                if amount != 0 {
                    s.scroll(SB_VERT, 0, amount);
                }
            }
        }
        WM_KEYDOWN => match wp as u16 {
            VK_UP => s.scroll(SB_VERT, SB_LINEUP, 0),
            VK_DOWN => s.scroll(SB_VERT, SB_LINEDOWN, 0),
            VK_PRIOR => s.action(UP),
            VK_NEXT | VK_SPACE => s.action(DOWN),
            VK_HOME => s.scroll(SB_VERT, SB_TOP, 0),
            VK_END => s.scroll(SB_VERT, SB_BOTTOM, 0),
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        },
        WM_LBUTTONDOWN => {
            SetFocus(hwnd);
        }
        WM_TIMER => {
            KillTimer(hwnd, wp);
            invalidate(hwnd);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

#[test]
fn continuous_layout_keeps_anchor_and_crosses_pages() {
    let sizes = [(600., 800.), (600., 1000.), (800., 600.)];
    let a = Layout::new(&sizes, 848, 1.);
    assert_eq!(a.pages[1].top, a.pages[0].top + a.pages[0].height + GAP);
    let y = a.pages[0].top + a.pages[0].height - 60;
    assert_eq!(a.visible(y, 400), 0..2);
    let anchor = a.anchor(a.pages[1].top + 300);
    let b = Layout::new(&sizes, 848, 1.5);
    let next = b.restore(anchor);
    assert_eq!(b.at(next), 1);
    assert!((b.anchor(next).1 - anchor.1).abs() < 0.002);
    assert_eq!(a.at(a.total - 1), 2);
}

#[test]
#[ignore = "Requires Windows PDF runtime and native controls"]
fn native_continuous_reader_settles_after_scrolling() {
    use lopdf::{dictionary, Document, Object, Stream};
    use std::time::{Duration, Instant};
    let mut doc = Document::with_version("1.7");
    let pages = doc.new_object_id();
    let font =
        doc.add_object(dictionary! {"Type"=>"Font", "Subtype"=>"Type1", "BaseFont"=>"Helvetica"});
    let mut ids = Vec::new();
    for n in 1..=12 {
        let content = doc.add_object(Stream::new(dictionary!{}, format!("BT /F1 26 Tf 50 730 Td (Chapter {n} - continuous scrolling) Tj 0 -60 Td /F1 14 Tf (Wheel through page boundaries. No page switching.) Tj ET").into_bytes()));
        ids.push(doc.add_object(dictionary! {"Type"=>"Page", "Parent"=>pages, "MediaBox"=>vec![0.into(),0.into(),600.into(),800.into()], "Resources"=>dictionary!{"Font"=>dictionary!{"F1"=>font}}, "Contents"=>content}));
    }
    doc.set_object(pages, dictionary! {"Type"=>"Pages", "Count"=>12, "Kids"=>ids.iter().map(|id|Object::Reference(*id)).collect::<Vec<_>>()});
    let outline = doc.new_object_id();
    let first = doc.new_object_id();
    let second = doc.new_object_id();
    let child = doc.new_object_id();
    doc.set_object(first, dictionary!{"Title"=>Object::string_literal("Getting started"),"Parent"=>outline,"Next"=>second,"Dest"=>vec![ids[0].into(),Object::Name(b"Fit".to_vec())],"First"=>child,"Last"=>child,"Count"=>1});
    doc.set_object(child, dictionary!{"Title"=>Object::string_literal("Reading details"),"Parent"=>first,"Dest"=>vec![ids[2].into(),Object::Name(b"Fit".to_vec())]});
    doc.set_object(second, dictionary!{"Title"=>Object::string_literal("Chapter six"),"Parent"=>outline,"Prev"=>first,"Dest"=>vec![ids[5].into(),Object::Name(b"Fit".to_vec())]});
    doc.set_object(
        outline,
        dictionary! {"Type"=>"Outlines","First"=>first,"Last"=>second,"Count"=>3},
    );
    let root = doc.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages,"Outlines"=>outline});
    doc.trailer.set("Root", root);
    std::fs::create_dir_all("tmp").unwrap();
    let path = std::env::current_dir()
        .unwrap()
        .join("tmp/reader-regression.pdf");
    doc.save(&path).unwrap();
    unsafe {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TREEVIEW_CLASSES,
        });
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("Reader regression").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            1100,
            780,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let fonts = Fonts::new();
        let reader = Reader::create(parent, fonts.ui, fonts.small);
        MoveWindow(reader.0, 0, 0, 1050, 700, 0);
        reader.open(path);
        let pump = |duration: Duration| {
            let start = Instant::now();
            while start.elapsed() < duration {
                let mut msg: MSG = zeroed();
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let start = Instant::now();
        loop {
            pump(Duration::from_millis(30));
            let mut ready = false;
            with(reader.0, |s| {
                ready = s.cache.len() >= 2 && s.bookmarks.len() == 3
            });
            if ready {
                break;
            }
            if start.elapsed() >= Duration::from_secs(10) {
                with(reader.0, |s| {
                    eprintln!(
                        "sizes={} cache={} bookmarks={} viewport={}x{} error={}",
                        s.sizes.len(),
                        s.cache.len(),
                        s.bookmarks.len(),
                        s.viewport_width,
                        s.viewport_height,
                        s.message
                    )
                });
                panic!("Reader did not load");
            }
        }
        with(reader.0, |s| {
            s.y = s.layout.pages[0].top + s.layout.pages[0].height - 100;
            s.scrollbars();
            s.schedule();
            assert_eq!(s.layout.visible(s.y, s.viewport_height), 0..2);
        });
        SendMessageW(
            reader.0,
            WM_MOUSEWHEEL,
            ((-120i16 as u16 as usize) << 16) as _,
            0,
        );
        pump(Duration::from_millis(500));
        let mut before = (0, 0, 0);
        with(reader.0, |s| {
            before = (s.y, s.render_count, s.request_count)
        });
        pump(Duration::from_millis(500));
        with(reader.0, |s| {
            assert_eq!(
                before,
                (s.y, s.render_count, s.request_count),
                "Idle layout/render loop"
            );
            let anchor = s.layout.anchor(s.y + 200);
            s.zoom(1.2, 200);
            assert!((s.layout.anchor(s.y + 200).1 - anchor.1).abs() < 0.002);
            s.jump(2);
            assert_eq!(s.layout.at(s.y + GAP), 5);
            assert!(s.cache.iter().map(|p| p.pixels.len()).sum::<usize>() <= CACHE_BYTES);
        });
        ShowWindow(parent, SW_SHOWNOACTIVATE);
        // Terminal close expands the outline after its old bounds were painted.
        MoveWindow(reader.0, 0, 0, 1050, 450, 0);
        SendMessageW(reader.0, RESIZE, 0, 0);
        MoveWindow(reader.0, 0, 0, 1050, 700, 0);
        SendMessageW(reader.0, RESIZE, 0, 0);
        // A final layout must also repaint after intermediate animation paints.
        with(reader.0, |s| {
            ValidateRect(s.tree, null());
        });
        SendMessageW(reader.0, RESIZE, 0, 0);
        with(reader.0, |s| {
            let mut dirty: RECT = zeroed();
            assert_ne!(GetUpdateRect(s.tree, &mut dirty, 0), 0);
            assert_eq!(
                dirty.bottom,
                client(s.tree).bottom,
                "Expanded outline must repaint the former terminal area"
            );
            let dc = GetDC(s.tree);
            let y = client(s.tree).bottom - 20;
            SetPixel(dc, 30, y, 0x00ff00ff);
            ReleaseDC(s.tree, dc);
            UpdateWindow(s.tree);
            let dc = GetDC(s.tree);
            assert_eq!(
                GetPixel(dc, 30, y),
                SURFACE,
                "Outline repaint must erase stale terminal pixels in empty space"
            );
            ReleaseDC(s.tree, dc);
        });
        assert_eq!(SendMessageW(reader.0, WM_ERASEBKGND, 0, 0), 1);
        drop(reader);
        DestroyWindow(parent);
    }
}
