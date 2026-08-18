//! Rendering expressions back to source text.
//!
//! This module is load-bearing, not cosmetic. Results are written *into* the
//! document as text, so whatever we print here is what the user sees, edits,
//! and re-parses. `render` must produce something `parse_expr` reads back to
//! the same tree.
//!
//! The spacing rules:
//!
//! * an alphabetic unit takes a space — `15 things`, `6.8889 miles/hour`,
//!   and a letter is a letter even when it looks like a symbol — `2 Ω`
//! * a symbolic one does not — `45°`, `80%`, `2i`
//! * some currency symbols lead instead of trail — `$1,550`, `¥5,406.8551`,
//!   but `188,817.9015€`

use crate::ast::*;
use crate::lexer::Radix;
use crate::num::{Num, NumFormat};

/// Currency symbols that precede the amount. Everything else trails.
const LEADING_SYMBOLS: &[&str] = &["$", "¥", "£", "₹", "₽", "₩", "¢"];

/// ISO codes that print as a symbol. Codes without an entry here print as the
/// code itself — `20brl in cny => 26.6434 cny`.
const CURRENCY_SYMBOLS: &[(&str, &str)] = &[
    ("usd", "$"),
    ("USD", "$"),
    ("eur", "€"),
    ("EUR", "€"),
    ("gbp", "£"),
    ("GBP", "£"),
    ("jpy", "¥"),
    ("JPY", "¥"),
    ("inr", "₹"),
    ("INR", "₹"),
    ("krw", "₩"),
    ("KRW", "₩"),
    ("rub", "₽"),
    ("RUB", "₽"),
];

fn currency_symbol(name: &str) -> Option<&'static str> {
    CURRENCY_SYMBOLS
        .iter()
        .find(|(code, _)| *code == name)
        .map(|(_, symbol)| *symbol)
}

/// Binding strength, used to decide where parentheses are required.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    Lowest,
    Convert,
    Or,
    And,
    Compare,
    Range,
    Add,
    Mul,
    Pow,
    Atom,
}

pub fn render(expr: &Expr) -> String {
    render_with(expr, &NumFormat::default())
}

pub fn render_with(expr: &Expr, fmt: &NumFormat) -> String {
    let mut out = String::new();
    // A whole result that is nothing but a unit still has a magnitude of one,
    // and dropping it makes a conversion look like it failed: `760 torr in atm`
    // should answer `1 atm`, not `atm`. Only at the top level — a bare `m`
    // inside a larger expression is a symbol, not a quantity.
    if matches!(expr, Expr::Var(name) if fmt.units.contains(name)) {
        out.push_str("1 ");
    }
    write_expr(&mut out, expr, Prec::Lowest, fmt);
    out
}

fn write_expr(out: &mut String, expr: &Expr, parent: Prec, fmt: &NumFormat) {
    // Rendering pays into the evaluation fuel budget: simplification renders
    // subtrees as collection keys, and on a runaway symbolic result that is
    // where the time actually goes. Burning per node makes fuel track real
    // work; nothing here checks the tank, so the final answer always renders.
    crate::eval::spend_fuel();
    let prec = precedence(expr);
    let needs_parens = prec < parent;
    if needs_parens {
        out.push('(');
    }
    write_bare(out, expr, fmt);
    if needs_parens {
        out.push(')');
    }
}

fn precedence(expr: &Expr) -> Prec {
    match expr {
        Expr::Add(_) => Prec::Add,
        Expr::Mul(_) | Expr::Mod(..) => Prec::Mul,
        Expr::Pow(..) => Prec::Pow,
        Expr::Cmp(..) | Expr::Relation(..) => Prec::Compare,
        Expr::Logic(LogicOp::Or, ..) | Expr::Bit(BitOp::Or, ..) => Prec::Or,
        Expr::Logic(LogicOp::And, ..) | Expr::Bit(BitOp::And, ..) => Prec::And,
        Expr::Range(..) => Prec::Range,
        // Parsed tightly, printed defensively: parenthesized anywhere inside
        // a product or power, bare on its own and in sums.
        Expr::PlusMinus(..) => Prec::Add,
        Expr::Convert(..) => Prec::Convert,
        Expr::If(..) | Expr::Let(..) => Prec::Lowest,
        _ => Prec::Atom,
    }
}

