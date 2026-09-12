use crate::theme::*;
use std::{
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, Shell::*, WindowsAndMessaging::*},
};

struct Node {
    path: PathBuf,
    directory: bool,
    loaded: bool,
}
pub struct Workspace {
    pub hwnd: HWND,
    pub root: PathBuf,
    nodes: Vec<Node>,
    images: HIMAGELIST,
}
impl Workspace {
    pub unsafe fn create(parent: HWND, root: PathBuf, font: HFONT) -> Result<Self, String> {
        let root = std::path::absolute(&root).map_err(|e| e.to_string())?;
        if !root.is_dir() {
            return Err("Choose a folder".into());
        }
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TREEVIEW_CLASSES,
        });
        let hwnd = CreateWindowExW(
            0,
            wide("SysTreeView32").as_ptr(),
            wide("Files").as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | TVS_HASBUTTONS
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS
                | TVS_FULLROWSELECT
                | TVS_NOTOOLTIPS
                | TVS_NOHSCROLL,
            0,
            0,
            240,
            500,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if hwnd.is_null() {
            return Err("Could not create file tree".into());
        }
        SetWindowTheme(hwnd, wide("").as_ptr(), wide("").as_ptr());
        SendMessageW(hwnd, WM_SETFONT, font as usize, 0);
        SendMessageW(hwnd, TVM_SETBKCOLOR, 0, SURFACE as isize);
        SendMessageW(hwnd, TVM_SETTEXTCOLOR, 0, INK as isize);
        SendMessageW(hwnd, TVM_SETITEMHEIGHT, px(hwnd, 26) as usize, 0);
        SendMessageW(
            hwnd,
            TVM_SETEXTENDEDSTYLE,
            TVS_EX_DOUBLEBUFFER as usize,
            TVS_EX_DOUBLEBUFFER as isize,
        );
        crate::scroll::attach(hwnd, SURFACE);
        let mut tree = Self {
            hwnd,
            root,
            nodes: Vec::new(),
            images: 0,
        };
        tree.update_icons()?;
        tree.refresh()?;
        Ok(tree)
    }
    pub unsafe fn update_icons(&mut self) -> Result<(), String> {
        let images = tree_icons(px(self.hwnd, 18))?;
        SendMessageW(self.hwnd, TVM_SETIMAGELIST, TVSIL_NORMAL as usize, images);
        if self.images != 0 {
            ImageList_Destroy(self.images);
        }
        self.images = images;
        Ok(())
    }
    pub unsafe fn refresh(&mut self) -> Result<(), String> {
        // Read before replacing the tree so an inaccessible directory retains its previous view.
        let entries = entries(&self.root)?;
        SendMessageW(self.hwnd, TVM_DELETEITEM, 0, TVI_ROOT);
        self.nodes.clear();
        let root = self.insert(TVI_ROOT, self.root.clone(), true);
        for (path, directory) in entries {
            self.insert(root, path, directory);
        }
        self.nodes[0].loaded = true;
        SendMessageW(self.hwnd, TVM_EXPAND, TVE_EXPAND as usize, root);
        Ok(())
    }
    unsafe fn insert(&mut self, parent: HTREEITEM, path: PathBuf, directory: bool) -> HTREEITEM {
        let label = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        let mut label = wide(&label);
        let icon = file_icon(&path, directory);
        let item = TVITEMW {
            mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
            pszText: label.as_mut_ptr(),
            lParam: self.nodes.len() as isize,
            cChildren: i32::from(directory),
            iImage: icon,
            iSelectedImage: icon,
            ..zeroed()
        };
        let insert = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 { item },
        };
        self.nodes.push(Node {
            path,
            directory,
            loaded: false,
        });
        SendMessageW(self.hwnd, TVM_INSERTITEMW, 0, &insert as *const _ as isize)
    }
    pub unsafe fn notify(&mut self, lp: isize) -> Result<Option<PathBuf>, String> {
        let hdr = &*(lp as *const NMHDR);
        if hdr.hwndFrom != self.hwnd {
            return Ok(None);
        }
        if !matches!(hdr.code, TVN_ITEMEXPANDINGW | TVN_SELCHANGEDW) {
            return Ok(None);
        }
        let event = &*(lp as *const NMTREEVIEWW);
        let index = event.itemNew.lParam as usize;
        let Some(node) = self.nodes.get(index) else {
            return Ok(None);
        };
        if hdr.code == TVN_SELCHANGEDW && !node.directory {
            return Ok(Some(node.path.clone()));
        }
        if hdr.code == TVN_ITEMEXPANDINGW
            && event.action == TVE_EXPAND
            && node.directory
            && !node.loaded
        {
            let entries = entries(&node.path)?;
            self.nodes[index].loaded = true;
            for (path, directory) in entries {
                self.insert(event.itemNew.hItem, path, directory);
            }
        }
        Ok(None)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
            ImageList_Destroy(self.images);
        }
    }
}

