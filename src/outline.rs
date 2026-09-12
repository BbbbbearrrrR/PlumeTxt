use lopdf::{Document, LoadOptions, Object, ObjectId};
use std::{
    collections::{HashMap, HashSet},
    os::windows::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub title: String,
    pub level: usize,
    pub page: Option<u32>,
    pub top: Option<f32>,
}

pub fn read(path: &Path) -> Result<Vec<Bookmark>, String> {
    // Deny concurrent writers/deletion for the lifetime of the read-only mapping.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mapped = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| e.to_string())?;
    let options = LoadOptions {
        filter: Some(metadata_only),
        max_decompressed_size: Some(16 * 1024 * 1024),
        ..Default::default()
    };
    let document = Document::load_mem_with_options(&mapped, options).map_err(|e| e.to_string())?;
    extract(&document)
}

fn metadata_only(id: ObjectId, object: &mut Object) -> Option<(ObjectId, Object)> {
    if let Object::Stream(stream) = object {
        if !stream.dict.has_type(b"ObjStm") {
            return None;
        }
    }
    if let Ok(dict) = object.as_dict_mut() {
        for key in [
            b"Resources".as_slice(),
            b"Contents",
            b"Annots",
            b"Font",
            b"XObject",
        ] {
            dict.remove(key);
        }
    }
    Some((id, object.clone()))
}

fn resolved<'a>(doc: &'a Document, mut object: &'a Object) -> Option<&'a Object> {
    for _ in 0..32 {
        match object {
            Object::Reference(id) => object = doc.objects.get(id)?,
            _ => return Some(object),
        }
    }
    None
}

