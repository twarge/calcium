//! Rendering a document to Typst markup.
//!
//! Prose passes through with Typst's specials escaped, headings map to
//! Typst headings, and every calculation becomes a display equation with
//! its computed answer folded in: `x = body = answer`.
//!
//! Long names become symbols through a *symbols section*: a section headed
//! `Symbols` (or `Notation`), conventionally at the bottom of the document,
//! whose calculation lines pair a name with a Typst math expression in a
//! trailing comment:
//!
//! ```text
//! ## Symbols
//!
//!     operating current  # I
//!     threshold current  # I_"th"
//!     tank resistance    # R_2
//! ```
//!
//! Each line is a bare expression with a comment, so the engine evaluates
//! nothing and `calcium check` is oblivious to the section; the mapping is
//! read back through the engine's own lexer and parser rather than a second
//! grammar. The section applies to the whole document regardless of where it
//! sits, and typesets as a notation table.
//!
//! Names with no symbol fall back gracefully: units typeset through the
//! `fancy-units` package (`#unit[mA]`), single letters and Greek names stay
//! italic (`h`, `eta`), and anything else becomes quoted upright text, which
//! is the nudge to add it to the symbols section.
//!
//! A quantity — a number against nothing but units — becomes one
//! `#qty[...][...]` call, exponents and all: `31.6144 pW/sqrt(Hz)` typesets
//! as `#qty[31.6144][pW / Hz^0.5]`. Fractional exponents print as decimals
//! because fancy-units has no radical form and rejects `^(1/2)`. A number
//! fancy-units cannot read — group separators, a radix literal — stays in
//! math beside a plain `#unit[...]` call.
//!
//! Fenced code blocks are the document speaking other languages. A fence
//! tagged `typst` splices its body into the output verbatim — a diagram, a
//! table, anything Typst can say. Any other fence passes through whole,
//! which in Typst markup reads as the code listing it is. A document with a
//! `typst` fence also gets a preamble dictionary named `calcium`, mapping
//! every defined name to its computed value as typeset math, so raw markup
//! can quote results that stay live under recalculation:
//! `#calcium.at("trap capacitor")`.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::*;
use crate::doc::{self, Block, BlockKind};
use crate::eval::Env;
use crate::lexer::Radix;
use crate::num::{Num, NumFormat};
use crate::parser::{parse_expr, parse_line};

/// One converted block, remembering where it came from so hard-wrapped
/// prose can reassemble into paragraphs and adjacent equations can align.
enum Piece {
    Prose { line: usize, text: String },
    Equation { line: usize, row: EquationRow },
    Raw(String),
}

/// Converts a whole document to Typst markup.
pub fn to_typst(source: &str) -> String {
    let mut env = Env::with_prelude();
    let document = doc::evaluate_in(source, &mut env);

    // Answers by source line, in order, for folding into equations.
    let mut answers: HashMap<usize, VecDeque<&doc::Answer>> = HashMap::new();
    for answer in &document.answers {
        answers.entry(answer.line).or_default().push_back(answer);
    }

    // Sampled plots by source line, for typesetting as lilaq diagrams.
    let has_plots = !document.plots.is_empty();
    let mut plots: HashMap<usize, VecDeque<&crate::plot::Plot>> = HashMap::new();
    for plot in &document.plots {
        plots.entry(plot.line).or_default().push_back(plot);
    }

    let renderer = Renderer::new(&document.blocks);

    let mut pieces: Vec<Piece> = Vec::new();
    // Rows collected for the notation table of the current symbols section.
    let mut notation: Vec<(String, String)> = Vec::new();
    let mut in_symbols = false;

    let flush_notation = |pieces: &mut Vec<Piece>, notation: &mut Vec<(String, String)>| {
        if notation.is_empty() {
            return;
        }
        let mut table =
            String::from("#table(\n  columns: 2,\n  stroke: none,\n  column-gutter: 1.5em,\n");
        for (symbol, name) in notation.iter() {
            table.push_str(&format!("  [${symbol}$], [{}],\n", escape_prose(name)));
        }
        table.push(')');
        pieces.push(Piece::Raw(table));
        notation.clear();
    };

    for block in &document.blocks {
        match block.kind {
            BlockKind::Heading => {
                flush_notation(&mut pieces, &mut notation);
                let trimmed = block.lines[0].trim();
                let level = trimmed.chars().take_while(|c| *c == '#').count().min(6);
                let title = trimmed.trim_start_matches('#').trim();
                in_symbols = is_symbols_heading(title);
                pieces.push(Piece::Raw(format!(
                    "{} {}",
                    "=".repeat(level),
                    escape_prose(title)
                )));
            }
            BlockKind::Prose => pieces.push(Piece::Prose {
                line: block.line,
                text: escape_prose(block.lines[0].trim()),
            }),
            BlockKind::Code if in_symbols => {
                for line in &block.lines {
                    if let Some((name, symbol)) = symbol_mapping(line) {
                        notation.push((symbol, name));
                    }
                }
            }
            BlockKind::Code => {
                let joined = block
                    .lines
                    .iter()
                    .map(|l| l.trim())
                    .collect::<Vec<_>>()
                    .join(" ");
                let statements = parse_line(&joined);
                // The classifier reads an unindented line with an operator as
                // a calculation, which sweeps up hard-wrapped prose like
                // "the shot-noise". Evaluation shrugs — nothing without `=>`
                // is computed — but typesetting must not: only what parses
                // cleanly as a definition or equation is worth an equation
                // here, and the rest reads better as the prose it was.
                let indented = block.lines[0]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_whitespace());
                let calculates = indented
                    || statements.iter().any(|s| {
                        s.arrow
                            || matches!(
                                &s.stmt,
                                Stmt::Define { .. } | Stmt::Equation { .. } | Stmt::Directive { .. }
                            )
                            || matches!(&s.stmt, Stmt::Expr(Expr::Call(name, _)) if name == "plot")
                    });
                if !calculates || statements.iter().any(statement_holds_error) {
                    pieces.push(Piece::Prose {
                        line: block.line,
                        text: escape_prose(block.lines[0].trim()),
                    });
                    continue;
                }
                let arrow_lines = arrow_lines(block);
                let mut arrow_index = 0;
                for statement in statements {
                    let answer = if statement.arrow {
                        let line = arrow_lines.get(arrow_index).copied();
                        arrow_index += 1;
                        line.and_then(|l| answers.get_mut(&l))
                            .and_then(|q| q.pop_front())
                    } else {
                        None
                    };
                    // A plot statement typesets as its sampled diagram; one
                    // that failed to sample falls through and echoes as the
                    // equation it is.
                    if matches!(&statement.stmt, Stmt::Expr(Expr::Call(name, _)) if name == "plot")
                    {
                        let key = block.line + block.lines.len() - 1;
                        if let Some(plot) =
                            plots.get_mut(&key).and_then(|queue| queue.pop_front())
                        {
                            pieces.push(Piece::Raw(renderer.lilaq(plot)));
                            continue;
                        }
                    }
                    if let Some(row) = renderer.statement(&statement.stmt, answer) {
                        pieces.push(Piece::Equation { line: block.line, row });
                    }
                }
            }
            BlockKind::Raw => {
                // A `typst` fence is the document speaking Typst directly:
                // its body splices in verbatim. Any other fence stays whole,
                // fence lines and all — Typst markup for a code listing.
                if fence_tag(&block.lines) == Some("typst") {
                    pieces.push(Piece::Raw(fence_body(&block.lines)));
                } else {
                    pieces.push(Piece::Raw(block.lines.join("\n")));
                }
            }
        }
    }
    flush_notation(&mut pieces, &mut notation);

    // A document that speaks raw Typst gets the `calcium` dictionary: every
    // defined name against its computed value, rendered exactly as an answer
    // would be, so a fence can quote results that follow the calculation.
    let speaks_typst = document
        .blocks
        .iter()
        .any(|b| b.kind == BlockKind::Raw && fence_tag(&b.lines) == Some("typst"));
    let dictionary = speaks_typst.then(|| {
        let mut entries = String::new();
        for name in &renderer.defined_order {
            let target = Expr::var(name);
            let value = env.eval_uncertain(&target).unwrap_or_else(|| env.eval(&target));
            if expr_holds_error(&value) {
                continue;
            }
            let text = doc::render_answer(&value, &env);
            let math = renderer.expr(&parse_expr(&text), Prec::Lowest);
            entries.push_str(&format!("  \"{name}\": ${math}$,\n"));
        }
        if entries.is_empty() {
            "#let calcium = (:)".to_string()
        } else {
            format!("#let calcium = (\n{entries})")
        }
    });

    assemble(pieces, dictionary, has_plots)
}

