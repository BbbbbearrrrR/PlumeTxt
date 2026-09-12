use crate::document::{self, Encoding};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub struct Snapshot {
    pub source: String,
    pub encoding: Encoding,
    pub fingerprint: u64,
}
pub struct Watch {
    stop: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<Result<Snapshot, String>>>>,
}
impl Watch {
    pub fn new(path: PathBuf, original: u64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let latest = Arc::new(Mutex::new(None));
        let result = Self {
            stop: stop.clone(),
            latest: latest.clone(),
        };
        std::thread::spawn(move || {
            let mut seen = original;
            let mut accepted = None;
            let mut candidate = None;
            let mut failed = false;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(400));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let stamp = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok().map(|time| (m.len(), time)));
                if stamp.is_none() {
                    if !failed {
                        *latest.lock().unwrap() =
                            Some(Err("File unavailable; waiting for it to return".into()));
                        failed = true;
                    }
                    accepted = None;
                    continue;
                }
                // Wait for two matching samples so truncate/write and atomic replacements settle.
                if stamp != candidate {
                    candidate = stamp;
                    continue;
                }
                if stamp == accepted {
                    continue;
                }
                let read = document::read(&path);
                let after = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok().map(|time| (m.len(), time)));
                if after != stamp {
                    candidate = after;
                    continue;
                }
                match read {
                    Ok((source, encoding, fingerprint)) => {
                        accepted = stamp;
                        if fingerprint != seen || failed {
                            seen = fingerprint;
                            *latest.lock().unwrap() = Some(Ok(Snapshot {
                                source,
                                encoding,
                                fingerprint,
                            }));
                        }
                        failed = false;
                    }
                    Err(e) => {
                        accepted = stamp;
                        if !failed {
                            *latest.lock().unwrap() = Some(Err(e));
                            failed = true;
                        }
                    }
                }
            }
        });
        result
    }
    pub fn take(&self) -> Option<Result<Snapshot, String>> {
        self.latest.lock().unwrap().take()
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// Unique shared lines anchor a patience diff; repeated lines in gaps retain common ends.
// This bounds work to O(n log n), avoiding quadratic comparisons on large rewrites.
pub fn lines<'a>(old: &'a str, new: &'a str) -> Vec<(char, usize, &'a str)> {
    let a: Vec<_> = old.split('\n').map(|s| s.trim_end_matches('\r')).collect();
    let b: Vec<_> = new.split('\n').map(|s| s.trim_end_matches('\r')).collect();
    let mut index = HashMap::new();
    for (i, line) in b.iter().enumerate() {
        index
            .entry(*line)
            .and_modify(|v| *v = usize::MAX)
            .or_insert(i);
    }
    let mut counts = HashMap::new();
    for line in &a {
        *counts.entry(*line).or_insert(0usize) += 1;
    }
    let pairs: Vec<_> = a
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            index
                .get(line)
                .filter(|&&j| j != usize::MAX && counts[line] == 1)
                .map(|&j| (i, j))
        })
        .collect();
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; pairs.len()];
    for (i, &(_, j)) in pairs.iter().enumerate() {
        let p = tails.partition_point(|&k| pairs[k].1 < j);
        if p > 0 {
            previous[i] = Some(tails[p - 1]);
        }
        if p == tails.len() {
            tails.push(i);
        } else {
            tails[p] = i;
        }
    }
    let mut anchors = Vec::new();
    let mut at = tails.last().copied();
    while let Some(i) = at {
        anchors.push(pairs[i]);
        at = previous[i];
    }
    anchors.reverse();
    anchors.push((a.len(), b.len()));
    let mut out = Vec::new();
    let (mut x, mut y) = (0, 0);
    for (ax, by) in anchors {
        while x < ax && y < by && a[x] == b[y] {
            out.push((' ', y + 1, b[y]));
            x += 1;
            y += 1;
        }
        let (mut ex, mut ey) = (ax, by);
        while ex > x && ey > y && a[ex - 1] == b[ey - 1] {
            ex -= 1;
            ey -= 1;
        }
        for (i, line) in a.iter().enumerate().take(ex).skip(x) {
            out.push(('-', i + 1, *line));
        }
        for (i, line) in b.iter().enumerate().take(ey).skip(y) {
            out.push(('+', i + 1, *line));
        }
        for (i, line) in b.iter().enumerate().take(by).skip(ey) {
            out.push((' ', i + 1, *line));
        }
        if by < b.len() {
            out.push((' ', by + 1, b[by]));
        }
        x = ax + 1;
        y = by + 1;
    }
    out
}
pub fn rtf(old: &str, new: &str) -> String {
    use std::fmt::Write;
    let old = old.replace("\r\n", "\n").replace('\r', "\n");
    let new = new.replace("\r\n", "\n").replace('\r', "\n");
    let changes = lines(&old, &new);
    let added = changes.iter().filter(|v| v.0 == '+').count();
    let removed = changes.iter().filter(|v| v.0 == '-').count();
    let mut out = String::from("{\\rtf1\\ansi\\deff0\\uc1{\\fonttbl{\\f0 Consolas;}}{\\colortbl;\\red222\\green232\\blue233;\\red63\\green221\\blue207;\\red244\\green151\\blue153;\\red23\\green57\\blue58;\\red59\\green30\\blue35;}\\f0\\fs28\\cf1 ");
    let _ = write!(out, "\\cf2 Disk changes   +{added}  -{removed}\\cf1\\par ");
    for (kind, number, line) in changes {
        out.push_str(match kind {
            '+' => "\\cf2\\highlight4 ",
            '-' => "\\cf3\\highlight5 ",
            _ => "\\cf1\\highlight0 ",
        });
        let _ = write!(out, "{kind} {number:>5}  ");
        crate::markdown::escape(&mut out, line);
        out.push_str("\\highlight0\\par ");
    }
    out.push('}');
    out
}