pub fn extract(doc: &Document) -> Result<Vec<Bookmark>, String> {
    let catalog = doc.catalog().map_err(|e| e.to_string())?;
    let Ok(root) = catalog.get(b"Outlines") else {
        return Ok(Vec::new());
    };
    let Some(root) = resolved(doc, root).and_then(|o| o.as_dict().ok()) else {
        return Err("Invalid PDF outline".into());
    };
    let Ok(first) = root.get(b"First") else {
        return Ok(Vec::new());
    };
    let pages: HashMap<_, _> = doc
        .get_pages()
        .into_iter()
        .map(|(n, id)| (id, n - 1))
        .collect();
    let mut names = HashMap::<Vec<u8>, Object>::new();
    if let Ok(legacy) = catalog.get(b"Dests") {
        if let Some(dict) = resolved(doc, legacy).and_then(|o| o.as_dict().ok()) {
            for (key, value) in dict.iter() {
                names.insert(key.clone(), value.clone());
            }
        }
    }
    if let Ok(tree) = doc
        .get_dict_in_dict(catalog, b"Names")
        .and_then(|d| d.get(b"Dests"))
    {
        let mut stack = vec![tree];
        let mut seen = HashSet::new();
        let mut nodes = 0;
        while let Some(node) = stack.pop() {
            nodes += 1;
            if nodes > 20_000 {
                break;
            }
            if let Object::Reference(id) = node {
                if !seen.insert(*id) {
                    continue;
                }
            }
            let Some(dict) = resolved(doc, node).and_then(|o| o.as_dict().ok()) else {
                continue;
            };
            if let Ok(pairs) = dict.get(b"Names").and_then(Object::as_array) {
                for pair in pairs.chunks_exact(2) {
                    if let Ok(name) = pair[0].as_str() {
                        names.insert(name.to_vec(), pair[1].clone());
                    }
                }
            }
            if let Ok(kids) = dict.get(b"Kids").and_then(Object::as_array) {
                stack.extend(kids.iter().rev());
            }
        }
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![(first, 0)];
    while let Some((node, level)) = stack.pop() {
        if level > 64 || result.len() >= 20_000 {
            return Err("PDF outline exceeds the nesting or item limit".into());
        }
        if let Object::Reference(id) = node {
            if !seen.insert(*id) {
                continue;
            }
        }
        let Some(dict) = resolved(doc, node).and_then(|o| o.as_dict().ok()) else {
            continue;
        };
        if let Ok(next) = dict.get(b"Next") {
            stack.push((next, level));
        }
        if let Ok(child) = dict.get(b"First") {
            stack.push((child, level + 1));
        }
        let title = dict
            .get(b"Title")
            .ok()
            .and_then(|v| resolved(doc, v))
            .and_then(|v| lopdf::decode_text_string(v).ok())
            .unwrap_or_else(|| "Untitled".into());
        let dest = dict.get(b"Dest").ok().or_else(|| {
            let action = dict
                .get(b"A")
                .ok()
                .and_then(|v| resolved(doc, v))?
                .as_dict()
                .ok()?;
            // Only local navigation. Never execute URI, JavaScript, Launch or remote actions.
            if action.get(b"S").ok()?.as_name().ok()? != b"GoTo" {
                return None;
            }
            action.get(b"D").ok()
        });
        let (page, top) = dest
            .and_then(|v| destination(doc, v, &names, &pages))
            .unwrap_or((None, None));
        result.push(Bookmark {
            title: title
                .chars()
                .filter(|c| !c.is_control())
                .take(512)
                .collect(),
            level,
            page,
            top,
        });
    }
    Ok(result)
}

fn destination(
    doc: &Document,
    object: &Object,
    names: &HashMap<Vec<u8>, Object>,
    pages: &HashMap<ObjectId, u32>,
) -> Option<(Option<u32>, Option<f32>)> {
    let mut current = object;
    for _ in 0..32 {
        current = resolved(doc, current)?;
        match current {
            Object::Name(name) | Object::String(name, _) => current = names.get(name)?,
            Object::Dictionary(dict) => current = dict.get(b"D").ok()?,
            Object::Array(array) => {
                let page = match array.first()? {
                    Object::Reference(id) => pages.get(id).copied(),
                    Object::Integer(n) if *n >= 0 => u32::try_from(*n)
                        .ok()
                        .filter(|n| (*n as usize) < pages.len()),
                    _ => None,
                };
                let mode = array.get(1).and_then(|o| o.as_name().ok());
                let y = match mode {
                    Some(b"XYZ") => array.get(3),
                    Some(b"FitH" | b"FitBH") => array.get(2),
                    _ => None,
                };
                let top = y
                    .and_then(|n| match n {
                        Object::Real(v) => Some(*v),
                        Object::Integer(v) => Some(*v as f32),
                        _ => None,
                    })
                    .filter(|n| n.is_finite());
                return Some((page, top));
            }
            _ => return None,
        }
    }
    None
}

#[test]
fn nested_bookmarks_named_destinations_and_cycles() {
    use lopdf::{dictionary, StringFormat};
    let mut d = Document::with_version("1.7");
    let pages = d.new_object_id();
    let p1 = d.add_object(dictionary! {"Type"=>"Page", "Parent"=>pages});
    let p2 = d.add_object(dictionary! {"Type"=>"Page", "Parent"=>pages});
    d.set_object(
        pages,
        dictionary! {"Type"=>"Pages", "Kids"=>vec![p1.into(), p2.into()], "Count"=>2},
    );
    let first = d.new_object_id();
    let child = d.new_object_id();
    d.set_object(first, dictionary! {"Title"=>Object::String(vec![0xfe,0xff,0x7b,0x2c,0x4e,0x00,0x7a,0xe0],StringFormat::Hexadecimal), "Dest"=>vec![p1.into(), Object::Name(b"Fit".to_vec())], "First"=>child, "Next"=>first});
    d.set_object(child, dictionary! {"Title"=>Object::string_literal("Detail"), "A"=>dictionary!{"S"=>"GoTo","D"=>Object::string_literal("chapter2")}});
    let root = d.add_object(dictionary! {"Type"=>"Catalog", "Pages"=>pages, "Outlines"=>dictionary!{"First"=>first}, "Names"=>dictionary!{"Dests"=>dictionary!{"Names"=>vec![Object::string_literal("chapter2"), Object::Array(vec![p2.into(),Object::Name(b"XYZ".to_vec()),0.into(),700.into(),Object::Null])]}}});
    d.trailer.set("Root", root);
    let toc = extract(&d).unwrap();
    assert_eq!(toc.len(), 2);
    assert_eq!(toc[0].title, "第一章");
    assert_eq!(toc[1].level, 1);
    assert_eq!(toc[1].page, Some(1));
    assert_eq!(toc[1].top, Some(700.));
    std::fs::create_dir_all("tmp").unwrap();
    d.save("tmp/bookmark-fixture.pdf").unwrap();
    assert_eq!(
        read(Path::new("tmp/bookmark-fixture.pdf")).unwrap().len(),
        2
    );
}

#[test]
#[ignore = "Set FEATHERPAD_PDF_REGRESSION to verify a local PDF's outline geometry"]
fn local_pdf_destination_units_match_winrt() {
    let Ok(path) = std::env::var("FEATHERPAD_PDF_REGRESSION") else {
        return;
    };
    let d = Document::load(&path).unwrap();
    let pages = d.get_pages();
    let bookmarks = extract(&d).unwrap();
    unsafe {
        windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
        )
        .unwrap();
    }
    {
        let f = windows::Storage::StorageFile::GetFileFromPathAsync(&windows::core::HSTRING::from(
            path,
        ))
        .unwrap()
        .join()
        .unwrap();
        let pdf = windows::Data::Pdf::PdfDocument::LoadFromFileAsync(&f)
            .unwrap()
            .join()
            .unwrap();
        let mut checked = 0;
        for b in bookmarks {
            let (Some(index), Some(top)) = (b.page, b.top) else {
                continue;
            };
            let dict = d
                .get_object(pages[&(index + 1)])
                .unwrap()
                .as_dict()
                .unwrap();
            let bounds = dict.get(b"MediaBox").unwrap().as_array().unwrap();
            let number = |v: &Object| match v {
                Object::Integer(v) => *v as f32,
                Object::Real(v) => *v,
                _ => panic!("Invalid page bounds"),
            };
            let height = number(&bounds[3]) - number(&bounds[1]);
            let page = pdf.GetPage(index).unwrap();
            let display_height = page.Size().unwrap().Height;
            page.Close().unwrap();
            let expected = 1. - top / height;
            let actual = 1. - top * (96. / 72.) / display_height;
            assert!(
                (actual - expected).abs() < 0.0001,
                "Destination mismatch: {}",
                b.title
            );
            checked += 1;
        }
        assert!(checked > 0);
        eprintln!("Verified {checked} chapter destinations against PDF point geometry.");
    }
    unsafe {
        windows::Win32::System::WinRT::RoUninitialize();
    }
}