/// The info string of a fence's opening line: the `typst` of ```` ```typst ````.
fn fence_tag(lines: &[String]) -> Option<&str> {
    lines.first()?.trim_start_matches('`').split_whitespace().next()
}

/// A fence's body: the lines between the fence markers.
fn fence_body(lines: &[String]) -> String {
    let ticks = doc::fence_open(&lines[0]).unwrap_or(3);
    let mut body = &lines[1..];
    if body.last().is_some_and(|l| doc::fence_close(l, ticks)) {
        body = &body[..body.len() - 1];
    }
    body.join("\n")
}

/// Joins pieces into final markup: prose from adjacent source lines flows
/// back together into paragraphs, and equations from adjacent lines share
/// one aligned block, the way the source lined them up.
fn assemble(pieces: Vec<Piece>, preamble: Option<String>, has_plots: bool) -> String {
    let mut header = String::from(
        "// Generated by `calcium typst` — edit the .calcium source instead.\n\
         #import \"@preview/fancy-units:0.1.1\": fancy-units-configure, unit, qty\n",
    );
    if has_plots {
        header.push_str("#import \"@preview/lilaq:0.6.0\" as lq\n");
    }
    header.push_str("#fancy-units-configure(\n  per-mode: \"slash\",\n)");
    let mut out: Vec<String> = vec![header];
    out.extend(preamble);
    let mut index = 0;
    while index < pieces.len() {
        match &pieces[index] {
            Piece::Raw(text) => {
                out.push(text.clone());
                index += 1;
            }
            Piece::Prose { .. } => {
                let mut paragraph: Vec<&str> = Vec::new();
                let mut last = None;
                while let Some(Piece::Prose { line, text }) = pieces.get(index) {
                    if last.is_some_and(|l: usize| *line > l + 1) {
                        break;
                    }
                    paragraph.push(text);
                    last = Some(*line);
                    index += 1;
                }
                out.push(paragraph.join("\n"));
            }
            Piece::Equation { .. } => {
                let mut rows: Vec<&EquationRow> = Vec::new();
                let mut last = None;
                while let Some(Piece::Equation { line, row }) = pieces.get(index) {
                    if last.is_some_and(|l: usize| *line > l + 1) {
                        break;
                    }
                    rows.push(row);
                    last = Some(*line);
                    index += 1;
                }
                out.push(render_equations(&rows));
            }
        }
    }
    let mut text = out.join("\n\n");
    text.push('\n');
    text
}

/// Whether a statement carries a parse error anywhere in it.
fn statement_holds_error(statement: &Statement) -> bool {
    match &statement.stmt {
        Stmt::Define { body, .. } => expr_holds_error(body),
        Stmt::Equation { lhs, rhs } => expr_holds_error(lhs) || expr_holds_error(rhs),
        Stmt::Expr(expr) => expr_holds_error(expr),
        Stmt::SumDefine { .. } => false,
        Stmt::Directive { value, .. } => value.as_ref().map(expr_holds_error).unwrap_or(false),
    }
}

/// Whether an error node sits anywhere in an expression.
fn expr_holds_error(expr: &Expr) -> bool {
    {
        if matches!(expr, Expr::Error(_)) {
            return true;
        }
        let mut found = false;
        let mut check = |e: &Expr| found |= expr_holds_error(e);
        match expr {
            Expr::Add(items) | Expr::Mul(items) => items.iter().for_each(&mut check),
            Expr::Matrix(rows) => rows.iter().flatten().for_each(&mut check),
            Expr::Dict(entries) => entries.iter().for_each(|(_, v)| check(v)),
            Expr::Call(_, args) => args.iter().for_each(|a| check(&a.value)),
            Expr::Index(base, indices) => {
                check(base);
                indices.iter().for_each(&mut check);
            }
            Expr::Pow(a, b)
            | Expr::Range(a, b)
            | Expr::PlusMinus(a, b)
            | Expr::Cmp(_, a, b)
            | Expr::Logic(_, a, b)
            | Expr::Bit(_, a, b)
            | Expr::Mod(a, b)
            | Expr::Convert(a, b)
            | Expr::Relation(a, b) => {
                check(a);
                check(b);
            }
            Expr::Abs(a) | Expr::Not(a) | Expr::Transpose(a) | Expr::Norm(a, None) => check(a),
            Expr::Norm(a, Some(p)) => {
                check(a);
                check(p);
            }
            Expr::If(c, t, f) => {
                check(c);
                check(t);
                check(f);
            }
            Expr::Let(_, value, body) => {
                check(value);
                check(body);
            }
            Expr::Num(..) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(_) | Expr::AiQuery => {}
            Expr::Error(_) => found = true,
        }
        found
    }
}

fn is_symbols_heading(title: &str) -> bool {
    let lower = title.trim().to_lowercase();
    lower == "symbols" || lower == "notation"
}