// Seven shared, DPI-sized images; never load Shell handlers or cache per file.
fn file_icon(path: &std::path::Path, directory: bool) -> i32 {
    if directory {
        return 0;
    }
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" | "tgz" | "cab" => 1,
        "md" | "markdown" | "mdown" => 2,
        "pdf" => 3,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "avif" | "tif"
        | "tiff" => 4,
        "rs" | "py" | "js" | "jsx" | "ts" | "tsx" | "html" | "css" | "c" | "h" | "cpp" | "hpp"
        | "cs" | "go" | "java" | "json" | "toml" | "yaml" | "yml" | "xml" | "sh" | "ps1" => 5,
        _ => 6,
    }
}

unsafe fn tree_icons(size: i32) -> Result<HIMAGELIST, String> {
    let images = ImageList_Create(size, size, ILC_COLOR32, 7, 0);
    if images == 0 {
        return Err("Could not create file icons".into());
    }
    // Supersample once on the CPU; keep only the small premultiplied-alpha images.
    let high = size * 4;
    let dc = CreateCompatibleDC(null_mut());
    let bitmap = |side: i32, bits: &mut *mut std::ffi::c_void| {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side,
                biHeight: -side,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..zeroed()
            },
            ..zeroed()
        };
        CreateDIBSection(dc, &info, DIB_RGB_COLORS, bits, null_mut(), 0)
    };
    let mut source = null_mut();
    let mut target = null_mut();
    let large = bitmap(high, &mut source);
    let small = bitmap(size, &mut target);
    if dc.is_null() || large.is_null() || small.is_null() {
        DeleteObject(large);
        DeleteObject(small);
        DeleteDC(dc);
        ImageList_Destroy(images);
        return Err("Could not draw file icons".into());
    }
    let previous = SelectObject(dc, large);
    let pen = CreatePen(PS_SOLID, (high * 16 / 180).max(1), WHITE);
    let cutout = CreatePen(PS_SOLID, (high * 16 / 180).max(1), 0);
    let old_pen = SelectObject(dc, pen);
    let old_brush = SelectObject(dc, GetStockObject(WHITE_BRUSH));
    let line = |points: &[(i32, i32)]| {
        let points: Vec<POINT> = points
            .iter()
            .map(|&(x, y)| POINT {
                x: x * high / 18,
                y: y * high / 18,
            })
            .collect();
        Polyline(dc, points.as_ptr(), points.len() as i32);
    };
    let solid = |points: &[(i32, i32)]| {
        let points: Vec<POINT> = points
            .iter()
            .map(|&(x, y)| POINT {
                x: x * high / 18,
                y: y * high / 18,
            })
            .collect();
        SelectObject(dc, GetStockObject(NULL_PEN));
        Polygon(dc, points.as_ptr(), points.len() as i32);
        SelectObject(dc, cutout);
    };
    let colors = [
        rgb(248, 203, 102),
        rgb(211, 180, 250),
        rgb(99, 235, 212),
        rgb(255, 165, 157),
        rgb(141, 207, 250),
        rgb(175, 195, 255),
        rgb(199, 212, 218),
    ];
    let mut ok = true;
    for (kind, color) in colors.into_iter().enumerate() {
        PatBlt(dc, 0, 0, high, high, BLACKNESS);
        match kind {
            0 => {
                solid(&[(1, 4), (7, 4), (9, 6), (17, 6), (17, 15), (1, 15)]);
                line(&[(2, 8), (16, 8)]);
            }
            1 => {
                // A broad archive box, not another folded document silhouette.
                solid(&[(2, 3), (16, 3), (16, 16), (2, 16)]);
                for y in [4, 7, 10] {
                    line(&[(8, y), (11, y)]);
                }
                line(&[(8, 13), (11, 13)]);
            }
            4 => {
                solid(&[(1, 4), (17, 4), (17, 15), (1, 15)]);
                line(&[(3, 13), (7, 9), (10, 12), (13, 8), (15, 11)]);
                line(&[(4, 7), (6, 7)]);
            }
            5 => {
                SelectObject(dc, pen);
                line(&[(6, 4), (2, 9), (6, 14)]);
                line(&[(12, 4), (16, 9), (12, 14)]);
                line(&[(10, 3), (8, 15)]);
            }
            _ => {
                solid(&[(3, 1), (11, 1), (16, 6), (16, 17), (3, 17)]);
                line(&[(11, 2), (11, 6), (15, 6)]);
                match kind {
                    2 => line(&[(6, 14), (6, 9), (9, 12), (12, 9), (12, 14)]),
                    3 => line(&[(7, 15), (7, 9), (12, 9), (12, 12), (7, 12)]),
                    _ => {
                        line(&[(6, 9), (13, 9)]);
                        line(&[(6, 13), (11, 13)]);
                    }
                }
            }
        }
        GdiFlush();
        let src = std::slice::from_raw_parts(source.cast::<u32>(), (high * high) as usize);
        let dst = std::slice::from_raw_parts_mut(target.cast::<u32>(), (size * size) as usize);
        for y in 0..size {
            for x in 0..size {
                let mut alpha = 0;
                for dy in 0..4 {
                    for dx in 0..4 {
                        alpha += src[((y * 4 + dy) * high + x * 4 + dx) as usize] & 255;
                    }
                }
                alpha /= 16;
                let r = (color & 255) * alpha / 255;
                let g = ((color >> 8) & 255) * alpha / 255;
                let b = ((color >> 16) & 255) * alpha / 255;
                dst[(y * size + x) as usize] = (alpha << 24) | (r << 16) | (g << 8) | b;
            }
        }
        ok &= ImageList_Add(images, small, null_mut()) == kind as i32;
    }
    SelectObject(dc, old_pen);
    DeleteObject(pen);
    DeleteObject(cutout);
    SelectObject(dc, old_brush);
    SelectObject(dc, previous);
    DeleteObject(large);
    DeleteObject(small);
    DeleteDC(dc);
    if ok {
        Ok(images)
    } else {
        ImageList_Destroy(images);
        Err("Could not store file icons".into())
    }
}

