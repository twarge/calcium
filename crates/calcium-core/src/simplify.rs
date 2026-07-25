//! Canonicalization and algebraic simplification.
//!
//! `simplify` turns a tree into a canonical sum-of-products: numeric factors
//! folded into one coefficient, like terms collected, factors sorted. This is
//! what makes `10*(x + 0.5x)` become `15 x` and `x + 2x + 4x = 42` solvable.
//!
//! Complex numbers are *not* a separate numeric type. `i` is an ordinary
//! symbol plus the rewrite `i^2 -> -1`, which is enough to make `i*i => -1`,
//! `i^43 => -i` and `(1+2i)*i => i - 2` all fall out of the same machinery
//! that handles units.

use crate::ast::*;
use crate::format::render;
use crate::lexer::Radix;
use crate::num::Num;
use std::collections::HashMap;

pub fn simplify(expr: &Expr) -> Expr {
    match expr {
        Expr::Add(terms) => simplify_sum(terms),
        Expr::Mul(factors) => simplify_product(factors),
        Expr::Pow(base, exp) => simplify_power(&simplify(base), &simplify(exp)),

        Expr::Cmp(op, a, b) => simplify_compare(*op, &simplify(a), &simplify(b)),
        Expr::Not(inner) => match simplify(inner) {
            Expr::Bool(b) => Expr::Bool(!b),
            // Numeric operands give numeric results, matching `&&` and `||`:
            // `!0 => 1`, `!1 => 0`.
            Expr::Num(n, _) => Expr::num(if n.is_zero() { 1 } else { 0 }),
            // De Morgan, which the Reference calls out explicitly.
            Expr::Logic(LogicOp::And, a, b) => simplify(&Expr::Logic(
                LogicOp::Or,
                Box::new(Expr::Not(a)),
                Box::new(Expr::Not(b)),
            )),
            Expr::Logic(LogicOp::Or, a, b) => simplify(&Expr::Logic(
                LogicOp::And,
                Box::new(Expr::Not(a)),
                Box::new(Expr::Not(b)),
            )),
            Expr::Cmp(op, a, b) => Expr::Cmp(negate_cmp(op), a, b),
            Expr::Not(inner) => *inner,
            other => Expr::Not(Box::new(other)),
        },
        Expr::Logic(op, a, b) => simplify_logic(*op, &simplify(a), &simplify(b)),
        Expr::Bit(op, a, b) => simplify_bitwise(*op, &simplify(a), &simplify(b)),
        Expr::Mod(a, b) => {
            let (a, b) = (simplify(a), simplify(b));
            match (a.as_num(), b.as_num()) {
                (Some(x), Some(y)) => Expr::Num(x.modulo(y), Radix::Dec),
                _ => Expr::Mod(Box::new(a), Box::new(b)),
            }
        }

        Expr::If(cond, then_branch, else_branch) => {
            let cond = simplify(cond);
            match cond {
                Expr::Bool(true) => simplify(then_branch),
                Expr::Bool(false) => simplify(else_branch),
                Expr::Num(ref n, _) if !n.is_zero() => simplify(then_branch),
                Expr::Num(_, _) => simplify(else_branch),
                other => Expr::If(
                    Box::new(other),
                    Box::new(simplify(then_branch)),
                    Box::new(simplify(else_branch)),
                ),
            }
        }

        Expr::Abs(inner) => simplify_abs(&simplify(inner)),
        Expr::Transpose(inner) => match simplify(inner) {
            Expr::Matrix(rows) => Expr::Matrix(transpose(&rows)),
            other => Expr::Transpose(Box::new(other)),
        },
        Expr::Matrix(rows) => Expr::Matrix(
            rows.iter()
                .map(|row| row.iter().map(simplify).collect())
                .collect(),
        ),
        Expr::Range(lo, hi) => Expr::Range(Box::new(simplify(lo)), Box::new(simplify(hi))),
        Expr::Norm(inner, p) => Expr::Norm(
            Box::new(simplify(inner)),
            p.as_ref().map(|p| Box::new(simplify(p))),
        ),
        Expr::Relation(a, b) => Expr::Relation(Box::new(simplify(a)), Box::new(simplify(b))),

        // Leaves and forms the simplifier does not rewrite.
        other => other.clone(),
    }
}

