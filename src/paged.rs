//! Disk-backed document. RichEdit is a viewport, never the authoritative file.
use crate::document::{self, Encoding};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const BLOCK: usize = 32 * 1024;
#[derive(Clone, Debug, PartialEq)]
struct Piece {
    offset: u64,
    bytes: usize,
    chars: u64,
    lines: u64,
}
struct Storage {
    file: Mutex<File>,
    path: PathBuf,
}
impl Drop for Storage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
#[derive(Clone)]
pub struct Document {
    storage: Arc<Storage>,
    pieces: Vec<Piece>,
    ends: Vec<(u64, u64)>,
    pub path: PathBuf,
    pub encoding: Encoding,
    pub crlf: bool,
    stamp: (u64, std::time::SystemTime),
}
fn stamp(path: &Path) -> io::Result<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path)?;
    Ok((m.len(), m.modified()?))
}
fn temporary(parent: &Path) -> PathBuf {
    parent.join(format!(
        ".plumetxt-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}
fn normal(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
fn byte_at(text: &str, cp: u64) -> usize {
    let mut n = 0;
    for (byte, c) in text.char_indices() {
        if n >= cp {
            return byte;
        }
        n += c.len_utf16() as u64;
    }
    text.len()
}
impl Document {
    pub fn open(path: &Path, mut keep_going: impl FnMut() -> bool) -> Result<Self, String> {
        let initial = stamp(path).map_err(|e| e.to_string())?;
        let (header, _) = document::Chunk::read(path, 0)?;
        let scratch = temporary(&std::env::temp_dir());
        // DELETE_ON_CLOSE also removes the scratch file if the process exits unexpectedly.
        use std::os::windows::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(7)
            .custom_flags(0x04000000)
            .open(&scratch)
            .map_err(|e| e.to_string())?;
        let mut doc = Self {
            storage: Arc::new(Storage {
                file: Mutex::new(file),
                path: scratch,
            }),
            pieces: Vec::new(),
            ends: Vec::new(),
            path: path.into(),
            encoding: header.encoding,
            crlf: header.crlf,
            stamp: initial,
        };
        for section in document::Chunk::sections(path.into())? {
            if !keep_going() {
                return Err("Opening cancelled".into());
            }
            let text = normal(&section?);
            let pieces = doc.append(&text).map_err(|e| e.to_string())?;
            doc.pieces.extend(pieces);
        }
        if stamp(path).map_err(|e| e.to_string())? != initial {
            return Err("File changed while opening; please retry".into());
        }
        doc.reindex();
        Ok(doc)
    }
    fn reindex(&mut self) {
        self.ends.clear();
        let (mut chars, mut lines) = (0, 0);
        for p in &self.pieces {
            chars += p.chars;
            lines += p.lines;
            self.ends.push((chars, lines));
        }
    }
    fn locate(&self, cp: u64) -> (usize, u64, u64) {
        let i = self.ends.partition_point(|(end, _)| *end <= cp);
        let (chars, lines) = i.checked_sub(1).map_or((0, 0), |i| self.ends[i]);
        (i, chars, lines)
    }
    pub fn len(&self) -> u64 {
        self.ends.last().map_or(0, |e| e.0)
    }
    pub fn lines(&self) -> u64 {
        1 + self.ends.last().map_or(0, |e| e.1)
    }
    fn read(&self, piece: &Piece) -> io::Result<String> {
        let mut data = vec![0; piece.bytes];
        let mut file = self
            .storage
            .file
            .lock()
            .map_err(|_| io::Error::other("Document storage unavailable"))?;
        file.seek(SeekFrom::Start(piece.offset))?;
        file.read_exact(&mut data)?;
        String::from_utf8(data).map_err(io::Error::other)
    }
    fn append(&self, text: &str) -> io::Result<Vec<Piece>> {
        let mut pieces = Vec::new();
        let mut file = self
            .storage
            .file
            .lock()
            .map_err(|_| io::Error::other("Document storage unavailable"))?;
        let mut offset = file.seek(SeekFrom::End(0))?;
        let mut rest = text;
        while !rest.is_empty() {
            let mut end = rest.len().min(BLOCK);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let part = &rest[..end];
            file.write_all(part.as_bytes())?;
            pieces.push(Piece {
                offset,
                bytes: end,
                chars: part.encode_utf16().count() as u64,
                lines: part.bytes().filter(|c| *c == b'\n').count() as u64,
            });
            offset += end as u64;
            rest = &rest[end..];
        }
        Ok(pieces)
    }
    pub fn text(&self, start: u64, end: u64) -> io::Result<String> {
        let mut result = String::new();
        let (index, mut cp, _) = self.locate(start);
        for p in &self.pieces[index..] {
            if cp >= end {
                break;
            }
            if cp + p.chars > start {
                let text = self.read(p)?;
                result.push_str(
                    &text[byte_at(&text, start.saturating_sub(cp))
                        ..byte_at(&text, end.saturating_sub(cp))],
                );
            }
            cp += p.chars;
        }
        Ok(result)
    }
    fn align(&self, target: u64) -> io::Result<u64> {
        let (i, cp, _) = self.locate(target);
        if let Some(p) = self.pieces.get(i) {
            let text = self.read(p)?;
            Ok(cp + text[..byte_at(&text, target - cp)].encode_utf16().count() as u64)
        } else {
            Ok(cp)
        }
    }
    fn line_start(&self, target: u64) -> io::Result<u64> {
        if target <= 1 {
            return Ok(0);
        }
        let i = self.ends.partition_point(|(_, lines)| *lines < target - 1);
        let (mut cp, mut line) = i.checked_sub(1).map_or((0, 0), |i| self.ends[i]);
        if let Some(p) = self.pieces.get(i) {
            for c in self.read(p)?.chars() {
                cp += c.len_utf16() as u64;
                if c == '\n' {
                    line += 1;
                    if line == target - 1 {
                        break;
                    }
                }
            }
        }
        Ok(cp)
    }
    pub fn sections(&self) -> impl Iterator<Item = Result<String, String>> + '_ {
        self.pieces
            .iter()
            .map(|piece| self.read(piece).map_err(|e| e.to_string()))
    }
    pub fn line_at(&self, target: u64) -> io::Result<u64> {
        let (i, cp, mut lines) = self.locate(target);
        if let Some(p) = self.pieces.get(i) {
            let text = self.read(p)?;
            lines += text[..byte_at(&text, target - cp)]
                .bytes()
                .filter(|b| *b == b'\n')
                .count() as u64;
        }
        Ok(lines + 1)
    }
    fn split(&mut self, target: u64) -> io::Result<usize> {
        let mut cp = 0;
        for i in 0..self.pieces.len() {
            let p = &self.pieces[i];
            if target == cp {
                return Ok(i);
            }
            if target < cp + p.chars {
                let text = self.read(p)?;
                let byte = byte_at(&text, target - cp);
                let chars = text[..byte].encode_utf16().count() as u64;
                let lines = text[..byte].bytes().filter(|b| *b == b'\n').count() as u64;
                let tail = Piece {
                    offset: p.offset + byte as u64,
                    bytes: p.bytes - byte,
                    chars: p.chars - chars,
                    lines: p.lines - lines,
                };
                self.pieces[i].bytes = byte;
                self.pieces[i].chars = chars;
                self.pieces[i].lines = lines;
                self.pieces.insert(i + 1, tail);
                return Ok(i + 1);
            }
            cp += p.chars;
        }
        Ok(self.pieces.len())
    }
    fn replace_parts(
        &mut self,
        start: u64,
        end: u64,
        parts: impl Iterator<Item = io::Result<String>>,
    ) -> io::Result<()> {
        if start > end || end > self.len() {
            return Err(io::Error::other("Invalid document edit range"));
        }
        let mut pieces = Vec::new();
        for part in parts {
            pieces.extend(self.append(&normal(&part?))?);
        }
        let mut next = self.clone();
        let a = next.split(start)?;
        let b = next.split(end)?;
        next.pieces.splice(a..b, pieces);
        self.pieces = next.pieces;
        self.reindex();
        Ok(())
    }
    pub fn replace(&mut self, start: u64, end: u64, text: &str) -> io::Result<()> {
        self.replace_parts(start, end, std::iter::once(Ok(text.into())))
    }
    fn replace_units(&mut self, start: u64, end: u64, mut units: &[u16]) -> io::Result<()> {
        self.replace_parts(
            start,
            end,
            std::iter::from_fn(move || {
                if units.is_empty() {
                    return None;
                }
                let mut end = units.len().min(BLOCK);
                if end < units.len()
                    && ((0xd800..=0xdbff).contains(&units[end - 1])
                        || units[end - 1] == 13 && units[end] == 10)
                {
                    end -= 1;
                }
                let text = String::from_utf16(&units[..end]).map_err(io::Error::other);
                units = &units[end..];
                Some(text)
            }),
        )
    }
    pub fn save(&self, destination: &Path) -> Result<(), String> {
        if destination == self.path && stamp(destination).map_err(|e| e.to_string())? != self.stamp
        {
            return Err("The file changed on disk. Save as a different file.".into());
        }
        let tmp = temporary(destination.parent().unwrap_or(Path::new(".")));
        let result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            let bom = document::encode("", self.encoding, self.crlf);
            file.write_all(&bom)?;
            for p in &self.pieces {
                let data = document::encode(&self.read(p)?, self.encoding, self.crlf);
                file.write_all(&data[bom.len()..])?;
            }
            file.sync_all()?;
            drop(file);
            if destination == self.path && stamp(destination)? != self.stamp {
                return Err(io::Error::other("The file changed while saving"));
            }
            document::replace_file(&tmp, destination)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(tmp);
        }
        result.map_err(|e| e.to_string())
    }
    pub fn saved(&mut self, path: PathBuf) -> io::Result<()> {
        self.stamp = stamp(&path)?;
        self.path = path;
        Ok(())
    }
}

#[test]
fn disk_document_edits_across_pages_and_saves_the_real_end() {
    let path = temporary(&std::env::temp_dir());
    for encoding in [
        Encoding::Utf8,
        Encoding::Utf8Bom,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ] {
        let source = "中文😀 abc\n".repeat(30000) + "END";
        std::fs::write(&path, document::encode(&source, encoding, true)).unwrap();
        let mut doc = Document::open(&path, || true).unwrap();
        let original = doc.clone();
        assert_eq!(doc.text(0, doc.len()).unwrap(), source);
        assert_eq!(doc.lines(), 30001);
        doc.replace(doc.len(), doc.len(), " tail😀").unwrap();
        doc.replace(32766, 65539, "跨页\n").unwrap();
        let expected = source[..byte_at(&source, 32766)].to_owned()
            + "跨页\n"
            + &source[byte_at(&source, 65539)..]
            + " tail😀";
        assert_eq!(doc.text(0, doc.len()).unwrap(), expected);
        assert_eq!(original.text(0, original.len()).unwrap(), source);
        let mut pasted = original.clone();
        let clipboard = "中文😀\r\n".repeat(12000);
        pasted
            .replace_units(
                0,
                pasted.len(),
                &clipboard.encode_utf16().collect::<Vec<_>>(),
            )
            .unwrap();
        assert_eq!(pasted.text(0, pasted.len()).unwrap(), normal(&clipboard));
        let before = pasted.text(0, pasted.len()).unwrap();
        assert!(pasted.replace_units(0, pasted.len(), &[0xd800]).is_err());
        assert_eq!(pasted.text(0, pasted.len()).unwrap(), before);
        doc.save(&path).unwrap();
        doc.saved(path.clone()).unwrap();
        let reopened = Document::open(&path, || true).unwrap();
        assert_eq!(reopened.text(0, reopened.len()).unwrap(), expected);
        assert_eq!(reopened.line_at(reopened.len()).unwrap(), reopened.lines());
        std::fs::write(&path, b"external").unwrap();
        assert!(doc.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
    }
    std::fs::remove_file(path).unwrap();
}

use std::cell::RefCell;
use windows_sys::Win32::{
    Foundation::*,
    UI::{Controls::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};
pub const WINDOW_CHANGED: u32 = WM_APP + 181;
const KEY: &str = "PlumeTxtPagedDocument";
const SETTLE: usize = 918;
const WINDOW: u64 = 8 * 1024;
struct Change {
    at: usize,
    before: Vec<Piece>,
    after: Vec<Piece>,
    selection: (u64, u64),
}
struct Editor {
    doc: Document,
    saved_pieces: Vec<Piece>,
    start: u64,
    buffer: String,
    cached_start: u64,
    cached_end: u64,
    cached: String,
    last_target: u64,
    undo: Vec<Change>,
    redo: Vec<Change>,
    selection: Option<(u64, u64)>,
    composing: bool,
    merge_next: bool,
    anchor: Option<u64>,
    preview_inflight: bool,
    pending_scroll: Option<i32>,
}
unsafe fn state(hwnd: HWND) -> Option<&'static RefCell<Editor>> {
    (GetPropW(hwnd, crate::ui::wide(KEY).as_ptr()) as *const RefCell<Editor>).as_ref()
}
pub unsafe fn active(hwnd: HWND) -> bool {
    state(hwnd).is_some()
}
pub unsafe fn detach(hwnd: HWND) {
    let data = RemovePropW(hwnd, crate::ui::wide(KEY).as_ptr());
    if !data.is_null() {
        KillTimer(hwnd, SETTLE);
        KillTimer(hwnd, SETTLE + 1);
        RemoveWindowSubclass(hwnd, Some(editor_proc), SETTLE);
        drop(Box::from_raw(data as *mut RefCell<Editor>));
    }
}
unsafe fn native_text(hwnd: HWND) -> String {
    let mut units = vec![0; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
    let n = GetWindowTextW(hwnd, units.as_mut_ptr(), units.len() as i32).max(0) as usize;
    normal(&String::from_utf16_lossy(&units[..n]))
}
unsafe fn local_selection(hwnd: HWND) -> (u64, u64) {
    let (mut a, mut b) = (0u32, 0u32);
    SendMessageW(
        hwnd,
        EM_GETSEL,
        &mut a as *mut _ as usize,
        &mut b as *mut _ as isize,
    );
    (a as u64, b as u64)
}
impl Editor {
    unsafe fn selected(&self, hwnd: HWND) -> (u64, u64) {
        self.selection.unwrap_or_else(|| {
            let (a, b) = local_selection(hwnd);
            (self.start + a, self.start + b)
        })
    }
    unsafe fn install(&mut self, hwnd: HWND, next: Document) {
        let before = &self.doc.pieces;
        let after = &next.pieces;
        let prefix = before.iter().zip(after).take_while(|(a, b)| a == b).count();
        let suffix = before[prefix..]
            .iter()
            .rev()
            .zip(after[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let change = Change {
            at: prefix,
            before: before[prefix..before.len() - suffix].to_vec(),
            after: after[prefix..after.len() - suffix].to_vec(),
            selection: self.selected(hwnd),
        };
        if self.undo.len() == 100 {
            self.undo.remove(0);
        }
        self.undo.push(change);
        self.redo.clear();
        self.doc = next;
        self.cached.clear();
        self.cached_end = 0;
    }
    unsafe fn commit(&mut self, hwnd: HWND) -> io::Result<()> {
        let text = native_text(hwnd);
        if text == self.buffer {
            return Ok(());
        }
        let mut prefix = 0;
        for (a, b) in self.buffer.chars().zip(text.chars()) {
            if a != b {
                break;
            }
            prefix += a.len_utf8();
        }
        let mut suffix = 0;
        for (a, b) in self.buffer[prefix..]
            .chars()
            .rev()
            .zip(text[prefix..].chars().rev())
        {
            if a != b {
                break;
            }
            suffix += a.len_utf8();
        }
        let a = self.start + self.buffer[..prefix].encode_utf16().count() as u64;
        let b = self.start
            + self.buffer[..self.buffer.len() - suffix]
                .encode_utf16()
                .count() as u64;
        let mut next = self.doc.clone();
        next.replace(a, b, &text[prefix..text.len() - suffix])?;
        let merged = if self.merge_next {
            self.undo.pop()
        } else {
            None
        };
        self.merge_next = false;
        if let Some(change) = &merged {
            self.doc.pieces.splice(
                change.at..change.at + change.after.len(),
                change.before.clone(),
            );
        }
        self.install(hwnd, next);
        if let Some(change) = merged {
            self.undo.last_mut().unwrap().selection = change.selection;
        }
        self.buffer = text;
        self.selection = None;
        SendMessageW(hwnd, EM_SETMODIFY, 1, 0);
        SendMessageW(hwnd, EM_EMPTYUNDOBUFFER, 0, 0);
        Ok(())
    }
    unsafe fn position(&self, hwnd: HWND, target: u64, y: i32) {
        crate::scroll::measure(hwnd);
        let mut point = POINT { x: 0, y: 0 };
        SendMessageW(
            hwnd,
            EM_POSFROMCHAR,
            &mut point as *mut _ as usize,
            target.saturating_sub(self.start) as isize,
        );
        let mut viewport = POINT { x: 0, y: 0 };
        SendMessageW(hwnd, WM_USER + 221, 0, &mut viewport as *mut _ as isize);
        viewport.y = (viewport.y + point.y - y).max(0);
        crate::scroll::set_position(hwnd, true, viewport.y);
        crate::theme::invalidate(hwnd);
    }
    unsafe fn seek(&mut self, hwnd: HWND, target: u64) -> io::Result<bool> {
        let length = self.buffer.encode_utf16().count() as u64;
        if target >= self.start + WINDOW / 8 && target + WINDOW / 4 < self.start + length {
            self.position(hwnd, target, crate::theme::px(hwnd, 20));
            self.last_target = target;
            return Ok(false);
        }
        self.load(
            hwnd,
            target,
            self.selected(hwnd),
            crate::theme::px(hwnd, 20),
        )?;
        Ok(true)
    }
    unsafe fn load(
        &mut self,
        hwnd: HWND,
        target: u64,
        selection: (u64, u64),
        y: i32,
    ) -> io::Result<()> {
        let target = target.min(self.doc.len());
        let rough = target.saturating_sub(WINDOW / 2);
        let end = (target + WINDOW).min(self.doc.len());
        if self.cached.is_empty() || rough < self.cached_start || end > self.cached_end {
            // Read ahead in the travel direction, keeping a reverse margin. At
            // most 32K UTF-16 units (~96 KiB UTF-8), independent of file size.
            let backward = target < self.last_target;
            let cached_start = self.doc.align(target.saturating_sub(if backward {
                WINDOW * 3
            } else {
                WINDOW
            }))?;
            let cached = self.doc.text(
                cached_start,
                (target + if backward { WINDOW } else { WINDOW * 3 }).min(self.doc.len()),
            )?;
            self.cached_end = cached_start + cached.encode_utf16().count() as u64;
            self.cached_start = cached_start;
            self.cached = cached;
        }
        let target_byte = byte_at(&self.cached, target - self.cached_start);
        let target = self.cached_start + self.cached[..target_byte].encode_utf16().count() as u64;
        let rough_byte = byte_at(&self.cached, rough.saturating_sub(self.cached_start));
        let prefix = &self.cached[rough_byte..target_byte];
        let start_byte = rough_byte + prefix.find('\n').map_or(0, |at| at + 1);
        let start = self.cached_start + self.cached[..start_byte].encode_utf16().count() as u64;
        let end_byte = byte_at(&self.cached, end - self.cached_start);
        let buffer = self.cached[start_byte..end_byte].to_owned();
        self.last_target = target;
        let visible = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE != 0;
        let captured = GetCapture() == hwnd;
        let dirty = SendMessageW(hwnd, EM_GETMODIFY, 0, 0);
        SendMessageW(hwnd, WM_USER + 69, 0, 0);
        // WM_SETREDRAW(TRUE) makes a hidden RichEdit visible. Never send it to the source of a reading view.
        if visible {
            SendMessageW(hwnd, WM_SETREDRAW, 0, 0);
        }
        let old_end = self.start + self.buffer.encode_utf16().count() as u64;
        let new_end = start + buffer.encode_utf16().count() as u64;
        let overlap_start = self.start.max(start);
        let overlap_end = old_end.min(new_end);
        if overlap_start < overlap_end
            && self.buffer[byte_at(&self.buffer, overlap_start - self.start)
                ..byte_at(&self.buffer, overlap_end - self.start)]
                == buffer
                    [byte_at(&buffer, overlap_start - start)..byte_at(&buffer, overlap_end - start)]
        {
            // Preserve the overlapping paragraphs and their native layout/colours.
            // Replace the tail first so prefix edits cannot shift its offsets.
            let tail = crate::ui::wide(&buffer[byte_at(&buffer, overlap_end - start)..]);
            SendMessageW(
                hwnd,
                EM_SETSEL,
                (overlap_end - self.start) as usize,
                (old_end - self.start) as isize,
            );
            SendMessageW(hwnd, EM_REPLACESEL, 0, tail.as_ptr() as isize);
            let head = crate::ui::wide(&buffer[..byte_at(&buffer, overlap_start - start)]);
            SendMessageW(hwnd, EM_SETSEL, 0, (overlap_start - self.start) as isize);
            SendMessageW(hwnd, EM_REPLACESEL, 0, head.as_ptr() as isize);
        } else {
            SetWindowTextW(hwnd, crate::ui::wide(&buffer).as_ptr());
        }
        self.start = start;
        self.buffer = buffer;
        self.selection = (selection.0 < start
            || selection.1 > start + self.buffer.encode_utf16().count() as u64)
            .then_some(selection);
        let length = self.buffer.encode_utf16().count() as u64;
        SendMessageW(
            hwnd,
            EM_SETSEL,
            selection.0.saturating_sub(start).min(length) as usize,
            selection.1.saturating_sub(start).min(length) as isize,
        );
        self.position(hwnd, target, y);
        SendMessageW(hwnd, EM_EMPTYUNDOBUFFER, 0, 0);
        SendMessageW(hwnd, EM_SETMODIFY, dirty as usize, 0);
        SendMessageW(hwnd, WM_USER + 69, 0, 1);
        if visible {
            SendMessageW(hwnd, WM_SETREDRAW, 1, 0);
        }
        if captured {
            SetCapture(hwnd);
        }
        crate::theme::invalidate(hwnd);
        PostMessageW(GetParent(hwnd), WINDOW_CHANGED, hwnd as usize, 0);
        Ok(())
    }
    unsafe fn settle(&mut self, hwnd: HWND) -> io::Result<()> {
        if self.composing {
            return Ok(());
        }
        self.commit(hwnd)?;
        let line = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
        let local = SendMessageW(hwnd, EM_LINEINDEX, line as usize, 0).max(0) as u64;
        let length = self.buffer.encode_utf16().count() as u64;
        if (local < WINDOW / 8 && self.start > 0)
            || (local + WINDOW / 4 > length && self.start + length < self.doc.len())
        {
            let mut p = POINT { x: 0, y: 0 };
            SendMessageW(
                hwnd,
                EM_POSFROMCHAR,
                &mut p as *mut _ as usize,
                local as isize,
            );
            self.load(hwnd, self.start + local, self.selected(hwnd), p.y)?;
        }
        Ok(())
    }
}
pub unsafe fn attach(hwnd: HWND, doc: Document) -> Result<(), String> {
    detach(hwnd);
    let saved_pieces = doc.pieces.clone();
    let editor = Box::new(RefCell::new(Editor {
        doc,
        saved_pieces,
        start: 0,
        buffer: String::new(),
        cached_start: 0,
        cached_end: 0,
        cached: String::new(),
        last_target: 0,
        undo: Vec::new(),
        redo: Vec::new(),
        selection: None,
        composing: false,
        merge_next: false,
        anchor: None,
        preview_inflight: false,
        pending_scroll: None,
    }));
    let raw = Box::into_raw(editor);
    SetPropW(hwnd, crate::ui::wide(KEY).as_ptr(), raw as _);
    SetWindowSubclass(hwnd, Some(editor_proc), SETTLE, 0);
    let result = (*raw)
        .borrow_mut()
        .load(hwnd, 0, (0, 0), crate::theme::px(hwnd, 20))
        .map_err(|e| e.to_string());
    if result.is_err() {
        detach(hwnd);
    }
    result
}
pub unsafe fn snapshot(hwnd: HWND) -> Result<Document, String> {
    let mut s = state(hwnd).ok_or("No dynamic document")?.borrow_mut();
    s.commit(hwnd).map_err(|e| e.to_string())?;
    Ok(s.doc.clone())
}
pub unsafe fn saved(hwnd: HWND, path: PathBuf) -> Result<(), String> {
    let mut s = state(hwnd).ok_or("Document was closed")?.borrow_mut();
    s.doc.saved(path).map_err(|e| e.to_string())?;
    s.saved_pieces = s.doc.pieces.clone();
    SendMessageW(hwnd, EM_SETMODIFY, 0, 0);
    Ok(())
}
pub unsafe fn line_offset(hwnd: HWND) -> u64 {
    state(hwnd)
        .and_then(|s| s.try_borrow().ok())
        .and_then(|s| s.doc.line_at(s.start).ok())
        .unwrap_or(1)
        - 1
}
pub unsafe fn lines(hwnd: HWND) -> Option<(u64, u64)> {
    let s = state(hwnd)?.try_borrow().ok()?;
    Some((s.doc.line_at(s.selected(hwnd).0).ok()?, s.doc.lines()))
}
pub unsafe fn preview(preview: HWND, edit: HWND) {
    if preview.is_null() {
        return;
    }
    if active(edit) {
        SetPropW(
            preview,
            crate::ui::wide("PlumeTxtPagedSource").as_ptr(),
            edit,
        );
    } else {
        RemovePropW(preview, crate::ui::wide("PlumeTxtPagedSource").as_ptr());
    }
}
unsafe fn source(hwnd: HWND) -> HWND {
    let edit = GetPropW(hwnd, crate::ui::wide("PlumeTxtPagedSource").as_ptr());
    if edit.is_null() {
        hwnd
    } else {
        edit
    }
}
pub unsafe fn settle(hwnd: HWND) {
    if active(hwnd) {
        SetTimer(hwnd, SETTLE, 1, None);
    }
}
pub unsafe fn reveal_hit(hwnd: HWND, hit: &crate::search::Hit) -> Result<bool, String> {
    let mut s = state(hwnd).ok_or("No dynamic document")?.borrow_mut();
    let result = (|| -> io::Result<bool> {
        s.commit(hwnd)?;
        let line = s.doc.line_start(hit.line as u64)?;
        let text = s.doc.text(
            line,
            (line + hit.column as u64 * 2 + hit.matched.encode_utf16().count() as u64)
                .min(s.doc.len()),
        )?;
        let at = text
            .chars()
            .take(hit.column.saturating_sub(1))
            .map(|c| c.len_utf16() as u64)
            .sum::<u64>();
        let a = line + at;
        let b = a + hit.matched.encode_utf16().count() as u64;
        if s.doc.text(a, b)? != hit.matched {
            return Ok(false);
        }
        s.load(hwnd, a, (a, b), crate::theme::px(hwnd, 20))?;
        Ok(true)
    })();
    result.map_err(|e| e.to_string())
}
pub unsafe fn scroll_info(hwnd: HWND) -> Option<SCROLLINFO> {
    let hwnd = source(hwnd);
    let s = state(hwnd)?.try_borrow().ok()?;
    let line = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
    let local = SendMessageW(hwnd, EM_LINEINDEX, line as usize, 0).max(0) as u64;
    Some(SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        nMin: 0,
        nMax: 1_000_000,
        nPage: 1,
        nPos: s
            .pending_scroll
            .unwrap_or(((s.start + local) as f64 / s.doc.len().max(1) as f64 * 1_000_000.) as i32)
            .clamp(0, 1_000_000),
        nTrackPos: 0,
    })
}
pub unsafe fn scroll_to(hwnd: HWND, pos: i32) -> bool {
    let preview = source(hwnd) != hwnd;
    let hwnd = source(hwnd);
    let Some(s) = state(hwnd) else {
        return false;
    };
    let Ok(mut s) = s.try_borrow_mut() else {
        return false;
    };
    if preview && s.preview_inflight {
        s.pending_scroll = Some(pos);
        return true;
    }
    let result = s.commit(hwnd).and_then(|()| {
        let cp = (s.doc.len() as f64 * pos.clamp(0, 1_000_000) as f64 / 1_000_000.) as u64;
        let changed = s.seek(hwnd, cp)?;
        if !changed {
            PostMessageW(GetParent(hwnd), crate::scroll::SYNC, hwnd as usize, 0);
        }
        Ok(changed)
    });
    if preview {
        s.preview_inflight = matches!(result, Ok(true));
    }
    if let Err(e) = result {
        report(hwnd, &e.to_string());
    }
    true
}
pub unsafe fn preview_ready(hwnd: HWND) {
    let Some(cell) = state(hwnd) else {
        return;
    };
    let Ok(mut s) = cell.try_borrow_mut() else {
        return;
    };
    s.preview_inflight = false;
    if let Some(pos) = s.pending_scroll.take() {
        let result = s.commit(hwnd).and_then(|()| {
            let cp = (s.doc.len() as f64 * pos.clamp(0, 1_000_000) as f64 / 1_000_000.) as u64;
            let changed = s.seek(hwnd, cp)?;
            if !changed {
                PostMessageW(GetParent(hwnd), crate::scroll::SYNC, hwnd as usize, 0);
            }
            Ok(changed)
        });
        s.preview_inflight = matches!(result, Ok(true));
        if let Err(e) = result {
            report(hwnd, &e.to_string());
        }
    }
}
unsafe fn report(hwnd: HWND, message: &str) {
    MessageBoxW(
        GetParent(hwnd),
        crate::ui::wide(message).as_ptr(),
        crate::ui::wide("PlumeTxt").as_ptr(),
        MB_OK | MB_ICONERROR,
    );
}
unsafe extern "system" fn editor_proc(
    hwnd: HWND,
    msg: u32,
    wp: usize,
    lp: isize,
    _: usize,
    _: usize,
) -> isize {
    if msg == WM_NCDESTROY {
        detach(hwnd);
        return DefSubclassProc(hwnd, msg, wp, lp);
    }
    if msg == WM_TIMER && wp == SETTLE + 1 {
        if GetCapture() != hwnd {
            KillTimer(hwnd, SETTLE + 1);
            return 0;
        }
        let mut point = POINT { x: 0, y: 0 };
        GetCursorPos(&mut point);
        windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
        if point.y < 0 || point.y >= crate::theme::client(hwnd).bottom {
            SendMessageW(
                hwnd,
                WM_MOUSEMOVE,
                1,
                ((point.y as u16 as usize) << 16 | point.x as u16 as usize) as isize,
            );
        }
        return 0;
    }
    if msg == WM_LBUTTONUP || msg == WM_CAPTURECHANGED {
        KillTimer(hwnd, SETTLE + 1);
    }
    let Some(cell) = state(hwnd) else {
        return DefSubclassProc(hwnd, msg, wp, lp);
    };
    let Ok(mut s) = cell.try_borrow_mut() else {
        return DefSubclassProc(hwnd, msg, wp, lp);
    };
    let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
    let shift = GetKeyState(VK_SHIFT as i32) < 0;
    let key = msg == WM_KEYDOWN;
    if GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & ES_READONLY as u32 != 0 {
        drop(s);
        return DefSubclassProc(hwnd, msg, wp, lp);
    }
    if msg == WM_CHAR && matches!(wp, 1 | 3 | 24..=26) {
        return 0;
    }
    let mut navigation = None;
    let result = (|| -> io::Result<Option<isize>> {
        if msg == WM_UNDO || msg == WM_USER + 84 || msg == WM_LBUTTONDOWN {
            s.merge_next = false;
        }
        if msg == EM_SETSEL || msg == WM_USER + 55 {
            s.anchor = None;
            s.selection = None;
        }
        if msg == WM_MOUSEMOVE && wp & 1 != 0 && GetCapture() == hwnd {
            if let Some(anchor) = s.anchor {
                let mut point = POINT {
                    x: lp as u16 as i16 as i32,
                    y: (lp >> 16) as u16 as i16 as i32,
                };
                let height = crate::theme::client(hwnd).bottom;
                if point.y < 0 || point.y >= height {
                    let pos = crate::scroll::info(hwnd, true).nPos;
                    crate::scroll::set_position(
                        hwnd,
                        true,
                        pos + if point.y < 0 { -48 } else { 48 },
                    );
                    s.settle(hwnd)?;
                }
                point.y = point.y.clamp(0, height.saturating_sub(1));
                let cp = SendMessageW(hwnd, EM_CHARFROMPOS, 0, &point as *const _ as isize).max(0)
                    as u64;
                let active = s.start + cp.min(s.buffer.encode_utf16().count() as u64);
                let selected = (anchor.min(active), anchor.max(active));
                let length = s.buffer.encode_utf16().count() as u64;
                s.selection =
                    (selected.0 < s.start || selected.1 > s.start + length).then_some(selected);
                let mut viewport = POINT { x: 0, y: 0 };
                SendMessageW(hwnd, WM_USER + 221, 0, &mut viewport as *mut _ as isize);
                SendMessageW(
                    hwnd,
                    EM_SETSEL,
                    anchor.saturating_sub(s.start).min(length) as usize,
                    active.saturating_sub(s.start).min(length) as isize,
                );
                SendMessageW(hwnd, WM_USER + 222, 0, &viewport as *const _ as isize);
                return Ok(Some(0));
            }
        }
        if msg == WM_IME_STARTCOMPOSITION {
            s.composing = true;
        }
        if msg == WM_IME_ENDCOMPOSITION {
            s.composing = false;
            SetTimer(hwnd, SETTLE, 1, None);
        }
        if msg == WM_TIMER && wp == SETTLE {
            KillTimer(hwnd, SETTLE);
            s.settle(hwnd)?;
            return Ok(Some(0));
        }
        if msg == WM_UNDO
            || msg == WM_USER + 84
            || (key && ctrl && (wp == b'Z' as usize || wp == b'Y' as usize))
        {
            s.commit(hwnd)?;
            let redo = msg == WM_USER + 84 || wp == b'Y' as usize || shift;
            let next = if redo { s.redo.pop() } else { s.undo.pop() };
            if let Some(mut change) = next {
                let current = s.selected(hwnd);
                let selection = change.selection;
                if redo {
                    s.doc.pieces.splice(
                        change.at..change.at + change.before.len(),
                        change.after.clone(),
                    );
                } else {
                    s.doc.pieces.splice(
                        change.at..change.at + change.after.len(),
                        change.before.clone(),
                    );
                }
                s.doc.reindex();
                s.cached.clear();
                s.cached_end = 0;
                change.selection = current;
                if redo {
                    s.undo.push(change);
                } else {
                    s.redo.push(change);
                }
                s.selection = None;
                s.load(hwnd, selection.1, selection, crate::theme::px(hwnd, 20))?;
                SendMessageW(
                    hwnd,
                    EM_SETMODIFY,
                    usize::from(s.doc.pieces != s.saved_pieces),
                    0,
                );
            }
            return Ok(Some(1));
        }
        if msg == EM_CANUNDO {
            return Ok(Some(isize::from(!s.undo.is_empty())));
        }
        if key && ctrl && wp == b'A' as usize {
            s.selection = Some((0, s.doc.len()));
            SendMessageW(hwnd, EM_SETSEL, 0, -1);
            return Ok(Some(0));
        }
        if key && ctrl && (wp == VK_HOME as usize || wp == VK_END as usize) {
            s.commit(hwnd)?;
            let cp = if wp == VK_HOME as usize {
                0
            } else {
                s.doc.len()
            };
            let selected = if shift {
                let old = s.selected(hwnd);
                (old.0.min(cp), old.1.max(cp))
            } else {
                (cp, cp)
            };
            s.selection = (selected.0 != selected.1).then_some(selected);
            s.load(hwnd, cp, selected, crate::theme::px(hwnd, 20))?;
            SendMessageW(hwnd, EM_SCROLLCARET, 0, 0);
            return Ok(Some(0));
        }
        if msg == WM_COPY
            || msg == WM_CUT
            || (key && ctrl && (wp == b'C' as usize || wp == b'X' as usize))
        {
            {
                let (a, b) = s.selected(hwnd);
                if a == b {
                    return Ok(Some(0));
                }
                let text = s.doc.text(a, b)?;
                if !crate::assets::copy_text(hwnd, &text.replace('\n', "\r\n")) {
                    return Err(io::Error::other("Could not copy text"));
                }
                if msg == WM_CUT || wp == b'X' as usize {
                    let mut next = s.doc.clone();
                    next.replace(a, b, "")?;
                    s.install(hwnd, next);
                    s.selection = None;
                    s.load(hwnd, a, (a, a), crate::theme::px(hwnd, 20))?;
                    SendMessageW(hwnd, EM_SETMODIFY, 1, 0);
                }
                return Ok(Some(0));
            }
        }
        if msg == WM_USER + 64 {
            use windows_sys::Win32::System::{DataExchange::*, Memory::*};
            if OpenClipboard(hwnd) == 0 {
                return Err(io::Error::other("Clipboard is busy. Try pasting again."));
            }
            let result = (|| -> io::Result<Option<(Document, u64)>> {
                let handle = GetClipboardData(13);
                if handle.is_null() {
                    return Ok(None);
                }
                let ptr = GlobalLock(handle) as *const u16;
                if ptr.is_null() {
                    return Err(io::Error::other("Could not read clipboard text"));
                }
                let units = std::slice::from_raw_parts(ptr, GlobalSize(handle) / 2);
                let units = &units[..units.iter().position(|c| *c == 0).unwrap_or(units.len())];
                let (a, b) = s.selected(hwnd);
                let mut next = s.doc.clone();
                let result = next.replace_units(a, b, units);
                GlobalUnlock(handle);
                result?;
                let caret = a + next.len() - (s.doc.len() - (b - a));
                Ok(Some((next, caret)))
            })();
            CloseClipboard();
            if let Some((next, caret)) = result? {
                s.install(hwnd, next);
                s.load(hwnd, caret, (caret, caret), crate::theme::px(hwnd, 20))?;
                SendMessageW(hwnd, EM_SETMODIFY, 1, 0);
            }
            return Ok(Some(0));
        }
        if let Some((a, b)) = s.selection {
            if msg == WM_CHAR && wp >= 32
                || msg == WM_CHAR && (wp == 8 || wp == 13 || wp == 9)
                || msg == WM_CLEAR
                || key && wp == VK_DELETE as usize
                || msg == WM_USER + 64
                || msg == EM_REPLACESEL
                || msg == WM_IME_STARTCOMPOSITION
            {
                let mut next = s.doc.clone();
                next.replace(a, b, "")?;
                s.install(hwnd, next);
                s.selection = None;
                s.load(hwnd, a, (a, a), crate::theme::px(hwnd, 20))?;
                SendMessageW(hwnd, EM_SETMODIFY, 1, 0);
                if msg == WM_CLEAR || key && wp == VK_DELETE as usize || msg == WM_CHAR && wp == 8 {
                    return Ok(Some(0));
                }
                s.merge_next = a != b;
            }
        }
        if key && shift && !ctrl && matches!(wp, 33..=40) {
            s.commit(hwnd)?;
            let (a, b) = s.selected(hwnd);
            let anchor = *s.anchor.get_or_insert(a);
            let active = if a == anchor { b } else { a };
            let length = s.buffer.encode_utf16().count() as u64;
            if active < s.start + WINDOW / 8 || active + WINDOW / 8 > s.start + length {
                s.load(hwnd, active, (active, active), crate::theme::px(hwnd, 20))?;
            }
            SendMessageW(
                hwnd,
                EM_SETSEL,
                active.saturating_sub(s.start) as usize,
                active.saturating_sub(s.start) as isize,
            );
            navigation = Some((anchor, matches!(wp, 33 | 36..=38)));
            return Ok(None);
        }
        if msg == WM_LBUTTONDOWN || key && !ctrl && matches!(wp, 33..=40) {
            s.anchor = None;
            s.selection = None;
        }
        if key && matches!(wp, 8 | 33..=40 | 46) {
            let (a, b) = local_selection(hwnd);
            let length = s.buffer.encode_utf16().count() as u64;
            if a < WINDOW / 8 && s.start > 0
                || b + WINDOW / 8 > length && s.start + length < s.doc.len()
            {
                let selection = s.selected(hwnd);
                s.commit(hwnd)?;
                s.load(hwnd, selection.1, selection, crate::theme::px(hwnd, 20))?;
            }
        }
        Ok(None)
    })();
    match result {
        Ok(Some(value)) => return value,
        Err(e) => {
            report(hwnd, &e.to_string());
            return 0;
        }
        _ => (),
    }
    drop(s);
    let value = DefSubclassProc(hwnd, msg, wp, lp);
    if msg == WM_LBUTTONDOWN {
        if let Ok(mut s) = cell.try_borrow_mut() {
            s.anchor = Some(s.start + local_selection(hwnd).0);
            SetCapture(hwnd);
            SetTimer(hwnd, SETTLE + 1, 40, None);
        }
    }
    if let Some((anchor, backward)) = navigation {
        if let Ok(mut s) = cell.try_borrow_mut() {
            let (a, b) = local_selection(hwnd);
            let active = s.start + if backward { a } else { b };
            let selected = (anchor.min(active), anchor.max(active));
            let length = s.buffer.encode_utf16().count() as u64;
            s.selection =
                (selected.0 < s.start || selected.1 > s.start + length).then_some(selected);
            SendMessageW(
                hwnd,
                EM_SETSEL,
                anchor.saturating_sub(s.start).min(length) as usize,
                active.saturating_sub(s.start).min(length) as isize,
            );
        }
    }
    if matches!(
        msg,
        WM_CHAR
            | WM_KEYDOWN
            | WM_PASTE
            | WM_CUT
            | WM_CLEAR
            | EM_REPLACESEL
            | WM_MOUSEWHEEL
            | WM_VSCROLL
            | WM_LBUTTONUP
    ) || msg == WM_USER + 64
    {
        if let Ok(mut s) = cell.try_borrow_mut() {
            if !s.composing {
                if let Err(e) = s.commit(hwnd) {
                    report(hwnd, &e.to_string());
                }
            }
        }
        SetTimer(hwnd, SETTLE, 1, None);
    }
    value
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_dynamic_edit_scroll_undo_and_save() {
    unsafe {
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
        LoadLibraryW(crate::ui::wide("Msftedit.dll").as_ptr());
        let parent = CreateWindowExW(
            0,
            crate::ui::wide("STATIC").as_ptr(),
            null(),
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
        let fonts = crate::theme::Fonts::new();
        let edit = crate::ui::rich_edit(parent, false, fonts.body);
        crate::scroll::resize(edit, 0, 0, 900, 600);
        ShowWindow(edit, SW_SHOW);
        let path = temporary(&std::env::temp_dir());
        let source = "Line 中文😀 text abcdefghijklmnopqrstuvwxyz\n".repeat(20000) + "REAL_END";
        std::fs::write(&path, source.as_bytes()).unwrap();
        attach(edit, Document::open(&path, || true).unwrap()).unwrap();
        assert!(native_text(edit).len() < 100_000);
        assert_eq!(lines(edit).unwrap().1, 20001);
        for pos in [250000, 500000, 750000, 1000000, 0] {
            assert!(scroll_to(edit, pos));
            let s = state(edit).unwrap().borrow();
            assert_eq!(
                native_text(edit),
                s.doc
                    .text(s.start, s.start + s.buffer.encode_utf16().count() as u64)
                    .unwrap()
            );
            assert!(s.buffer.encode_utf16().count() as u64 <= WINDOW * 2);
        }
        assert!(scroll_to(edit, 1_000_000));
        let doc = crate::syntax::document(edit).unwrap();
        let end = doc.Range(0, 0).unwrap().GetStoryLength().unwrap() - 1;
        let range = windows::Win32::UI::Controls::RichEdit::CHARRANGE {
            cpMin: end,
            cpMax: end,
        };
        SendMessageW(edit, WM_USER + 55, 0, &range as *const _ as isize); // EM_EXSETSEL
        SendMessageW(
            edit,
            EM_REPLACESEL,
            1,
            crate::ui::wide(" SAVED😀").as_ptr() as isize,
        );
        assert!(snapshot(edit)
            .unwrap()
            .text(0, snapshot(edit).unwrap().len())
            .unwrap()
            .ends_with("REAL_END SAVED😀"));
        scroll_to(edit, 0);
        SendMessageW(edit, EM_SETSEL, 0, 0);
        SendMessageW(
            edit,
            EM_REPLACESEL,
            1,
            crate::ui::wide("START\n").as_ptr() as isize,
        );
        SendMessageW(edit, WM_UNDO, 0, 0);
        let doc = snapshot(edit).unwrap();
        assert_eq!(doc.text(0, doc.len()).unwrap(), source.clone() + " SAVED😀");
        SendMessageW(edit, WM_UNDO, 0, 0);
        let doc = snapshot(edit).unwrap();
        assert_eq!(doc.text(0, doc.len()).unwrap(), source);
        SendMessageW(edit, WM_USER + 84, 0, 0);
        let doc = snapshot(edit).unwrap();
        doc.save(&path).unwrap();
        saved(edit, path.clone()).unwrap();
        assert_eq!(SendMessageW(edit, EM_GETMODIFY, 0, 0), 0);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            source.clone() + " SAVED😀"
        );
        // Adjacent windows use the bounded read-ahead and preserve native paragraphs.
        unsafe extern "system" fn count_resets(
            hwnd: HWND,
            msg: u32,
            wp: usize,
            lp: isize,
            _: usize,
            data: usize,
        ) -> isize {
            if msg == WM_SETTEXT {
                let count = &*(data as *const std::cell::Cell<usize>);
                count.set(count.get() + 1);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        let resets = std::cell::Cell::new(0usize);
        SetWindowSubclass(edit, Some(count_resets), 9197, &resets as *const _ as usize);
        {
            let mut s = state(edit).unwrap().borrow_mut();
            s.load(edit, WINDOW * 4, (0, 0), 20).unwrap();
            let cached = s.cached.as_ptr();
            let count = resets.get();
            s.load(edit, WINDOW * 4 + WINDOW / 2, (0, 0), 20).unwrap();
            assert_eq!(
                s.cached.as_ptr(),
                cached,
                "Nearby scrolling must reuse read-ahead data"
            );
            assert!(s.cached.encode_utf16().count() as u64 <= WINDOW * 4 + 1);
            assert_eq!(
                resets.get(),
                count,
                "Overlapping windows must not reset the entire editor"
            );
            assert_eq!(native_text(edit), s.buffer);
        }
        RemoveWindowSubclass(edit, Some(count_resets), 9197);
        // Shift the viewport continuously through its trailing boundary.
        scroll_to(edit, 0);
        for _ in 0..20 {
            let max = crate::scroll::limit(&crate::scroll::info(edit, true));
            crate::scroll::set_position(edit, true, max);
            let line = SendMessageW(edit, EM_GETFIRSTVISIBLELINE, 0, 0);
            let local = SendMessageW(edit, EM_LINEINDEX, line as usize, 0).max(0) as u64;
            let global = state(edit).unwrap().borrow().start + local;
            let mut before = POINT { x: 0, y: 0 };
            SendMessageW(
                edit,
                EM_POSFROMCHAR,
                &mut before as *mut _ as usize,
                local as isize,
            );
            SendMessageW(edit, WM_TIMER, SETTLE, 0);
            let start = state(edit).unwrap().borrow().start;
            let mut after = POINT { x: 0, y: 0 };
            SendMessageW(
                edit,
                EM_POSFROMCHAR,
                &mut after as *mut _ as usize,
                (global - start) as isize,
            );
            assert_eq!(
                after.y, before.y,
                "Refilling must keep the visible line at the same pixel"
            );
        }
        assert!(state(edit).unwrap().borrow().start > WINDOW);
        let doc = snapshot(edit).unwrap();
        assert_eq!(doc.text(0, doc.len()).unwrap(), source + " SAVED😀");
        ShowWindow(parent, SW_SHOWNA);
        SetFocus(edit);
        scroll_to(edit, 0);
        SendMessageW(edit, EM_SETSEL, 0, 0);
        let mut keyboard = [0u8; 256];
        GetKeyboardState(keyboard.as_mut_ptr());
        let original_keyboard = keyboard;
        keyboard[VK_SHIFT as usize] = 0x80;
        SetKeyboardState(keyboard.as_ptr());
        for _ in 0..40 {
            SendMessageW(edit, WM_KEYDOWN, VK_NEXT as usize, 0);
        }
        SetKeyboardState(original_keyboard.as_ptr());
        let selected = state(edit).unwrap().borrow().selected(edit);
        assert_eq!(selected.0, 0);
        assert!(
            selected.1 > WINDOW,
            "Shift+PageDown must extend across text windows: {selected:?}, local {:?}",
            local_selection(edit)
        );
        let before = snapshot(edit).unwrap();
        SendMessageW(edit, WM_CLEAR, 0, 0);
        let cut = snapshot(edit).unwrap();
        assert_eq!(
            cut.text(0, cut.len()).unwrap(),
            before.text(selected.1, before.len()).unwrap()
        );
        SendMessageW(edit, WM_UNDO, 0, 0);
        let before = snapshot(edit).unwrap();
        let mut keys = [0u8; 256];
        GetKeyboardState(keys.as_mut_ptr());
        let saved_keys = keys;
        keys[VK_CONTROL as usize] = 0x80;
        SetKeyboardState(keys.as_ptr());
        SendMessageW(edit, WM_KEYDOWN, b'A' as usize, 0);
        SetKeyboardState(saved_keys.as_ptr());
        SendMessageW(edit, WM_CHAR, b'X' as usize, 0);
        assert_eq!(snapshot(edit).unwrap().text(0, 10).unwrap(), "X");
        SendMessageW(edit, WM_UNDO, 0, 0);
        let restored = snapshot(edit).unwrap();
        assert_eq!(
            restored.text(0, restored.len()).unwrap(),
            before.text(0, before.len()).unwrap()
        );
        scroll_to(edit, 0);
        SendMessageW(edit, EM_SETSEL, 0, 0);
        SendMessageW(edit, WM_LBUTTONDOWN, 1, (24 << 16) | 40);
        for _ in 0..200 {
            SendMessageW(edit, WM_MOUSEMOVE, 1, (620 << 16) | 100);
        }
        SendMessageW(edit, WM_LBUTTONUP, 0, (620 << 16) | 100);
        let dragged = state(edit).unwrap().borrow().selected(edit);
        assert!(
            dragged.1 - dragged.0 > WINDOW,
            "Mouse selection must cross windows: {dragged:?}"
        );
        // A final line after a long line can leave both local scroll ranges empty.
        let edit = crate::ui::rich_edit(parent, false, fonts.body);
        crate::scroll::resize(edit, 0, 0, 900, 600);
        std::fs::write(&path, format!("{}\nEND", "x".repeat(WINDOW as usize * 4))).unwrap();
        attach(edit, Document::open(&path, || true).unwrap()).unwrap();
        let preview_control = crate::ui::rich_edit(parent, true, fonts.body);
        crate::scroll::resize(preview_control, 0, 0, 900, 600);
        SetWindowTextW(preview_control, crate::ui::wide("END").as_ptr());
        ShowWindow(edit, SW_SHOWNOACTIVATE);
        ShowWindow(preview_control, SW_SHOWNOACTIVATE);
        preview(preview_control, edit);
        crate::scroll::pair(edit, preview_control);
        crate::scroll::pair(preview_control, edit);
        for control in [edit, preview_control] {
            assert!(scroll_to(edit, 1_000_000));
            // Match the zero native range reported by the short final preview.
            for local in [edit, preview_control] {
                crate::scroll::measure(local);
                let empty = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                    nPage: 1,
                    ..std::mem::zeroed()
                };
                SetScrollInfo(local, SB_VERT, &empty, 0);
            }
            assert_eq!(crate::scroll::limit(&crate::scroll::info(edit, true)), 0);
            crate::scroll::set_position(preview_control, true, 0);
            let bar = FindWindowExW(
                parent,
                std::ptr::null_mut(),
                crate::ui::wide("PlumeTxtScroll").as_ptr(),
                crate::ui::wide("Vertical scroll").as_ptr(),
            );
            assert!(IsWindowVisible(bar) != 0, "Global scrollbar must stay visible at EOF");
            let end_start = state(edit).unwrap().borrow().start;
            SendMessageW(control, WM_MOUSEWHEEL, 120usize << 16, 0);
            assert!(
                state(edit).unwrap().borrow().start < end_start,
                "Wheel must leave a short final page in both editing and preview"
            );
        }
        DestroyWindow(parent);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
#[ignore = "Writes a 1 GiB text file and its temporary disk document"]
fn gigabyte_document_keeps_only_an_index_in_memory() {
    let path = temporary(&std::env::temp_dir());
    let mut file = File::create(&path).unwrap();
    let block = "abcdefghijklmno\n".repeat(65536);
    for _ in 0..1024 {
        file.write_all(block.as_bytes()).unwrap();
    }
    drop(file);
    let started = std::time::Instant::now();
    let mut doc = Document::open(&path, || true).unwrap();
    let index_bytes = doc.pieces.capacity() * std::mem::size_of::<Piece>();
    eprintln!(
        "1 GiB indexed in {:?}; index allocation {} bytes; editor window <= {} UTF-16 units",
        started.elapsed(),
        index_bytes,
        WINDOW * 2
    );
    assert_eq!(doc.len(), 1024 * 1024 * 1024);
    assert!(index_bytes < 4 * 1024 * 1024);
    let end = doc.len();
    doc.replace(end, end, "REAL_END😀").unwrap();
    doc.save(&path).unwrap();
    let mut file = File::open(&path).unwrap();
    let expected = "REAL_END😀".as_bytes();
    file.seek(SeekFrom::End(-(expected.len() as i64))).unwrap();
    let mut tail = vec![0; expected.len()];
    file.read_exact(&mut tail).unwrap();
    assert_eq!(tail, expected);
    drop(file);
    drop(doc);
    std::fs::remove_file(path).unwrap();
}