fn entries(path: &std::path::Path) -> Result<Vec<(PathBuf, bool)>, String> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .map(|entry| {
            let entry = entry.map_err(|e| e.to_string())?;
            let directory = entry.file_type().map_err(|e| e.to_string())?.is_dir();
            Ok((entry.path(), directory))
        })
        .collect::<Result<Vec<_>, String>>()?;
    entries.sort_by_cached_key(|(path, directory)| {
        (
            !directory,
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
        )
    });
    Ok(entries)
}
pub unsafe fn choose_folder(parent: HWND) -> Option<PathBuf> {
    let info = BROWSEINFOW {
        hwndOwner: parent,
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX,
        ..zeroed()
    };
    // Keep the title buffer alive while the native dialog uses it.
    let title = wide("Open folder");
    let info = BROWSEINFOW {
        lpszTitle: title.as_ptr(),
        ..info
    };
    let item = SHBrowseForFolderW(&info);
    if item.is_null() {
        return None;
    }
    let mut path = [0u16; 32768];
    let ok = SHGetPathFromIDListEx(item, path.as_mut_ptr(), path.len() as u32, 0);
    windows::Win32::System::Com::CoTaskMemFree(Some(item.cast()));
    if ok == 0 {
        return None;
    }
    use std::os::windows::ffi::OsStringExt;
    let end = path.iter().position(|c| *c == 0).unwrap_or(path.len());
    Some(std::ffi::OsString::from_wide(&path[..end]).into())
}