fn negate_cmp(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Lt => CmpOp::Ge,
        CmpOp::Ge => CmpOp::Lt,
        CmpOp::Gt => CmpOp::Le,
        CmpOp::Le => CmpOp::Gt,
        CmpOp::Eq => CmpOp::Ne,
        CmpOp::Ne => CmpOp::Eq,
    }
}

// ---------------------------------------------------------------------------
// Sums
// ---------------------------------------------------------------------------

/// Splits a term into its numeric coefficient and its symbolic remainder.
/// `-4.9 m*t^2` becomes `(-4.9, m*t^2)`.
fn split_coefficient(term: &Expr) -> (Num, Expr) {
    match term {
        Expr::Num(value, _) => (value.clone(), Expr::num(1)),
        Expr::Mul(factors) => {
            let mut coefficient = Num::one();
            let mut rest = Vec::new();
            for factor in factors {
                match factor {
                    Expr::Num(value, _) => coefficient = coefficient.mul(value),
                    other => rest.push(other.clone()),
                }
            }
            (coefficient, Expr::mul(rest))
        }
        other => (Num::one(), other.clone()),
    }
}

/// The notation a result should carry: any non-decimal literal in the
/// expression wins, so `0xFF + 1` answers `0x100`.
fn radix_of_terms<'a>(terms: impl Iterator<Item = &'a Expr>) -> Radix {
    for term in terms {
        let found = match term {
            Expr::Num(_, style) => *style,
            Expr::Mul(factors) => radix_of_terms(factors.iter()),
            _ => Radix::Dec,
        };
        if found != Radix::Dec {
            return found;
        }
    }
    Radix::Dec
}

fn simplify_sum(terms: &[Expr]) -> Expr {
    let mut flat = Vec::new();
    for term in terms {
        match simplify(term) {
            Expr::Add(inner) => flat.extend(inner),
            other => flat.push(other),
        }
    }

    // Ranges double as intervals, so a sum of them is interval addition.
    if flat.iter().any(|t| matches!(t, Expr::Range(..))) {
        if let Some(result) = interval_sum(&flat) {
            return result;
        }
    }

    // Matrices add element-wise, and cannot be mixed into scalar collection.
    if flat.iter().any(|t| matches!(t, Expr::Matrix(_))) {
        return sum_matrices(flat);
    }

    let radix = radix_of_terms(flat.iter());
    let mut constant = Num::zero();
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, (Num, Expr)> = HashMap::new();

    for term in flat {
        let (coefficient, rest) = split_coefficient(&term);
        if rest.is_one() {
            constant = constant.add(&coefficient);
            continue;
        }
        let key = render(&rest);
        match buckets.get_mut(&key) {
            Some((total, _)) => *total = total.add(&coefficient),
            None => {
                order.push(key.clone());
                buckets.insert(key, (coefficient, rest));
            }
        }
    }

    // Deterministic output order: symbolic terms sorted by key, constant last.
    // Any stable order will do; what matters is that recomputing a document
    // never reshuffles an answer.
    order.sort();

    let mut out = Vec::new();
    for key in order {
        let (coefficient, rest) = buckets.remove(&key).unwrap();
        if coefficient.is_zero() {
            continue;
        }
        out.push(attach_coefficient(coefficient, rest));
    }
    if !constant.is_zero() || out.is_empty() {
        out.push(Expr::Num(constant, radix));
    }
    Expr::add(out)
}

// ---------------------------------------------------------------------------
// Interval arithmetic
//
// The Reference uses ranges to carry measurement error: a room measured as
// `(10 - 5/100)..(10 + 5/100)` has area `99.0025..101.0025`.
// ---------------------------------------------------------------------------

/// A numeric interval, widening a plain number into a degenerate one.
fn as_interval(expr: &Expr) -> Option<(Num, Num)> {
    match expr {
        Expr::Range(lo, hi) => Some((lo.as_num()?.clone(), hi.as_num()?.clone())),
        Expr::Num(value, _) => Some((value.clone(), value.clone())),
        _ => None,
    }
}

fn interval(lo: Num, hi: Num) -> Expr {
    if lo.eq_num(&hi) {
        return Expr::Num(lo, Radix::Dec);
    }
    let flipped = lo.cmp_num(&hi) == Some(std::cmp::Ordering::Greater);
    let (lo, hi) = if flipped { (hi, lo) } else { (lo, hi) };
    Expr::Range(
        Box::new(Expr::Num(lo, Radix::Dec)),
        Box::new(Expr::Num(hi, Radix::Dec)),
    )
}

