//! `calcium` — run and check calculating documents from the command line.
//!
//!   calcium run   <file.calcium>   rewrite the document with fresh answers
//!   calcium check <file.calcium>   compare fresh answers against the ones
//!                                already in the file, and report a pass rate
//!   calcium typst <file.calcium>   convert the document to Typst markup

use calcium_core::check::{check_source, Report};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("run") if args.len() >= 2 => run(&args[1..]),
        Some("check") if args.len() >= 2 => check(&args[1..]),
        Some("kinds") if args.len() >= 2 => kinds(&args[1..]),
        Some("typst") if args.len() >= 2 => typst(&args[1..]),
        _ => {
            eprintln!("usage: calcium <run|check|kinds|typst> <file.calcium>...");
            ExitCode::from(2)
        }
    }
}

fn run(paths: &[String]) -> ExitCode {
    for path in paths {
        match std::fs::read_to_string(path) {
            Ok(source) => println!("{}", calcium_core::doc::rewrite(&source)),
            Err(err) => {
                eprintln!("{path}: {err}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// Converts documents to Typst markup on stdout: answers recomputed fresh,
/// long names swapped for the symbols a `Symbols` section declares.
fn typst(paths: &[String]) -> ExitCode {
    for path in paths {
        match std::fs::read_to_string(path) {
            Ok(source) => println!("{}", calcium_core::typst::to_typst(&source)),
            Err(err) => {
                eprintln!("{path}: {err}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// Prints how each line is classified. A debugging aid: the editor colours
/// prose differently, and this is how to see what the engine actually thinks.
fn kinds(paths: &[String]) -> ExitCode {
    for path in paths {
        let Ok(source) = std::fs::read_to_string(path) else {
            eprintln!("{path}: cannot read");
            return ExitCode::FAILURE;
        };
        for (kind, line) in calcium_core::doc::line_kinds(&source)
            .iter()
            .zip(source.lines())
        {
            println!("{kind:?}\t{line}");
        }
    }
    ExitCode::SUCCESS
}

fn check(paths: &[String]) -> ExitCode {
    let verbose = std::env::var("CALCIUM_QUIET").is_err();
    let mut overall = Report::default();
    for path in paths {
        let Ok(source) = std::fs::read_to_string(path) else {
            eprintln!("{path}: cannot read");
            return ExitCode::FAILURE;
        };
        println!("\n{path}");
        let report = check_source(&source, verbose);
        println!(
            "  {:>4} exact  {:>4} same-value  {:>4} wrong  {:>4} no-answer   ({:.1}% of {})",
            report.exact,
            report.equivalent,
            report.wrong,
            report.missing,
            report.rate(),
            report.total()
        );
        overall.merge(&report);
    }
    println!(
        "\nTOTAL  {} exact + {} same-value = {}/{} passing ({:.1}%)",
        overall.exact,
        overall.equivalent,
        overall.passing(),
        overall.total(),
        overall.rate()
    );
    ExitCode::SUCCESS
}