/// Reads one `name # symbol` line of a symbols section. The head must be a
/// bare name — parsed by the real parser, so multi-word names just work — and
/// the symbol is the comment text, taken verbatim as Typst math.
fn symbol_mapping(line: &str) -> Option<(String, String)> {
    let at = comment_byte(line)?;
    let (head, tail) = line.split_at(at);
    let symbol = tail.trim_start_matches('#').trim();
    if symbol.is_empty() {
        return None;
    }
    match parse_line(head).as_slice() {
        [Statement { stmt: Stmt::Expr(Expr::Var(name)), arrow: false }] => {
            Some((name.clone(), symbol.to_string()))
        }
        _ => None,
    }
}

/// Byte offset of a line's `#` comment, via the engine's lexer — which
/// reports in UTF-16, the currency of text views, so convert back.
fn comment_byte(line: &str) -> Option<usize> {
    let utf16 = crate::lexer::comment_start(line)?;
    let mut count = 0;
    for (byte, ch) in line.char_indices() {
        if count >= utf16 {
            return Some(byte);
        }
        count += ch.len_utf16();
    }
    Some(line.len())
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

/// One display equation: an optional left-hand side to align on, and the
/// pieces that follow it, joined by `=`.
struct EquationRow {
    lhs: Option<String>,
    rest: Vec<String>,
}

fn render_equations(rows: &[&EquationRow]) -> String {
    let aligned = rows.len() > 1;
    let mut lines = Vec::new();
    for row in rows {
        let mut line = String::new();
        if let Some(lhs) = &row.lhs {
            line.push_str(lhs);
            line.push_str(if aligned { " &= " } else { " = " });
        }
        line.push_str(&row.rest.join(" = "));
        lines.push(line);
    }
    if aligned {
        format!("$ {} $", lines.join(" \\\n  "))
    } else {
        format!("$ {} $", lines.join(" "))
    }
}

/// Escapes a line of prose for Typst markup. `*` and `_` pass through —
/// Markdown emphasis and Typst emphasis agree closely enough — but the
/// characters that switch Typst into another mode do not.
fn escape_prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '#' | '$' | '@' | '<' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    // `//` would start a Typst comment mid-sentence.
    out.replace("//", "\\/\\/")
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// Binding strength, used to decide where parentheses are required.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    Lowest,
    Add,
    Mul,
    Atom,
}

struct Renderer {
    symbols: HashMap<String, String>,
    defined: HashSet<String>,
    /// Names the document itself declared as units with `@unit`.
    declared: HashSet<String>,
    /// Parameterless definitions in first-appearance order — the keys of
    /// the preamble dictionary.
    defined_order: Vec<String>,
    env: Env,
    fmt: NumFormat,
}

impl Renderer {
    fn new(blocks: &[Block]) -> Renderer {
        let mut symbols = HashMap::new();
        let mut defined = HashSet::new();
        let mut declared = HashSet::new();
        let mut defined_order: Vec<String> = Vec::new();
        let mut in_symbols = false;
        for block in blocks {
            match block.kind {
                BlockKind::Heading => {
                    let title = block.lines[0].trim().trim_start_matches('#').trim();
                    in_symbols = is_symbols_heading(title);
                }
                BlockKind::Code if in_symbols => {
                    for line in &block.lines {
                        if let Some((name, symbol)) = symbol_mapping(line) {
                            symbols.insert(name, symbol);
                        }
                    }
                }
                BlockKind::Code => {
                    let joined = block
                        .lines
                        .iter()
                        .map(|l| l.trim())
                        .collect::<Vec<_>>()
                        .join(" ");
                    for statement in parse_line(&joined) {
                        match &statement.stmt {
                            Stmt::Define { name, params, .. } => {
                                if defined.insert(name.clone()) && params.is_none() {
                                    defined_order.push(name.clone());
                                }
                            }
                            Stmt::SumDefine { name } => {
                                if defined.insert(name.clone()) {
                                    defined_order.push(name.clone());
                                }
                            }
                            Stmt::Directive { name, value }
                                if matches!(name.as_str(), "@unit" | "@units") =>
                            {
                                declared.extend(doc::unit_names(value.as_ref()));
                            }
                            _ => {}
                        }
                    }
                }
                BlockKind::Prose | BlockKind::Raw => {}
            }
        }
        Renderer {
            symbols,
            defined,
            declared,
            defined_order,
            env: Env::with_prelude(),
            fmt: NumFormat::default(),
        }
    }

    /// One statement as an equation row, or nothing for statements with no
    /// typeset form (directives, `+=` headers).
    fn statement(&self, stmt: &Stmt, answer: Option<&doc::Answer>) -> Option<EquationRow> {
        let answer_math = answer
            .filter(|a| !a.is_error && !a.text.is_empty())
            .map(|a| self.expr(&parse_expr(&a.text), Prec::Lowest));
        match stmt {
            Stmt::Define { name, params, body } => {
                let mut lhs = self.name(name);
                if let Some(params) = params {
                    let list: Vec<String> = params.iter().map(|p| self.name(p)).collect();
                    // The space matters: a letter or string subscript would
                    // swallow a directly attached argument list.
                    lhs.push_str(&format!(" ({})", list.join(", ")));
                }
                let mut rest = vec![self.expr(body, Prec::Lowest)];
                rest.extend(answer_math);
                Some(EquationRow { lhs: Some(lhs), rest })
            }
            Stmt::Equation { lhs, rhs } => Some(EquationRow {
                lhs: Some(self.expr(lhs, Prec::Lowest)),
                rest: vec![self.expr(rhs, Prec::Lowest)],
            }),
            Stmt::Expr(expr) => {
                let rendered = self.expr(expr, Prec::Lowest);
                match answer_math {
                    Some(answer) => Some(EquationRow {
                        lhs: Some(rendered),
                        rest: vec![answer],
                    }),
                    None => Some(EquationRow { lhs: None, rest: vec![rendered] }),
                }
            }
            Stmt::SumDefine { .. } | Stmt::Directive { .. } => None,
        }
    }

    /// A name as Typst math: its declared symbol, a fancy-units unit, a bare
    /// letter, or quoted text as the last resort.
    fn name(&self, name: &str) -> String {
        if let Some(symbol) = self.symbols.get(name) {
            return symbol.clone();
        }
        if self.is_unit(name) {
            // Word-like names go to fancy-units; a currency symbol would
            // derail its parser — `$` opens math inside the body — so those
            // stay quoted.
            return if unit_word_like(name) {
                format!("#unit[{name}]")
            } else {
                format!("\"{name}\"")
            };
        }
        if name.chars().count() == 1 || is_greek(name) {
            return name.to_string();
        }
        format!("\"{name}\"")
    }