fn interval_sum(terms: &[Expr]) -> Option<Expr> {
    let mut lo = Num::zero();
    let mut hi = Num::zero();
    for term in terms {
        let (a, b) = as_interval(term)?;
        lo = lo.add(&a);
        hi = hi.add(&b);
    }
    Some(interval(lo, hi))
}

fn interval_product(factors: &[Expr]) -> Option<Expr> {
    let mut lo = Num::one();
    let mut hi = Num::one();
    for factor in factors {
        let (a, b) = as_interval(factor)?;
        // The extremes of a product live at the corners.
        let corners = [lo.mul(&a), lo.mul(&b), hi.mul(&a), hi.mul(&b)];
        lo = corners.iter().cloned().reduce(min_num)?;
        hi = corners.iter().cloned().reduce(max_num)?;
    }
    Some(interval(lo, hi))
}

fn min_num(a: Num, b: Num) -> Num {
    if a.cmp_num(&b) == Some(std::cmp::Ordering::Greater) {
        b
    } else {
        a
    }
}

fn max_num(a: Num, b: Num) -> Num {
    if a.cmp_num(&b) == Some(std::cmp::Ordering::Less) {
        b
    } else {
        a
    }
}

fn attach_coefficient(coefficient: Num, rest: Expr) -> Expr {
    if coefficient.is_one() {
        rest
    } else {
        Expr::mul(vec![Expr::Num(coefficient, Radix::Dec), rest])
    }
}

fn sum_matrices(terms: Vec<Expr>) -> Expr {
    let mut total: Option<Vec<Vec<Expr>>> = None;
    let mut leftovers = Vec::new();
    for term in terms {
        match term {
            Expr::Matrix(rows) => match &mut total {
                None => total = Some(rows),
                Some(acc) => {
                    if acc.len() != rows.len() || acc[0].len() != rows[0].len() {
                        return Expr::Error("matrix dimensions do not match".to_string());
                    }
                    for (r, row) in rows.into_iter().enumerate() {
                        for (c, cell) in row.into_iter().enumerate() {
                            acc[r][c] = simplify(&Expr::add(vec![acc[r][c].clone(), cell]));
                        }
                    }
                }
            },
            other => leftovers.push(other),
        }
    }
    let Some(mut acc) = total else {
        return Expr::add(leftovers);
    };
    // A scalar added to a matrix broadcasts element-wise.
    for leftover in leftovers {
        for row in acc.iter_mut() {
            for cell in row.iter_mut() {
                *cell = simplify(&Expr::add(vec![cell.clone(), leftover.clone()]));
            }
        }
    }
    Expr::Matrix(acc)
}

// ---------------------------------------------------------------------------
// Products
// ---------------------------------------------------------------------------

