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
    // `#?` likewise: it is an autocomplete request, and it must win over the
    // sentence-punctuation test below, which would otherwise read the `?` as
    // the end of a question.
    let outside = crate::check::outside_code_spans(trimmed);
    if outside.contains("=>") || outside.contains("#?") {
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
                // An error anywhere in the result makes the whole answer an
                // error, and the message alone is far more use than the
                // half-built expression it was found in.
                let text = match first_error(&value) {
                    Some(message) => message,
                    None => render_with(&value, &env.fmt),
                };
                answers.push(Answer {
                    line,
                    text,
                    is_error: holds_error(&value),
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

/// The first error message in a result, if there is one.
fn first_error(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Error(message) => Some(message.clone()),
        Expr::Add(items) | Expr::Mul(items) => items.iter().find_map(first_error),
        Expr::Matrix(rows) => rows.iter().flatten().find_map(first_error),
        Expr::Pow(a, b) | Expr::Convert(a, b) | Expr::Relation(a, b) => {
            first_error(a).or_else(|| first_error(b))
        }
        Expr::Call(_, args) => args.iter().find_map(|a| first_error(&a.value)),
        Expr::Abs(a) | Expr::Not(a) | Expr::Transpose(a) => first_error(a),
        Expr::If(c, t, f) => first_error(c).or_else(|| first_error(t)).or_else(|| first_error(f)),
        _ => None,
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

/// How a source line reads, and where its comment starts if it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineInfo {
    pub kind: BlockKind,
    /// UTF-16 offset of a trailing `#` comment within the line.
    pub comment: Option<usize>,
    /// UTF-16 offset of a `#?` autocomplete request.
    pub query: Option<usize>,
    /// UTF-16 offset and length of a name this line *re*defines — one that
    /// the prelude already provides, or that the document defined earlier.
    /// The tesla incident is why an editor wants to mark these.
    pub redefines: Option<(usize, usize)>,
    /// Heading depth, for headings: the number of leading `#`, capped at 6.
    pub heading_level: Option<u8>,
}

/// How each source line reads, with its comment, one entry per line.
pub fn line_info(source: &str) -> Vec<LineInfo> {
    let kinds = line_kinds(source);
    let mut infos: Vec<LineInfo> = source
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let kind = kinds.get(i).copied().unwrap_or(BlockKind::Prose);
            LineInfo {
                kind,
                // Only a calculation can carry these; a `#` on a prose line is
                // Markdown, and at the start of one it is a heading.
                comment: match kind {
                    BlockKind::Code => crate::lexer::comment_start(line),
                    _ => None,
                },
                query: match kind {
                    BlockKind::Code => crate::lexer::query_start(line),
                    _ => None,
                },
                redefines: None,
                heading_level: match kind {
                    BlockKind::Heading => Some(
                        line.trim_start()
                            .chars()
                            .take_while(|c| *c == '#')
                            .count()
                            .min(6) as u8,
                    ),
                    _ => None,
                },
            }
        })
        .collect();

    // Second pass for redefinitions, which need document order: a name is a
    // redefinition if the prelude provides it or an earlier block defined it.
    let env = Env::with_prelude();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let lines: Vec<&str> = source.lines().collect();
    for block in split_blocks(source) {
        if block.kind != BlockKind::Code {
            continue;
        }
        for statement in parse_line(&joined(&block)) {
            let name = match &statement.stmt {
                Stmt::Define { name, .. } => name.clone(),
                Stmt::SumDefine { name } => name.clone(),
                _ => continue,
            };
            let already = seen.contains(&name) || env.prelude_defines(&name);
            if already {
                // The name sits on the block's first line, textually before
                // its `=`; the first occurrence is the definition site.
                if let Some(line) = lines.get(block.line) {
                    if let Some(byte) = line.find(name.as_str()) {
                        let offset = line[..byte].encode_utf16().count();
                        let length = name.encode_utf16().count();
                        if let Some(slot) = infos.get_mut(block.line) {
                            slot.redefines = Some((offset, length));
                        }
                    }
                }
            }
            seen.insert(name);
        }
    }
    infos
}

/// How each source line reads, one entry per line.
///
/// Exposed so an editor can colour prose differently from calculations without
/// inventing its own rule. The two must agree: a line the engine treats as a
/// calculation but the editor greys out as prose looks broken, and the
/// heuristic here is subtler than it first appears.
pub fn line_kinds(source: &str) -> Vec<BlockKind> {
    let mut kinds = vec![BlockKind::Prose; source.lines().count()];
    for block in split_blocks(source) {
        for offset in 0..block.lines.len() {
            if let Some(slot) = kinds.get_mut(block.line + offset) {
                *slot = block.kind;
            }
        }
    }
    kinds
}

/// Removes the answer after every `=>`, leaving the arrow in place.
///
/// The editor keeps answers out of the text buffer while you type — they are
/// shown alongside instead — and materializes them again on save. Stripping
/// here rather than in the UI keeps one implementation of "which `=>` is real",
/// including the rule that an arrow inside `inline code` is prose.
pub fn strip_answers(source: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in source.lines() {
        if !crate::check::outside_code_spans(line).contains("=>") {
            out.push(line.to_string());
            continue;
        }
        match line.find("=>") {
            Some(at) => {
                let head = &line[..at];
                out.push(format!("{head}=>").trim_end().to_string());
            }
            None => out.push(line.to_string()),
        }
    }
    let mut text = out.join("\n");
    if source.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// One coloured span within a source line, in UTF-16 units — what a text
/// view counts in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenSpan {
    pub offset: usize,
    pub length: usize,
    pub class: TokenClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenClass {
    Number,
    Str,
    Operator,
    Keyword,
    Function,
    /// The name being defined on this line, left of its `=`.
    Definition,
    Name,
    Directive,
}

/// Lexical token spans for every line, one entry per source line; empty for
/// prose and headings, which are not calculations and take no code colours.
///
/// From the same lexer that evaluation uses, so an editor colouring by these
/// can never disagree with what the engine computes. Spans stop at the `=>`:
/// what follows is the answer, which the editor styles as an answer.
pub fn tokens(source: &str) -> Vec<Vec<TokenSpan>> {
    let kinds = line_kinds(source);
    source
        .lines()
        .enumerate()
        .map(|(i, line)| match kinds.get(i) {
            Some(BlockKind::Code) => line_tokens(line),
            _ => Vec::new(),
        })
        .collect()
}

fn line_tokens(line: &str) -> Vec<TokenSpan> {
    use crate::lexer::{lex, Tok};
    let utf16 = |byte: usize| line[..byte].encode_utf16().count();

    // Where the defined name sits, so its Word tokens — possibly several,
    // names may contain spaces — read as the definition.
    let definition: Option<(usize, usize)> = parse_line(line).iter().find_map(|s| match &s.stmt {
        Stmt::Define { name, .. } | Stmt::SumDefine { name } => {
            line.find(name.as_str()).map(|at| (at, at + name.len()))
        }
        _ => None,
    });

    let toks = lex(line);
    let mut spans = Vec::new();
    let mut iter = toks.iter().peekable();
    while let Some(token) = iter.next() {
        let class = match &token.tok {
            Tok::Eof => break,
            // Not colours: the query is styled by its own report, and
            // invalid input is the error's to underline.
            Tok::HashQuestion | Tok::Invalid(_) => continue,
            Tok::Num(..) => TokenClass::Number,
            Tok::Str(_) => TokenClass::Str,
            Tok::Directive(_) => TokenClass::Directive,
            Tok::Word(word) => {
                if definition.is_some_and(|(from, to)| token.start >= from && token.end <= to) {
                    TokenClass::Definition
                } else if crate::parser::is_keyword(word) {
                    TokenClass::Keyword
                } else if matches!(iter.peek().map(|n| &n.tok), Some(Tok::LParen)) {
                    TokenClass::Function
                } else {
                    TokenClass::Name
                }
            }
            _ => TokenClass::Operator,
        };
        spans.push(TokenSpan {
            offset: utf16(token.start),
            length: line[token.start..token.end].encode_utf16().count(),
            class,
        });
        // The `=>` is the last thing coloured; the answer follows it.
        if token.tok == Tok::Arrow {
            break;
        }
    }
    spans
}

/// One completion candidate: a name in scope, with its current value if the
/// document gives it one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub name: String,
    /// The name's value as of `line`, rendered as an answer would be; empty
    /// for prelude names and names that do not evaluate.
    pub value: String,
    pub from_document: bool,
}

/// Names usable at `line` that match `prefix`: the document's own
/// definitions first, in definition order, then prelude names
/// alphabetically — every one carrying its current value.
///
/// A name matches if it — or any word of it, names may contain spaces —
/// starts with the prefix, case-insensitively. An empty prefix matches all.
///
/// Values come from one evaluation: the document up to `line` with a probe
/// `=>` appended per *matching* name — matching, so the per-keystroke cost
/// scales with the menu, not the prelude — and each answer is exactly what
/// the editor would print for that name at that point. A name that merely
/// echoes itself, as a base unit like `gallon` does, keeps an empty value:
/// "gallon   gallon" is noise.
pub fn completions(source: &str, line: usize, prefix: &str) -> Vec<Completion> {
    let upto = source
        .lines()
        .take(line)
        .collect::<Vec<_>>()
        .join("\n");

    let mut names: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in split_blocks(&upto) {
        if block.kind != BlockKind::Code {
            continue;
        }
        for statement in parse_line(&joined(&block)) {
            if let Stmt::Define { name, .. } | Stmt::SumDefine { name } = &statement.stmt {
                if seen.insert(name.clone()) {
                    names.push(name.clone());
                }
            }
        }
    }
    let doc_matches: Vec<String> = names
        .iter()
        .filter(|name| completion_match(name, prefix))
        .cloned()
        .collect();

    let env = Env::with_prelude();
    let mut prelude_matches: Vec<String> = env
        .prelude_names()
        .filter(|name| !seen.contains(*name) && completion_match(name, prefix))
        .cloned()
        .collect();
    prelude_matches.sort();

    let base = upto.lines().count();
    let mut probed = upto;
    for name in &doc_matches {
        probed.push_str("\n    ");
        probed.push_str(name);
        probed.push_str(" =>");
    }
    let document = evaluate(&probed);

    let mut out: Vec<Completion> = doc_matches
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let value = document
                .answers
                .iter()
                .find(|a| a.line == base + i && !a.is_error)
                .map(|a| a.text.clone())
                .filter(|value| value != name)
                .unwrap_or_default();
            Completion {
                name: name.clone(),
                value,
                from_document: true,
            }
        })
        .collect();

    // A prelude name cannot be probed — prelude definitions are opaque
    // until conversion, so `pi =>` answers `pi` — but its definition body
    // *is* its value: `pi` beside 3.1416, `gauss` beside T/10000. Functions
    // and self-referential base units show nothing.
    out.extend(prelude_matches.into_iter().map(|name| {
        let value = env
            .prelude_def(&name)
            .filter(|def| def.params.is_none())
            .map(|def| crate::format::render(&def.body))
            .filter(|body| body != &name && body.len() <= 40)
            .unwrap_or_default();
        Completion {
            name,
            value,
            from_document: false,
        }
    }));
    out
}

