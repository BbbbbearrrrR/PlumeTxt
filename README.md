# PlumeTxt

Previously named FeatherPad; this project is unrelated to the Linux Qt editor of that name.

A small native Windows editor, Markdown preview, PDF reader, and integrated terminal. Written in Rust. English by default, with a black and cyan interface.

## Run

Open `PlumeTxt.exe`, drop a document onto the window, or pass a path:

```powershell
.\PlumeTxt.exe .\examples\welcome.md
.\PlumeTxt.exe "D:\Documents\paper.pdf"
```

Windows 10 version 1809 or later / Windows 11, x64. PDF export requires Microsoft Print to PDF and Print Spooler. No browser runtime or external PDF reader is needed.

Keep the `runtime` folder beside `PlumeTxt.exe` when moving or distributing the app; it contains the PDF text-search library and its licenses. This library loads on demand for searches.

## Controls

| Action | Shortcut |
| --- | --- |
| Commands | Ctrl+Shift+P |
| New / Open | Ctrl+N / Ctrl+O |
| Open folder / workspace | Ctrl+Shift+O |
| Search workspace | Ctrl+Shift+F |
| Find in current saved file | Ctrl+F |
| Save / Save as | Ctrl+S / Ctrl+Shift+S |
| Toggle Markdown reading / editing | Ctrl+E |
| Refresh preview | Ctrl+Shift+R |
| Export Markdown to PDF | Ctrl+Shift+E |
| Print image / PDF | Ctrl+P |
| File tree | Ctrl+Shift+B |
| Text size / Reset | Ctrl + or -, Ctrl+0 |
| PDF outline | Ctrl+Shift+L |
| PDF zoom / Fit width / Fit page | Ctrl + or -, Ctrl+0 / Ctrl+Shift+0 |
| Markdown bold / Italic / Inline code | Ctrl+B / Ctrl+I / Ctrl+K |
| Large-file overview | Commands → Document overview |
| Terminal | Ctrl+Shift+J or the bottom-right `>_` button |
| Insert image | Ctrl+Shift+I |
| Paste image / text | Ctrl+V |
| Quit | Ctrl+Q |

Normal and region editing use the same Save, Save as, Preview, Refresh, Export, Terminal, Insert image and Paste shortcuts. Ctrl+E toggles Markdown reading/editing, edits the selected large-file region, or returns from PDF/image viewing to the retained editor. Ctrl+Shift+E exports PDF; the large-file overview is available from Commands.

The command panel supports search, Up/Down to select, Enter to execute, and Escape to close. Arrow navigation keeps the search field focused. A mouse click selects; a double-click executes. The panel contains only essential actions for the current document and workspace. Formatting, zoom steps, New and Quit remain available through their shortcuts; they no longer fill the command list. The footer shows the current reading/editing state (or live PDF page number), only the most relevant shortcuts, a file-type tag (`*` means unsaved edits), and fixed Commands/Terminal controls. Narrow windows show fewer hints. Temporary status messages replace the contextual hints and then clear. Alt alone no longer opens Commands; AltGr and terminal editing combinations stay with their input control.

## Markdown

Preview headings have a clickable disclosure arrow. Collapsing a heading hides its body and subordinate headings until the next heading of the same or higher level; clicking again expands it. Folding affects neither the source nor PDF export. Code-block contents are not headings. Folds reset when the source changes, preventing stale positions from hiding another section. Region previews support the same folding behavior within their loaded content.

## Workspace

Opening a file normally keeps the single-file layout. Use Ctrl+Shift+O / **Open folder**, drop a folder onto the window, or pass a folder as the command-line argument to open a workspace. Its dark left file tree shows folders first, with compact text and seven high-contrast, distinct silhouettes for folders, archives, Markdown, PDF, images, code and other files. Icons are shared across rows and regenerated only when DPI changes; they do not depend on installed file associations. Drag the right divider to widen the file tree or PDF outline for long names. A cyan guide follows the pointer; document layout changes once on release to avoid tearing and repeated PDF rendering; neither directory panel has a horizontal scrollbar. Native hover tooltips are disabled. Subdirectories are read only when expanded; selecting a file uses the normal open path, including unsaved-change prompts and all supported viewers.

Run `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/register-file-types.ps1` once to add **Open with PlumeTxt** to Explorer's folder, folder background and drive context menus for the current user. On Windows 11, this classic menu entry is under **Show more options**. It opens the chosen directory as a workspace with the feather icon in the menu.

Use **Toggle file tree** / Ctrl+Shift+B to hide or show the file tree, and **Toggle PDF outline** / Ctrl+Shift+L to hide or show the PDF outline. Hidden panels retain their width and tree state.