fn simplify_product(factors: &[Expr]) -> Expr {
    let mut flat = Vec::new();
    for factor in factors {
        match simplify(factor) {
            Expr::Mul(inner) => flat.extend(inner),
            other => flat.push(other),
        }
    }

    if flat.iter().any(|f| matches!(f, Expr::Range(..))) {
        if let Some(result) = interval_product(&flat) {
            return result;
        }
    }

    if flat.iter().any(|f| matches!(f, Expr::Matrix(_))) {
        return multiply_matrices(flat);
    }

    let mut coefficient = Num::one();
    let mut order: Vec<String> = Vec::new();
    let mut powers: HashMap<String, (Expr, Expr)> = HashMap::new();
    // A hex or binary literal takes part in arithmetic like any other number,
    // but its notation infects the result: `round(f*0xFF)` answers in hex.
    let mut radix = Radix::Dec;

    for factor in flat {
        match factor {
            // Infinities go through power collection rather than the
            // coefficient, so `∞ * ∞^-1` cancels to 1 instead of folding to
            // `inf * 0 = NaN`. `∞ / ∞` answers 1.
            Expr::Num(value, _) if !value.is_finite_number() => {
                add_power(&mut order, &mut powers, Expr::Num(value, Radix::Dec), Expr::num(1));
            }
            Expr::Num(value, style) => {
                if style != Radix::Dec {
                    radix = style;
                }
                coefficient = coefficient.mul(&value);
            }
            Expr::Pow(base, exp) => add_power(&mut order, &mut powers, *base, *exp),
            other => add_power(&mut order, &mut powers, other, Expr::num(1)),
        }
    }
    let opaque: Vec<Expr> = Vec::new();

    if coefficient.is_zero() {
        return Expr::num(0);
    }

    // `i^n` collapses on a period of four.
    if let Some((_, exponent)) = powers.get("i") {
        if let Some(n) = exponent.as_num().and_then(|e| e.to_i64()) {
            let (factor, remainder) = reduce_imaginary(n);
            coefficient = coefficient.mul(&factor);
            powers.remove("i");
            order.retain(|k| k != "i");
            if remainder != 0 {
                add_power(&mut order, &mut powers, Expr::var("i"), Expr::num(remainder));
            }
        }
    }

    order.sort();
    let mut rebuilt = Vec::new();
    for key in &order {
        let Some((base, exponent)) = powers.remove(key) else {
            continue;
        };
        if exponent.is_zero() {
            continue;
        }
        if exponent.is_one() {
            rebuilt.push(base);
        } else {
            rebuilt.push(Expr::Pow(Box::new(base), Box::new(exponent)));
        }
    }
    rebuilt.extend(opaque);

    // Two or more sums: expanding is only worth it when the result collapses.
    // (See `expand_if_it_collapses`.)
    // `(2 + 3i)*(2 - 3i)` becomes the single term `13`, but `(a + 1)*(a + 2)`
    // would grow to three terms, and the Reference shows
    // `(a + 1)*(a + 2)*(a + 3)` staying factored. So we try the expansion and
    // keep it only if it is no longer than the widest factor.
    let sum_count = rebuilt.iter().filter(|f| matches!(f, Expr::Add(_))).count();
    if sum_count > 1 {
        let widest = rebuilt
            .iter()
            .map(|f| match f {
                Expr::Add(terms) => terms.len(),
                _ => 1,
            })
            .max()
            .unwrap_or(1);
        let mut factors = rebuilt.clone();
        factors.push(Expr::Num(coefficient.clone(), radix));
        if let Some(collapsed) = expand_if_it_collapses(&factors, widest) {
            return collapsed;
        }
    }

    // Exactly one sum: distribute into it. `2*(3 + x)` becomes `2x + 6`, and
    // `(2i + 1)*i` becomes `i - 2`.
    if sum_count == 1 && (rebuilt.len() > 1 || !coefficient.is_one()) {
        let i = rebuilt
            .iter()
            .position(|f| matches!(f, Expr::Add(_)))
            .unwrap();
        let Expr::Add(terms) = rebuilt.remove(i) else {
            unreachable!()
        };
        let mut multipliers = rebuilt;
        if !coefficient.is_one() {
            multipliers.push(Expr::Num(coefficient, Radix::Dec));
        }
        let distributed: Vec<Expr> = terms
            .into_iter()
            .map(|term| {
                let mut parts = multipliers.clone();
                parts.push(term);
                Expr::mul(parts)
            })
            .collect();
        return simplify_sum(&distributed);
    }

    if !coefficient.is_one() || rebuilt.is_empty() {
        rebuilt.insert(0, Expr::Num(coefficient, radix));
    }
    Expr::mul(rebuilt)
}

/// Multiplies a list of factors out, keeping the result only if it is no wider
/// than `widest` terms. That is what lets `(2 + 3i)*(2 - 3i)` collapse to `13`
/// and `(1 + i)^2` to `2i`, while `(a + 1)*(a + 2)*(a + 3)` stays factored.
fn expand_if_it_collapses(factors: &[Expr], widest: usize) -> Option<Expr> {
    const LIMIT: usize = 64;
    let mut expanded = vec![Expr::num(1)];
    for factor in factors {
        let terms: Vec<Expr> = match factor {
            Expr::Add(terms) => terms.clone(),
            other => vec![other.clone()],
        };
        let mut next = Vec::with_capacity(expanded.len() * terms.len());
        for partial in &expanded {
            for term in &terms {
                next.push(Expr::mul(vec![partial.clone(), term.clone()]));
            }
        }
        if next.len() > LIMIT {
            return None;
        }
        expanded = next;
    }
    let collapsed = simplify_sum(&expanded);
    let width = match &collapsed {
        Expr::Add(terms) => terms.len(),
        _ => 1,
    };
    (width <= widest).then_some(collapsed)
}

