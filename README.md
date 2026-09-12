# FeatherPad

A small native Windows editor, Markdown preview, PDF reader, and integrated terminal. Written in Rust. English by default, with a black and cyan interface.

## Run

Open `FeatherPad.exe`, drop a document onto the window, or pass a path:

```powershell
.\FeatherPad.exe .\examples\welcome.md
.\FeatherPad.exe "D:\Documents\paper.pdf"
```

Windows 10 version 1809 or later / Windows 11, x64. PDF export requires Microsoft Print to PDF and Print Spooler. No browser runtime or external PDF reader is needed.

## Controls

| Action | Shortcut |
| --- | --- |
| Commands | Ctrl+Shift+P or Alt |
| New / Open | Ctrl+N / Ctrl+O |
| Save / Save as | Ctrl+S / Ctrl+Shift+S |
| Markdown preview | Ctrl+Shift+M |
| Refresh preview | Ctrl+Shift+R |
| Export Markdown to PDF | Ctrl+P |
| PDF outline | Ctrl+Shift+L |
| PDF zoom / Fit width | Ctrl + or -, Ctrl+0 |
| Return to editor | Ctrl+E |
| Large-file overview | Ctrl+Shift+E |
| Terminal | Ctrl+J, Ctrl+backtick, or the bottom-right `>_` button |
| Insert image | Ctrl+Shift+I |
| Paste image / text | Ctrl+V |
| Quit | Ctrl+Q |

Normal and region editing use the same Save, Save as, Preview, Refresh, Export, Terminal, Insert image and Paste shortcuts. Ctrl+E always focuses/returns to the editor; Ctrl+Shift+E is the separate large-file overview command.

The command panel supports search, arrow keys, Enter, mouse selection, and Escape. Font and text-size commands are available there. Status messages disappear automatically.

## Markdown

The source and preview share scroll progress. Scroll either pane or drag the single rightmost track. The synchronization uses document progress, not an exact source-line mapping; Markdown constructs can have different rendered heights.

Both panes use Segoe UI with approximately 20 px body text at 100%. Headings retain their hierarchy. Consolas is available for source text. Text zoom changes both panes. Preview updates are debounced; large documents require Ctrl+Shift+R.

Headings, emphasis, lists, tasks, quotes, code, tables, and link text are supported. Local supported raster images render in preview and PDF export; missing or unsupported images show alt text. HTML is literal text. No remote content is loaded. Export uses light paper colors and selectable text.

## Images and shortcuts

Open an image directly with Ctrl+O or a command-line path to view it inside FeatherPad. The viewer fits the image to the window, supports wheel zoom and drag-to-pan, and uses Ctrl+0 to reset the view. Ctrl+E returns to the retained text document. Images are read-only. The terminal starts in the image's folder when creating a new session.

Dropping an image into an empty editor opens the viewer. In a saved Markdown document, dropping still inserts an attachment; hold Shift while dropping to open the viewer instead.

In a Markdown document, paste a screenshot or a copied image with Ctrl+V. You can also copy one image file in Explorer and paste it, drop an image file onto the editor, or use Ctrl+Shift+I to choose one. FeatherPad saves an attachment into `<document-name>.assets` beside the Markdown file and inserts a relative Markdown link. An untitled document first opens Save as; cancelling creates no attachment. Move the Markdown file and its assets folder together. Save as does not relocate existing attachments.

PNG/APNG, JPEG/JFIF, GIF, BMP/DIB, TIFF, ICO and JPEG XR (`.jxr`, `.wdp`, `.hdp`) are accepted through Windows Imaging Component. WebP, HEIC/HEIF and AVIF use the matching installed Windows codecs; import reports a clear error if the decoder is unavailable or the file is damaged. SVG and layered formats such as PSD are not supported.

Selected, dropped or pasted image files retain their original bytes and extension in the assets folder. Clipboard bitmaps become PNG; clipboard PNG preserves transparency. Animated and multipage attachments retain all frames, while preview/PDF currently show only the first frame. EXIF orientation is applied to rendered copies. Direct relative Markdown links support the same formats as imported files.

Input is capped at 16 MiB and 16 megapixels. The decoder checks dimensions before requesting pixels. The standalone viewer retains one full-resolution raster for zooming; Markdown scales rendered copies to a maximum 1,600 px long edge and caps total embedded data / decoded pixels at 16 MiB / 16 megapixels. Originals are unchanged. Transparency is composited onto the preview or paper background. Missing, damaged, unavailable-codec or over-budget images show their alt text in preview. Markdown absolute paths, network shares and remote URLs are not loaded.

The bottom status bar shows context-specific shortcut hints when there is no temporary status message. File commands, Ctrl+Shift+P, Ctrl+Shift+M, Ctrl+Shift+I and Ctrl+J also work while the terminal has focus. Terminal Ctrl+C and Ctrl+V retain their shell behaviour. Ctrl+J is an alternative for keyboard layouts where Ctrl+backtick is awkward.

## Syntax highlighting

