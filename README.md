<img src="assets/feather.png" alt="PlumeTxt logo" width="128" height="128">

# PlumeTxt

A native Windows text editor with Markdown preview, PDF reading, and an integrated terminal. Built in Rust, with a dark interface and no UI animations.

**[Download for Windows x64](https://github.com/BbbbbearrrrR/PlumeTxt/releases/latest)**

Windows 10 1809 or later. No dedicated GPU or browser runtime required. The installer includes the PDF runtime and offers optional shortcuts and file associations.

## Features

- Edit text and code with line numbers, line counts, and syntax highlighting.
- Read Markdown with tables, highlighted code blocks, formulas, local images, and foldable headings.
- View PDFs and images; search and copy PDF text.
- Browse folders and search saved files across a workspace.
- Use an adjustable PowerShell terminal with your profile; PowerShell 7 is preferred when installed.
- Edit large files with a disk-backed document, on-demand text windows, and whole-file saving.
- Review external file changes before replacing your edits.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Command palette | Ctrl+Shift+P |
| New / Open / Save | Ctrl+N / Ctrl+O / Ctrl+S |
| Open folder | Ctrl+Shift+O |
| Find in saved file / Search workspace | Ctrl+F / Ctrl+Shift+F |
| Toggle Markdown reading and editing | Ctrl+E |
| Toggle file tree | Ctrl+Shift+B |
| Toggle terminal | Ctrl+Shift+J |
| Export Markdown to PDF | Ctrl+Shift+E |

## Notes

- The installer is unsigned. On Windows 11, the Explorer command appears under **Show more options**.
- PDF selection and search require a text layer; OCR is not included.
- Search uses saved files, not unsaved edits. PDF export requires Microsoft Print to PDF.
- Files over 32 MiB are indexed into temporary disk storage before editing; only the current text window is kept in the editor.
- When moving the app manually, keep the `runtime` folder beside `PlumeTxt.exe`.

## Build

Requires Rust MSVC, Visual Studio C++ Build Tools, and the Windows SDK.

```powershell
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets -- -D warnings
```

The executable is `target/release/plumetxt.exe`. To build the installer, install PowerShell 7 and Inno Setup 6.6 or later, then run:

```powershell
./scripts/build-setup.ps1
```

The Setup executable and SHA-256 checksum are written to `dist/`.

## License

MIT. Previously named FeatherPad.