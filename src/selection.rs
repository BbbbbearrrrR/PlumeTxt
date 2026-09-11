use crate::{syntax, theme::*};
use windows::Win32::UI::Controls::RichEdit::{
    tomAllowOffClient, tomClientCoord, tomConstants, tomStart,
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
    for line in first..first + rc.bottom.max(1) as isize {
        let from = SendMessageW(hwnd, EM_LINEINDEX, line as usize, 0);
        if from < 0 || from >= end as isize {
            break;
        }
        let next = SendMessageW(hwnd, EM_LINEINDEX, line as usize + 1, 0);
        if next >= 0 && next <= start as isize {
            continue;
        }
        let a = (from as u32).max(start) as i32;
        let b = if next < 0 { end } else { end.min(next as u32) } as i32;
        if a >= b {
            continue;
        }
        let Some(left) = point(a, 0) else {
            continue;
        };
        if left.y >= rc.bottom {
            break;
        }
        let Some(bottom) = point(a, TA_BOTTOM as i32) else {
            continue;
        };
        let Some(mut right) = point(b, 0) else {
            continue;
        };
        if right.y != left.y {
            right = point(b - 1, TA_RIGHT as i32).unwrap_or(left);
        }
        let x = left.x.min(right.x).max(0);
        let edge = left
            .x
            .max(right.x)
            .max(left.x.min(right.x) + 6)
            .min(rc.right - 12);
        if edge <= x || bottom.y <= 0 {
            continue;
        }
        let row = CreateRoundRectRgn(x, left.y.max(0), edge, bottom.y.min(rc.bottom), 5, 5);
        CombineRgn(region, region, row, RGN_OR);
        DeleteObject(row);
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
