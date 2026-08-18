use std::time::Instant;
fn main() {
    let path = std::env::args().nth(1).expect("usage: benchdoc <file>");
    let source = std::fs::read_to_string(&path).unwrap();
    for _ in 0..2 { calcium_core::doc::evaluate(&source); }
    let start = Instant::now();
    let runs = 10;
    for _ in 0..runs { calcium_core::doc::evaluate(&source); }
    println!("{:.2} ms per full re-evaluation of {} lines",
        start.elapsed().as_secs_f64() * 1000.0 / runs as f64,
        source.lines().count());
}
