//! Mutation-fuzzing harness: hunt for panics, aborts, and hangs in the
//! document pipeline.
//!
//! Valgrind has no Apple-Silicon macOS port, and the engine is safe Rust
//! anyway — the failure modes worth hunting are panics, stack overflows from
//! recursive descent, and non-termination. Each input therefore runs in a
//! child process: a panic exits 101, a stack overflow dies on a signal, and a
//! hang is killed at the deadline. All three are caught without poisoning the
//! driver.
//!
//!     cargo run --release --example crashhunt                # drive
//!     cargo run --release --example crashhunt -- --one FILE  # one input
//!
//! Runs are deterministic for a given `--seed`, so any failure can be
//! regenerated; failing inputs are also saved under `--out` as `repro_*.calcium`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use calcium::check::check_source;
use calcium::{doc, typst};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--one" {
        run_one(Path::new(&args[1]));
        return;
    }
    // Like --one, but on a 512 KiB stack — the macOS default for secondary
    // threads, which is where the app's FFI calls land.
    if args.len() == 2 && args[0] == "--one-small" {
        let path = PathBuf::from(&args[1]);
        std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || run_one(&path))
            .unwrap()
            .join()
            .unwrap();
        return;
    }
    drive(parse_flags(&args));
}

/// Worker mode: push one document through every public entry point.
/// `CRASHHUNT_ONLY` narrows to a single one when isolating a failure.
fn run_one(path: &Path) {
    let source = fs::read_to_string(path).expect("worker cannot read input");
    let only = std::env::var("CRASHHUNT_ONLY").unwrap_or_default();
    let want = |name: &str| only.is_empty() || only == name;
    if want("evaluate") {
        let _ = doc::evaluate(&source);
    }
    if want("rewrite") {
        let _ = doc::rewrite(&source);
    }
    if want("tokens") {
        let _ = doc::tokens(&source);
    }
    if want("line_info") {
        let _ = doc::line_info(&source);
    }
    if want("line_kinds") {
        let _ = doc::line_kinds(&source);
    }
    if want("strip_answers") {
        let _ = doc::strip_answers(&source);
    }
    if want("completions") {
        let _ = doc::completions(&source, 0, "s");
    }
    if want("typst") {
        let _ = typst::to_typst(&source);
    }
    if want("check") {
        let _ = check_source(&source, false);
    }
}

struct Flags {
    runs: u64,
    seed: u64,
    timeout: Duration,
    out: PathBuf,
}

fn parse_flags(args: &[String]) -> Flags {
    let mut flags = Flags {
        runs: 4000,
        seed: 0x5eed_ca1c,
        timeout: Duration::from_secs(3),
        out: PathBuf::from("target/crashhunt"),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().expect("flag needs a value").clone();
        match arg.as_str() {
            "--runs" => flags.runs = value().parse().unwrap(),
            "--seed" => flags.seed = value().parse().unwrap(),
            "--timeout-ms" => flags.timeout = Duration::from_millis(value().parse().unwrap()),
            "--out" => flags.out = PathBuf::from(value()),
            other => panic!("unknown flag {other}"),
        }
    }
    flags
}

fn drive(flags: Flags) {
    let seeds = load_seeds();
    fs::create_dir_all(&flags.out).unwrap();
    let exe = std::env::current_exe().unwrap();

    let mut rng = Rng(flags.seed | 1);
    let mut failures: BTreeMap<String, (u64, PathBuf)> = BTreeMap::new();
    let mut counts = [0u64; 3]; // panics, signals, hangs
    let started = Instant::now();

    let handcrafted = adversarial_inputs();
    let total = handcrafted.len() as u64 + flags.runs;

    for i in 0..total {
        let input = match handcrafted.get(i as usize) {
            Some(fixed) => fixed.clone(),
            None => mutate(&seeds, &mut rng),
        };
        let input_path = flags.out.join("current.calcium");
        fs::write(&input_path, &input).unwrap();

        if let Some(outcome) = execute(&exe, &input_path, flags.timeout, &flags.out) {
            let (kind, signature) = outcome;
            counts[kind as usize] += 1;
            // Hangs share one signature but rarely one cause; keep every input.
            if matches!(kind, Kind::Hang) {
                fs::write(flags.out.join(format!("hang_{i}.calcium")), &input).unwrap();
            }
            let next_repro = flags.out.join(format!("repro_{}.calcium", failures.len()));
            let entry = failures.entry(signature.clone()).or_insert_with(|| {
                fs::write(&next_repro, &input).unwrap();
                (0, next_repro)
            });
            entry.0 += 1;
            if entry.0 == 1 {
                println!("[{i}] {} — {}", kind.label(), signature);
            }
        }

        if i % 500 == 499 {
            println!(
                "... {i} of {total} in {:.0?} ({} panics, {} signals, {} hangs)",
                started.elapsed(),
                counts[0],
                counts[1],
                counts[2]
            );
        }
    }

    println!(
        "\ndone: {total} inputs in {:.0?} — {} panics, {} signals, {} hangs, {} distinct",
        started.elapsed(),
        counts[0],
        counts[1],
        counts[2],
        failures.len()
    );
    for (signature, (count, repro)) in &failures {
        println!("  {count:5}×  {signature}\n         repro: {}", repro.display());
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Panic,
    Signal,
    Hang,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Panic => "PANIC",
            Kind::Signal => "SIGNAL",
            Kind::Hang => "HANG",
        }
    }
}

