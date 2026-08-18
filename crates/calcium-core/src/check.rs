//! Checking a document against the answers it already contains.
//!
//! Calca's own `intro`, `Reference` and `Examples` documents are the best test
//! corpus available: every `=>` in them is a worked example with the correct
//! answer written on the right. This module extracts those and scores us
//! against them.

use crate::doc;
use crate::format::render;
use crate::parser::parse_expr;
use crate::simplify::simplify;
use std::collections::HashMap;

/// One `=>` in a source file, with the answer the file already carries.
#[derive(Clone, Debug)]
pub struct Expectation {
    pub line: usize,
    pub source: String,
    pub expected: String,
}

/// Pulls every `expr => answer` out of a document. Skips arrows with no answer
/// after them, and arrows inside `inline code`, which the prose uses when
/// talking *about* the operator rather than applying it.
pub fn expectations(source: &str) -> Vec<Expectation> {
    let kinds = doc::line_kinds(source);
    let mut out = Vec::new();
    for (index, line) in source.lines().enumerate() {
        // An arrow inside a fenced block is foreign text — a Typst lambda,
        // not a worked example.
        if kinds.get(index) == Some(&doc::BlockKind::Raw) {
            continue;
        }
        if !outside_code_spans(line).contains("=>") {
            continue;
        }
        // A line may hold several `;`-separated statements, each with its own
        // arrow. Only the first is attributed to this line, matching how the
        // document layer reports answers.
        let Some(at) = line.find("=>") else { continue };
        let (head, tail) = line.split_at(at);
        let rest = &tail[2..];
        let expected = rest[..statement_end(rest)].trim().to_string();
        if expected.is_empty() {
            continue;
        }
        out.push(Expectation {
            line: index,
            source: head.trim().to_string(),
            expected,
        });
    }
    out
}

/// Where the first statement's answer ends: at a top-level `;`, but not one
/// inside a matrix like `[1, 3; 2, 4]`.
fn statement_end(text: &str) -> usize {
    let mut depth = 0i32;
    for (offset, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ';' if depth <= 0 => return offset,
            _ => {}
        }
    }
    text.len()
}

/// Blanks out `` `inline code` `` spans.
pub fn outside_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut inside = false;
    for ch in line.chars() {
        if ch == '`' {
            inside = !inside;
            out.push(' ');
        } else if inside {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Verdict {
    /// Byte-for-byte identical to the answer already in the document.
    Exact,
    /// Same value, different spelling — almost always term ordering.
    Equivalent,
    Wrong,
}

/// Compares two answers, tolerating differences that are only formatting.
pub fn compare(actual: &str, expected: &str) -> Verdict {
    if actual == expected {
        return Verdict::Exact;
    }
    // Re-parse and canonicalize both, so a difference that is only spelling —
    // term order, spacing — does not read as a wrong answer.
    let normalize = |text: &str| render(&simplify(&parse_expr(text)));
    if normalize(actual) == normalize(expected) {
        return Verdict::Equivalent;
    }
    Verdict::Wrong
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Report {
    pub exact: usize,
    pub equivalent: usize,
    pub wrong: usize,
    /// A `=>` we produced no answer for at all — usually a parse failure or a
    /// line we misclassified as prose.
    pub missing: usize,
}

impl Report {
    pub fn total(&self) -> usize {
        self.exact + self.equivalent + self.wrong + self.missing
    }
    pub fn passing(&self) -> usize {
        self.exact + self.equivalent
    }
    pub fn rate(&self) -> f64 {
        if self.total() == 0 {
            return 0.0;
        }
        self.passing() as f64 * 100.0 / self.total() as f64
    }
    pub fn merge(&mut self, other: &Report) {
        self.exact += other.exact;
        self.equivalent += other.equivalent;
        self.wrong += other.wrong;
        self.missing += other.missing;
    }
}

/// Checks one document, optionally printing every mismatch.
pub fn check_source(source: &str, verbose: bool) -> Report {
    let document = doc::evaluate(source);
    let mut answers: HashMap<usize, String> = HashMap::new();
    for answer in &document.answers {
        answers.entry(answer.line).or_insert(answer.text.clone());
    }

    let mut report = Report::default();
    for expectation in expectations(source) {
        let line = expectation.line + 1;
        match answers.get(&expectation.line) {
            None => {
                report.missing += 1;
                if verbose {
                    println!(
                        "  {line:>4}  NO ANSWER  {}\n              expected  {}",
                        expectation.source, expectation.expected
                    );
                }
            }
            Some(actual) => match compare(actual, &expectation.expected) {
                Verdict::Exact => report.exact += 1,
                Verdict::Equivalent => {
                    report.equivalent += 1;
                    if verbose {
                        println!(
                            "  {line:>4}  ORDERING   {}\n              got       {actual}\n              expected  {}",
                            expectation.source, expectation.expected
                        );
                    }
                }
                Verdict::Wrong => {
                    report.wrong += 1;
                    if verbose {
                        println!(
                            "  {line:>4}  WRONG      {}\n              got       {actual}\n              expected  {}",
                            expectation.source, expectation.expected
                        );
                    }
                }
            },
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_arrows_are_not_expectations() {
        let source = "```typst\n#let f = (x) => x + 1\n```\n    2 + 2 => 4\n";
        let found = expectations(source);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].expected, "4");
    }

    #[test]
    fn extracts_expectations_but_not_prose_about_the_operator() {
        let source = "\
    2 + 2           => 4
Pay attention to the `=>` symbol, everything to its right is computed.
    1 + 2 * =>
    sin(pi/6)       => 0.5";
        let found = expectations(source);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].expected, "4");
        assert_eq!(found[1].expected, "0.5");
    }

    #[test]
    fn compare_tolerates_term_ordering_only() {
        assert_eq!(compare("4", "4"), Verdict::Exact);
        assert_eq!(compare("b + 10 m", "10 m + b"), Verdict::Equivalent);
        assert_eq!(compare("5", "4"), Verdict::Wrong);
    }
}
