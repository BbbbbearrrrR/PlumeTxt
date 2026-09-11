use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

pub const MAX_TEXT_BYTES: u64 = 32 * 1024 * 1024;

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
