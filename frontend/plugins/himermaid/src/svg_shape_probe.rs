#[test]
#[ignore]
fn what_the_pipelines_emit() {
    let source = "flowchart TD\n    A[alpha] --> B[beta]\n";
    let plain = merman::svg::HeadlessRenderer::new()
        .render_svg_sync(source)
        .unwrap()
        .unwrap();
    eprintln!("[plain] head: {}", &plain[..plain.len().min(400)]);
    eprintln!(
        "[plain] foreignObject={} style_block={} text={}",
        plain.contains("foreignObject"),
        plain.contains("<style"),
        plain.contains("<text")
    );
    let safe = merman::svg::HeadlessRenderer::new()
        .with_svg_pipeline(merman::svg::SvgPipeline::resvg_safe())
        .render_svg_sync(source)
        .unwrap()
        .unwrap();
    eprintln!("[safe] head: {}", &safe[..safe.len().min(400)]);
    eprintln!(
        "[safe] foreignObject={} style_block={} text={}",
        safe.contains("foreignObject"),
        safe.contains("<style"),
        safe.contains("<text")
    );
    if let Some(at) = safe.find("<rect") {
        eprintln!("[safe] rect: {}", &safe[at..(at + 300).min(safe.len())]);
    }
    if let Some(at) = safe.find("<text") {
        eprintln!("[safe] text: {}", &safe[at..(at + 300).min(safe.len())]);
    }
    if let Some(at) = safe.find("<path") {
        eprintln!("[safe] path: {}", &safe[at..(at + 260).min(safe.len())]);
    }
}