**Ctrl+Shift+F** opens workspace search in the left sidebar, including before any file has been opened. **Ctrl+F** searches only the current saved file; without a file it shows a hint to open/save one or use workspace search. Type to search saved text files and PDF text (case-insensitive literal text); results stream in, grouped by relative file path, with line/column numbers or PDF page numbers and cyan matches. Press Enter in the search field to refresh and jump to the first match. Click another result, or press Down and Enter, to jump to its line/page and highlight the matching content. The position and highlight remain when switching focus between search and the editor. Markdown results open for editing; large-file results load only the matching region. Escape or Ctrl+Shift+B returns to the file tree, or closes the search sidebar in single-file mode. Reopening the same search scope retains the query. Workspace search asks for a folder if no workspace is open.

Search supports UTF-8 and BOM-marked UTF-16, uses bounded reads, and cancels superseded queries. It excludes `.git`, `node_modules`, `target`, `.cargo-cache`, `.venv`, and `__pycache__`, and does not follow junctions/symlinks. Results stop at 2,000 (`+` indicates the limit); binary/unreadable files and lines over 1 MiB are reported as skipped. Unsaved editor changes are not included; save and press Enter to refresh. Regex, replacement, and custom `.gitignore` rules are not implemented.

PDF search reads the text layer one page at a time and highlights the actual text rectangles, including page crop and rotation. Scanned PDFs without a text layer require OCR, which is not included. Password-protected PDFs and PDFs of 4 GiB or larger are not supported by search. PDFium calls are serialized off the UI thread; cancellation takes effect between native PDF operations.

Use **Refresh files** in the command panel to reload directory entries and **Close folder** to return to single-file mode while keeping the current document. Opening a folder does not discard the current document or automatically restore a workspace at the next launch. A new terminal uses the current document's directory, or the workspace root when no document has a path.

## Markdown preview

Markdown opens in a single preview column. Ctrl+E is the only reading/editing switch: press once to change modes and again to return. Folding applies only to the preview.

The source and preview share scroll progress. Scroll either pane or drag the single rightmost track. The synchronization uses document progress, not an exact source-line mapping; Markdown constructs can have different rendered heights.

Both panes use regular-weight Segoe UI with approximately 20 px body text at 100%. Reading mode centers the content in a column up to 880 px wide. Headings retain their hierarchy. Consolas is available for source text. Text zoom changes both panes. Preview updates are debounced; large documents require Ctrl+Shift+R. Markdown parsing and image decoding run in one background task at a time. Repeated requests keep only a pending flag, not extra document snapshots; obsolete results are discarded before display. Native RichEdit installation still runs on the UI thread.

Headings, emphasis, lists, tasks, quotes, code, tables, and link text are supported. Local supported raster images render in preview and PDF export; missing or unsupported images show alt text. HTML is literal text. No remote content is loaded. Export uses light paper colors and selectable text.

## Images and shortcuts

Open an image directly with Ctrl+O or a command-line path to view it inside PlumeTxt. The viewer fits the image to the window, supports wheel zoom and drag-to-pan, and uses Ctrl+0 to reset the view. Ctrl+E returns to the retained text document. Images are read-only. The terminal starts in the image's folder when creating a new session.

Dropping an image into an empty editor opens the viewer. In a saved Markdown document, dropping still inserts an attachment; hold Shift while dropping to open the viewer instead.

In a Markdown document, paste a screenshot or a copied image with Ctrl+V. You can also copy one image file in Explorer and paste it, drop an image file onto the editor, or use Ctrl+Shift+I to choose one. PlumeTxt saves an attachment into `<document-name>.assets` beside the Markdown file and inserts a relative Markdown link. An untitled document first opens Save as; cancelling creates no attachment. Move the Markdown file and its assets folder together. Save as does not relocate existing attachments.

PNG/APNG, JPEG/JFIF, GIF, BMP/DIB, TIFF, ICO and JPEG XR (`.jxr`, `.wdp`, `.hdp`) are accepted through Windows Imaging Component. WebP, HEIC/HEIF and AVIF use the matching installed Windows codecs; import reports a clear error if the decoder is unavailable or the file is damaged. SVG and layered formats such as PSD are not supported.

Selected, dropped or pasted image files retain their original bytes and extension in the assets folder. Clipboard bitmaps become PNG; clipboard PNG preserves transparency. Animated and multipage attachments retain all frames, while preview/PDF currently show only the first frame. EXIF orientation is applied to rendered copies. Direct relative Markdown links support the same formats as imported files.

