//! `outline_fig` lists what a `.fig` holds; `import_fig_with` imports only the frames picked.

use desigma_doc::NodeKind;
use desigma_fig::{import_fig, import_fig_with, outline_fig, testkit::frames_fig, ImportOptions};
use std::collections::BTreeSet;

fn only(ids: &[&str]) -> ImportOptions {
    ImportOptions { only_frames: Some(ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>()) }
}

fn frame_names(doc: &desigma_doc::Document) -> Vec<String> {
    doc.pages
        .iter()
        .flat_map(|p| doc.get(*p).unwrap().children.iter().filter_map(|c| doc.get(*c)).filter(|n| matches!(n.kind, NodeKind::Frame(_))).map(|n| n.name.clone()))
        .collect()
}

#[test]
fn the_outline_lists_pages_frames_and_what_each_weighs() {
    let o = outline_fig(&frames_fig()).unwrap();
    assert_eq!((o.stored_nodes, o.image_count, o.image_bytes), (19, 4, 15_000));
    let pages: Vec<_> = o.pages.iter().map(|p| (p.name.as_str(), p.internal, p.frames.len())).collect();
    assert_eq!(pages, [("Landing", false, 3), ("Other page", false, 1), ("Internal Only Canvas", true, 2)]);
    let f = |page: usize, i: usize| {
        let f = &o.pages[page].frames[i];
        (f.name.as_str(), f.kind.as_str(), f.nodes, f.image_bytes)
    };
    assert_eq!(f(0, 0), ("Hero", "FRAME", 3, 1_000));
    assert_eq!(f(0, 1), ("Footer", "FRAME", 2, 3_000), "images 1 and 2");
    assert_eq!(f(0, 2), ("Empty", "FRAME", 2, 0), "the instance counts once, not as its copies");
    assert_eq!(f(1, 0), ("Other", "FRAME", 4, 4_000));
}

#[test]
fn importing_everything_is_unchanged() {
    let all = import_fig(&frames_fig()).unwrap();
    assert_eq!(frame_names(&all), ["Hero", "Footer", "Empty", "Other"]);
    assert_eq!(all.images.len(), 4, "the unused image is kept when nothing was picked");
    assert_eq!(frame_names(&import_fig_with(&frames_fig(), &ImportOptions::default()).unwrap()), frame_names(&all));
}

#[test]
fn only_the_picked_frames_come_in_with_their_images() {
    let o = outline_fig(&frames_fig()).unwrap();
    let footer = o.pages[0].frames[1].id.clone();
    let doc = import_fig_with(&frames_fig(), &only(&[&footer])).unwrap();
    assert_eq!(frame_names(&doc), ["Footer"]);
    assert_eq!(doc.pages.len(), 1, "the page with nothing picked is left out");
    let mut kept: Vec<_> = doc.images.keys().cloned().collect();
    kept.sort();
    assert_eq!(kept, [desigma_fig::kiwi::hex(&[0xa0, 1, 0, 0]), desigma_fig::kiwi::hex(&[0xa0, 2, 0, 0])]);
    assert_eq!(doc.nodes.len(), 1 + 2, "the page, the frame and its picture");
    assert!(doc.library_page.is_none(), "Footer uses nothing from the library");
}

#[test]
fn frames_from_several_pages_can_be_picked_together() {
    let o = outline_fig(&frames_fig()).unwrap();
    let ids = [o.pages[0].frames[0].id.clone(), o.pages[1].frames[0].id.clone()];
    let doc = import_fig_with(&frames_fig(), &only(&[&ids[0], &ids[1]])).unwrap();
    assert_eq!(frame_names(&doc), ["Hero", "Other"]);
    assert_eq!(doc.pages.len(), 2);
    assert_eq!(doc.images.len(), 2);
}

#[test]
fn a_pick_that_matches_nothing_is_an_error() {
    let err = import_fig_with(&frames_fig(), &only(&["9:9"])).unwrap_err().to_string();
    assert!(err.contains("no top-level frame with id 9:9"), "{err}");
    let err = import_fig_with(&frames_fig(), &only(&[])).unwrap_err().to_string();
    assert!(err.contains("no frames were picked"), "{err}");
    // a page id, or a node below a frame, is not a top-level frame
    let o = outline_fig(&frames_fig()).unwrap();
    assert!(import_fig_with(&frames_fig(), &only(&[&o.pages[0].id])).is_err());
}

/// Names of the components in the hidden library page.
fn library_names(doc: &desigma_doc::Document) -> Vec<String> {
    let Some(page) = doc.library_page else { return vec![] };
    doc.get(page).unwrap().children.iter().filter_map(|c| doc.get(*c)).map(|n| n.name.clone()).collect()
}

#[test]
fn only_the_library_parts_a_pick_uses_come_along() {
    let o = outline_fig(&frames_fig()).unwrap();
    let hero = o.pages[0].frames[0].id.clone();
    let doc = import_fig_with(&frames_fig(), &only(&[&hero])).unwrap();
    assert_eq!(frame_names(&doc), ["Hero"]);
    assert_eq!(library_names(&doc), ["LibButton"], "LibUnused stays out");
    assert!(doc.images.len() == 1, "image 4 went with LibUnused");
    // the whole import keeps both
    assert_eq!(library_names(&import_fig(&frames_fig()).unwrap()), ["LibButton", "LibUnused"]);
}

#[test]
fn a_pick_brings_the_frames_it_depends_on() {
    // `Empty` holds an instance of `Badge`, defined in the frame `Other` on another page.
    let o = outline_fig(&frames_fig()).unwrap();
    let empty = o.pages[0].frames[2].id.clone();
    let doc = import_fig_with(&frames_fig(), &only(&[&empty])).unwrap();
    assert_eq!(frame_names(&doc), ["Empty", "Other"], "the frame with the component comes along");
    assert_eq!(doc.pages.len(), 2);
    let instance = doc.nodes.values().find(|n| n.name == "Use Badge").unwrap();
    assert!(matches!(&instance.kind, NodeKind::Instance { .. }));
    assert!(!doc.nodes.values().any(|n| n.name == "Hero" || n.name == "Footer"));
}
