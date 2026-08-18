// Temporary regression harness: prints every answer and every sampled plot
// point so cache changes can be diffed against a baseline.
fn main() {
    let path = std::env::args().nth(1).expect("usage: dumpdoc <file>");
    let source = std::fs::read_to_string(&path).unwrap();
    let document = calcium_core::doc::evaluate(&source);
    for answer in &document.answers {
        println!("{}|{}|{}", answer.line, answer.is_error, answer.text);
    }
    for plot in &document.plots {
        println!("plot@{} x={:?} xu={:?} yu={:?}", plot.line, plot.x_label, plot.x_unit, plot.y_unit);
        for series in &plot.series {
            println!("  series {} swept={} n={}", series.label, series.swept, series.points.len());
            for (x, y) in &series.points {
                println!("    {x:.12e} {y:.12e}");
            }
        }
    }
}