Input is capped at 16 MiB and 16 megapixels. The decoder checks dimensions before requesting pixels. The standalone viewer retains one full-resolution raster for zooming; Markdown scales rendered copies to a maximum 1,600 px long edge and caps total embedded data / decoded pixels at 16 MiB / 16 megapixels. Originals are unchanged. Transparency is composited onto the preview or paper background. Missing, damaged, unavailable-codec or over-budget images show their alt text in preview. Markdown absolute paths, network shares and remote URLs are not loaded.

Markdown formatting shortcuts only modify source while editing Markdown; they do not insert markers into ordinary text or read-only previews. PDF outline, page navigation and Fit page apply only to PDFs; images retain zoom and Fit.

The bottom status bar shows context-specific shortcut hints when there is no temporary status message. In the terminal, plain Ctrl combinations (including Ctrl+C/V/E/F/J/N/O/S/P/Q) belong to the shell. Ctrl+E retains its shell behavior. Ctrl+Shift+J closes the terminal and returns focus to the document. Application Ctrl+Shift commands such as Commands, workspace search, Save as and Export remain available; document zoom and page navigation never intercept terminal input. Close the terminal with Ctrl+Shift+J before ordinary file shortcuts. Ctrl+1 and Ctrl+Shift+M are no longer application shortcuts; Ctrl+J / Ctrl+backtick terminal aliases are also removed.

## Syntax highlighting

The source editor automatically colours Markdown headings, emphasis, links, lists, quotes and code fences. Fenced code uses the same language rules as standalone files. All syntax-bearing text extensions in `assets/file-types.tsv` are covered: Rust, JS/TS and module/component variants, C/C++ headers, C#/Java/Go, Python/R/Ruby/Perl/Lua, Kotlin/Swift/Dart/Groovy, shell/PowerShell/batch, SQL, JSON/notebooks, TOML/YAML/INI/environment/config files, CSS/HTML/XML/SVG, TeX/BibTeX, CSV/TSV, Diff/Patch, reStructuredText/AsciiDoc and Git patterns. Dockerfile, Makefile, CMakeLists.txt, dotfiles and common shebang interpreters are detected too. Unknown extensions remain plain text; untitled documents default to Markdown.

CSV/TSV use consistent column colours, including unquoted text, escaped quotes and multiline quoted fields. Colours for code use cyan keywords, blue functions/punctuation, mint strings, muted comments and warm numbers. Highlighting changes no font sizes or file contents. Native text ranges preserve selection and undo/redo; clipboard paste stays plain text. IME composition postpones highlighting.

Work is debounced and limited to 32K UTF-16 units near the viewport, with at most 4,096 colour runs per update. The >8 MiB bounded viewer colours only its bounded page on its worker thread, without a full-file scan or index. This is lexical highlighting, not semantic analysis: mixed-language components use shared basic rules, and multiline constructs starting outside the bounded context can have approximate colours. Soft wrapping preserves logical record boundaries. Dense token windows spend the bounded formatting budget on visible text first; colours beyond that budget are deferred until scrolling.

## Large files

Files larger than 8 MiB open directly in the editor, including gigabyte Markdown. The first Unicode-aligned region (up to 64 KiB) is loaded on a worker and is immediately editable once displayed; no double-click or mode-switch command is required. The title shows `Region` to distinguish the loaded portion from the whole file. No whole-file string, UTF-16 copy, Markdown parse, or line index is built.

Use the wheel, arrow keys, Page Up/Down, Home/End, or drag the right scrollbar. Position is based on byte offsets, so a jump does not require scanning preceding lines. Long lines wrap into bounded display rows. UTF-8 and BOM-marked UTF-16 are decoded locally; malformed sequences display replacement characters. Disk changes are reflected in subsequent reads, without mapping mutable file memory.

The regular editor supports typing, deletion, selection, copy/paste and undo within the loaded region. **Document overview** in Commands opens an overview for navigating elsewhere; double-click a row or press Ctrl+E again to edit there. The overview reads at most 512 KiB per request and retains at most 256 display rows. Ctrl+S streams the unchanged prefix and suffix around the replacement into a temporary file, flushes it, then replaces the destination. Save as writes the whole file too. Encoding is preserved; line endings within the edited region use its detected style. External changes detected before saving are rejected rather than overwritten. Saving runs in the background; editing and document switching wait until it finishes. A close/open request that initiates saving must be repeated after completion. Failures retain your edits.

**Document overview** opens the large viewport, with a save/discard prompt if needed; successful saving keeps the edited position in an editable view. This is region editing, not a full-file editable buffer: cross-region selection/undo, search and a full-document preview remain unavailable. The integrated terminal starts in the large file's folder when creating a new session.