fn add_power(
    order: &mut Vec<String>,
    powers: &mut HashMap<String, (Expr, Expr)>,
    base: Expr,
    exponent: Expr,
) {
    let key = render(&base);
    match powers.get_mut(&key) {
        Some((_, total)) => {
            *total = simplify(&Expr::add(vec![total.clone(), exponent]));
        }
        None => {
            order.push(key.clone());
            powers.insert(key, (base, exponent));
        }
    }
}

/// `i^n` for integer `n`, as a numeric factor times `i^remainder`.
fn reduce_imaginary(n: i64) -> (Num, i64) {
    match n.rem_euclid(4) {
        0 => (Num::one(), 0),
        1 => (Num::one(), 1),
        2 => (Num::from_i64(-1), 0),
        _ => (Num::from_i64(-1), 1),
    }
}

fn multiply_matrices(factors: Vec<Expr>) -> Expr {
    let mut scalars = Vec::new();
    let mut matrices = Vec::new();
    for factor in factors {
        match factor {
            Expr::Matrix(rows) => matrices.push(rows),
            other => scalars.push(other),
        }
    }
    let mut acc = matrices.remove(0);
    for next in matrices {
        match matmul(&acc, &next) {
            Ok(product) => acc = product,
            Err(message) => return Expr::Error(message),
        }
    }
    if !scalars.is_empty() {
        let scale = Expr::mul(scalars);
        for row in acc.iter_mut() {
            for cell in row.iter_mut() {
                *cell = simplify(&Expr::mul(vec![cell.clone(), scale.clone()]));
            }
        }
    }
    Expr::Matrix(acc)
}

pub fn matmul(a: &[Vec<Expr>], b: &[Vec<Expr>]) -> Result<Vec<Vec<Expr>>, String> {
    let inner = a[0].len();
    if inner != b.len() {
        return Err(format!(
            "cannot multiply a {}x{} matrix by a {}x{} one",
            a.len(),
            inner,
            b.len(),
            b[0].len()
        ));
    }
    let mut out = vec![vec![Expr::num(0); b[0].len()]; a.len()];
    for (r, out_row) in out.iter_mut().enumerate() {
        for (c, cell) in out_row.iter_mut().enumerate() {
            let terms: Vec<Expr> = (0..inner)
                .map(|k| Expr::mul(vec![a[r][k].clone(), b[k][c].clone()]))
                .collect();
            *cell = simplify(&Expr::add(terms));
        }
    }
    Ok(out)
}

pub fn transpose(rows: &[Vec<Expr>]) -> Vec<Vec<Expr>> {
    if rows.is_empty() {
        return Vec::new();
    }
    (0..rows[0].len())
        .map(|c| rows.iter().map(|row| row[c].clone()).collect())
        .collect()
}

// ---------------------------------------------------------------------------
// Powers
// ---------------------------------------------------------------------------

fn simplify_power(base: &Expr, exp: &Expr) -> Expr {
    if exp.is_zero() {
        return Expr::num(1);
    }
    if exp.is_one() {
        return base.clone();
    }
    if let (Some(b), Some(e)) = (base.as_num(), exp.as_num()) {
        // Leave infinities symbolic so `∞ * ∞^-1` can cancel in the product
        // collector rather than folding to `inf * 0`.
        if !b.is_finite_number() {
            return Expr::Pow(Box::new(base.clone()), Box::new(exp.clone()));
        }
        // A negative base with a fractional exponent leaves the reals; the
        // square-root case comes back as an imaginary instead.
        if b.is_negative() && !e.is_integer() {
            if e.eq_num(&Num::ratio(1, 2)) {
                let magnitude = b.neg().pow(&Num::ratio(1, 2));
                return simplify(&Expr::mul(vec![
                    Expr::Num(magnitude, Radix::Dec),
                    Expr::var("i"),
                ]));
            }
        } else {
            return Expr::Num(b.pow(e), Radix::Dec);
        }
    }
    // `i^n` folds even when it is not inside a product.
    if matches!(base, Expr::Var(name) if name == "i") {
        if let Some(n) = exp.as_num().and_then(|e| e.to_i64()) {
            let (factor, remainder) = reduce_imaginary(n);
            let mut parts = vec![Expr::Num(factor, Radix::Dec)];
            if remainder != 0 {
                parts.push(Expr::var("i"));
            }
            return simplify(&Expr::mul(parts));
        }
    }
    if let (Expr::Range(..), Some(power)) = (base, exp.as_num().and_then(|e| e.to_i64())) {
        if (0..=64).contains(&power) {
            let repeated = vec![base.clone(); power as usize];
            if let Some(result) = interval_product(&repeated) {
                return result;
            }
        }
    }
    if let (Expr::Add(terms), Some(power)) = (base, exp.as_num().and_then(|e| e.to_i64())) {
        if (2..=8).contains(&power) {
            if let Some(collapsed) = expand_if_it_collapses(
                &vec![base.clone(); power as usize],
                terms.len(),
            ) {
                return collapsed;
            }
        }
    }
    match base {
        // `(a^b)^c` is `a^(b*c)`.
        Expr::Pow(inner_base, inner_exp) => simplify_power(
            inner_base,
            &simplify(&Expr::mul(vec![(**inner_exp).clone(), exp.clone()])),
        ),
        // A numeric power distributes over a product so units come apart:
        // `(m/s)^2` is `m^2/s^2`.
        Expr::Mul(factors) if exp.as_num().is_some() => {
            let raised: Vec<Expr> = factors
                .iter()
                .map(|f| Expr::Pow(Box::new(f.clone()), Box::new(exp.clone())))
                .collect();
            simplify(&Expr::mul(raised))
        }
        _ => Expr::Pow(Box::new(base.clone()), Box::new(exp.clone())),
    }
}