    /// Whether a name reads as a unit here: the prelude says so and the
    /// document has not shadowed it — a document that defines `T` means its
    /// T, not the tesla — or the document declared it with `@unit`, in which
    /// case even its own definition reads as a derived unit. A declared
    /// symbol wins over both.
    fn is_unit(&self, name: &str) -> bool {
        if self.symbols.contains_key(name) {
            return false;
        }
        self.declared.contains(name)
            || (!self.defined.contains(name) && self.env.is_unit_name(name))
    }

    /// A unit possibly under a power: `mA`, `Hz^2`, `sqrt(Hz)`, and any of
    /// those inverted. Returns the unit's name and its net exponent.
    fn unit_atom(&self, expr: &Expr) -> Option<(String, Num)> {
        match expr {
            Expr::Var(name) if self.is_unit(name) => Some((name.clone(), Num::one())),
            Expr::Pow(base, exp) => {
                let outer = exp.as_num()?.clone();
                let (name, inner) = self.unit_atom(base)?;
                Some((name, inner.mul(&outer)))
            }
            Expr::Call(f, args) if f == "sqrt" && args.len() == 1 && args[0].name.is_none() => {
                let (name, inner) = self.unit_atom(&args[0].value)?;
                Some((name, inner.mul(&Num::ratio(1, 2))))
            }
            _ => None,
        }
    }

    /// A product that is nothing but a coefficient and units — a quantity —
    /// typesets as a single `#qty[...][...]` call.
    fn try_quantity(&self, factors: &[Expr]) -> Option<String> {
        let mut coefficient: Option<&Expr> = None;
        let mut atoms: Vec<(String, Num)> = Vec::new();
        for (index, factor) in factors.iter().enumerate() {
            if let Some((name, exp)) = self.unit_atom(factor) {
                if !unit_word_like(&name) {
                    return None;
                }
                atoms.push((name, exp));
            } else if index == 0 && matches!(factor, Expr::Num(..)) {
                coefficient = Some(factor);
            } else {
                return None;
            }
        }
        if atoms.is_empty() {
            return None;
        }
        let unit = self.unit_markup(&atoms);
        let Some(coefficient) = coefficient else {
            return Some(unit);
        };
        // `1/sqrt(Hz)` already leads with a 1 inside the unit body; a
        // coefficient of exactly one would print the digit twice.
        if atoms.iter().all(|(_, exp)| exp.is_negative())
            && coefficient.as_num().is_some_and(|n| n.is_one())
        {
            return Some(unit);
        }
        // Degree and percent sit against their number, closer than the
        // quantity separator would put them.
        if atoms.len() == 1 && atoms[0].1.is_one() && tight_unit(&unit) {
            return Some(format!("{}{unit}", self.bare(coefficient)));
        }
        match self.qty_number(coefficient) {
            Some(number) => Some(format!("#qty[{number}][{}]", self.unit_body(&atoms))),
            None => Some(format!("{} thin {unit}", self.bare(coefficient))),
        }
    }

    /// A coefficient as a `#qty` number body, for numbers the fancy-units
    /// grammar can read: plain decimals and `e`-notation, where Typst's own
    /// shorthand turns `-` before a digit into the minus sign the grammar
    /// expects. Group separators and radix literals have no place in it.
    fn qty_number(&self, coefficient: &Expr) -> Option<String> {
        match coefficient {
            Expr::Num(value, Radix::Dec | Radix::Sig(_)) => {
                let text = value.format(&self.fmt);
                text.chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
                    .then_some(text)
            }
            _ => None,
        }
    }

    /// One `#unit[...]` call for a set of unit atoms.
    fn unit_markup(&self, atoms: &[(String, Num)]) -> String {
        format!("#unit[{}]", self.unit_body(atoms))
    }

    /// The body of a unit call. Division is written with a slash so the
    /// configured `per-mode` decides its face, and exponents are decimal
    /// because fancy-units panics on fraction forms like `^(1/2)`.
    fn unit_body(&self, atoms: &[(String, Num)]) -> String {
        let piece = |(name, exp): &(String, Num)| {
            let magnitude = exp.abs();
            if magnitude.is_one() {
                name.clone()
            } else {
                format!("{name}^{}", magnitude.format(&self.fmt))
            }
        };
        let numerator: Vec<String> = atoms
            .iter()
            .filter(|(_, exp)| !exp.is_negative())
            .map(piece)
            .collect();
        let denominator: Vec<String> = atoms
            .iter()
            .filter(|(_, exp)| exp.is_negative())
            .map(piece)
            .collect();
        let mut body = if numerator.is_empty() {
            "1".to_string()
        } else {
            numerator.join(" ")
        };
        if !denominator.is_empty() {
            body.push_str(" / ");
            body.push_str(&denominator.join(" "));
        }
        body
    }