Ctrl+E switches the current region between reading and synchronized editing; Ctrl+Shift+R refreshes it. Ctrl+Shift+E exports the **whole document**, including the current unsaved region, from a temporary snapshot without saving changes to the original file. Export reads bounded sections and sends their rendered pages through one PDF print job on a worker. Sections prefer paragraph boundaries and start new pages. Markdown context restarts between sections: fences, tables, links or other constructs spanning a boundary can format differently from an in-memory full-document render. Temporary snapshots are removed after completion or failure; the destination is replaced only after a complete PDF is available.

Files up to 8 MiB are read/decoded on a worker and inserted in adaptive batches through native text ranges, keeping the window responsive and displaying text before the entire buffer is ready. Editing is enabled after insertion completes. New/Open cancels the pending load without allowing an old result to replace the new document. Highlighting and Markdown preview run after insertion.

First-display speed depends on storage latency; a cold network file cannot be promised an instant open. The benchmark creates a full-content 1 GiB file and measures **warm-cache** first viewport and random seeks:

```powershell
cargo test --release native_gigabyte_first_viewport -- --ignored --nocapture --test-threads=1
```

Measured locally on 2026-09-12 (release build, one warm-cache run): 1 GiB first viewport data ready in 4.4 ms; middle and end seeks together in 6.6 ms. This excludes process startup and first screen presentation. The actual app opening a separate 1 GiB fixture used approximately 24 MiB working set and 4.5 MiB private memory; a settled two-second sample recorded no CPU-time increase. These are observations, not cold-storage guarantees.

## PDF

PDFs render inside PlumeTxt using Windows.Data.Pdf. Pages scroll continuously. The outline reads local PDF bookmarks and falls back to page numbers. Visible pages and neighboring pages are rendered on demand.

The CPU raster cache is capped at 48 MiB, with up to 16 MiB of shared page textures per GPU reader; each page is capped at about 8 million pixels. These limits exclude system decoder memory, in-flight frames, and window buffers. PDF text search and match highlighting are available with Ctrl+F. Arbitrary PDF text selection, annotations, forms, and password entry are not implemented.

## Terminal

The terminal slides in from the right, taking roughly one third of the window (320–520 px). The document area resizes alongside it; the bottom-right button and Ctrl+Shift+J toggle the panel.

The integrated terminal uses Windows ConPTY and a VT parser. PowerShell starts without profiles in the active document's folder; untitled documents use the application's working directory. Existing sessions keep their working directory when another document opens.

- Input, arrows, Tab completion, Ctrl+C, UTF-8 output, and ANSI colors are supported.
- Paste with Ctrl+V; drag to select and copy with Ctrl+Shift+C. Without a selection, copy captures the visible screen.
- Use the wheel for up to 2,000 rows of scrollback.
- Hiding the panel keeps the session alive. After `exit`, reopen the panel to start a new session.
- Closing PlumeTxt terminates its terminal session. This is a local interactive shell, not a sandbox.

Startup and terminal I/O run off the UI thread. Output is bounded and painting is double buffered. This is a basic terminal surface, not a complete VS Code terminal implementation; advanced terminal protocols and accessibility text providers are not yet implemented.

## Rendering and performance

The window uses Per-Monitor V2 DPI awareness. Fonts, main layout, command panel, search rows, terminal cells and scrollbar geometry scale with the window's monitor; DPI transitions update borrowed font handles before releasing the old set. Document text and the modified flag are preserved. Layout dimensions in this document refer to 96-DPI logical pixels. Buttons have rounded surfaces, pointer-hover and pressed feedback, a visible keyboard focus outline, and muted disabled text. Windows 11 can supply native rounded window corners; unsupported DWM attributes leave the platform default. No blur buffers or additional graphics libraries are needed.

No Electron, WebView, background service, or network resources. Markdown custom tracks are siblings of the RichEdit controls, so text scrolling cannot move them. The native RichEdit tracks stay enabled for correct range updates but are clipped outside the visible text region. Full line layout is measured after content or width changes, and a short pane shares the overflowing pane's range. PDF tracks keep their existing implementation. Custom dark tracks support dragging and wheel input. Command lists and PDF pages are double buffered.

Wheel scrolling, command-panel opening, and terminal expansion use elapsed-time easing and respect the Windows client-area animation setting. Terminal expansion uses a clip region over a fixed-size live control; the document and terminal grid are laid out once per open/close transition rather than every animation frame. Reversing a transition continues from its current progress. Timers stop after transitions settle; they do not run continuously while idle. The main window paints its simple background directly instead of retaining another full-window bitmap; content controls retain their own double buffers. Performance still depends on file size and the Windows PDF engine.

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