// ---------------------------------------------------------------------------
// Comparison, logic, absolute value
// ---------------------------------------------------------------------------

fn simplify_compare(op: CmpOp, a: &Expr, b: &Expr) -> Expr {
    // `[1, 6; 3, 8] < [5, 2; 7, 4]` compares element by element.
    if let (Expr::Matrix(left), Expr::Matrix(right)) = (a, b) {
        if left.len() == right.len() && left[0].len() == right[0].len() {
            let compared = left
                .iter()
                .zip(right)
                .map(|(lrow, rrow)| {
                    lrow.iter()
                        .zip(rrow)
                        .map(|(x, y)| simplify_compare(op, x, y))
                        .collect()
                })
                .collect();
            return Expr::Matrix(compared);
        }
    }
    // Move a lone constant across the comparison: `a - 2 < 0` becomes `a < 2`.
    if b.is_zero() {
        if let Expr::Add(terms) = a {
            let constant: Vec<&Expr> = terms.iter().filter(|t| t.as_num().is_some()).collect();
            if constant.len() == 1 && terms.len() > 1 {
                let shift = constant[0].as_num().unwrap().neg();
                let rest: Vec<Expr> = terms
                    .iter()
                    .filter(|t| t.as_num().is_none())
                    .cloned()
                    .collect();
                return Expr::Cmp(
                    op,
                    Box::new(simplify(&Expr::add(rest))),
                    Box::new(Expr::Num(shift, Radix::Dec)),
                );
            }
        }
    }
    if let (Some(x), Some(y)) = (a.as_num(), b.as_num()) {
        if let Some(ordering) = x.cmp_num(y) {
            use std::cmp::Ordering::*;
            let truth = match op {
                CmpOp::Lt => ordering == Less,
                CmpOp::Gt => ordering == Greater,
                CmpOp::Le => ordering != Greater,
                CmpOp::Ge => ordering != Less,
                CmpOp::Eq => ordering == Equal,
                CmpOp::Ne => ordering != Equal,
            };
            return Expr::Bool(truth);
        }
    }
    // Structural equality decides the symbolic cases the numbers cannot.
    if matches!(op, CmpOp::Eq | CmpOp::Ne) && a == b {
        return Expr::Bool(op == CmpOp::Eq);
    }
    if matches!(op, CmpOp::Eq | CmpOp::Ne) {
        let difference = simplify(&Expr::sub(a.clone(), b.clone()));
        if let Some(value) = difference.as_num() {
            return Expr::Bool((op == CmpOp::Eq) == value.is_zero());
        }
    }
    Expr::Cmp(op, Box::new(a.clone()), Box::new(b.clone()))
}

/// Truthiness for the logical operators, which accept both booleans and the
/// numbers `1` and `0` (`1 && 1 => 1`, `true && false => false`).
pub fn truth_of(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::Bool(b) => Some(*b),
        Expr::Num(n, _) => Some(!n.is_zero()),
        _ => None,
    }
}

