use std::time::Instant;
fn main() {
    let source = std::fs::read_to_string("corpus/reference.calcium").unwrap();
    // Warm up, then time the steady-state path a UI would hit per keystroke.
    for _ in 0..3 { calcium_core::doc::evaluate(&source); }
    let start = Instant::now();
    let runs = 50;
    for _ in 0..runs { calcium_core::doc::evaluate(&source); }
    println!("{:.2} ms per full re-evaluation of {} lines",
        start.elapsed().as_secs_f64() * 1000.0 / runs as f64,
        source.lines().count());
    let start = Instant::now();
    for _ in 0..50 { calcium_core::eval::Env::with_prelude(); }
    println!("{:.2} ms of that is parsing the prelude", start.elapsed().as_secs_f64() * 1000.0 / 50.0);
}
