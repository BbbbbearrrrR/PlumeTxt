# PDF search runtime

PDFium 155.0.8044.0, Windows x64, without JavaScript or XFA.

- Binary distribution: https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium/8044
- Archive: `pdfium-win-x64.tgz`
- `pdfium.dll` SHA-256: `04100c03e41cac1f979e36e5e26fb860bcb5a7461f53830d3c098716624a27a9`
- API: https://pdfium.googlesource.com/pdfium/+/refs/heads/main/public/fpdf_text.h
- Redistribution notices: `LICENSE` and every file in `licenses/`.

Keep this directory beside PlumeTxt.exe under `runtime/pdfium`. The build copies the runtime and licenses into the Cargo output directory. It loads only during PDF text searches and unloads afterward; the existing Windows renderer remains in use. Updating it requires replacing the binary and accompanying notices, checking the new hash, and rerunning PDF search tests.