    /// One sampled plot as a lilaq diagram, centred on the page. Swept
    /// curves draw as bare lines; literal data keeps its markers. Labels
    /// appear once there is more than one series to tell apart, typeset
    /// through the same math renderer as everything else.
    fn lilaq(&self, plot: &crate::plot::Plot) -> String {
        let mut out = String::from("#align(center, lq.diagram(\n");
        if let Some(x) = &plot.x_label {
            let math = self.expr(&parse_expr(x), Prec::Lowest);
            // The sweep's unit rides along in parentheses — `t (s)` — and
            // the math renderer already dresses unit names in fancy-units.
            match &plot.x_unit {
                Some(unit) => {
                    let unit = self.expr(&parse_expr(unit), Prec::Lowest);
                    out.push_str(&format!("  xlabel: [${math}$ (${unit}$)],\n"));
                }
                None => out.push_str(&format!("  xlabel: ${math}$,\n")),
            }
        }
        // The unit an `in` conversion asked for is the vertical axis.
        if let Some(unit) = &plot.y_unit {
            let unit = self.expr(&parse_expr(unit), Prec::Lowest);
            out.push_str(&format!("  ylabel: ${unit}$,\n"));
        }
        let labeled = plot.series.len() > 1;
        for series in &plot.series {
            let coordinate = |pick: fn(&(f64, f64)) -> f64| {
                series
                    .points
                    .iter()
                    .map(|p| crate::plot::format_point(pick(p)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push_str("  lq.plot(\n    (");
            out.push_str(&coordinate(|p| p.0));
            out.push_str("),\n    (");
            out.push_str(&coordinate(|p| p.1));
            out.push_str("),\n");
            if series.swept {
                out.push_str("    mark: none,\n");
            }
            if labeled {
                let math = self.expr(&parse_expr(&series.label), Prec::Lowest);
                out.push_str(&format!("    label: ${math}$,\n"));
            }
            out.push_str("  ),\n");
        }
        out.push_str("))");
        out
    }

    fn expr(&self, expr: &Expr, parent: Prec) -> String {
        let prec = precedence(expr);
        let body = self.bare(expr);
        if prec < parent {
            format!("({body})")
        } else {
            body
        }
    }

    fn bare(&self, expr: &Expr) -> String {
        // A unit under a power — `Hz^2`, `sqrt(Hz)` — is one unit call,
        // exponent inside, wherever it stands.
        if !matches!(expr, Expr::Var(_)) {
            if let Some((name, exp)) = self.unit_atom(expr) {
                if unit_word_like(&name) {
                    return self.unit_markup(&[(name, exp)]);
                }
            }
        }
        match expr {
            Expr::Num(value, radix) => self.number(value, *radix),
            Expr::Str(s) => format!("\"{}\"", s.replace('"', "\\\"")),
            Expr::Bool(b) => format!("\"{b}\""),
            Expr::Var(name) => self.name(name),
            Expr::AiQuery => "\"#?\"".to_string(),
            Expr::Error(message) => format!("\"error: {message}\""),
            Expr::Add(terms) => self.sum(terms),
            Expr::Mul(factors) => self.product(factors),
            Expr::Pow(base, exp) if is_half(exp) => {
                format!("sqrt({})", self.expr(base, Prec::Lowest))
            }
            Expr::Pow(base, exp) if is_minus_one(exp) => {
                format!("1/{}", self.expr(base, Prec::Atom))
            }
            Expr::Pow(base, exp) => {
                format!(
                    "{}^({})",
                    self.expr(base, Prec::Atom),
                    self.expr(exp, Prec::Lowest)
                )
            }
            Expr::Call(name, args) => {
                let list: Vec<String> = args
                    .iter()
                    .map(|arg| {
                        let value = self.expr(&arg.value, Prec::Lowest);
                        match &arg.name {
                            Some(label) => format!("{} = {value}", self.name(label)),
                            None => value,
                        }
                    })
                    .collect();
                // Typst's own operators take their arguments tightly; for
                // anything else the parentheses are juxtaposed, and the
                // space keeps a subscripted symbol from swallowing them.
                let gap = if is_builtin_math(name) { "" } else { " " };
                format!("{}{gap}({})", self.function_name(name), list.join(", "))
            }
            Expr::Index(base, indices) => {
                let list: Vec<String> =
                    indices.iter().map(|i| self.expr(i, Prec::Lowest)).collect();
                format!("{}_({})", self.expr(base, Prec::Atom), list.join(", "))
            }
            Expr::Matrix(rows) => {
                let body: Vec<String> = rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| self.expr(cell, Prec::Lowest))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .collect();
                format!("mat(delim: \"[\", {})", body.join("; "))
            }
            Expr::Dict(_) => format!("\"{}\"", crate::format::render(expr)),
            Expr::Range(lo, hi) => format!(
                "{} .. {}",
                self.expr(lo, Prec::Add),
                self.expr(hi, Prec::Add)
            ),
            Expr::PlusMinus(value, sigma) => format!(
                "{} plus.minus {}",
                self.expr(value, Prec::Add),
                self.expr(sigma, Prec::Add)
            ),
            Expr::Abs(inner) => format!("abs({})", self.expr(inner, Prec::Lowest)),
            Expr::Norm(inner, p) => match p {
                Some(p) => format!(
                    "norm({})_({})",
                    self.expr(inner, Prec::Lowest),
                    self.expr(p, Prec::Lowest)
                ),
                None => format!("norm({})", self.expr(inner, Prec::Lowest)),
            },
            Expr::Transpose(inner) => format!("{}^\"T\"", self.expr(inner, Prec::Atom)),
            Expr::Not(inner) => format!("not {}", self.expr(inner, Prec::Atom)),
            Expr::Cmp(op, a, b) => {
                let symbol = match op {
                    CmpOp::Lt => "<",
                    CmpOp::Gt => ">",
                    CmpOp::Le => "<=",
                    CmpOp::Ge => ">=",
                    CmpOp::Eq => "=",
                    CmpOp::Ne => "!=",
                };
                format!(
                    "{} {symbol} {}",
                    self.expr(a, Prec::Add),
                    self.expr(b, Prec::Add)
                )
            }
            Expr::Relation(a, b) => format!(
                "{} = {}",
                self.expr(a, Prec::Add),
                self.expr(b, Prec::Add)
            ),
            Expr::Logic(op, a, b) => {
                let word = match op {
                    LogicOp::And => "and",
                    LogicOp::Or => "or",
                };
                format!(
                    "{} {word} {}",
                    self.expr(a, Prec::Add),
                    self.expr(b, Prec::Add)
                )
            }
            Expr::Bit(op, a, b) => {
                let word = match op {
                    BitOp::And => "\" & \"",
                    BitOp::Or => "\" | \"",
                };
                format!(
                    "{}{word}{}",
                    self.expr(a, Prec::Add),
                    self.expr(b, Prec::Add)
                )
            }
            Expr::Mod(a, b) => format!(
                "{}\" mod \"{}",
                self.expr(a, Prec::Mul),
                self.expr(b, Prec::Atom)
            ),
            Expr::If(cond, then_branch, else_branch) => format!(
                "cases({} & \" if \" {}, {} & \" otherwise\")",
                self.expr(then_branch, Prec::Lowest),
                self.expr(cond, Prec::Lowest),
                self.expr(else_branch, Prec::Lowest)
            ),
            Expr::Let(name, value, body) => format!(
                "{} quad \"where\" quad {} = {}",
                self.expr(body, Prec::Lowest),
                self.name(name),
                self.expr(value, Prec::Lowest)
            ),
            // The conversion target vanishes: `x in mA` typesets as plain `x`,
            // and the folded-in answer already shows the unit it asked for.
            Expr::Convert(value, _unit) => self.bare(value),
        }
    }

    fn function_name(&self, name: &str) -> String {
        if let Some(symbol) = self.symbols.get(name) {
            return symbol.clone();
        }
        match name {
            "sqrt" | "sin" | "cos" | "tan" | "arcsin" | "arccos" | "arctan" | "ln" | "log"
            | "exp" | "min" | "max" | "abs" | "det" | "lim" | "sum" | "gcd" | "lcm" => {
                name.to_string()
            }
            "log10" => "log_(10)".to_string(),
            "log2" => "log_(2)".to_string(),
            "asin" => "arcsin".to_string(),
            "acos" => "arccos".to_string(),
            "atan" => "arctan".to_string(),
            other => self.name(other),
        }
    }

    fn number(&self, value: &Num, radix: Radix) -> String {
        let text = match radix {
            Radix::Dec | Radix::Sig(_) => value.format(&self.fmt),
            // Radix literals are text, not magnitudes to typeset.
            _ => return format!("\"{}\"", crate::format::render(&Expr::Num(value.clone(), radix))),
        };
        // Scientific notation typesets as a power of ten.
        if let Some(at) = text.find(['e', 'E']) {
            let (mantissa, exponent) = text.split_at(at);
            let exponent = &exponent[1..];
            let mantissa = if mantissa.contains(',') {
                format!("\"{mantissa}\"")
            } else {
                mantissa.to_string()
            };
            return format!("{mantissa} times 10^({exponent})");
        }
        // Group separators read as punctuation in math mode; quoting keeps
        // them inside the numeral. The sign stays outside, where it typesets
        // as a minus rather than a hyphen.
        if text.contains(',') {
            match text.strip_prefix('-') {
                Some(rest) => format!("-\"{rest}\""),
                None => format!("\"{text}\""),
            }
        } else {
            text
        }
    }

    fn sum(&self, terms: &[Expr]) -> String {
        if terms.is_empty() {
            return "0".to_string();
        }
        let mut out = String::new();
        for (i, term) in terms.iter().enumerate() {
            let (negative, magnitude) = split_sign(term);
            if i == 0 {
                if negative {
                    out.push('-');
                }
            } else {
                out.push_str(if negative { " - " } else { " + " });
            }
            out.push_str(&self.expr(&magnitude, Prec::Add));
        }
        out
    }

    fn product(&self, factors: &[Expr]) -> String {
        if factors.is_empty() {
            return "1".to_string();
        }
        if let Some(quantity) = self.try_quantity(factors) {
            return quantity;
        }
        // Split into numerator and denominator by the sign of each exponent,
        // exactly as the source formatter does.
        let mut numerator: Vec<Expr> = Vec::new();
        let mut denominator: Vec<Expr> = Vec::new();
        for factor in factors {
            match factor {
                Expr::Pow(base, exp) => match exp.as_num() {
                    Some(e) if e.is_negative() => {
                        if e.eq_num(&Num::from_i64(-1)) {
                            denominator.push((**base).clone());
                        } else {
                            denominator.push(Expr::Pow(
                                base.clone(),
                                Box::new(Expr::Num(e.neg(), Radix::Dec)),
                            ));
                        }
                    }
                    _ => numerator.push(factor.clone()),
                },
                other => numerator.push(other.clone()),
            }
        }

        let top = self.juxtapose(&numerator);
        if denominator.is_empty() {
            return top;
        }
        let bottom = self.juxtapose(&denominator);
        // Typst's `/` binds tighter than juxtaposition, so any side that is
        // not one atom needs parentheses — which Typst absorbs into the
        // drawn fraction, so wrapping costs nothing visually.
        let wrap = |text: String, factors: &[Expr]| {
            let needed = factors.len() > 1
                || factors.first().is_some_and(|f| needs_wrap(f, &text));
            if needed && !fully_parenthesized(&text) {
                format!("({text})")
            } else {
                text
            }
        };
        format!("{}/{}", wrap(top, &numerator), wrap(bottom, &denominator))
    }

    /// Factors side by side, with a separator chosen by what touches: `dot`
    /// keeps two numerals apart, a thin space sets a numeral or letter off
    /// from upright text, and letters simply sit next to each other.
    fn juxtapose(&self, factors: &[Expr]) -> String {
        if factors.is_empty() {
            return "1".to_string();
        }
        let rendered: Vec<String> = factors
            .iter()
            .map(|f| self.expr(f, Prec::Mul))
            .collect();
        let mut out = rendered[0].clone();
        for pair in rendered.windows(2) {
            let left = pair[0].chars().last().unwrap_or(' ');
            let right = pair[1].chars().next().unwrap_or(' ');
            // A factor that opens with a digit reads as part of whatever it
            // follows; an explicit dot keeps `2 E dot 1 #unit[Hz]` unambiguous.
            if right.is_ascii_digit() {
                out.push_str(" dot ");
            } else if tight_unit(&pair[1]) {
                // `45#unit[°]` — the degree sign sits against its number.
            } else if left == '"' || right == '"' || right == '#' || left == ']' {
                out.push_str(" thin ");
            } else {
                out.push(' ');
            }
            out.push_str(&pair[1]);
        }
        out
    }
}

/// Whether one pair of parentheses already encloses the whole text — a
/// second pair would survive Typst's stripping and show in print.
fn fully_parenthesized(text: &str) -> bool {
    if !text.starts_with('(') || !text.ends_with(')') {
        return false;
    }
    let mut depth = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return index == text.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

/// Whether a lone fraction operand still needs parentheses: sums and
/// products always, and numbers only when they typeset as more than one
/// token, as a power of ten does.
fn needs_wrap(factor: &Expr, text: &str) -> bool {
    match factor {
        Expr::Convert(inner, _) => needs_wrap(inner, text),
        Expr::Add(_) | Expr::Mul(_) => true,
        Expr::Num(..) => text.contains(' '),
        // A user function typesets as symbol-next-to-parentheses, and `/`
        // would take just the parentheses; Typst's own operators hold on to
        // their arguments.
        Expr::Call(name, _) => !is_builtin_math(name),
        _ => false,
    }
}

/// Function names Typst's math mode takes as operators, arguments and all.
fn is_builtin_math(name: &str) -> bool {
    matches!(
        name,
        "sqrt" | "sin" | "cos" | "tan" | "arcsin" | "arccos" | "arctan" | "asin" | "acos"
            | "atan" | "ln" | "log" | "log10" | "log2" | "exp" | "min" | "max" | "abs" | "det"
            | "lim" | "gcd" | "lcm"
    )
}

fn precedence(expr: &Expr) -> Prec {
    match expr {
        Expr::Add(_) => Prec::Add,
        Expr::Mul(_) | Expr::Mod(..) => Prec::Mul,
        Expr::Cmp(..) | Expr::Relation(..) | Expr::Logic(..) | Expr::Bit(..) => Prec::Lowest,
        Expr::Range(..) | Expr::PlusMinus(..) | Expr::If(..) | Expr::Let(..) => Prec::Lowest,
        Expr::Convert(value, _) => precedence(value),
        _ => Prec::Atom,
    }
}

fn is_half(exp: &Expr) -> bool {
    exp.as_num()
        .map(|e| e.eq_num(&Num::ratio(1, 2)))
        .unwrap_or(false)
}

fn is_minus_one(exp: &Expr) -> bool {
    exp.as_num()
        .map(|e| e.eq_num(&Num::from_i64(-1)))
        .unwrap_or(false)
}

/// Splits a leading minus out of a term, returning the positive remainder.
fn split_sign(term: &Expr) -> (bool, Expr) {
    match term {
        Expr::Num(value, radix) if value.is_negative() => (true, Expr::Num(value.neg(), *radix)),
        Expr::Mul(factors) => {
            if let Some(Expr::Num(value, radix)) = factors.first() {
                if value.is_negative() {
                    let mut rest = factors.clone();
                    if value.abs().is_one() && factors.len() > 1 {
                        rest.remove(0);
                    } else {
                        rest[0] = Expr::Num(value.neg(), *radix);
                    }
                    return (true, Expr::mul(rest));
                }
            }
            (false, term.clone())
        }
        _ => (false, term.clone()),
    }
}

/// Whether a unit name survives the fancy-units body grammar: letters only
/// (µ, Ω and friends included), plus the two symbols that convention sets
/// tight against their number.
fn unit_word_like(name: &str) -> bool {
    name == "°" || name == "%" || (!name.is_empty() && name.chars().all(|c| c.is_alphabetic()))
}

/// The two units that take no space before them: `45#unit[°]`, `37.5#unit[%]`.
fn tight_unit(text: &str) -> bool {
    matches!(text, "#unit[°]" | "#unit[%]")
}

/// Names Typst's math mode already knows as Greek letters.
fn is_greek(name: &str) -> bool {
    matches!(
        name,
        "alpha" | "beta" | "gamma" | "delta" | "epsilon" | "zeta" | "eta" | "theta" | "iota"
            | "kappa" | "lambda" | "mu" | "nu" | "xi" | "omicron" | "pi" | "rho" | "sigma"
            | "tau" | "upsilon" | "phi" | "chi" | "psi" | "omega" | "Gamma" | "Delta" | "Theta"
            | "Lambda" | "Xi" | "Pi" | "Sigma" | "Upsilon" | "Phi" | "Psi" | "Omega"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculations_become_equations_with_answers_folded_in() {
        let out = to_typst("    x = 2 + 3 => 5\n");
        assert!(out.contains("$ x = 2 + 3 = 5 $"), "got:\n{out}");
    }

    #[test]
    fn symbols_section_renames_and_typesets_as_a_table() {
        let source = "    threshold current = 50 mA\n    operating current = threshold current + 3 mA => 53 mA\n\n## Symbols\n\n    operating current  # I\n    threshold current  # I_\"th\"\n";
        let out = to_typst(source);
        assert!(
            out.contains("I &= I_\"th\" + #qty[3][mA] = #qty[53][mA]"),
            "got:\n{out}"
        );
        assert!(out.contains("== Symbols"), "got:\n{out}");
        assert!(out.contains("[$I_\"th\"$], [threshold current]"), "got:\n{out}");
        // The mapping lines themselves do not become equations.
        assert!(!out.contains("$ I $"), "got:\n{out}");
    }

    #[test]
    fn units_use_fancy_units_and_unmapped_names_become_text() {
        let out = to_typst("    laser power = 2 mW\n");
        assert!(
            out.contains("$ \"laser power\" = #qty[2][mW] $"),
            "got:\n{out}"
        );
        assert!(
            out.contains("#import \"@preview/fancy-units:0.1.1\": fancy-units-configure, unit, qty"),
            "got:\n{out}"
        );
        assert!(out.contains("per-mode: \"slash\""), "got:\n{out}");
    }

    #[test]
    fn prefixed_units_are_recognized() {
        let out = to_typst("    c = 31.6 pW\n");
        assert!(out.contains("#qty[31.6][pW]"), "got:\n{out}");
    }

    #[test]
    fn per_root_hertz_becomes_a_slash_with_a_decimal_exponent() {
        let out = to_typst("    a = 31.6144 pW/sqrt(Hz)\n    b = 1 nA/sqrt(Hz)\n    c = 1/sqrt(Hz)\n");
        assert!(out.contains("#qty[31.6144][pW / Hz^0.5]"), "got:\n{out}");
        assert!(out.contains("#qty[1][nA / Hz^0.5]"), "got:\n{out}");
        assert!(out.contains("c &= #unit[1 / Hz^0.5]"), "got:\n{out}");
    }

    #[test]
    fn degrees_sit_tight_against_their_number() {
        let out = to_typst("    a = 45°\n");
        assert!(out.contains("a = 45#unit[°]"), "got:\n{out}");
    }

    #[test]
    fn declared_units_join_fancy_units_markup() {
        let out = to_typst(
            "    @unit = burrito\n    burrito length = 1 ft / burrito\n    burrito cost = 8 USD / burrito\n",
        );
        assert!(out.contains("#qty[1][ft / burrito]"), "got:\n{out}");
        assert!(out.contains("#qty[8][USD / burrito]"), "got:\n{out}");
    }

    #[test]
    fn a_declared_unit_reads_as_a_unit_even_when_the_document_defines_it() {
        let out = to_typst("    @unit = firkin\n    firkin = 90 lb\n    load = 2 firkin\n");
        assert!(out.contains("#unit[firkin] &= #qty[90][lb]"), "got:\n{out}");
        assert!(out.contains("\"load\" &= #qty[2][firkin]"), "got:\n{out}");
    }

    #[test]
    fn division_becomes_a_fraction_and_sqrt_survives() {
        let out = to_typst("    y = sqrt(4*a*b)/(2*a)\n");
        assert!(out.contains("y = sqrt(4 a b)/(2 a)"), "got:\n{out}");
    }

    #[test]
    fn scientific_notation_typesets_as_a_power_of_ten() {
        let out = to_typst("    photon = 2.5e-19 J\n");
        assert!(out.contains("#qty[2.5e-19][J]"), "got:\n{out}");
    }

    #[test]
    fn conversions_vanish_from_the_typeset_form() {
        let out = to_typst("    speed = 60 mph in m/s => 26.8224 m/s\n");
        assert!(
            out.contains("\"speed\" = #qty[60][mph] = #qty[26.8224][m / s]"),
            "got:\n{out}"
        );
    }

    #[test]
    fn adjacent_calculations_align_and_separated_ones_do_not() {
        let out = to_typst("    a = 1\n    b = 2\n\n    c = 3\n");
        assert!(out.contains("$ a &= 1 \\\n  b &= 2 $"), "got:\n{out}");
        assert!(out.contains("$ c = 3 $"), "got:\n{out}");
    }

    #[test]
    fn headings_and_prose_translate_and_escape() {
        let source = "# Title\n\nCosts $5 at 50/60 Hz #tagged.\n";
        let out = to_typst(source);
        assert!(out.contains("= Title"), "got:\n{out}");
        assert!(out.contains("Costs \\$5 at 50/60 Hz \\#tagged."), "got:\n{out}");
    }

    #[test]
    fn functions_keep_their_parameters() {
        let source = "    quiet limit(power, slope) = sqrt(2*power)/slope\n\n## Symbols\n\n    quiet limit  # i_\"max\"\n    power        # P\n    slope        # eta\n";
        let out = to_typst(source);
        assert!(out.contains("i_\"max\" (P, eta) = sqrt(2 P)/eta"), "got:\n{out}");
    }

    #[test]
    fn a_documents_own_name_outranks_a_prelude_unit() {
        let out = to_typst("    T = 125\n    y = 2*T => 250\n");
        assert!(out.contains("y &= 2 T = 250"), "got:\n{out}");
    }

    #[test]
    fn uncertainties_typeset_with_the_plus_minus_symbol() {
        let out = to_typst("    q = 2 ± 1\n    q + 3 => 5 ± 1\n");
        assert!(out.contains("q + 3 &= 5 plus.minus 1"), "got:\n{out}");
    }

    #[test]
    fn grouped_numbers_stay_whole_in_math_mode() {
        let out = to_typst("    n = 2^10 => 1,024\n");
        assert!(out.contains("= \"1,024\""), "got:\n{out}");
    }

    #[test]
    fn grouped_coefficients_stay_beside_a_unit_call() {
        // fancy-units' number grammar has no group separators, so a grouped
        // coefficient keeps the quoted-number-and-#unit form.
        let out = to_typst("    f = 1500 Hz\n");
        assert!(out.contains("\"1,500\" thin #unit[Hz]"), "got:\n{out}");
    }

    #[test]
    fn a_parenthesized_sum_in_a_fraction_takes_one_pair_of_parens() {
        let out = to_typst("    y = a/(a + b)\n");
        assert!(out.contains("y = a/(a + b)"), "got:\n{out}");
        assert!(!out.contains("(("), "got:\n{out}");
    }

    #[test]
    fn wrapped_prose_with_operators_stays_prose() {
        // A hard-wrapped paragraph line that carries a hyphen or `=` reads as
        // code to the classifier, but must typeset as the prose it is.
        let source = "The transfer is flat, so a ripple δI becomes δP = η·δI. The floor\nunder everything is the shot-noise of the light itself, and that is\nthe requirement.\n";
        let out = to_typst(source);
        assert!(!out.contains("error"), "got:\n{out}");
        assert!(!out.contains("$"), "got:\n{out}");
        // The paragraph flows back together rather than splitting per line.
        assert!(
            out.contains("δP = η·δI. The floor\nunder everything"),
            "got:\n{out}"
        );
    }

    #[test]
    fn unindented_definitions_still_typeset_as_equations() {
        let out = to_typst("T = 125 degC\nye = 2*pi*2.8024 MHz/gauss\n");
        assert!(out.contains("T &= #qty[125][degC]"), "got:\n{out}");
        assert!(
            out.contains("\"ye\" &= (2 pi dot 2.8024 thin #unit[MHz])/#unit[gauss]"),
            "got:\n{out}"
        );
    }

    #[test]
    fn typst_fences_splice_verbatim_with_the_value_dictionary() {
        let source = "    x = 2 mW\n\n```typst\n#calcium.at(\"x\")\n```\n";
        let out = to_typst(source);
        assert!(
            out.contains("#let calcium = (\n  \"x\": $#qty[2][mW]$,\n)"),
            "got:\n{out}"
        );
        // The body splices in bare: no fence markers survive.
        assert!(out.contains("\n\n#calcium.at(\"x\")\n"), "got:\n{out}");
        assert!(!out.contains("```"), "got:\n{out}");
    }

    #[test]
    fn other_fences_typeset_as_code_listings() {
        let source = "A listing:\n\n```python\nprint(1)\n```\n";
        let out = to_typst(source);
        assert!(out.contains("```python\nprint(1)\n```"), "got:\n{out}");
        assert!(!out.contains("#let calcium"), "got:\n{out}");
    }

    #[test]
    fn documents_without_typst_fences_carry_no_dictionary() {
        let out = to_typst("    x = 2\n");
        assert!(!out.contains("#let calcium"), "got:\n{out}");
    }

    #[test]
    fn dictionary_values_keep_a_unit_quantitys_one() {
        let source = "    f = 1 kHz\n\n```typst\n#calcium.at(\"f\")\n```\n";
        let out = to_typst(source);
        assert!(out.contains("\"f\": $#qty[1][kHz]$"), "got:\n{out}");
    }

    #[test]
    fn the_dictionary_holds_final_values_and_skips_functions() {
        let source = "    f(x) = 2 x\n    y = 1\n    y = 3\n\n```typst\n#calcium.at(\"y\")\n```\n";
        let out = to_typst(source);
        assert!(out.contains("\"y\": $3$"), "got:\n{out}");
        assert!(!out.contains("\"f\""), "got:\n{out}");
        // One entry per name, even for the redefined.
        assert_eq!(out.matches("\"y\":").count(), 1, "got:\n{out}");
    }

    #[test]
    fn plots_typeset_as_lilaq_diagrams() {
        let out = to_typst("    plot(sin(t), cos(t), 0..1)\n");
        assert!(
            out.contains("#import \"@preview/lilaq:0.6.0\" as lq"),
            "got:\n{out}"
        );
        assert!(
            out.contains("#align(center, lq.diagram(\n  xlabel: $t$,"),
            "got:\n{out}"
        );
        assert!(out.contains("label: $sin(t)$"), "got:\n{out}");
        assert!(out.contains("label: $cos(t)$"), "got:\n{out}");
        assert!(out.contains("mark: none"), "got:\n{out}");
        // The call itself does not also typeset as an equation.
        assert!(!out.contains("$ \"plot\""), "got:\n{out}");
    }

    #[test]
    fn data_plots_keep_their_markers() {
        let out = to_typst("    plot([3, 1, 4])\n");
        assert!(out.contains("#align(center, lq.diagram("), "got:\n{out}");
        assert!(out.contains("(0, 1, 2)"), "got:\n{out}");
        assert!(out.contains("(3, 1, 4)"), "got:\n{out}");
        assert!(!out.contains("mark: none"), "got:\n{out}");
        assert!(!out.contains("xlabel"), "got:\n{out}");
    }

    #[test]
    fn documents_without_plots_skip_the_lilaq_import() {
        let out = to_typst("    x = 2\n");
        assert!(!out.contains("lilaq"), "got:\n{out}");
    }

    #[test]
    fn an_unplottable_plot_stays_an_equation() {
        let out = to_typst("    plot(\"words\")\n");
        assert!(!out.contains("lq.diagram"), "got:\n{out}");
        assert!(out.contains("\"plot\""), "got:\n{out}");
    }

    #[test]
    fn symbols_sections_are_inert_to_evaluation() {
        // The same document with and without its symbols section computes
        // identically — the section is invisible to the engine.
        let body = "    x = 2\n    x + 1 => 3\n";
        let with = format!("{body}\n## Symbols\n\n    x  # X\n");
        let plain = doc::evaluate(body);
        let spec = doc::evaluate(&with);
        assert_eq!(plain.answers.len(), spec.answers.len());
        assert_eq!(plain.answers[0].text, spec.answers[0].text);
    }
}
