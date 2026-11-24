//! Prints a `.fig` outline; with a second argument, imports only the top-level frames whose
//! name contains it and prints what came in. Run under `/usr/bin/time -l` for the peak memory.
use std::path::Path;
fn main() {
    let path = std::env::args().nth(1).expect("usage: frame_pick <file.fig> [frame-name-part]");
    let t = std::time::Instant::now();
    let outline = desigma_library::inspect_external(Path::new(&path)).expect("outline");
    println!(
        "outline in {:.1}s: {} stored nodes, {} images, {:.1} MiB",
        t.elapsed().as_secs_f64(),
        outline.stored_nodes,
        outline.image_count,
        outline.image_bytes as f64 / 1048576.0
    );
    for p in &outline.pages {
        println!("page {:?} internal={} ({} frames, {} nodes)", p.name, p.internal, p.frames.len(), p.frames.iter().map(|f| f.nodes).sum::<usize>());
        if p.internal {
            continue;
        }
        for f in &p.frames {
            println!("  {:>6} nodes {:>7.1} MiB  {} {:?}  {}", f.nodes, f.image_bytes as f64 / 1048576.0, f.kind, f.name, f.id);
        }
    }
    let Some(part) = std::env::args().nth(2) else { return };
    let ids = outline.pages.iter().filter(|p| !p.internal).flat_map(|p| &p.frames).filter(|f| f.name.contains(&part)).map(|f| f.id.clone()).collect();
    let t = std::time::Instant::now();
    let doc = desigma_library::load_external_with(Path::new(&path), &desigma_library::ImportOptions { only_frames: Some(ids) }).expect("import");
    let imgs: u64 = doc.images.values().map(|v| v.len() as u64).sum();
    println!("picked {part:?}: {} nodes, {} images, {:.1} MiB, {:.1}s", doc.nodes.len(), doc.images.len(), imgs as f64 / 1048576.0, t.elapsed().as_secs_f64());
}
