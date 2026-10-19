//! Adversarial case on decision 4: `inference.graph.AddPort/added` now sets `Port.pipeline_id`
//! from its input, and a port may name a pipeline other than its node's, so the port's pipeline
//! is no longer derivable from its node. A field the specification sets and no view or event
//! publishes is one no conformance scenario reads: a target storing the node's pipeline instead
//! answers the suite the same.

/// The specification of the graph domain, as committed.
const GRAPH: &str = include_str!("../../../ess/domains/graph.yaml");

/// The field names of the `- name: <declaration>` block of `section` (`views:` or `events:`).
fn fields_of(section: &str, declaration: &str) -> Vec<String> {
    let start = GRAPH
        .lines()
        .position(|line| line == section)
        .unwrap_or_else(|| panic!("graph.yaml has a `{section}` section"));
    let mut lines = GRAPH
        .lines()
        .skip(start + 1)
        .take_while(|line| line.is_empty() || line.starts_with(' ') || line.starts_with('#'))
        .skip_while(|line| line.trim() != format!("- name: {declaration}"));
    lines.next().unwrap_or_else(|| {
        panic!("graph.yaml declares `{declaration}` under `{section}`");
    });
    lines
        .take_while(|line| !line.starts_with("  - name:"))
        .filter_map(|line| line.trim().strip_prefix("- name: "))
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_pipeline_a_port_was_declared_under_is_published() {
    let view = fields_of("views:", "inference.graph.Ports");
    let event = fields_of("events:", "inference.graph.PortAdded");
    assert!(
        view.contains(&"port_id".to_owned()) && event.contains(&"port_id".to_owned()),
        "the parse found the declarations: {view:?} {event:?}"
    );
    assert!(
        view.contains(&"pipeline_id".to_owned()) || event.contains(&"pipeline_id".to_owned()),
        "`Port.pipeline_id` is set by `AddPort/added` and published by neither the `Ports` view \
         ({view:?}) nor `PortAdded` ({event:?}), so no scenario can tell the pipeline a port was \
         declared under from its node's"
    );
}
