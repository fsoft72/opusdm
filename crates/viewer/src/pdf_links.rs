//! Links of a PDF page: the `/Link` annotations that go to another page of the
//! document or to a URI. Boxes are in page points with y going down, like the
//! text of `pdf_text`.

use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier, Rect as PdfRect, String as PdfString};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::{Point, Rect};

/// Deepest name tree level followed (a broken file could loop).
const MAX_TREE_DEPTH: usize = 32;

/// Where a link leads.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// A page of the same document (0-based).
    Page(usize),
    /// A web address, a mail address...: opened with the default application.
    Uri(String),
}

/// A clickable box of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub rect: Rect,
    pub target: LinkTarget,
}

/// The links of page `index` (0-based) that lead somewhere this viewer can go.
pub fn page_links(pdf: &Pdf, index: usize) -> Vec<Link> {
    let Some(page) = pdf.pages().get(index) else { return Vec::new() };
    let Some(annots) = page.raw().get::<Array>(b"Annots") else { return Vec::new() };
    let transform = page.initial_transform(true).to_kurbo();
    let mut links = Vec::new();
    for annot in annots.iter::<Dict>() {
        if !annot.get::<Name>(b"Subtype").is_some_and(|n| n.as_ref() == b"Link") { continue; }
        let Some(r) = annot.get::<PdfRect>(b"Rect") else { continue };
        let Some(target) = _target(pdf, &annot) else { continue };
        let rect = transform.transform_rect_bbox(Rect::new(r.x0, r.y0, r.x1, r.y1));
        links.push(Link { rect, target });
    }
    links
}

/// The link at `point` (page points), the last one drawn when several overlap.
pub fn link_at(links: &[Link], point: (f64, f64)) -> Option<&Link> {
    links.iter().rev().find(|l| l.rect.contains(Point::new(point.0, point.1)))
}

/// Target of a link annotation: its action (`/A`, URI or GoTo) or its `/Dest`.
fn _target(pdf: &Pdf, annot: &Dict<'_>) -> Option<LinkTarget> {
    let Some(action) = annot.get::<Dict>(b"A") else { return _destination(pdf, annot.get::<Object>(b"Dest")?, 0) };
    match action.get::<Name>(b"S")?.as_ref() {
        b"URI" => {
            let uri = action.get::<PdfString>(b"URI")?;
            let uri = String::from_utf8_lossy(uri.as_bytes()).trim().to_string();
            (!uri.is_empty()).then_some(LinkTarget::Uri(uri))
        }
        b"GoTo" => _destination(pdf, action.get::<Object>(b"D")?, 0),
        _ => None,
    }
}

/// Page of a destination: an explicit array `[page /XYZ ...]`, a dictionary
/// with `/D`, or a name looked up in the catalog.
fn _destination(pdf: &Pdf, dest: Object<'_>, depth: usize) -> Option<LinkTarget> {
    if depth > 2 { return None; }
    match dest {
        Object::Array(array) => _page_of_array(pdf, &array).map(LinkTarget::Page),
        Object::Dict(dict) => _destination(pdf, dict.get::<Object>(b"D")?, depth + 1),
        Object::Name(name) => _destination(pdf, _named(pdf, name.as_ref())?, depth + 1),
        Object::String(name) => _destination(pdf, _named(pdf, name.as_bytes())?, depth + 1),
        _ => None,
    }
}

/// Page index of an explicit destination: its first item is a page reference
/// (or, in some files, a page number).
fn _page_of_array(pdf: &Pdf, array: &Array<'_>) -> Option<usize> {
    let first = array.raw_iter().next()?;
    if let Some(r) = first.as_obj_ref() {
        let id = ObjectIdentifier::from(r);
        return pdf.pages().iter().position(|p| p.raw().obj_id() == Some(id));
    }
    let MaybeRef::NotRef(Object::Number(n)) = first else { return None };
    let index = n.as_f64();
    (index >= 0.0 && (index as usize) < pdf.pages().len()).then_some(index as usize)
}