fn write_bare(out: &mut String, expr: &Expr, fmt: &NumFormat) {
    match expr {
        Expr::Num(value, radix) => out.push_str(&format_number(value, *radix, fmt)),
        Expr::Str(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        Expr::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        // A currency amount prints with its symbol: `$20 in eur` answers
        // `17.5808€`, not `17.5808 eur`.
        Expr::Var(name) => out.push_str(currency_symbol(name).unwrap_or(name)),
        Expr::AiQuery => out.push_str("#?"),
        Expr::Error(msg) => {
            out.push_str("error: ");
            out.push_str(msg);
        }
        Expr::Add(terms) => write_sum(out, terms, fmt),
        Expr::Mul(factors) => write_product(out, factors, fmt),
        // A half power reads back as `sqrt`, which is how a user would write it.
        Expr::Pow(base, exp) if exp.as_num().map(|e| e.eq_num(&Num::ratio(1, 2))).unwrap_or(false) => {
            out.push_str("sqrt(");
            write_expr(out, base, Prec::Lowest, fmt);
            out.push(')');
        }
        // `x^-1` reads better as `1/x`. Higher negative powers keep the
        // exponent form, so `a^-3` stays `a^-3`.
        Expr::Pow(base, exp)
            if exp.as_num().map(|e| e.eq_num(&Num::from_i64(-1))).unwrap_or(false)
                && !matches!(&**base, Expr::Num(..)) =>
        {
            write_product(out, &[expr.clone()], fmt)
        }
        Expr::Pow(base, exp) => {
            write_expr(out, base, Prec::Atom, fmt);
            out.push('^');
            // `a^-3` prints bare, but `a^(-b)` needs parentheses to read back.
            let simple = matches!(**exp, Expr::Num(..) | Expr::Var(_));
            if simple {
                write_bare(out, exp, fmt);
            } else {
                out.push('(');
                write_bare(out, exp, fmt);
                out.push(')');
            }
        }
        Expr::Call(name, args) => {
            out.push_str(name);
            out.push('(');
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if let Some(label) = &arg.name {
                    out.push_str(label);
                    out.push_str(" = ");
                }
                write_expr(out, &arg.value, Prec::Lowest, fmt);
            }
            out.push(')');
        }
        Expr::Index(base, indices) => {
            write_expr(out, base, Prec::Atom, fmt);
            out.push('[');
            for (i, index) in indices.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_expr(out, index, Prec::Lowest, fmt);
            }
            out.push(']');
        }
        Expr::Matrix(rows) => {
            out.push('[');
            for (r, row) in rows.iter().enumerate() {
                if r > 0 {
                    out.push_str("; ");
                }
                for (c, cell) in row.iter().enumerate() {
                    if c > 0 {
                        out.push_str(", ");
                    }
                    write_expr(out, cell, Prec::Lowest, fmt);
                }
            }
            out.push(']');
        }
        Expr::Dict(entries) => {
            out.push('{');
            for (i, (key, value)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(key);
                out.push(':');
                write_expr(out, value, Prec::Lowest, fmt);
            }
            out.push('}');
        }
        Expr::Range(lo, hi) => {
            write_expr(out, lo, Prec::Add, fmt);
            out.push_str("..");
            write_expr(out, hi, Prec::Add, fmt);
        }
        Expr::PlusMinus(value, sigma) => {
            if let Some(text) = rounded_uncertain(value, sigma, fmt) {
                out.push_str(&text);
                return;
            }
            // The unfused `±` reads like `+` and `-`, so each side may be a
            // product but a sum needs its parentheses.
            write_expr(out, value, Prec::Mul, fmt);
            out.push_str(" ± ");
            write_expr(out, sigma, Prec::Mul, fmt);
        }
        Expr::Abs(inner) => {
            out.push('|');
            write_expr(out, inner, Prec::Lowest, fmt);
            out.push('|');
        }
        Expr::Norm(inner, p) => {
            out.push_str("||");
            write_expr(out, inner, Prec::Lowest, fmt);
            out.push_str("||");
            if let Some(p) = p {
                write_bare(out, p, fmt);
            }
        }
        Expr::Transpose(inner) => {
            write_expr(out, inner, Prec::Atom, fmt);
            out.push_str("^T");
        }
        Expr::Not(inner) => {
            out.push('!');
            write_expr(out, inner, Prec::Atom, fmt);
        }
        Expr::Cmp(op, a, b) => {
            write_expr(out, a, Prec::Range, fmt);
            out.push_str(match op {
                CmpOp::Lt => " < ",
                CmpOp::Gt => " > ",
                CmpOp::Le => " <= ",
                CmpOp::Ge => " >= ",
                CmpOp::Eq => " == ",
                CmpOp::Ne => " != ",
            });
            write_expr(out, b, Prec::Range, fmt);
        }
        Expr::Relation(a, b) => {
            write_expr(out, a, Prec::Range, fmt);
            out.push_str(" == ");
            write_expr(out, b, Prec::Range, fmt);
        }
        Expr::Logic(op, a, b) => {
            let (text, prec) = match op {
                LogicOp::And => (" && ", Prec::And),
                LogicOp::Or => (" || ", Prec::Or),
            };
            write_expr(out, a, prec, fmt);
            out.push_str(text);
            write_expr(out, b, prec, fmt);
        }
        Expr::Bit(op, a, b) => {
            let (text, prec) = match op {
                BitOp::And => (" & ", Prec::And),
                BitOp::Or => (" | ", Prec::Or),
            };
            write_expr(out, a, prec, fmt);
            out.push_str(text);
            write_expr(out, b, prec, fmt);
        }
        Expr::Mod(a, b) => {
            write_expr(out, a, Prec::Mul, fmt);
            out.push_str(" mod ");
            write_expr(out, b, Prec::Pow, fmt);
        }
        Expr::If(cond, then_branch, else_branch) => {
            out.push_str("if ");
            write_expr(out, cond, Prec::Lowest, fmt);
            out.push_str(" then ");
            write_expr(out, then_branch, Prec::Lowest, fmt);
            out.push_str(" else ");
            write_expr(out, else_branch, Prec::Lowest, fmt);
        }
        Expr::Let(name, value, body) => {
            out.push_str("let ");
            out.push_str(name);
            out.push_str(" = ");
            write_expr(out, value, Prec::Lowest, fmt);
            out.push_str(" in ");
            write_expr(out, body, Prec::Lowest, fmt);
        }
        Expr::Convert(value, unit) => {
            write_expr(out, value, Prec::Or, fmt);
            out.push_str(" in ");
            write_expr(out, unit, Prec::Or, fmt);
        }
    }
}

