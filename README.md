<img src="assets/feather.png" alt="PlumeTxt logo" width="128" height="128">

# PlumeTxt

A lightweight, native Windows editor for text, Markdown, PDFs, and images. Built in Rust, with a dark interface, an integrated terminal, and no UI animations.

**[Download Windows x64 Setup](https://github.com/BbbbbearrrrR/PlumeTxt/releases/latest)**

Windows 10 1809 or later. No dedicated GPU or browser runtime required.

## Features

- Text and code editing with syntax highlighting, line numbers, and line counts.
- Markdown preview with tables, highlighted code blocks, formulas, local images, and foldable headings.
- Large-file editing with on-demand loading and whole-file saving.
- PDF reading, text selection, and search, plus a built-in image viewer.
- A file tree, workspace search, and a resizable PowerShell terminal.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Command palette | Ctrl+Shift+P |
| New / Open / Save | Ctrl+N / Ctrl+O / Ctrl+S |
| Open folder | Ctrl+Shift+O |
| Find / Search workspace | Ctrl+F / Ctrl+Shift+F |
| Toggle Markdown reading and editing | Ctrl+E |
| Toggle file tree | Ctrl+Shift+B |
| Toggle terminal | Ctrl+Shift+J |
| Export Markdown to PDF | Ctrl+Shift+E |

## Installation

Run Setup to install the app and PDF runtime. Shortcuts and file associations are optional. The installer is unsigned; on Windows 11, the Explorer command appears under **Show more options**.

PDF search and selection require a text layer. PDF export uses Microsoft Print to PDF. File search reads saved content. If you move the app manually, keep its `runtime` folder alongside `PlumeTxt.exe`.

## Build

Requires Rust MSVC, Visual Studio C++ Build Tools, and the Windows SDK.

```powershell
cargo build --release --locked
cargo test --locked
```

The executable is written to `target/release/plumetxt.exe`. With Inno Setup 6.6+ installed, build Setup and its SHA-256 checksum in `dist/`:

```powershell
./scripts/build-setup.ps1
```

## License

[MIT](LICENSE)
