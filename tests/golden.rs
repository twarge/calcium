//! Regression test against the documents in `corpus/`.
//!
//! Every `=>` in those files is an expectation. They are written by hand, not
//! generated from this engine: numeric answers were derived independently
//! (exact `Fraction` arithmetic in Python, rounded the same way), and the
//! algebraic ones by working the algebra out. That matters — a corpus blessed
//! from the engine's own output would pass by construction and prove nothing.
//!
//! Run `cargo run -p calcium-cli -- check corpus/*.calcium` to see any mismatch.

use calcium_core::check::check_source;

/// The corpus is expected to pass completely. A drop here is a regression, not
/// a divergence to be documented.
const FLOORS: &[(&str, f64)] = &[
    ("corpus/tour.calcium", 100.0),
    ("corpus/reference.calcium", 100.0),
    ("corpus/worked.calcium", 100.0),
    ("corpus/uncertainty.calcium", 100.0),
];

fn corpus_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

#[test]
fn corpus_documents_all_pass() {
    let mut failures = Vec::new();
    let mut combined = calcium_core::check::Report::default();

    for (name, floor) in FLOORS {
        let source = std::fs::read_to_string(corpus_path(name))
            .unwrap_or_else(|err| panic!("cannot read {name}: {err}"));
        let report = check_source(&source, false);
        combined.merge(&report);
        println!(
            "{name}: {}/{} ({:.1}%)  [exact {}, same-value {}, wrong {}, no-answer {}]",
            report.passing(),
            report.total(),
            report.rate(),
            report.exact,
            report.equivalent,
            report.wrong,
            report.missing
        );
        if report.rate() < *floor {
            failures.push(format!(
                "{name}: {:.1}% is below the {floor:.1}% floor",
                report.rate()
            ));
        }
        assert_eq!(
            report.equivalent, 0,
            "{name} has answers that differ from the corpus in spelling; \
             the corpus records this engine's own conventions, so update one or the other"
        );
    }

    println!(
        "TOTAL: {}/{} ({:.1}%)",
        combined.passing(),
        combined.total(),
        combined.rate()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every `=>` must produce *some* answer. A missing one means a line failed to
/// parse or was misread as prose, which is a harder failure than a wrong value.
#[test]
fn every_arrow_gets_an_answer() {
    for (name, _) in FLOORS {
        let source = std::fs::read_to_string(corpus_path(name)).unwrap();
        let report = check_source(&source, false);
        assert_eq!(
            report.missing, 0,
            "{name} left {} arrows without an answer",
            report.missing
        );
    }
}

/// Answers must survive a round trip: whatever we write into the document has
/// to parse back to the same value, because the user edits that text.
#[test]
fn answers_reparse_to_themselves() {
    use calcium_core::{format::render, parser::parse_expr, simplify::simplify};

    for (name, _) in FLOORS {
        let source = std::fs::read_to_string(corpus_path(name)).unwrap();
        for answer in calcium_core::doc::evaluate(&source).answers {
            if answer.is_error || answer.text.is_empty() {
                continue;
            }
            // Culture-formatted output (`3 141,59` under `@fr-FR`) is not
            // re-readable: the lexer only accepts invariant-culture numbers.
            // See "Known divergences" in the README.
            if answer.text.contains('\u{202f}') || answer.text.contains('\u{a0}') {
                continue;
            }
            let reparsed = render(&simplify(&parse_expr(&answer.text)));
            let again = render(&simplify(&parse_expr(&reparsed)));
            assert_eq!(
                reparsed, again,
                "{name} line {}: answer {:?} is not stable under re-parsing",
                answer.line + 1,
                answer.text
            );
        }
    }
}