The source editor automatically colours Markdown headings, emphasis, links, lists, quotes and code fences. Fenced code uses the same language rules as standalone files. All syntax-bearing text extensions in `assets/file-types.tsv` are covered: Rust, JS/TS and module/component variants, C/C++ headers, C#/Java/Go, Python/R/Ruby/Perl/Lua, Kotlin/Swift/Dart/Groovy, shell/PowerShell/batch, SQL, JSON/notebooks, TOML/YAML/INI/environment/config files, CSS/HTML/XML/SVG, TeX/BibTeX, CSV/TSV, Diff/Patch, reStructuredText/AsciiDoc and Git patterns. Dockerfile, Makefile, CMakeLists.txt, dotfiles and common shebang interpreters are detected too. Unknown extensions remain plain text; untitled documents default to Markdown.

CSV/TSV use consistent column colours, including unquoted text, escaped quotes and multiline quoted fields. Colours for code use cyan keywords, blue functions/punctuation, mint strings, muted comments and warm numbers. Highlighting changes no font sizes or file contents. Native text ranges preserve selection and undo/redo; clipboard paste stays plain text. IME composition postpones highlighting.

Work is debounced and limited to 32K UTF-16 units near the viewport, with at most 4,096 colour runs per update. The >8 MiB bounded viewer colours only its bounded page on its worker thread, without a full-file scan or index. This is lexical highlighting, not semantic analysis: mixed-language components use shared basic rules, and multiline constructs starting outside the bounded context can have approximate colours. Soft wrapping preserves logical record boundaries. Dense token windows spend the bounded formatting budget on visible text first; colours beyond that budget are deferred until scrolling.

## Large files

Files larger than 8 MiB open directly in the editor, including gigabyte Markdown. The first Unicode-aligned region (up to 64 KiB) is loaded on a worker and is immediately editable once displayed; no double-click or mode-switch command is required. The title shows `Region` to distinguish the loaded portion from the whole file. No whole-file string, UTF-16 copy, Markdown parse, or line index is built.

Use the wheel, arrow keys, Page Up/Down, Home/End, or drag the right scrollbar. Position is based on byte offsets, so a jump does not require scanning preceding lines. Long lines wrap into bounded display rows. UTF-8 and BOM-marked UTF-16 are decoded locally; malformed sequences display replacement characters. Disk changes are reflected in subsequent reads, without mapping mutable file memory.

The regular editor supports typing, deletion, selection, copy/paste and undo within the loaded region. Ctrl+Shift+E optionally opens an overview for navigating elsewhere; double-click a row or press Ctrl+E again to edit there. The overview reads at most 512 KiB per request and retains at most 256 display rows. Ctrl+S streams the unchanged prefix and suffix around the replacement into a temporary file, flushes it, then replaces the destination. Save as writes the whole file too. Encoding is preserved; line endings within the edited region use its detected style. External changes detected before saving are rejected rather than overwritten. Saving runs in the background; editing and document switching wait until it finishes. A close/open request that initiates saving must be repeated after completion. Failures retain your edits.

Ctrl+Shift+E opens the large viewport, with a save/discard prompt if needed; successful saving keeps the edited position in an editable view. This is region editing, not a full-file editable buffer: cross-region selection/undo, search and a full-document preview remain unavailable. The integrated terminal starts in the large file's folder when creating a new session.

Ctrl+Shift+M previews the current region with synchronized scrolling and live local edits; Ctrl+Shift+R refreshes it. Ctrl+P exports the **whole document**, including the current unsaved region, from a temporary snapshot without saving changes to the original file. Export reads bounded sections and sends their rendered pages through one PDF print job on a worker. Sections prefer paragraph boundaries and start new pages. Markdown context restarts between sections: fences, tables, links or other constructs spanning a boundary can format differently from an in-memory full-document render. Temporary snapshots are removed after completion or failure; the destination is replaced only after a complete PDF is available.

Files up to 8 MiB are read/decoded on a worker and inserted in adaptive batches through native text ranges, keeping the window responsive and displaying text before the entire buffer is ready. Editing is enabled after insertion completes. New/Open cancels the pending load without allowing an old result to replace the new document. Highlighting and Markdown preview run after insertion.

First-display speed depends on storage latency; a cold network file cannot be promised an instant open. The benchmark creates a full-content 1 GiB file and measures **warm-cache** first viewport and random seeks:

```powershell
cargo test --release native_gigabyte_first_viewport -- --ignored --nocapture --test-threads=1
```

Measured locally on 2026-09-12 (release build, one warm-cache run): 1 GiB first viewport data ready in 4.4 ms; middle and end seeks together in 6.6 ms. This excludes process startup and first screen presentation. The actual app opening a separate 1 GiB fixture used approximately 24 MiB working set and 4.5 MiB private memory; a settled two-second sample recorded no CPU-time increase. These are observations, not cold-storage guarantees.

## PDF

PDFs render inside FeatherPad using Windows.Data.Pdf. Pages scroll continuously. The outline reads local PDF bookmarks and falls back to page numbers. Visible pages and neighboring pages are rendered on demand.