#[test]
fn changed_lines_preserve_both_versions() {
    for (old, new) in [
        ("a\nb\nc", "a\nB\nc\nd"),
        ("a\na\nb", "a\nb"),
        ("", "中😀"),
        ("a\nb\nc\nd", "A\nb\nc\nD"),
    ] {
        let diff = lines(old, new);
        assert_eq!(
            diff.iter()
                .filter(|v| v.0 != '+')
                .map(|v| v.2)
                .collect::<Vec<_>>()
                .join("\n"),
            old
        );
        assert_eq!(
            diff.iter()
                .filter(|v| v.0 != '-')
                .map(|v| v.2)
                .collect::<Vec<_>>()
                .join("\n"),
            new
        );
    }
    let diff = lines("a\nb\nc\nd", "A\nb\nc\nD");
    assert_eq!(diff.iter().filter(|v| v.0 == ' ').count(), 2);
    assert!(rtf("{old}", "中\\new").contains("\\u20013?"));
}

#[test]
fn watcher_tracks_replacements_and_recovery() {
    let dir = std::env::temp_dir().join(format!(
        "plumetxt-watch-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("notes.md");
    std::fs::write(&path, "old").unwrap();
    let watch = Watch::new(path.clone(), document::fingerprint(b"old"));
    let wait = || {
        let start = std::time::Instant::now();
        loop {
            if let Some(result) = watch.take() {
                break result;
            }
            assert!(
                start.elapsed() < Duration::from_secs(6),
                "Watcher did not report an external change"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    document::atomic_write(&path, b"new\nline").unwrap();
    assert_eq!(wait().unwrap().source, "new\nline");
    document::atomic_write(&path, "new\n中文".as_bytes()).unwrap();
    assert_eq!(wait().unwrap().source, "new\n中文");
    std::fs::remove_file(&path).unwrap();
    assert!(wait().is_err());
    std::fs::write(&path, "restored").unwrap();
    assert_eq!(wait().unwrap().source, "restored");
    drop(watch);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}
