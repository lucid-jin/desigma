//! Imports one file through `load_external` and prints the node count; run under
//! `/usr/bin/time -l` to read the peak resident set size.
fn main() {
    let path = std::env::args().nth(1).expect("usage: import_peak <file>");
    let doc = desigma_library::load_external(std::path::Path::new(&path)).expect("import failed");
    println!("nodes={}", doc.nodes.len());
}
