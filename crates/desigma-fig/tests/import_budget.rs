//! The import stops expanding instances once the document reaches its node budget.

use desigma_fig::import_rest_json_limited;
use serde_json::json;

/// One component with `parts` children and `instances` childless instances of it.
fn file(parts: usize, instances: usize) -> String {
    let rects: Vec<_> = (0..parts).map(|i| json!({"id": format!("1:{}", 10 + i), "name": "Part", "type": "RECTANGLE", "size": {"x": 4, "y": 4}})).collect();
    let uses: Vec<_> =
        (0..instances).map(|i| json!({"id": format!("2:{i}"), "name": "Use", "type": "INSTANCE", "componentId": "1:1", "size": {"x": 10, "y": 10}})).collect();
    let mut kids = vec![json!({"id": "1:1", "name": "Comp", "type": "COMPONENT", "size": {"x": 10, "y": 10}, "children": rects})];
    kids.extend(uses);
    json!({"name": "budget", "document": {"type": "DOCUMENT", "children": [{"id": "0:1", "type": "CANVAS", "name": "P", "children": kids}]}}).to_string()
}

#[test]
fn expansion_within_the_budget_succeeds() {
    // page + component + 5 parts + 20 instances x (1 + 5 copies) = 127 nodes
    let doc = import_rest_json_limited(&file(5, 20), 1_000).unwrap();
    assert_eq!(doc.nodes.len(), 1 + 1 + 5 + 20 * 6);
}

#[test]
fn expansion_past_the_budget_fails_instead_of_half_copying() {
    let err = import_rest_json_limited(&file(5, 20), 60).unwrap_err().to_string();
    assert!(err.contains("exceed 60 nodes"), "{err}");
}

/// A page of `n` instances of components that are not in the file (each a different remote one).
fn remote_file(n: usize) -> String {
    let uses: Vec<_> = (0..n)
        .map(|i| json!({"id": format!("2:{i}"), "name": "Use", "type": "INSTANCE", "componentId": format!("9:{i}"), "size": {"x": 10, "y": 10}}))
        .collect();
    json!({"name": "remote", "document": {"type": "DOCUMENT", "children": [{"id": "0:1", "type": "CANVAS", "name": "P", "children": uses}]}}).to_string()
}

#[test]
fn remote_component_placeholders_count_toward_the_budget() {
    // 1 page + 50 instances = 51 nodes pass expansion, but linking 50 distinct remote components
    // adds a library page and a placeholder each: well past 60.
    let err = import_rest_json_limited(&remote_file(50), 60).unwrap_err().to_string();
    assert!(err.contains("past the limit of 60"), "{err}");
    assert!(import_rest_json_limited(&remote_file(50), 1_000).is_ok());
}

#[test]
fn the_budget_stops_the_copying_early() {
    // 20,000 instances of a 50-part component would copy a million nodes; the import must give
    // up near the budget rather than build them first.
    let started = std::time::Instant::now();
    let err = import_rest_json_limited(&file(50, 20_000), 5_000).unwrap_err().to_string();
    assert!(err.contains("exceed 5000 nodes"), "{err}");
    assert!(started.elapsed().as_secs() < 5, "took {:?}", started.elapsed());
}
