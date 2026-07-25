//! The document layer: turning a `.calcium` file into blocks, evaluating them
//! top to bottom, and writing answers back after each `=>`.
//!
//! A document is Markdown in which some lines are calculations. The rule: an
//! indented line is code; an unindented line is guessed at, and unindented text
//! ending in sentence punctuation is prose.

use crate::ast::*;
use crate::eval::{Ctx, Def, Env};
use crate::format::render_with;
use crate::num::NumFormat;
use crate::parser::parse_line;
use crate::solve;

/// One logical unit of the document: a run of source lines that parse
/// together, because continuation lines are indented further than the first.
#[derive(Clone, Debug)]
pub struct Block {
    /// Index of the first source line, 0-based.
    pub line: usize,
    /// The source lines making up this block.
    pub lines: Vec<String>,
    pub kind: BlockKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Prose,
    Heading,
    Code,
}

/// An answer to be shown after a `=>`.
#[derive(Clone, Debug)]
pub struct Answer {
    /// The source line the `=>` sits on.
    pub line: usize,
    pub text: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub answers: Vec<Answer>,
}

/// Splits source into blocks. A line indented further than the block's first
/// line continues that block.
pub fn split_blocks(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        let indent = indent_of(line);
        let kind = classify(line);

        let mut collected = vec![line.to_string()];
        let start = index;
        index += 1;

        // A `name +=` line never takes continuations: the definitions below
        // it are separate blocks that it accumulates, not part of its own
        // expression.
        let is_sum_header = line.trim_end().ends_with("+=");
        // Only code blocks take continuations; prose lines stand alone.
        if kind == BlockKind::Code && !is_sum_header {
            while index < lines.len() {
                let next = lines[index];
                if next.trim().is_empty() || indent_of(next) <= indent {
                    break;
                }
                // A more-indented line that is itself a definition starts a new
                // block; this is what makes the `+=` summing form work.
                collected.push(next.to_string());
                index += 1;
            }
        }

        blocks.push(Block {
            line: start,
            lines: collected,
            kind,
        });
    }
    blocks
}

fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Decides whether a line is prose or a calculation.
fn classify(line: &str) -> BlockKind {
    let trimmed = line.trim();
    if trimmed.starts_with('#') && !trimmed.starts_with("#?") {
        return BlockKind::Heading;
    }
    // Indented lines are code by fiat.
    if indent_of(line) > 0 {
        return BlockKind::Code;
    }
    // Markdown list items and link definitions are prose.
    if trimmed.starts_with("* ")
        || trimmed.starts_with("- ")
        || trimmed.starts_with("> ")
        || (trimmed.starts_with('[') && trimmed.contains("]:"))
    {
        return BlockKind::Prose;
    }
    // An explicit `=>` always means the author wants an answer — unless it is
    // inside `inline code`, where the prose is talking *about* the operator.
    if crate::check::outside_code_spans(trimmed).contains("=>") {
        return BlockKind::Code;
    }
    // Otherwise: prose if it ends with sentence punctuation.
    if trimmed.ends_with('.')
        || trimmed.ends_with('!')
        || trimmed.ends_with('?')
        || trimmed.ends_with(':')
        || trimmed.ends_with(',')
    {
        return BlockKind::Prose;
    }
    // A bare line with no operators at all reads as prose.
    if !trimmed.contains(['=', '+', '-', '*', '/', '^', '(']) {
        return BlockKind::Prose;
    }
    BlockKind::Code
}