The executable is `target/release/plumetxt.exe`. Test output is written to `tmp/`. The root `PlumeTxt.exe` is the packaged build. Feather assets are embedded in the executable.

## Rendering and materials

PDF and image scaling, terminal text and the large-file overview use an on-demand Direct2D hardware target with DirectWrite text. RichEdit, input fields, trees and other native controls retain their native editing, IME, keyboard navigation and accessibility. The command panel, search fields, buttons, status bar, selection states and scrollbars share the dark surface palette, inset borders and visible focus accents. Supported Windows versions supply the dark Mica title bar and native window shadow; other versions keep the opaque caption. Page and panel shadows use static layers, not a background animation or blur texture.

Rendering starts only when a custom view paints. GPU textures are reused and capped at 16 MiB per view; PDF CPU rasters are capped at 48 MiB. Hidden custom views release their rendering buffers. A successful GPU frame releases the GDI viewport buffer. Hardware creation, resize or draw failure repaints through GDI immediately, with a five-second retry cooldown. A visible page set or image beyond the texture budget uses GDI until the source or view is reset, avoiding repeated texture eviction and upload. These limits exclude driver/device memory, window back buffers and decoder working memory. Hardware acceleration can increase process memory and is not guaranteed to outperform GDI for small static views.

For troubleshooting or minimum graphics overhead, launch with `$env:PLUMETXT_RENDERER = "gdi"`; remove that environment variable to restore hardware rendering. No dedicated GPU is required. Run `cargo test --bin plumetxt --locked native_renderer_draws_reuses_and_recovers -- --ignored --nocapture` to compare the two paths locally and verify cached textures, resize, immediate fallback and resource release.

## Windows Explorer file icons

The 93 file icons use a cyan-blue feather, a distinct color and a format abbreviation (MD, TXT, PDF, JSX, TSX, H, HPP, JSONC, LOG, etc.), covering 96 extensions. Additional source/configuration formats open as text; icon registration does not add specialized parsers or viewers. After copying the release build to the root `PlumeTxt.exe`, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/register-file-types.ps1
```

Registration is per user and verifies all 93 embedded icons first. Explorer icons are installed under `%LOCALAPPDATA%\PlumeTxt\FileIcons` using content-hashed names so an updated design receives a new cache key. Registration then queries the effective Windows associations and reports legacy icons still in use. In Windows Settings > Apps > Default apps > PlumeTxt, choose the extensions you want it to open. Select the named format entry, such as `PlumeTxt (PDF)`, rather than the legacy `PlumeTxt.exe` entry (scroll the choice list if needed). Extension-specific auto associations pointing to this exact executable also get their icon repaired. Registration does not overwrite protected Windows default choices or other applications' defaults. Keep the executable at its registered path; rerun registration after moving it.

Run `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-shell-icons.ps1` to register and verify the actual Shell-returned MD and TOML icons against the installed artwork. This requires those two formats already associated with PlumeTxt. The test writes example files and diagnostic PNGs under `tmp/shell-icon-check/`, and allows only one RGB level of native alpha-rounding difference.

## Printing images and PDFs

Press Ctrl+P while viewing an image or PDF, or choose **Print image or PDF** in the command panel. The Windows print dialog provides printer selection, paper/orientation settings, copies and PDF page ranges. Pages fit the printable area without cropping; transparent images print on white. Images print the displayed first frame. PDF printing rasterizes one page at a time using a bounded buffer (target 300 DPI, reduced for very large pages). Rendering/spooling runs in the background. **Cancel print** stops at the next page boundary; already spooled pages may require cancellation in the Windows print queue. Markdown PDF export uses Ctrl+Shift+E; Ctrl+P is reserved for printing images and PDFs.

PDFs open centered with margins at a reading scale showing about three quarters of a page vertically, limited by the available width to avoid horizontal overflow. The default adapts to panel/window resizing until you zoom manually. Ctrl+Shift+0 restores whole-page reading; Ctrl+0 fits the page width.

### Upgrading from FeatherPad

Use `PlumeTxt.exe` with the existing `runtime` folder. Run `scripts/register-file-types.ps1` again to register the new name. It redirects legacy FeatherPad associations only when they point to the former executable in this same directory; existing protected default selections remain unchanged. Old executable backups and historical benchmark records retain their original names. `PLUMETXT_RENDERER` replaces `FEATHERPAD_RENDERER`; the old variable remains a fallback for existing launch scripts.