fn completion_match(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let matches = |word: &str| {
        word.len() >= prefix.len()
            && word
                .chars()
                .zip(prefix.chars())
                .all(|(a, b)| a.eq_ignore_ascii_case(&b))
    };
    matches(name) || name.split_whitespace().any(matches)
}

/// Evaluates a definition body eagerly, used by tests and tooling.
pub fn define_and_eval(env: &mut Env, name: &str, body: Expr) -> Expr {
    env.insert(
        name.to_string(),
        Def {
            params: None,
            body,
            is_unit: false,
            from_prelude: false,
        },
    );
    let mut ctx = Ctx::default();
    env.eval_in(&Expr::var(name), &mut ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_classify_a_definition_line() {
        let spans = &tokens("    fuel price = 3.45 $/gallon # per AAA")[0];
        let classes: Vec<TokenClass> = spans.iter().map(|s| s.class).collect();
        assert_eq!(
            classes,
            vec![
                TokenClass::Definition, // fuel
                TokenClass::Definition, // price
                TokenClass::Operator,   // =
                TokenClass::Number,     // 3.45
                TokenClass::Name,       // $
                TokenClass::Operator,   // /
                TokenClass::Name,       // gallon
            ],
            "got {spans:?}"
        );
    }

    #[test]
    fn tokens_stop_at_the_arrow_and_skip_prose() {
        let spans = tokens("A sentence.\n    sqrt(9) in cm => 3 cm");
        assert!(spans[0].is_empty(), "prose takes no code colours");
        let classes: Vec<TokenClass> = spans[1].iter().map(|s| s.class).collect();
        assert_eq!(
            classes,
            vec![
                TokenClass::Function, // sqrt
                TokenClass::Operator, // (
                TokenClass::Number,   // 9
                TokenClass::Operator, // )
                TokenClass::Keyword,  // in
                TokenClass::Name,     // cm
                TokenClass::Operator, // =>
            ],
            "got {:?}",
            spans[1]
        );
    }

    #[test]
    fn completions_carry_current_values() {
        let source = "    speed = 30 mph\n    distance = 60 miles\n";
        let all = completions(source, 2, "");
        let speed = all.iter().find(|c| c.name == "speed").expect("speed");
        assert!(speed.from_document);
        assert_eq!(speed.value, "30 mph");
        // Document names come before any prelude name.
        assert!(all[0].from_document && all[1].from_document);
        // Prelude names are offered too.
        assert!(all.iter().any(|c| c.name == "gallon" && !c.from_document));
    }

    #[test]
    fn prelude_completions_carry_their_definitions_as_values() {
        let pi = completions("", 0, "pi");
        let pi = pi.iter().find(|c| c.name == "pi").expect("pi");
        assert!(!pi.value.is_empty(), "pi should show its value");
        let gallon = completions("", 0, "gallon");
        let gallon = gallon.iter().find(|c| c.name == "gallon").expect("gallon");
        assert!(!gallon.value.is_empty(), "gallon should show its meaning");
    }

    #[test]
    fn completions_match_inner_words_and_respect_position() {
        let source = "    fuel price = 3.45\n\n    x = 1\n";
        // `pr` matches the second word of `fuel price`.
        let hits = completions(source, 3, "pr");
        assert!(hits.iter().any(|c| c.name == "fuel price"));
        // At line 0 nothing from the document is defined yet.
        assert!(completions(source, 0, "fu").iter().all(|c| !c.from_document));
    }

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
    fn strips_and_restores_answers() {
        let source = "    x = 2\n    x + 3 => 5\nProse mentioning `=>` stays put.";
        let stripped = strip_answers(source);
        assert_eq!(
            stripped,
            "    x = 2\n    x + 3 =>\nProse mentioning `=>` stays put."
        );
        // Round trip: stripping then rewriting reproduces the answers.
        assert_eq!(rewrite(&stripped), source);
    }

    #[test]
    fn stripping_is_idempotent_and_keeps_trailing_newline() {
        let source = "    1 + 1 => 2\n";
        let once = strip_answers(source);
        assert_eq!(once, "    1 + 1 =>\n");
        assert_eq!(strip_answers(&once), once);
    }

    #[test]
    fn an_autocomplete_request_is_code_despite_its_question_mark() {
        let info = line_info("speed of light = #?");
        assert_eq!(info[0].kind, BlockKind::Code);
        assert_eq!(info[0].query, Some(17));
    }

    #[test]
    fn redefinitions_are_marked_where_they_happen() {
        let source = [
            "    price = 3",     // fresh: unmarked
            "    price = 4",     // redefines the document's own name
            "    T = 125 degC",  // shadows the tesla
            "    fresh = T",     // uses, does not define: unmarked
        ]
        .join("\n");
        let info = line_info(&source);
        assert_eq!(info[0].redefines, None);
        assert_eq!(info[1].redefines, Some((4, 5)));
        assert_eq!(info[2].redefines, Some((4, 1)));
        assert_eq!(info[3].redefines, None);
    }

    #[test]
    fn comments_are_found_on_calculations_only() {
        let source = ["    I = 3/2 # nuclear spin", "# A heading", "Prose with # in it."]
            .join("\n");
        let info = line_info(&source);
        assert_eq!(info[0].comment, Some(12));
        assert_eq!(info[1].kind, BlockKind::Heading);
        assert_eq!(info[1].comment, None);
        assert_eq!(info[2].kind, BlockKind::Prose);
        assert_eq!(info[2].comment, None);
    }

    #[test]
    fn line_kinds_agree_with_how_lines_are_evaluated() {
        let source = [
            "# A heading",              // heading
            "",                         // blank
            "T = 125 degC",             // code: unindented, but assigns
            "ye = 2*pi*2.8024 MHz/gauss", // code
            "This is a sentence.",      // prose: ends with a full stop
            "    indented = 1",         // code by indentation
            "* a list item",            // prose
        ]
        .join("\n");
        assert_eq!(
            line_kinds(&source),
            vec![
                BlockKind::Heading,
                BlockKind::Prose,
                BlockKind::Code,
                BlockKind::Code,
                BlockKind::Prose,
                BlockKind::Code,
                BlockKind::Prose,
            ]
        );
    }

    #[test]
    fn a_stale_answer_cannot_break_its_own_line() {
        // Whatever is sitting after the `=>` is the previous answer, possibly
        // mid-edit. It must never be reported as the calculation's error.
        for source in ["    1+2=> 3'", "    1+2=> 3`x", "    1+2=> \"oops"] {
            let answers = evaluate(source).answers;
            assert_eq!(answers.len(), 1, "no answer for {source:?}");
            assert_eq!(answers[0].text, "3", "wrong answer for {source:?}");
            assert!(!answers[0].is_error, "flagged an error for {source:?}");
        }
    }

    #[test]
    fn a_broken_line_still_answers_when_an_arrow_was_asked_for() {
        // Neither of these can be computed — one fails to tokenize, the other
        // to parse — but both asked for an answer, and silence would leave the
        // author with no idea why nothing happened.
        for source in ["    3 + . =>", "    1 + 2 * =>", "    5 m in kg =>"] {
            let answers = evaluate(source).answers;
            assert_eq!(answers.len(), 1, "no answer for {source:?}");
            assert!(answers[0].is_error, "not flagged as an error: {source:?}");
            assert!(!answers[0].text.is_empty(), "empty message for {source:?}");
        }
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
