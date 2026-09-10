#[test]
#[ignore]
fn one_render() {
    let source = "flowchart TD\n    Edit --> Parse\n    Parse --> Paint\n";
    let started = std::time::Instant::now();
    let first = super::MermaidView::render(source);
    let first_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = std::time::Instant::now();
    for _ in 0..10 {
        let _ = super::MermaidView::render(source);
    }
    let warm_ms = started.elapsed().as_secs_f64() * 1000.0 / 10.0;
    eprintln!(
        "[probe] mermaid render: first={first_ms:.1}ms warm={warm_ms:.1}ms ok={}",
        first.is_diagram()
    );
}