fn simplify_logic(op: LogicOp, a: &Expr, b: &Expr) -> Expr {
    // `&&` and `||` return one of their operands rather than a fresh boolean.
    // That single rule explains every case in the Reference: `true && false`
    // gives `false`, `1 && 0` gives `0`, and `0b0100 || 1` gives `0b100`
    // — notation and all.
    if let (Some(x), Some(_)) = (truth_of(a), truth_of(b)) {
        let short_circuits = match op {
            LogicOp::And => !x,
            LogicOp::Or => x,
        };
        return if short_circuits { a.clone() } else { b.clone() };
    }
    match (op, truth_of(a), truth_of(b)) {
        (LogicOp::And, Some(false), _) | (LogicOp::And, _, Some(false)) => Expr::Bool(false),
        (LogicOp::Or, Some(true), _) | (LogicOp::Or, _, Some(true)) => Expr::Bool(true),
        (LogicOp::And, Some(true), _) => b.clone(),
        (LogicOp::And, _, Some(true)) => a.clone(),
        (LogicOp::Or, Some(false), _) => b.clone(),
        (LogicOp::Or, _, Some(false)) => a.clone(),
        _ => Expr::Logic(op, Box::new(a.clone()), Box::new(b.clone())),
    }
}

fn simplify_bitwise(op: BitOp, a: &Expr, b: &Expr) -> Expr {
    if let (Some(x), Some(y)) = (
        a.as_num().and_then(|n| n.to_i64()),
        b.as_num().and_then(|n| n.to_i64()),
    ) {
        // Computer numbers are 32-bit, per the Reference.
        let (x, y) = (x as u32, y as u32);
        let result = match op {
            BitOp::And => x & y,
            BitOp::Or => x | y,
        };
        let radix = match (a, b) {
            (Expr::Num(_, Radix::Bin), _) | (_, Expr::Num(_, Radix::Bin)) => Radix::Bin,
            (Expr::Num(_, Radix::Hex), _) | (_, Expr::Num(_, Radix::Hex)) => Radix::Hex,
            _ => Radix::Dec,
        };
        // A zero result prints plainly, matching `0b0101 & 0b1010 => 0`.
        let radix = if result == 0 { Radix::Dec } else { radix };
        return Expr::Num(Num::from_i64(result as i64), radix);
    }
    Expr::Bit(op, Box::new(a.clone()), Box::new(b.clone()))
}