#[test]
#[ignore = "Requires Windows native controls"]
fn native_tree_loads_only_expanded_directories() {
    use windows_sys::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT};
    let root = std::env::temp_dir().join(format!("plumetxt-workspace-{}", std::process::id()));
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("note.md"), "# Note").unwrap();
    std::fs::write(root.join("sub/deep.txt"), "nested").unwrap();
    unsafe {
        let parent = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            0,
            0,
            600,
            400,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        let mut tree =
            Workspace::create(parent, root.clone(), GetStockObject(DEFAULT_GUI_FONT)).unwrap();
        assert_eq!(tree.nodes.len(), 3);
        assert!(!tree.root.to_string_lossy().starts_with(r"\\?\"));
        let images = SendMessageW(tree.hwnd, TVM_GETIMAGELIST, TVSIL_NORMAL as usize, 0);
        assert_ne!(images, 0);
        assert_eq!(ImageList_GetImageCount(images as _), 7);
        for dpi in [96, 144, 192, 96] {
            SetPropW(parent, wide("PlumeTxtDpi").as_ptr(), dpi as usize as _);
            tree.update_icons().unwrap();
            let (mut width, mut height) = (0, 0);
            assert_ne!(
                ImageList_GetIconSize(tree.images, &mut width, &mut height),
                0
            );
            assert_eq!((width, height), (scale(18, dpi), scale(18, dpi)));
            assert_eq!(ImageList_GetImageCount(tree.images), 7);
        }
        for (name, directory, expected) in [
            ("a.ZIP", false, 1),
            ("a.tar.gz", false, 1),
            ("a.zip", true, 0),
            ("a.md", false, 2),
            ("a.pdf", false, 3),
            ("a.png", false, 4),
            ("a.rs", false, 5),
            ("LICENSE", false, 6),
        ] {
            assert_eq!(file_icon(std::path::Path::new(name), directory), expected);
        }
        assert_ne!(
            GetWindowLongW(tree.hwnd, GWL_STYLE) as u32 & TVS_NOTOOLTIPS,
            0
        );
        assert!(tree.nodes[1].directory && !tree.nodes[1].loaded);
        let root_item = SendMessageW(tree.hwnd, TVM_GETNEXTITEM, TVGN_ROOT as usize, 0);
        let child = SendMessageW(tree.hwnd, TVM_GETNEXTITEM, TVGN_CHILD as usize, root_item);
        let mut event: NMTREEVIEWW = zeroed();
        event.hdr.hwndFrom = tree.hwnd;
        event.hdr.code = TVN_ITEMEXPANDINGW;
        event.action = TVE_EXPAND;
        event.itemNew.hItem = child;
        event.itemNew.lParam = 1;
        tree.notify(&event as *const _ as isize).unwrap();
        tree.notify(&event as *const _ as isize).unwrap();
        assert_eq!(tree.nodes.len(), 4);
        assert!(tree.nodes[3].path.ends_with("sub/deep.txt"));
        event.hdr.code = TVN_SELCHANGEDW;
        event.itemNew.lParam = 3;
        assert_eq!(
            tree.notify(&event as *const _ as isize).unwrap(),
            Some(tree.nodes[3].path.clone())
        );
        drop(tree);
        DestroyWindow(parent);
    }
    std::fs::remove_file(root.join("note.md")).unwrap();
    std::fs::remove_file(root.join("sub/deep.txt")).unwrap();
    std::fs::remove_dir(root.join("sub")).unwrap();
    std::fs::remove_dir(root).unwrap();
}