/// A named destination: the catalog `/Dests` dictionary (PDF 1.1) or the
/// `/Names /Dests` name tree.
fn _named<'a>(pdf: &'a Pdf, name: &[u8]) -> Option<Object<'a>> {
    let catalog = pdf.xref().get::<Dict>(pdf.xref().root_id())?;
    if let Some(found) = catalog.get::<Dict>(b"Dests").and_then(|d| d.get::<Object>(name)) { return Some(found); }
    let tree = catalog.get::<Dict>(b"Names")?.get::<Dict>(b"Dests")?;
    _tree_lookup(&tree, name, 0)
}

/// Find `name` in a name tree node and its kids.
fn _tree_lookup<'a>(node: &Dict<'a>, name: &[u8], depth: usize) -> Option<Object<'a>> {
    if depth > MAX_TREE_DEPTH { return None; }
    if let Some(names) = node.get::<Array>(b"Names") {
        let mut items = names.iter::<Object>();
        while let (Some(key), Some(value)) = (items.next(), items.next()) {
            if let Object::String(key) = key {
                if key.as_bytes() == name { return Some(value); }
            }
        }
    }
    let kids = node.get::<Array>(b"Kids")?;
    kids.iter::<Dict>().find_map(|kid| _tree_lookup(&kid, name, depth + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::{build_pdf, page_obj};

    /// A link annotation over the 10..50 x 10..30 box of the page (PDF space).
    fn link(target: &str) -> String {
        format!("<< /Type /Annot /Subtype /Link /Rect [10 10 50 30] {target} >>")
    }

    /// Links of page 0 of a 3 page PDF with `annot` on page 0.
    fn links_of(annot: &str, catalog_extra: &str) -> Vec<Link> {
        let data = build_pdf(&["", "", ""], &[annot], catalog_extra);
        page_links(&Pdf::new(data).unwrap(), 0)
    }

    #[test]
    fn uri_link_with_its_box_in_page_space() {
        let links = links_of(&link("/A << /S /URI /URI (https://example.org/a) >>"), "");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, LinkTarget::Uri("https://example.org/a".to_string()));
        // y goes down: PDF 10..30 on a 100 point page is 70..90.
        assert_eq!(links[0].rect, Rect::new(10.0, 70.0, 50.0, 90.0));
    }

    #[test]
    fn explicit_destinations_go_to_the_page() {
        let dest = format!("/Dest [{} 0 R /XYZ 0 100 0]", page_obj(2));
        assert_eq!(links_of(&link(&dest), "")[0].target, LinkTarget::Page(2));
        let goto = format!("/A << /S /GoTo /D [{} 0 R /Fit] >>", page_obj(1));
        assert_eq!(links_of(&link(&goto), "")[0].target, LinkTarget::Page(1));
    }

    #[test]
    fn named_destinations_are_looked_up() {
        let old_style = format!("/Dests << /chapter [{} 0 R /Fit] >>", page_obj(1));
        assert_eq!(links_of(&link("/Dest /chapter"), &old_style)[0].target, LinkTarget::Page(1));
        let tree = format!("/Names << /Dests << /Kids [<< /Names [(a) [{} 0 R /Fit] (b) << /D [{} 0 R /Fit] >>] >>] >> >>", page_obj(1), page_obj(2));
        assert_eq!(links_of(&link("/Dest (b)"), &tree)[0].target, LinkTarget::Page(2));
        assert_eq!(links_of(&link("/A << /S /GoTo /D (a) >>"), &tree)[0].target, LinkTarget::Page(1));
        assert!(links_of(&link("/Dest (missing)"), &tree).is_empty());
    }

    #[test]
    fn other_annotations_and_actions_are_skipped() {
        assert!(links_of("<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] >>", "").is_empty());
        assert!(links_of(&link("/A << /S /Launch /F (x.exe) >>"), "").is_empty());
        assert!(links_of(&link("/A << /S /URI /URI () >>"), "").is_empty());
    }

    #[test]
    fn link_at_finds_the_box_under_the_point() {
        let links = vec![Link { rect: Rect::new(0.0, 0.0, 10.0, 10.0), target: LinkTarget::Page(1) }, Link { rect: Rect::new(5.0, 5.0, 20.0, 20.0), target: LinkTarget::Page(2) }];
        assert_eq!(link_at(&links, (2.0, 2.0)).unwrap().target, LinkTarget::Page(1));
        assert_eq!(link_at(&links, (7.0, 7.0)).unwrap().target, LinkTarget::Page(2));
        assert!(link_at(&links, (30.0, 30.0)).is_none());
    }
}
