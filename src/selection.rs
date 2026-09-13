use crate::{syntax, theme::*};
use windows::Win32::UI::Controls::RichEdit::{
    tomAllowOffClient, tomCell, tomClientCoord, tomConstants, tomExtend, tomStart,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{Controls::*, Input::KeyboardAndMouse::GetFocus, WindowsAndMessaging::*},
};

// A visual overlay only: selection, clipboard, undo and document formatting stay native.
pub unsafe fn paint(hwnd: HWND, dc: HDC, tint: &mut Buffer) {
    let (mut start, mut end) = (0u32, 0u32);
    SendMessageW(
        hwnd,
        EM_GETSEL,
        &mut start as *mut _ as usize,
        &mut end as *mut _ as isize,
    );
    if start == end {
        return;
    }
    let Some(doc) = syntax::document(hwnd) else {
        return;
    };
    let rc = client(hwnd);
    let first = SendMessageW(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0).max(0);
    let region = CreateRectRgn(0, 0, 0, 0);
    let point = |cp: i32, align: i32| -> Option<POINT> {
        let range = doc.Range(cp, cp).ok()?;
        let (mut x, mut y) = (0, 0);
        range
            .GetPoint(
                tomConstants(tomStart.0 | tomClientCoord.0 | tomAllowOffClient.0 | align),
                &mut x,
                &mut y,
            )
            .ok()?;
        Some(POINT { x, y })
    };
    // Cell-local positions are ordered vertically, even when EM_LINEINDEX treats
    // a wrapped table row or an entire code block as a single line.
    let rows = |mut a: i32, b: i32| {
        while a < b {
            let Some(left) = point(a, 0) else { break };
            if left.y >= rc.bottom {
                break;
            }
            let Some(bottom) = point(a, TA_BOTTOM as i32) else {
                break;
            };
            let (mut lo, mut hi) = (a + 1, b);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let same_row = if bottom.y <= 0 {
                    point(mid, TA_BOTTOM as i32).is_some_and(|p| p.y <= 0)
                } else {
                    point(mid, 0).is_some_and(|p| p.y <= left.y)
                };
                if same_row {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let row_end = lo;
            if bottom.y > 0 {
                let mut right = point(row_end, 0).unwrap_or(left);
                if right.y != left.y {
                    right = point(row_end - 1, TA_RIGHT as i32).unwrap_or(left);
                }
                let x = left.x.min(right.x).max(0);
                let edge = left
                    .x
                    .max(right.x)
                    .max(left.x.min(right.x) + 6)
                    .min(rc.right - 12);
                if edge > x {
                    let row =
                        CreateRoundRectRgn(x, left.y.max(0), edge, bottom.y.min(rc.bottom), 5, 5);
                    CombineRgn(region, region, row, RGN_OR);
                    DeleteObject(row);
                }
            }
            a = row_end;
        }
    };
    for line in first..first + rc.bottom.max(1) as isize {
        let from = SendMessageW(hwnd, EM_LINEINDEX, line as usize, 0);
        if from < 0 || from >= end as isize {
            break;
        }
        let next = SendMessageW(hwnd, EM_LINEINDEX, line as usize + 1, 0);
        if next >= 0 && next <= start as isize {
            continue;
        }
        let limit = if next < 0 { end as i32 } else { next as i32 };
        let a = (from as u32).max(start) as i32;
        let b = limit.min(end as i32);
        if a >= b {
            continue;
        }
        let Ok(cell) = doc.Range(from as i32 + 1, from as i32 + 1) else {
            continue;
        };
        let _ = cell.EndOf(tomCell.0, tomExtend.0);
        let cell_end = cell.GetEnd().unwrap_or(from as i32 + 1);
        if cell_end <= from as i32 + 1 || cell_end > limit {
            rows(a, b);
            continue;
        }
        let mut cp = from as i32 + 1;
        while cp < b {
            let _ = cell.SetRange(cp, cp);
            let _ = cell.EndOf(tomCell.0, tomExtend.0);
            let cell_end = cell.GetEnd().unwrap_or(cp);
            if cell_end > limit {
                break;
            }
            if cell_end <= cp {
                cp += 1;
                continue;
            }
            // Exclude the cell marker: its right edge includes the empty cell width.
            rows(cp.max(a), (cell_end - 1).min(b));
            cp = cell_end;
        }
    }
    if tint.ensure(dc, 1, 1) {
        fill(
            tint.dc,
            RECT {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            ACCENT,
        );
        let saved = SaveDC(dc);
        ExtSelectClipRgn(dc, region, RGN_AND);
        GdiAlphaBlend(
            dc,
            0,
            0,
            rc.right,
            rc.bottom,
            tint.dc,
            0,
            0,
            1,
            1,
            BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: if GetFocus() == hwnd { 58 } else { 28 },
                AlphaFormat: 0,
            },
        );
        RestoreDC(dc, saved);
    }
    DeleteObject(region);
}

#[test]
#[ignore = "Requires Windows RichEdit"]
fn native_code_and_table_multiline_selection() {
    unsafe {
        let _ole = crate::assets::Ole::new().unwrap();
        windows_sys::Win32::System::LibraryLoader::LoadLibraryW(
            crate::ui::wide("Msftedit.dll").as_ptr(),
        );
        let fonts = Fonts::new();
        let hwnd = crate::ui::rich_edit(std::ptr::null_mut(), true, fonts.code);
        MoveWindow(hwnd, 0, 0, 700, 500, 0);
        let rect = RECT {
            left: 20,
            top: 20,
            right: 660,
            bottom: 480,
        };
        SendMessageW(hwnd, EM_SETRECT, 0, &rect as *const _ as isize);
        editor_colors(hwnd);
        dark_scrollbars(hwnd, CANVAS);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(hwnd);
        let dc = GetDC(hwnd);
        let mut image = Buffer::default();
        let mut tint = Buffer::default();

        assert!(image.ensure(dc, 700, 500));
        for source in [
            "alpha one\n\nbeta two\n\ngamma three".to_owned(),
            "```text\nalpha one\nbeta two\ngamma three\n```".to_owned(),
            "| alpha | one |\n|---|---|\n| beta | two |\n| gamma | three |".to_owned(),
            format!(
                "| alpha {0} beta {0} gamma | one {0} two {0} three |\n|---|---|",
                "wrapped ".repeat(6)
            ),
            format!(
                "```text\n{}alpha one\nbeta two\ngamma three\n```",
                "earlier row\n".repeat(200)
            ),
        ] {
            crate::ui::set_rtf(hwnd, &crate::markdown::preview(&source, 9000)).unwrap();
            let doc = syntax::document(hwnd).unwrap();
            let text_before = doc
                .Range(0, doc.Range(0, 0).unwrap().GetStoryLength().unwrap())
                .unwrap()
                .GetText()
                .unwrap();
            let anchor = doc.Range(0, 0).unwrap();
            anchor
                .FindText(
                    &windows_core::BSTR::from("alpha"),
                    i32::MAX,
                    tomConstants(4),
                )
                .unwrap();
            let (mut x, mut y) = (0, 0);
            anchor
                .GetPoint(
                    tomConstants(tomStart.0 | tomClientCoord.0 | tomAllowOffClient.0),
                    &mut x,
                    &mut y,
                )
                .unwrap();
            if y > 400 {
                let pos = POINT { x: 0, y: y - 40 };
                SendMessageW(hwnd, WM_USER + 222, 0, &pos as *const _ as isize);
            }
            let mut points = Vec::new();
            for token in ["alpha", "one", "beta", "two", "gamma", "three"] {
                let range = doc.Range(0, 0).unwrap();
                range
                    .FindText(&windows_core::BSTR::from(token), i32::MAX, tomConstants(4))
                    .unwrap();
                let cp = range.GetStart().unwrap();
                let caret = doc.Range(cp + 1, cp + 1).unwrap();
                let (mut x, mut y) = (0, 0);
                caret
                    .GetPoint(tomConstants(tomStart.0 | tomClientCoord.0), &mut x, &mut y)
                    .unwrap();
                points.push((cp, x, y));
            }
            let mut scroll = POINT { x: 0, y: 0 };
            SendMessageW(hwnd, WM_USER + 221, 0, &mut scroll as *mut _ as isize);
            SendMessageW(hwnd, EM_SETSEL, 0, 0);
            SendMessageW(hwnd, WM_USER + 222, 0, &scroll as *const _ as isize);
            fill(image.dc, client(hwnd), CANVAS);
            SendMessageW(hwnd, WM_PRINTCLIENT, image.dc as usize, PRF_CLIENT as isize);
            let pixels = |x, y| {
                (y..y + 12)
                    .flat_map(|y| (x..x + 12).map(move |x| GetPixel(image.dc, x, y)))
                    .collect::<Vec<_>>()
            };
            let plain: Vec<_> = points.iter().map(|&(_, x, y)| pixels(x, y)).collect();
            SendMessageW(
                hwnd,
                EM_SETSEL,
                points[0].0 as usize,
                (points[5].0 + 5) as isize,
            );
            SendMessageW(hwnd, WM_USER + 222, 0, &scroll as *const _ as isize);
            let native_selection = doc.GetSelection().unwrap();
            let expected = (
                native_selection.GetStart().unwrap(),
                native_selection.GetEnd().unwrap(),
            );
            fill(image.dc, client(hwnd), CANVAS);
            SendMessageW(hwnd, WM_PRINTCLIENT, image.dc as usize, PRF_CLIENT as isize);
            for (&(_, x, y), before) in points.iter().zip(&plain) {
                assert!(
                    pixels(x, y) == *before,
                    "Native selection must remain hidden at {x},{y}: {}",
                    &source[..source.len().min(100)]
                );
            }
            let mut after_scroll = POINT { x: 0, y: 0 };
            SendMessageW(hwnd, WM_USER + 221, 0, &mut after_scroll as *mut _ as isize);
            assert_eq!((scroll.x, scroll.y), (after_scroll.x, after_scroll.y));
            paint(hwnd, image.dc, &mut tint);
            for ((cp, x, y), before) in points.iter().copied().zip(plain) {
                assert_ne!(
                    pixels(x, y),
                    before,
                    "Missing highlight at cp={cp}, source={source}"
                );
            }
            let selected = doc.GetSelection().unwrap();
            assert_eq!(selected.GetStart().unwrap(), expected.0);
            assert_eq!(selected.GetEnd().unwrap(), expected.1);
            let flags = selected.GetFlags().unwrap() | 1;
            selected.SetFlags(flags).unwrap();
            SendMessageW(hwnd, WM_PRINTCLIENT, image.dc as usize, PRF_CLIENT as isize);
            assert_eq!(
                selected.GetFlags().unwrap(),
                flags,
                "Repaint must preserve backward selection"
            );
            assert_eq!(selected.GetStart().unwrap(), expected.0);
            assert_eq!(selected.GetEnd().unwrap(), expected.1);
            assert_eq!(
                doc.Range(0, doc.Range(0, 0).unwrap().GetStoryLength().unwrap())
                    .unwrap()
                    .GetText()
                    .unwrap(),
                text_before
            );
            assert!(selected.GetStart().unwrap() <= points[0].0 + 1);
            assert!(
                selected.GetEnd().unwrap() > points[4].0,
                "Selection must span multiple rows"
            );
            let copied = selected.GetText().unwrap().to_string();
            assert!(
                copied.contains("beta"),
                "Selected text must include the middle row: {copied:?}"
            );
        }
        ReleaseDC(hwnd, dc);
        DestroyWindow(hwnd);
    }
}