/// `centre ± sigma` in the physics convention: the uncertainty shown to two
/// significant figures, the centre rounded to the same decimal place, and a
/// shared unit written once around the pair — `(50 ± 1) mA`. Only when both
/// sides are quantities of the same thing — same units, same symbols — so
/// the rounding is honest; anything stranger prints plainly.
fn rounded_uncertain(value: &Expr, sigma: &Expr, fmt: &NumFormat) -> Option<String> {
    let (v, v_rest) = crate::simplify::split_coefficient(value);
    let (s, s_rest) = crate::simplify::split_coefficient(sigma);
    if render(&v_rest) != render(&s_rest) {
        return None;
    }
    let width = s.to_f64();
    if !width.is_finite() || width <= 0.0 {
        return None;
    }
    // The decimal place of the uncertainty's second significant digit.
    let place = width.abs().log10().floor() as i32 - 1;
    let quantum = 10f64.powi(place);
    let round_to = |x: f64| (x / quantum).round() * quantum;
    let sigma_text = format_number(&Num::from_f64(round_to(width)), Radix::Dec, fmt);
    let value_text = format_number(&Num::from_f64(round_to(v.to_f64())), Radix::Dec, fmt);
    if v_rest.is_one() {
        return Some(format!("{value_text} ± {sigma_text}"));
    }
    let mut rest = String::new();
    write_expr(&mut rest, &v_rest, Prec::Mul, fmt);
    Some(format!("({value_text} ± {sigma_text}) {rest}"))
}

fn format_number(value: &Num, radix: Radix, fmt: &NumFormat) -> String {
    match radix {
        // A sig-figs tag keeps its typed decimal places — `2.50` stays
        // `2.50` — which is also how a rounded sig-figs result shows where
        // its digits stop meaning anything.
        Radix::Sig(decimals) if decimals > 0 => fixed_decimals(value, decimals as usize, fmt),
        Radix::Dec | Radix::Sig(_) => value.format(fmt),
        Radix::Hex => value
            .to_bigint()
            .map(|v| format!("0x{v:X}"))
            .unwrap_or_else(|| value.format(fmt)),
        Radix::Oct => value
            .to_bigint()
            .map(|v| format!("0o{v:o}"))
            .unwrap_or_else(|| value.format(fmt)),
        Radix::Bin => value
            .to_bigint()
            .map(|v| format!("0b{v:b}"))
            .unwrap_or_else(|| value.format(fmt)),
    }
}