/// Run the worker on one input. `None` means it finished cleanly.
fn execute(exe: &Path, input: &Path, timeout: Duration, out: &Path) -> Option<(Kind, String)> {
    let stderr_path = out.join("stderr.txt");
    let stderr_file = fs::File::create(&stderr_path).unwrap();
    let mut child = Command::new(exe)
        .arg("--one")
        .arg(input)
        .stdout(Stdio::null())
        .stderr(stderr_file)
        .spawn()
        .unwrap();

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let mut stderr = String::new();
    let _ = fs::File::open(&stderr_path).and_then(|mut f| f.read_to_string(&mut stderr));

    match status {
        None => Some((Kind::Hang, format!("hang: exceeded {timeout:?}"))),
        Some(status) if status.success() => None,
        Some(status) => {
            // Thread ids vary run to run; strip them so identical failures
            // collapse into one signature. The panic payload is on the line
            // after the "panicked at" header, so keep both.
            let lines: Vec<&str> = stderr.lines().collect();
            let signature = lines
                .iter()
                .position(|line| line.contains("panicked") || line.contains("overflowed"))
                .map(|at| {
                    let header = lines[at].split(") ").last().unwrap();
                    match lines.get(at + 1) {
                        Some(message) => format!("{header} {message}"),
                        None => header.to_string(),
                    }
                })
                .unwrap_or_else(|| "(no panic message)".into());
            let kind = if status.code() == Some(101) { Kind::Panic } else { Kind::Signal };
            Some((kind, format!("{signature} [{status}]")))
        }
    }
}

fn load_seeds() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut seeds = Vec::new();
    for name in [
        "corpus/tour.calcium",
        "corpus/reference.calcium",
        "corpus/worked.calcium",
        "corpus/uncertainty.calcium",
        "examples/showcase.calcium",
    ] {
        seeds.push(fs::read_to_string(root.join(name)).expect("seed missing"));
    }
    seeds
}

/// Inputs aimed at the classic failure shapes: deep recursion, bignum
/// blowups, and degenerate numerics. Each is cheap to try and would be found
/// by mutation only slowly.
fn adversarial_inputs() -> Vec<String> {
    let mut inputs = Vec::new();
    for depth in [64usize, 256, 1024, 4096, 16384, 65536] {
        inputs.push(format!("{}1{}", "(".repeat(depth), ")".repeat(depth)));
        inputs.push(format!("{}1", "-".repeat(depth)));
        inputs.push(format!("{}1{}", "sqrt(".repeat(depth / 4), ")".repeat(depth / 4)));
        inputs.push("(".repeat(depth)); // unbalanced
    }
    inputs.push("x = ".to_string() + &"1 + ".repeat(20000) + "1");
    inputs.push("2^1000000".into());
    inputs.push("9^9^9".into());
    inputs.push("1e99999999".into());
    inputs.push("1e-99999999".into());
    inputs.push("100000!".into());
    inputs.push("1/0".into());
    inputs.push("0/0".into());
    inputs.push("0^0".into());
    inputs.push("0^-1".into());
    inputs.push("sqrt(-1)".into());
    inputs.push("(-8)^(1/3)".into());
    inputs.push("@sigfigs 999999999\n1/3".into());
    inputs.push("@sigfigs 0\n1/3".into());
    inputs.push("@sigfigs -3\n1/3".into());
    inputs.push("1 ± 1 ± 1 ± 1".into());
    inputs.push("0 ± 0".into());
    inputs.push("1 ± -1".into());
    inputs.push("1 ± 1e999999".into());
    inputs.push("x = x + 1\nx".into());
    inputs.push("f(x) = f(x)\nf(1)".into());
    inputs.push("a = b\nb = a\na".into());
    inputs.push("solve x^2 = -1".into());
    inputs.push("solve 0 = 0".into());
    inputs.push("solve x = x".into());
    inputs
}

/// Splice, corrupt, and cross-breed the seed documents. Mutations act on
/// bytes; `from_utf8_lossy` repairs the result into the valid UTF-8 the
/// engine's API requires.
fn mutate(seeds: &[String], rng: &mut Rng) -> String {
    const DICT: &[&str] = &[
        "±", "=>", "=", "(", ")", "^", "/", "*", "-", "+", "!", "%", "$", "`", "#", "_",
        "solve", "sqrt", "pi", "i", "e", "@sigfigs", "1e999", "0", "9999999999999999999",
        "ft", "m", "s", "kg", "²", "µ", "→", "\n", "\t", " ", "\"", "'", "[", "]", "{", "}",
    ];
    let seed = &seeds[rng.below(seeds.len() as u64) as usize];
    let mut bytes = seed.as_bytes().to_vec();

    for _ in 0..=rng.below(8) {
        if bytes.is_empty() {
            break;
        }
        let at = rng.below(bytes.len() as u64) as usize;
        match rng.below(6) {
            0 => bytes[at] = rng.below(256) as u8,
            1 => {
                let token = DICT[rng.below(DICT.len() as u64) as usize];
                bytes.splice(at..at, token.bytes());
            }
            2 => {
                let end = (at + 1 + rng.below(64) as usize).min(bytes.len());
                bytes.drain(at..end);
            }
            3 => {
                let end = (at + 1 + rng.below(64) as usize).min(bytes.len());
                let span: Vec<u8> = bytes[at..end].to_vec();
                bytes.splice(at..at, span);
            }
            4 => {
                let other = seeds[rng.below(seeds.len() as u64) as usize].as_bytes();
                let from = rng.below(other.len() as u64) as usize;
                let end = (from + 1 + rng.below(128) as usize).min(other.len());
                bytes.splice(at..at, other[from..end].iter().copied());
            }
            _ => bytes.truncate(at),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// xorshift64*: tiny, deterministic, no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }
}