/// Joins a block's lines into one logical line for the parser.
fn joined(block: &Block) -> String {
    block
        .lines
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Evaluates a whole document, producing an answer for every `=>`.
pub fn evaluate(source: &str) -> Document {
    let mut env = Env::with_prelude();
    evaluate_in(source, &mut env)
}

pub fn evaluate_in(source: &str, env: &mut Env) -> Document {
    let blocks = split_blocks(source);
    let mut answers = Vec::new();

    // `name +=` sums the definitions that follow it, until something that is
    // not an indented definition comes along.
    let mut pending_sum: Option<PendingSum> = None;

    for block in &blocks {
        if block.kind != BlockKind::Code {
            // A heading or paragraph closes any open summing definition.
            if let Some(sum) = pending_sum.take() {
                sum.close(env);
            }
            continue;
        }

        // Dedenting out of the summing block closes it too.
        if let Some(sum) = &pending_sum {
            if indent_of(&block.lines[0]) <= sum.indent {
                pending_sum.take().unwrap().close(env);
            }
        }

        let text = joined(block);
        let statements = parse_line(&text);
        // Track which `=>` on this block each statement belongs to.
        let arrow_lines = arrow_lines(block);
        let mut arrow_index = 0;

        for statement in statements {
            let mut result: Option<Expr> = None;

            match &statement.stmt {
                Stmt::Define { name, params, body } => {
                    // Feed a running `+=` accumulator.
                    if let Some(sum) = pending_sum.as_mut() {
                        sum.parts.push(Expr::var(name));
                    }
                    env.define(name, params.clone(), body.clone());
                    if statement.arrow {
                        result = Some(env.eval(&Expr::var(name)));
                    }
                }
                Stmt::SumDefine { name } => {
                    if let Some(sum) = pending_sum.take() {
                        sum.close(env);
                    }
                    pending_sum = Some(PendingSum {
                        name: name.clone(),
                        parts: Vec::new(),
                        indent: indent_of(&block.lines[0]),
                    });
                }
                Stmt::Equation { lhs, rhs } => {
                    env.equations.push((lhs.clone(), rhs.clone()));
                    if statement.arrow {
                        result = Some(env.eval(&Expr::Relation(
                            Box::new(lhs.clone()),
                            Box::new(rhs.clone()),
                        )));
                    }
                }
                Stmt::Directive { name, value } => {
                    apply_directive(env, name, value.as_ref());
                }
                Stmt::Expr(expr) => {
                    if let Some(sum) = pending_sum.take() {
                        sum.close(env);
                    }
                    if statement.arrow {
                        result = Some(solve::evaluate_or_solve(env, expr));
                    }
                }
            }

            if statement.arrow {
                let line = arrow_lines
                    .get(arrow_index)
                    .copied()
                    .unwrap_or(block.line + block.lines.len() - 1);
                arrow_index += 1;
                let value = result.unwrap_or_else(|| Expr::Error("no result".to_string()));
                let is_error = holds_error(&value);
                answers.push(Answer {
                    line,
                    text: render_with(&value, &env.fmt),
                    is_error,
                });
            }
        }
    }

    if let Some(sum) = pending_sum.take() {
        sum.close(env);
    }

    Document { blocks, answers }
}

/// An open `name +=` accumulator.
struct PendingSum {
    name: String,
    parts: Vec<Expr>,
    indent: usize,
}

impl PendingSum {
    fn close(self, env: &mut Env) {
        env.define(&self.name, None, Expr::add(self.parts));
    }
}

/// Whether an error is anywhere in the result, not just at the top. A nested
/// error means the printed answer is not valid source text.
fn holds_error(expr: &Expr) -> bool {
    match expr {
        Expr::Error(_) => true,
        Expr::Add(items) | Expr::Mul(items) => items.iter().any(holds_error),
        Expr::Matrix(rows) => rows.iter().flatten().any(holds_error),
        Expr::Pow(a, b) | Expr::Convert(a, b) | Expr::Relation(a, b) => {
            holds_error(a) || holds_error(b)
        }
        Expr::Call(_, args) => args.iter().any(|a| holds_error(&a.value)),
        Expr::Abs(a) | Expr::Not(a) | Expr::Transpose(a) => holds_error(a),
        Expr::If(c, t, f) => holds_error(c) || holds_error(t) || holds_error(f),
        _ => false,
    }
}

/// Source line numbers of each `=>` inside a block.
fn arrow_lines(block: &Block) -> Vec<usize> {
    let mut out = Vec::new();
    for (offset, line) in block.lines.iter().enumerate() {
        for _ in line.matches("=>") {
            out.push(block.line + offset);
        }
    }
    out
}

fn apply_directive(env: &mut Env, name: &str, value: Option<&Expr>) {
    let key = name.trim_start_matches('@');
    match key {
        "precision" | "p" | "prec" => {
            if let Some(n) = value
                .map(|v| env.eval(v))
                .and_then(|v| v.as_num().and_then(|n| n.to_i64()))
            {
                // Guard against `@p = 1000` asking for a thousand digits.
                env.fmt.precision = n.clamp(0, 40) as usize;
            }
        }
        "group" | "g" | "grouping" => {
            if let Some(v) = value.map(|v| env.eval(v)) {
                env.fmt.grouping = !matches!(v, Expr::Bool(false))
                    && !matches!(&v, Expr::Num(n, _) if n.is_zero());
            }
        }
        culture => apply_culture(&mut env.fmt, culture),
    }
}

/// `@fr-FR` and friends. Only the separators matter to the engine. Switching
/// culture also restores grouping, so `3 141,59` still groups after an earlier
/// `@group = false`.
fn apply_culture(fmt: &mut NumFormat, culture: &str) {
    fmt.grouping = true;
    let comma_decimal = matches!(
        culture.split('-').next().unwrap_or(""),
        "fr" | "de" | "es" | "it" | "pt" | "nl" | "ru" | "pl" | "tr" | "sv" | "da" | "fi" | "cs"
            | "id" | "vi" | "ro" | "uk" | "el" | "hu" | "nb" | "ca"
    );
    if comma_decimal {
        fmt.decimal_sep = ',';
        fmt.group_sep = if culture.starts_with("fr") {
            '\u{202f}' // narrow no-break space, as French convention wants
        } else {
            '.'
        };
    } else {
        fmt.decimal_sep = '.';
        fmt.group_sep = ',';
    }
}

/// Rewrites a document so every `=>` is followed by its freshly computed
/// answer. This is what a UI layer applies to the text buffer.
pub fn rewrite(source: &str) -> String {
    let document = evaluate(source);
    let mut lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();
    for answer in &document.answers {
        if let Some(line) = lines.get_mut(answer.line) {
            if let Some(at) = line.find("=>") {
                let (head, _) = line.split_at(at);
                *line = format!("{head}=> {}", answer.text);
            }
        }
    }
    lines.join("\n")
}

/// Evaluates a definition body eagerly, used by tests and tooling.
pub fn define_and_eval(env: &mut Env, name: &str, body: Expr) -> Expr {
    env.insert(
        name.to_string(),
        Def {
            params: None,
            body,
            is_unit: false,
        },
    );
    let mut ctx = Ctx::default();
    env.eval_in(&Expr::var(name), &mut ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_prose_and_code() {
        assert_eq!(classify("Welcome to Calca - the text editor."), BlockKind::Prose);
        assert_eq!(classify("# Introduction"), BlockKind::Heading);
        assert_eq!(classify("    2 + 2           => 4"), BlockKind::Code);
        assert_eq!(classify("1 + 2 * 3   => 7"), BlockKind::Code);
        assert_eq!(classify("* [Finance][]"), BlockKind::Prose);
        assert_eq!(classify("[markdown]: http://example.com"), BlockKind::Prose);
    }

    #[test]
    fn joins_indented_continuations_into_one_block() {
        let source = "    job total = callout +\n      (parts + labour) * hours\n\n    hours = 6";
        let blocks = split_blocks(source);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].lines.len(), 2);
        assert_eq!(blocks[1].lines.len(), 1);
    }

    #[test]
    fn evaluates_a_document_and_writes_answers_back() {
        let source = "    x = 2\n    x + 3 => stale";
        let output = rewrite(source);
        assert!(output.ends_with("=> 5"), "got {output:?}");
    }

    #[test]
    fn summing_definitions_accumulate_the_block_below() {
        let source = "\
    expenses +=

      rent  = 750
      utils = 200

    expenses => 0";
        let document = evaluate(source);
        assert_eq!(document.answers.last().unwrap().text, "950");
    }

    #[test]
    fn precision_directive_changes_output() {
        let source = "    @precision = 8\n    pi => 0";
        let document = evaluate(source);
        assert_eq!(document.answers[0].text, "3.14159265");
    }

    #[test]
    fn grouping_directive_changes_output() {
        let source = "    @group = false\n    1234567890 => 0";
        let document = evaluate(source);
        assert_eq!(document.answers[0].text, "1234567890");
    }
}