/// A value at a fixed number of decimal places, honouring the separators
/// and grouping of the ambient format.
fn fixed_decimals(value: &Num, decimals: usize, fmt: &NumFormat) -> String {
    let text = format!("{:.*}", decimals, value.to_f64());
    let (integer, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let (sign, digits) = match integer.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", integer),
    };
    let grouped = if fmt.grouping && digits.len() > 3 {
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push(fmt.group_sep);
            }
            out.push(c);
        }
        out
    } else {
        digits.to_string()
    };
    if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}{}{fraction}", fmt.decimal_sep)
    }
}

// ---------------------------------------------------------------------------
// Sums: reconstruct subtraction
// ---------------------------------------------------------------------------

fn write_sum(out: &mut String, terms: &[Expr], fmt: &NumFormat) {
    if terms.is_empty() {
        out.push('0');
        return;
    }
    for (i, term) in terms.iter().enumerate() {
        // A term whose numeric coefficient is negative renders as a
        // subtraction of its absolute value.
        let (negative, magnitude) = split_sign(term);
        if i == 0 {
            if negative {
                out.push('-');
            }
        } else if negative {
            out.push_str(" - ");
        } else {
            out.push_str(" + ");
        }
        if needs_explicit_one(&magnitude, fmt) {
            out.push_str("1 ");
        }
        write_expr(out, &magnitude, Prec::Add, fmt);
    }
}

/// A term made only of units carries an implicit coefficient of 1 that is
/// worth printing when it sits beside other terms: `1 hr + 1 mins + 1 s`.
fn needs_explicit_one(term: &Expr, fmt: &NumFormat) -> bool {
    if !mentions_unit(term, fmt) {
        return false;
    }
    match term {
        Expr::Var(_) => true,
        Expr::Mul(factors) => !factors.iter().any(|f| matches!(f, Expr::Num(..))),
        Expr::Pow(..) => true,
        _ => false,
    }
}

/// Splits a leading minus out of a term, returning the positive remainder.
fn split_sign(term: &Expr) -> (bool, Expr) {
    match term {
        Expr::Num(value, radix) if value.is_negative() => {
            (true, Expr::Num(value.neg(), *radix))
        }
        Expr::Mul(factors) => {
            let mut factors = factors.clone();
            for factor in factors.iter_mut() {
                if let Expr::Num(value, radix) = factor {
                    if value.is_negative() {
                        *factor = Expr::Num(value.neg(), *radix);
                        let rebuilt = if factors.len() == 1 {
                            factors.pop().unwrap()
                        } else {
                            Expr::Mul(factors)
                        };
                        return (true, rebuilt);
                    }
                    break;
                }
            }
            (false, term.clone())
        }
        _ => (false, term.clone()),
    }
}

// ---------------------------------------------------------------------------
// Products: reconstruct division, and place units
// ---------------------------------------------------------------------------

/// True for names that attach directly to a number with no space: `2i`,
/// `45°`, `80%`. A letter — Latin or not — is a unit name and takes a
/// space instead: `2 Ω`.
fn is_tight_symbol(name: &str) -> bool {
    if name == "i" {
        return true;
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => !c.is_alphabetic(),
        _ => false,
    }
}

fn is_leading_symbol(name: &str) -> bool {
    LEADING_SYMBOLS.contains(&name)
}

/// Whether an expression names a unit anywhere in it.
fn mentions_unit(expr: &Expr, fmt: &NumFormat) -> bool {
    match expr {
        Expr::Var(name) => fmt.units.contains(name),
        Expr::Pow(base, _) => mentions_unit(base, fmt),
        Expr::Mul(items) | Expr::Add(items) => items.iter().any(|i| mentions_unit(i, fmt)),
        _ => false,
    }
}