The raster cache is capped at 64 MiB; each page is capped at about 8 million pixels. These limits exclude system decoder memory, in-flight frames, and window buffers. Text selection, PDF search, annotations, forms, and password entry are not implemented.

## Terminal

The terminal slides in from the right, taking roughly one third of the window (320–520 px). The document area resizes alongside it; the bottom-right button and Ctrl+backtick toggle the panel.

The integrated terminal uses Windows ConPTY and a VT parser. PowerShell starts without profiles in the active document's folder; untitled documents use the application's working directory. Existing sessions keep their working directory when another document opens.

- Input, arrows, Tab completion, Ctrl+C, UTF-8 output, and ANSI colors are supported.
- Paste with Ctrl+V; drag to select and copy with Ctrl+Shift+C. Without a selection, copy captures the visible screen.
- Use the wheel for up to 2,000 rows of scrollback.
- Hiding the panel keeps the session alive. After `exit`, reopen the panel to start a new session.
- Closing FeatherPad terminates its terminal session. This is a local interactive shell, not a sandbox.

Startup and terminal I/O run off the UI thread. Output is bounded and painting is double buffered. This is a basic terminal surface, not a complete VS Code terminal implementation; advanced terminal protocols and accessibility text providers are not yet implemented.

## Rendering and performance

No Electron, WebView, background service, or network resources. Markdown custom tracks are siblings of the RichEdit controls, so text scrolling cannot move them. The native RichEdit tracks stay enabled for correct range updates but are clipped outside the visible text region. Full line layout is measured after content or width changes, and a short pane shares the overflowing pane's range. PDF tracks keep their existing implementation. Custom dark tracks support dragging and wheel input. Command lists and PDF pages are double buffered.

Wheel scrolling, command-panel opening, and terminal expansion use short easing transitions. Timers stop after transitions settle; they do not run continuously while idle. Performance still depends on file size and the Windows PDF engine.

## File safety

Saved text documents are checked for external edits on a background thread. After a write settles (normally about one second), a live comparison opens on the right without replacing the current editor. Green `+` rows show new disk lines, red `-` rows show removed current lines, and line numbers identify each version. Repeated external writes update this panel; local edits update the comparison after a short debounce. An existing Markdown preview is temporarily replaced and restored after review.

`Keep current` retains the editor text and marks it unsaved; Ctrl+S then writes it to disk. `Use disk` adopts the latest reviewed disk version, keeping the previous text available through Undo. Both actions recheck the disk version before resolving, so a newer write requires another review. Missing, unreadable, binary or over-limit files report an error and preserve editor text. The watcher applies to saved editable text; the separate large-file viewport, PDF and image viewers do not display text diffs.

UTF-8 and BOM-marked UTF-16 LE/BE are supported. Saving preserves encoding and the detected line-ending style. Invalid text encodings and NUL bytes are rejected. The full editor has a 32 MiB save limit; files above 8 MiB use the bounded viewport with region editing and streaming save.

Text saves write a same-directory temporary file, synchronize it, and replace the destination. External changes are checked before saving. PDF exports finish in a temporary file before replacing their destination.

## Build and verify

Requires Rust MSVC, Visual Studio C++ Build Tools, and Windows SDK.

```powershell
$env:CARGO_HOME = "$PWD\.cargo-cache" # Optional local dependency cache
cargo build --release --locked
cargo clippy --all-targets -- -D warnings
cargo test --locked
# Includes native RichEdit, PDF export, and ConPTY checks:
cargo test --locked -- --include-ignored --test-threads=1
```

The executable is `target/release/featherpad.exe`. Test output is written to `tmp/`. The root `FeatherPad.exe` is the packaged build. Feather assets are embedded in the executable.

## Windows Explorer file icons

The 93 file icons use a cyan-blue feather, a distinct color and a format abbreviation (MD, TXT, PDF, JSX, TSX, H, HPP, JSONC, LOG, etc.), covering 96 extensions. Additional source/configuration formats open as text; icon registration does not add specialized parsers or viewers. After copying the release build to the root `FeatherPad.exe`, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/register-file-types.ps1
```

Registration is per user and verifies all 93 embedded icons first. Explorer icons are installed under `%LOCALAPPDATA%\FeatherPad\FileIcons` using content-hashed names so an updated design receives a new cache key. Registration then queries the effective Windows associations and reports legacy icons still in use. In Windows Settings > Apps > Default apps > FeatherPad, choose the extensions you want it to open. Select the named format entry, such as `FeatherPad (PDF)`, rather than the legacy `FeatherPad.exe` entry (scroll the choice list if needed). Extension-specific auto associations pointing to this exact executable also get their icon repaired. Registration does not overwrite protected Windows default choices or other applications' defaults. Keep the executable at its registered path; rerun registration after moving it.

Run `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-shell-icons.ps1` to register and verify the actual Shell-returned MD and TOML icons against the installed artwork. This requires those two formats already associated with FeatherPad. The test writes example files and diagnostic PNGs under `tmp/shell-icon-check/`, and allows only one RGB level of native alpha-rounding difference.
