use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

pub const MAX_TEXT_BYTES: u64 = 32 * 1024 * 1024;

/// A bounded editable slice. Untouched bytes are copied verbatim on save.
#[derive(Clone)]
pub struct Chunk {
    pub path: std::path::PathBuf,
    pub start: u64,
    end: u64,
    len: u64,
    modified: std::time::SystemTime,
    pub encoding: Encoding,
    pub crlf: bool,
}
impl Chunk {
    pub fn sections(
        path: std::path::PathBuf,
    ) -> Result<impl Iterator<Item = Result<String, String>>, String> {
        use std::os::windows::fs::OpenOptionsExt;
        let guard = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let len = guard.metadata().map_err(|e| e.to_string())?.len();
        let mut offset = 0;
        let mut done = false;
        Ok(std::iter::from_fn(move || {
            let _keep_locked = &guard;
            if done {
                return None;
            }
            match Self::read(&path, offset) {
                Ok((chunk, mut text)) => {
                    // Prefer a paragraph boundary without exceeding the bounded read window.
                    let mut end = chunk.end;
                    if end < len {
                        let boundary = text
                            .rfind("\n\n")
                            .map(|at| at + 2)
                            .or_else(|| text.rfind("\r\n\r\n").map(|at| at + 4));
                        if let Some(at) = boundary.filter(|at| *at > text.len() / 2) {
                            let tail_bytes = match chunk.encoding {
                                Encoding::Utf16Le | Encoding::Utf16Be => {
                                    text[at..].encode_utf16().count() * 2
                                }
                                _ => text.len() - at,
                            };
                            end -= tail_bytes as u64;
                            text.truncate(at);
                        }
                    }
                    done = end >= len;
                    if !done && end <= offset {
                        done = true;
                        return Some(Err("Could not advance through the document".into()));
                    }
                    offset = end;
                    Some(Ok(text))
                }
                Err(e) => {
                    done = true;
                    Some(Err(e))
                }
            }
        }))
    }
    pub fn read(path: &Path, offset: u64) -> Result<(Self, String), String> {
        use std::io::{Seek, SeekFrom};
        use std::os::windows::fs::OpenOptionsExt;
        let run = || -> io::Result<(Self, String)> {
            let mut file = OpenOptions::new().read(true).share_mode(1).open(path)?;
            let meta = file.metadata()?;
            let mut header = [0; 3];
            let n = file.read(&mut header)?;
            let (encoding, bom) = if header[..n].starts_with(&[0xff, 0xfe]) {
                (Encoding::Utf16Le, 2)
            } else if header[..n].starts_with(&[0xfe, 0xff]) {
                (Encoding::Utf16Be, 2)
            } else if header[..n].starts_with(&[0xef, 0xbb, 0xbf]) {
                (Encoding::Utf8Bom, 3)
            } else {
                (Encoding::Utf8, 0)
            };
            let utf16 = matches!(encoding, Encoding::Utf16Le | Encoding::Utf16Be);
            let mut start = offset.min(meta.len()).max(bom);
            if utf16 {
                start -= (start - bom) % 2;
            }
            file.seek(SeekFrom::Start(start))?;
            let mut bytes = Vec::new();
            (&mut file).take(64 * 1024).read_to_end(&mut bytes)?;
            let unit = |b: &[u8]| {
                if encoding == Encoding::Utf16Le {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            };
            let skip = if utf16 {
                usize::from(bytes.len() >= 2 && (0xdc00..=0xdfff).contains(&unit(&bytes))) * 2
            } else {
                bytes.iter().take_while(|b| **b & 0xc0 == 0x80).count()
            };
            bytes.drain(..skip);
            start += skip as u64;
            // Leave an entire CRLF outside the region if navigation landed on its LF.
            let width = if utf16 { 2 } else { 1 };
            if start >= bom + width
                && bytes.len() >= width as usize
                && (if utf16 {
                    unit(&bytes) == 10
                } else {
                    bytes[0] == b'\n'
                })
            {
                file.seek(SeekFrom::Start(start - width))?;
                let mut previous = [0u8; 2];
                file.read_exact(&mut previous[..width as usize])?;
                if if utf16 {
                    unit(&previous) == 13
                } else {
                    previous[0] == b'\r'
                } {
                    bytes.drain(..width as usize);
                    start += width;
                }
            }
            if start + (bytes.len() as u64) < meta.len() {
                if utf16 {
                    bytes.truncate(bytes.len() / 2 * 2);
                    if bytes.len() >= 2
                        && (0xd800..=0xdbff).contains(&unit(&bytes[bytes.len() - 2..]))
                    {
                        bytes.truncate(bytes.len() - 2);
                    }
                    if bytes.len() >= 2 && unit(&bytes[bytes.len() - 2..]) == 13 {
                        bytes.truncate(bytes.len() - 2);
                    }
                } else {
                    if let Err(e) = std::str::from_utf8(&bytes) {
                        if e.error_len().is_none() {
                            bytes.truncate(e.valid_up_to());
                        }
                    }
                    if bytes.last() == Some(&b'\r') {
                        bytes.pop();
                    }
                }
            }
            let end = start + bytes.len() as u64;
            let encoded = match encoding {
                Encoding::Utf16Le => [vec![0xff, 0xfe], bytes].concat(),
                Encoding::Utf16Be => [vec![0xfe, 0xff], bytes].concat(),
                // Add a synthetic BOM so a literal U+FEFF at the slice start is not stripped.
                _ => [vec![0xef, 0xbb, 0xbf], bytes].concat(),
            };
            let (text, _) = decode(&encoded).map_err(io::Error::other)?;
            Ok((
                Self {
                    path: path.into(),
                    start,
                    end,
                    len: meta.len(),
                    modified: meta.modified()?,
                    encoding,
                    crlf: text.contains("\r\n"),
                },
                text,
            ))
        };
        run().map_err(|e| e.to_string())
    }

    pub fn save(&self, destination: &Path, text: &str) -> Result<(), String> {
        use std::io::{Seek, SeekFrom};
        use std::os::windows::fs::OpenOptionsExt;
        let temp = destination.with_file_name(format!(
            ".featherpad-{}-{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let run = || -> io::Result<()> {
            // Deny writes/replacement while streaming, so the copied source is consistent.
            let mut source = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&self.path)?;
            let meta = source.metadata()?;
            if meta.len() != self.len || meta.modified()? != self.modified {
                return Err(io::Error::other(
                    "The file changed on disk. Reopen it before saving this region.",
                ));
            }
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            if io::copy(&mut (&mut source).take(self.start), &mut output)? != self.start {
                return Err(io::Error::other("Source became shorter while saving"));
            }
            let bytes = encode(text, self.encoding, self.crlf);
            let bom = match self.encoding {
                Encoding::Utf8 => 0,
                Encoding::Utf8Bom => 3,
                _ => 2,
            };
            output.write_all(&bytes[bom..])?;
            source.seek(SeekFrom::Start(self.end))?;
            if io::copy(&mut source, &mut output)? != self.len - self.end {
                return Err(io::Error::other("Source changed while saving"));
            }
            output.sync_all()?;
            drop(output);
            drop(source);
            replace_file(&temp, destination)
        };
        let result = run().map_err(|e| e.to_string());
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

#[test]
fn region_edits_preserve_surrounding_bytes_and_reject_external_changes() {
    let root = std::env::temp_dir().join(format!("featherpad-region-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    let copy = root.join("copy.txt");
    for encoding in [
        Encoding::Utf8,
        Encoding::Utf8Bom,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ] {
        let source = "中文😀\r\n\u{feff}Mixed text\r\n".repeat(9000);
        let original = encode(&source, encoding, true);
        for offset in [0, 11, 65533, original.len() as u64] {
            fs::write(&path, &original).unwrap();
            if offset == 0 {
                let sections: Vec<_> = Chunk::sections(path.clone())
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                assert!(sections.len() > 1);
                assert_eq!(
                    sections.concat(),
                    source,
                    "Streaming must include every character once"
                );
            }
            let (chunk, text) = Chunk::read(&path, offset).unwrap();
            assert!(chunk.end - chunk.start <= 65536);
            assert!(!text.contains('\u{fffd}'));
            chunk.save(&copy, &text).unwrap();
            assert_eq!(
                fs::read(&copy).unwrap(),
                original,
                "unchanged region must round-trip"
            );
            let replacement = format!("Inserted 中文😀\n{text}tail");
            let encoded = encode(&replacement, chunk.encoding, chunk.crlf);
            let bom = match encoding {
                Encoding::Utf8 => 0,
                Encoding::Utf8Bom => 3,
                _ => 2,
            };
            let expected = [
                &original[..chunk.start as usize],
                &encoded[bom..],
                &original[chunk.end as usize..],
            ]
            .concat();
            chunk.save(&copy, &replacement).unwrap();
            assert_eq!(fs::read(&copy).unwrap(), expected);
            assert_eq!(fs::read(&path).unwrap(), original);
            decode(&expected).unwrap();
            chunk.save(&path, &replacement).unwrap();
            assert_eq!(fs::read(&path).unwrap(), expected);
            let (stale, _) = Chunk::read(&path, 0).unwrap();
            fs::write(&path, b"external update").unwrap();
            assert!(stale.save(&path, "must not overwrite").is_err());
            assert_eq!(fs::read(&path).unwrap(), b"external update");
        }
    }
    fs::remove_file(path).unwrap();
    fs::remove_file(copy).unwrap();
    fs::remove_dir(root).unwrap();
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

pub fn decode(bytes: &[u8]) -> Result<(String, Encoding), String> {
    let (text, encoding) = if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if !bytes.len().is_multiple_of(2) {
            return Err("Invalid UTF-16 length".into());
        }
        let le = bytes[0] == 0xff;
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|b| {
                if le {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect();
        (
            String::from_utf16(&units).map_err(|_| "Invalid UTF-16 encoding")?,
            if le {
                Encoding::Utf16Le
            } else {
                Encoding::Utf16Be
            },
        )
    } else {
        let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
        (
            std::str::from_utf8(if bom { &bytes[3..] } else { bytes })
                .map_err(|_| "Expected UTF-8 or UTF-16 with BOM")?
                .to_owned(),
            if bom {
                Encoding::Utf8Bom
            } else {
                Encoding::Utf8
            },
        )
    };
    if text.contains('\0') {
        return Err("Cannot edit binary data as text".into());
    }
    Ok((text, encoding))
}

pub fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        return Err("Text exceeds the 32 MiB limit".into());
    }
    Ok(bytes)
}

pub fn read(path: &Path) -> Result<(String, Encoding, u64), String> {
    let bytes = read_bytes(path)?;
    let (text, encoding) = decode(&bytes)?;
    Ok((text, encoding, fingerprint(&bytes)))
}

pub fn encode(text: &str, encoding: Encoding, crlf: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = if crlf {
        text.replace('\n', "\r\n")
    } else {
        text
    };
    match encoding {
        Encoding::Utf8 => text.into_bytes(),
        Encoding::Utf8Bom => [vec![0xef, 0xbb, 0xbf], text.into_bytes()].concat(),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let le = encoding == Encoding::Utf16Le;
            let mut out = if le {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                out.extend(if le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            out
        }
    }
}

pub fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_file_name(format!(
        ".featherpad-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let src: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let dst: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe {
        MoveFileExW(
            src.as_ptr(),
            dst.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[test]
fn encoding_and_safe_save() {
    for e in [
        Encoding::Utf8,
        Encoding::Utf8Bom,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ] {
        let bytes = encode("中文 😀\nsecond", e, true);
        assert_eq!(decode(&bytes).unwrap(), ("中文 😀\r\nsecond".into(), e));
    }
    assert!(decode(&[0xff]).is_err());
    assert!(decode(b"a\0b").is_err());
    let path = std::env::temp_dir().join(format!("featherpad-test-{}.txt", std::process::id()));
    atomic_write(&path, b"first").unwrap();
    atomic_write(&path, b"second").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"second");
    fs::remove_file(path).unwrap();
}
