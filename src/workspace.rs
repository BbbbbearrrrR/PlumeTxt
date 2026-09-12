use crate::theme::*;
use std::{
    mem::{size_of, zeroed},
    path::PathBuf,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::HFONT,
    Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL},
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
        SendMessageW(hwnd, TVM_SETITEMHEIGHT, 24, 0);
        SendMessageW(
            hwnd,
            TVM_SETEXTENDEDSTYLE,
            TVS_EX_DOUBLEBUFFER as usize,
            TVS_EX_DOUBLEBUFFER as isize,
        );
        crate::scroll::attach(hwnd, SURFACE);
        let mut icon: SHFILEINFOW = zeroed();
        let images = SHGetFileInfoW(
            wide("folder").as_ptr(),
            FILE_ATTRIBUTE_DIRECTORY,
            &mut icon,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        );
        SendMessageW(
            hwnd,
            TVM_SETIMAGELIST,
            TVSIL_NORMAL as usize,
            images as isize,
        );
        let mut tree = Self {
            hwnd,
            root,
            nodes: Vec::new(),
        };
        tree.refresh()?;
        Ok(tree)
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
        let mut icon: SHFILEINFOW = zeroed();
        SHGetFileInfoW(
            label.as_ptr(),
            if directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                FILE_ATTRIBUTE_NORMAL
            },
            &mut icon,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        );
        let item = TVITEMW {
            mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
            pszText: label.as_mut_ptr(),
            lParam: self.nodes.len() as isize,
            cChildren: i32::from(directory),
            iImage: icon.iIcon,
            iSelectedImage: icon.iIcon,
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
        }
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
    let root = std::env::temp_dir().join(format!("featherpad-workspace-{}", std::process::id()));
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
        assert!(ImageList_GetImageCount(images as _) > 0);
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