fn write_product(out: &mut String, factors: &[Expr], fmt: &NumFormat) {
    if factors.is_empty() {
        out.push('1');
        return;
    }

    // Split into numerator and denominator by the sign of each exponent.
    let mut numerator: Vec<Expr> = Vec::new();
    let mut denominator: Vec<Expr> = Vec::new();
    for factor in factors {
        match factor {
            Expr::Pow(base, exp) => match exp.as_num() {
                Some(e) if e.is_negative() => {
                    if e.eq_num(&Num::from_i64(-1)) {
                        denominator.push((**base).clone());
                    } else {
                        denominator.push(Expr::Pow(base.clone(), Box::new(Expr::Num(e.neg(), Radix::Dec))));
                    }
                }
                _ => numerator.push(factor.clone()),
            },
            other => numerator.push(other.clone()),
        }
    }

    // A leading currency symbol moves in front of the coefficient: `$1,550`.
    let leading = numerator.iter().position(|f| {
        matches!(f, Expr::Var(name)
            if is_leading_symbol(currency_symbol(name).unwrap_or(name)))
    });
    let currency = leading.map(|i| match numerator.remove(i) {
        Expr::Var(name) => currency_symbol(&name).unwrap_or(&name).to_string(),
        _ => unreachable!(),
    });

    // Fold every numeric factor into a single coefficient. Unsimplified trees
    // reach the formatter (error paths, partial evaluation), so we cannot
    // assume the simplifier already merged them.
    let mut coefficient: Option<Expr> = None;
    let mut product = Num::one();
    let mut saw_number = false;
    let mut numbers = 0usize;
    let mut sole_style = Radix::Dec;
    numerator.retain(|factor| match factor {
        Expr::Num(value, style @ (Radix::Dec | Radix::Sig(_))) => {
            product = product.mul(value);
            saw_number = true;
            numbers += 1;
            sole_style = *style;
            false
        }
        _ => true,
    });
    // A lone number keeps its written style — the sig-figs tag on a rounded
    // `4.0 m` survives — while a folded product is plain decimal.
    let folded_style = if numbers == 1 { sole_style } else { Radix::Dec };
    if saw_number && !product.is_one() {
        coefficient = Some(Expr::Num(product.clone(), folded_style));
    } else if saw_number && numerator.is_empty() && denominator.is_empty() {
        coefficient = Some(Expr::Num(product.clone(), folded_style));
    } else if saw_number && product.is_one() {
        // `1*x` renders as plain `x`.
    }
    // Radix-tagged literals keep their own notation and are never folded.
    if coefficient.is_none() {
        if let Some(i) = numerator.iter().position(|f| matches!(f, Expr::Num(..))) {
            coefficient = Some(numerator.remove(i));
        }
    }

    // Whether this product measures something, which decides if a magnitude of
    // exactly one is worth printing.
    let names_a_unit = numerator
        .iter()
        .chain(denominator.iter())
        .any(|f| mentions_unit(f, fmt));

    let mut body = String::new();
    if let Some(coefficient) = &coefficient {
        let text = match coefficient {
            Expr::Num(value, radix) => format_number(value, *radix, fmt),
            other => render_with(other, fmt),
        };
        // `1 x` reads as `x`; `-1 x` as `-x`. But a unit keeps its
        // coefficient — `1 hr` and `1 kg*m/s^2` would otherwise lose the
        // number entirely.
        let is_one = coefficient.as_num().map(|n| n.abs().is_one()).unwrap_or(false)
            && !names_a_unit;
        if is_one && !numerator.is_empty() {
            if coefficient.as_num().map(|n| n.is_negative()).unwrap_or(false) {
                body.push('-');
            }
        } else {
            body.push_str(&text);
            if !numerator.is_empty() {
                let tight = matches!(&numerator[0], Expr::Var(name)
                    if is_tight_symbol(currency_symbol(name).unwrap_or(name)));
                // A half power prints as `sqrt(x)`, which reads as a call and
                // wants a `*`, not a space.
                let is_sqrt = matches!(&numerator[0], Expr::Pow(_, exp)
                    if exp.as_num().map(|e| e.eq_num(&Num::ratio(1, 2))).unwrap_or(false));
                let simple = !is_sqrt && matches!(&numerator[0], Expr::Var(_) | Expr::Pow(..));
                if tight {
                    // no separator
                } else if simple {
                    body.push(' ');
                } else {
                    body.push('*');
                }
            }
        }
    }

    for (i, factor) in numerator.iter().enumerate() {
        if i > 0 {
            body.push('*');
        }
        write_expr(&mut body, factor, Prec::Mul, fmt);
    }

    if body.is_empty() {
        body.push('1');
    } else if coefficient.is_none()
        && names_a_unit
        && !numerator.iter().any(|f| matches!(f, Expr::Range(..)))
    {
        // A quantity of exactly one still has a magnitude, and dropping it
        // makes a conversion look like it failed: `760 torr in atm` should
        // answer `1 atm`, not `atm`. An interval already carries its
        // magnitude: `(1..2)*m` takes no leading one.
        body.insert_str(0, "1 ");
    }

    if let Some(currency) = currency {
        out.push_str(&currency);
        // `-$5` rather than `$-5`.
        if let Some(stripped) = body.strip_prefix('-') {
            out.insert(0, '-');
            out.push_str(stripped);
        } else {
            out.push_str(&body);
        }
    } else {
        out.push_str(&body);
    }

    for (i, factor) in denominator.iter().enumerate() {
        out.push('/');
        // Only the first divisor may be bare; later ones would re-associate.
        let _ = i;
        write_expr(out, factor, Prec::Pow, fmt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_expr;

    fn round_trip(src: &str) {
        let once = render(&parse_expr(src));
        let twice = render(&parse_expr(&once));
        assert_eq!(once, twice, "not stable for {src:?}: {once:?} -> {twice:?}");
    }

    #[test]
    fn reconstructs_subtraction_and_division() {
        assert_eq!(render(&parse_expr("a - b")), "a - b");
        assert_eq!(render(&parse_expr("a / b")), "a/b");
        assert_eq!(render(&parse_expr("-a + b")), "-a + b");
        assert_eq!(render(&parse_expr("a*t + v0")), "a*t + v0");
    }

    #[test]
    fn units_take_a_space_but_symbols_do_not() {
        assert_eq!(render(&parse_expr("15 things")), "15 things");
        assert_eq!(render(&parse_expr("45°")), "45°");
        assert_eq!(render(&parse_expr("80%")), "80%");
        assert_eq!(render(&parse_expr("2i")), "2i");
        assert_eq!(render(&parse_expr("2Ω")), "2 Ω");
    }

    #[test]
    fn leading_currency_symbols_precede_the_amount() {
        assert_eq!(render(&parse_expr("$1550")), "$1,550");
        assert_eq!(render(&parse_expr("-$100")), "-$100");
        assert_eq!(render(&parse_expr("$150.4339/day")), "$150.4339/day");
    }

    #[test]
    fn parenthesizes_only_where_needed() {
        assert_eq!(render(&parse_expr("(a + 1)*(a + 2)")), "(a + 1)*(a + 2)");
        assert_eq!(render(&parse_expr("a^-3")), "a^-3");
        assert_eq!(render(&parse_expr("a^-b")), "a^(-b)");
        assert_eq!(render(&parse_expr("2*(3 + x)")), "2*(3 + x)");
    }

    #[test]
    fn output_reparses_to_the_same_thing() {
        for src in [
            "2x/3y",
            "-4.9*m*t^2/s^2 + 100*m*t/s + 490*m",
            "if x + y < 0 then -x - y else x + y",
            "[1, 2; 3, 4]",
            "|foo|^2 + |bar|^2",
            "let x = point[0] in x*ca - y*sa",
            "100 ft in m",
            "5 ft + 4 in",
            "9.95..10.05",
            "!a || (b <= 0)",
            "sqrt(b^2 - 4*a*c)",
            "g(g(1, 2), 3)",
            "0xCC",
            "1/2i",
        ] {
            round_trip(src);
        }
    }

    #[test]
    fn radix_literals_keep_their_notation() {
        assert_eq!(render(&parse_expr("0xCC")), "0xCC");
        assert_eq!(render(&parse_expr("0b101")), "0b101");
        assert_eq!(render(&parse_expr("0o310")), "0o310");
    }

    #[test]
    fn conversion_round_trips_without_becoming_inches() {
        // The parser must read `100 ft in m` back as a conversion, not as
        // `100*ft + ... in`.
        let rendered = render(&parse_expr("100 ft in m"));
        assert!(matches!(parse_expr(&rendered), Expr::Convert(..)));
    }
}