fn simplify_abs(inner: &Expr) -> Expr {
    match inner {
        Expr::Num(value, _) => Expr::Num(value.abs(), Radix::Dec),
        // `|m|` on a square matrix is the determinant; on a vector it is the
        // 2-norm. Both are handled in eval where the builtins live.
        Expr::Matrix(_) => Expr::Abs(Box::new(inner.clone())),
        // `|-x|` is `|x|`.
        Expr::Mul(factors) => {
            let mut coefficient = Num::one();
            let mut rest = Vec::new();
            for factor in factors {
                match factor {
                    Expr::Num(value, Radix::Dec) => coefficient = coefficient.mul(value),
                    other => rest.push(other.clone()),
                }
            }
            if coefficient.is_negative() || rest.len() != factors.len() {
                let magnitude = coefficient.abs();
                let inner = Expr::mul(rest);
                let wrapped = if inner.is_one() {
                    Expr::num(1)
                } else {
                    Expr::Abs(Box::new(inner))
                };
                return simplify(&Expr::mul(vec![
                    Expr::Num(magnitude, Radix::Dec),
                    wrapped,
                ]));
            }
            Expr::Abs(Box::new(inner.clone()))
        }
        _ => Expr::Abs(Box::new(inner.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_expr;

    fn s(src: &str) -> String {
        render(&simplify(&parse_expr(src)))
    }

    #[test]
    fn folds_arithmetic() {
        assert_eq!(s("1 + 2 * 3"), "7");
        assert_eq!(s("(1 + 2) * 3"), "9");
        assert_eq!(s("-22/7"), "-3.1429");
        assert_eq!(s("2^32 - 1"), "4,294,967,295");
        assert_eq!(s("2 mod 3"), "2");
        assert_eq!(s("12 mod 3"), "0");
    }

    #[test]
    fn collects_like_terms() {
        assert_eq!(s("10 * (x + 0.5x)"), "15 x");
        assert_eq!(s("x + 2x + 4x"), "7 x");
        assert_eq!(s("2*(3 + x)"), "2 x + 6");
        assert_eq!(s("2(3 + x)"), "2 x + 6");
        assert_eq!(s("x - x"), "0");
    }

    #[test]
    fn keeps_products_of_sums_factored() {
        // `prod(x + a, 1..3)` is documented to stay factored.
        assert_eq!(s("(a + 1)*(a + 2)*(a + 3)"), "(a + 1)*(a + 2)*(a + 3)");
    }

    #[test]
    fn collects_powers_of_the_same_base() {
        assert_eq!(s("x^2 * x^3"), "x^5");
        assert_eq!(s("x * x^-1"), "1");
        assert_eq!(s("(x^2)^3"), "x^6");
        assert_eq!(s("2x/3y"), "0.6667 x/y");
        assert_eq!(s("2*x/3*y"), "0.6667 x*y");
    }

    #[test]
    fn treats_i_as_a_symbol_with_one_rewrite_rule() {
        assert_eq!(s("i*i"), "-1");
        assert_eq!(s("i^2"), "-1");
        assert_eq!(s("i^43"), "-i");
        assert_eq!(s("(1+2i)*i"), "i - 2");
        assert_eq!(s("(5i)^2"), "-25");
        // The `sqrt` spelling is a builtin and is covered in eval; the power
        // form is what reaches the simplifier.
        assert_eq!(s("(-16)^0.5"), "4i");
    }

    #[test]
    fn units_are_just_symbols_that_collect() {
        // The whole unit system rests on this: `weeks` and `days` are ordinary
        // symbols, so unlike terms simply stay apart.
        assert_eq!(s("4 weeks + 2 weeks + 4 weeks"), "10 weeks");
        assert_eq!(s("6 days + 60 days"), "66 days");
        assert_eq!(s("4 weeks + 6 days"), "6 days + 4 weeks");
        assert_eq!(s("(m/s)^2"), "m^2/s^2");
    }

    #[test]
    fn applies_de_morgan() {
        assert_eq!(s("!(a && (b > 0))"), "!a || b <= 0");
        assert_eq!(s("!true"), "false");
    }

    #[test]
    fn evaluates_logic_and_comparison() {
        assert_eq!(s("true && false"), "false");
        assert_eq!(s("1 && 1"), "1");
        assert_eq!(s("1 && 0"), "0");
        assert_eq!(s("2 < 3"), "true");
        assert_eq!(s("3 <= 3"), "true");
        assert_eq!(s("2 == 3"), "false");
    }

    #[test]
    fn evaluates_bitwise_operations_in_the_input_radix() {
        assert_eq!(s("0b0101 | 0b1010"), "0b1111");
        assert_eq!(s("0b0101 & 0b1010"), "0");
        assert_eq!(s("0b0100 | 1"), "0b101");
        assert_eq!(s("0b0100 && 1"), "1");
    }

    #[test]
    fn simplifies_absolute_value() {
        assert_eq!(s("|-4|"), "4");
        assert_eq!(s("|-x|"), "|x|");
        assert_eq!(s("abs(-4)"), "abs(-4)"); // resolved later, in eval
    }

    #[test]
    fn multiplies_matrices() {
        assert_eq!(s("[1, 2, 3] * [1; 2; 3]"), "[14]");
        assert_eq!(s("[1, 2, 3] * 10"), "[10, 20, 30]");
        assert_eq!(s("[1, 6; 3, 8] + [5, 2; 7, 4]"), "[6, 8; 10, 12]");
        assert_eq!(s("[1, 2; 3, 4]^T"), "[1, 3; 2, 4]");
        assert_eq!(
            s("[1; 2; 3] * [1, 2, 3]"),
            "[1, 2, 3; 2, 4, 6; 3, 6, 9]"
        );
    }

    #[test]
    fn chooses_branches_of_a_known_condition() {
        assert_eq!(s("if 1 < 2 then 10 else 20"), "10");
        assert_eq!(s("if 3 < 2 then 10 else 20"), "20");
        // An unknown condition keeps both branches.
        assert_eq!(s("if x then 1 else 2"), "if x then 1 else 2");
    }

    #[test]
    fn is_idempotent() {
        for src in [
            "10 * (x + 0.5x)",
            "2x/3y",
            "i^43",
            "4 weeks + 6 days",
            "[1, 2; 3, 4]",
            "(a + 1)*(a + 2)",
            "1/2*a*t^2 + v0*t + x0",
        ] {
            let once = simplify(&parse_expr(src));
            let twice = simplify(&once);
            assert_eq!(render(&once), render(&twice), "not idempotent: {src}");
        }
    }
}
