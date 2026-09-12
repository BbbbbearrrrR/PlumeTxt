# Responsive loading and large-file editing

Final release SHA-256: `46192f98c294636006dd4b41a299524959f0a6166dce6f73229afd2c722900ad`.

Windows / i9-13900HX / 32 GB. One final warm-cache smoke run, not a three-round comparative benchmark. Raw timings: [JSON](2026-09-12-responsive-open.json).

| Fixture | Filename window observed | Text present in control | Full buffer verified and editable | Maximum sampled WM_NULL wait |
|---|---:|---:|---:|---:|
| CSV 1 MiB | 284 ms | 303 ms | 1,529 ms | 32 ms |
| Markdown 8 MiB | 72 ms | 109 ms | 3,953 ms | 52 ms |

The first-text probe checks native control content, not the first rendered frame. Editable completion verifies the entire buffer against the fixture, including line-ending normalization; its timing includes verification overhead. Highlighting/preview settling, scrolling FPS and continuous worst-case input latency are not measured. The maximum reply figures are sampled, not a guarantee. These are different metrics from the earlier Sublime settling heuristic.

Implementation: file reading, decoding and UTF-16 conversion run on a worker; UI insertion uses adaptive native text ranges without moving the caret to the document end. New/Open drops the old load receiver. Saving and editing cannot act on an incomplete buffer.

Files above 8 MiB retain bounded browsing. Double-click or Ctrl+E opens up to 64 KiB at the current position for editing; Ctrl+S streams the full file around the replacement on a worker. This does not provide whole-file selection or undo across regions. External length/modification-time changes reject saving; the source denies writes and replacement during the streaming copy. Save failures keep edits in the editor.

Validation:

- All 34 tests passed, including normally ignored Windows controls, PDF, image and terminal checks.
- Clippy passed with warnings denied.
- Native smoke test edited and saved a real 1 GiB Markdown file; full-file SHA-256 matched the expected insertion plus every original byte.
- Native smoke test cancelled a region load with New; its late result did not replace the empty document.
- Region tests cover UTF-8/BOM, UTF-16 LE/BE, surrogate and CRLF boundaries, no-op round trips, Save as, insertion and external-change rejection.

Reproduce after `cargo build --release`: `python scripts/check-responsive-open.py`. The test generates a 1 GiB fixture under `tmp/responsive-open/` and uses only its own app instances. The final run's large fixture was removed after verification; the script regenerates it.
