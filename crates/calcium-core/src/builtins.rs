//! The standard library.
//!
//! Functions come in three flavours:
//!
//! * **strict** — arguments are evaluated first (`sqrt`, `min`, `round`)
//! * **expanding** — arguments are evaluated with units expanded, so
//!   `cos(60°)` can reduce `°` to radians before computing
//! * **lazy** — arguments arrive unevaluated, because the function binds a
//!   variable of its own (`sum(x*x, x=1..5)` must not see an outer `x`)

use crate::ast::*;
use crate::eval::{Ctx, Env};
use crate::lexer::Radix;
use crate::num::Num;
use crate::simplify::{matmul, simplify, transpose};

/// Functions whose arguments bind variables, and so must not be pre-evaluated.
const LAZY: &[&str] = &[
    "sum", "∑", "prod", "∏", "map", "reduce", "filter", "der", "∂", "jacobian", "taylor", "solve",
    "plot",
];

/// Functions that need their arguments in base units.
const EXPANDING: &[&str] = &[
    "sin", "cos", "tan", "sinh", "cosh", "tanh", "tod", "exp", "ln", "log", "log2", "log10",
];

/// Every name the standard library claims. Used to keep builtins out of the
/// implicit parameter list of a definition: `color = round(f*0xFF)` takes one
/// parameter, `f`, not two.
const STRICT: &[&str] = &[
    "sqrt", "√", "abs", "sign", "inv", "conj", "re", "im", "round", "ceil", "floor", "truncate",
    "min", "max", "average", "mean", "choose", "nCr", "dot", "cross", "len", "atan2", "asin",
    "acos", "atan", "color", "exp",
];

pub fn is_builtin(name: &str) -> bool {
    STRICT.contains(&name) || LAZY.contains(&name) || EXPANDING.contains(&name)
}

pub fn is_lazy(name: &str) -> bool {
    LAZY.contains(&name)
}

pub fn expands_args(name: &str) -> bool {
    EXPANDING.contains(&name)
}

fn num_of(expr: &Expr) -> Option<Num> {
    expr.as_num().cloned()
}

/// Drops a `rad` factor, so an angle written with units becomes a plain
/// number: `60°` expands to `pi*rad/3` and comes back as `1.0472`.
fn radians(expr: &Expr) -> Option<Num> {
    if let Some(value) = num_of(expr) {
        return Some(value);
    }
    let stripped = simplify(&Expr::div(expr.clone(), Expr::var("rad")));
    num_of(&stripped)
}

fn angle(value: Num) -> Expr {
    simplify(&Expr::mul(vec![
        Expr::Num(value, Radix::Dec),
        Expr::var("rad"),
    ]))
}

fn float(value: f64) -> Expr {
    Expr::Num(Num::from_f64(value), Radix::Dec)
}

fn positional(args: &[Arg]) -> Vec<Expr> {
    args.iter().map(|a| a.value.clone()).collect()
}

// ---------------------------------------------------------------------------
// Strict builtins
// ---------------------------------------------------------------------------

/// Returns `None` when the call should stay symbolic.
pub fn call(env: &Env, name: &str, args: &[Arg], ctx: &mut Ctx) -> Option<Expr> {
    let values = positional(args);
    let first = values.first();

    // Elementary functions of one real argument.
    if values.len() == 1 {
        if let Some(result) = call_unary(name, first.unwrap()) {
            return Some(result);
        }
    }

    match name {
        "log" if values.len() == 2 => {
            if let (Some(x), Some(base)) = (num_of(&values[0]), num_of(&values[1])) {
                return Some(float(x.to_f64().log(base.to_f64())));
            }
            // Logarithms turn products into sums, which is the identity the
            // Math example is built around:
            //   log(x*y, b) => log(x, b) + log(y, b)
            //   log(x/y, b) => log(x, b) - log(y, b)
            if let Some(expanded) = expand_logarithm(&values[0], &values[1]) {
                return Some(expanded);
            }
            // A recognizable base gets its shorthand: `log(x, 10) => log10(x)`.
            let shorthand = match &values[1] {
                Expr::Num(n, _) if n.eq_num(&Num::from_i64(10)) => Some("log10"),
                Expr::Num(n, _) if n.eq_num(&Num::from_i64(2)) => Some("log2"),
                Expr::Num(n, _) if (n.to_f64() - std::f64::consts::E).abs() < 1e-12 => Some("ln"),
                _ => None,
            }?;
            Some(Expr::Call(
                shorthand.to_string(),
                vec![Arg::positional(values[0].clone())],
            ))
        }
        "atan2" if values.len() == 2 => {
            let (y, x) = (num_of(&values[0])?, num_of(&values[1])?);
            Some(angle(Num::from_f64(y.to_f64().atan2(x.to_f64()))))
        }
        "min" | "max" => {
            let items = spread(&values);
            let mut best: Option<Num> = None;
            for item in &items {
                let value = num_of(item)?;
                best = Some(match best {
                    None => value,
                    Some(current) => {
                        let take_new = if name == "min" {
                            value.cmp_num(&current)? == std::cmp::Ordering::Less
                        } else {
                            value.cmp_num(&current)? == std::cmp::Ordering::Greater
                        };
                        if take_new {
                            value
                        } else {
                            current
                        }
                    }
                });
            }
            Some(Expr::Num(best?, Radix::Dec))
        }
        "average" | "mean" => {
            let items = spread(&values);
            if items.is_empty() {
                return None;
            }
            let total = Expr::add(items.clone());
            Some(simplify(&Expr::div(
                total,
                Expr::num(items.len() as i64),
            )))
        }
        "choose" | "nCr" if values.len() == 2 => {
            let (n, k) = (num_of(&values[0])?.to_i64()?, num_of(&values[1])?.to_i64()?);
            Some(Expr::Num(binomial(n, k), Radix::Dec))
        }
        "len" => match first? {
            Expr::Matrix(rows) => Some(if rows.len() == 1 {
                Expr::num(rows[0].len() as i64)
            } else if rows.iter().all(|r| r.len() == rows[0].len()) {
                Expr::num(rows.len() as i64)
            } else {
                Expr::Matrix(vec![rows.iter().map(|r| Expr::num(r.len() as i64)).collect()])
            }),
            Expr::Str(s) => Some(Expr::num(s.chars().count() as i64)),
            Expr::Dict(entries) => Some(Expr::num(entries.len() as i64)),
            _ => None,
        },
        "dot" if values.len() == 2 => {
            let (a, b) = (flatten_matrix(&values[0])?, flatten_matrix(&values[1])?);
            if a.len() != b.len() {
                return Some(Expr::Error("dot needs equal-length vectors".to_string()));
            }
            let terms = a
                .iter()
                .zip(&b)
                .map(|(x, y)| Expr::mul(vec![x.clone(), y.clone()]))
                .collect();
            Some(simplify(&Expr::add(terms)))
        }
        "cross" if values.len() == 2 => {
            let (a, b) = (flatten_matrix(&values[0])?, flatten_matrix(&values[1])?);
            if a.len() != 3 || b.len() != 3 {
                return Some(Expr::Error("cross needs 3-vectors".to_string()));
            }
            let component = |i: usize, j: usize| {
                simplify(&Expr::sub(
                    Expr::mul(vec![a[i].clone(), b[j].clone()]),
                    Expr::mul(vec![a[j].clone(), b[i].clone()]),
                ))
            };
            Some(Expr::Matrix(vec![vec![
                component(1, 2),
                component(2, 0),
                component(0, 1),
            ]]))
        }
        "inv" => match first? {
            Expr::Matrix(rows) => Some(matrix_inverse(rows)),
            other => Some(simplify(&Expr::Pow(
                Box::new(other.clone()),
                Box::new(Expr::num(-1)),
            ))),
        },
        "conj" => Some(conjugate(first?)),
        "re" => Some(real_part(first?)),
        "im" => Some(imaginary_part(first?)),
        "color" => {
            let value = num_of(first?)?;
            Some(Expr::Num(value.round(), Radix::Hex))
        }
        "tod" => Some(time_of_day(first?)),
        "abs" => Some(abs(first?)),
        _ => {
            let _ = (env, ctx);
            None
        }
    }
}

/// Rewrites `log(a*b, base)` as a sum of logarithms, turning reciprocal
/// factors into subtractions.
fn expand_logarithm(value: &Expr, base: &Expr) -> Option<Expr> {
    let Expr::Mul(factors) = value else {
        return None;
    };
    if factors.len() < 2 {
        return None;
    }
    let mut terms = Vec::with_capacity(factors.len());
    for factor in factors {
        // `x/y` is `x * y^-1`, whose logarithm is `-log(y, base)`.
        let (inner, sign) = match factor {
            Expr::Pow(inner, exp) if exp.as_num().map(|e| e.is_negative()).unwrap_or(false) => {
                (inner.as_ref().clone(), -1)
            }
            other => (other.clone(), 1),
        };
        let term = Expr::Call(
            "log".to_string(),
            vec![Arg::positional(inner), Arg::positional(base.clone())],
        );
        terms.push(if sign < 0 { Expr::neg(term) } else { term });
    }
    Some(simplify(&Expr::add(terms)))
}

/// Principal square root of `a + bi`, treating `i` as the ordinary symbol it
/// is everywhere else in the engine.
fn complex_sqrt(expr: &Expr) -> Option<Expr> {
    let (real, imaginary) = split_complex(expr);
    let (a, b) = (num_of(&real)?, num_of(&imaginary)?);
    if b.is_zero() {
        return None; // the real case is handled by the ordinary power rule
    }
    let magnitude = (a.to_f64().powi(2) + b.to_f64().powi(2)).sqrt();
    let re = ((magnitude + a.to_f64()) / 2.0).sqrt();
    let im = ((magnitude - a.to_f64()) / 2.0).sqrt() * if b.is_negative() { -1.0 } else { 1.0 };
    Some(simplify(&Expr::add(vec![
        float(re),
        Expr::mul(vec![float(im), Expr::var("i")]),
    ])))
}

/// A monotone-increasing function applied to an interval endpoint-wise:
/// `ln(1..e^2)` is `0..2`.
fn monotone_interval(name: &str, arg: &Expr) -> Option<Expr> {
    let Expr::Range(lo, hi) = arg else { return None };
    let apply = |v: &Num| -> f64 {
        let x = v.to_f64();
        match name {
            "exp" => x.exp(),
            "ln" => x.ln(),
            "log2" => x.log2(),
            _ => x.log10(),
        }
    };
    let (a, b) = (apply(lo.as_num()?), apply(hi.as_num()?));
    if !a.is_finite() || !b.is_finite() {
        return None;
    }
    Some(Expr::Range(
        Box::new(float(a)),
        Box::new(float(b)),
    ))
}

fn call_unary(name: &str, arg: &Expr) -> Option<Expr> {
    // Trigonometry accepts an angle with units and returns a plain number.
    let trig = |f: fn(f64) -> f64| radians(arg).map(|v| float(f(v.to_f64())));
    // Rounding preserves how the argument was written, so the Reference's
    // `color = round(f*0xFF)` answers in hex.
    let radix = match arg {
        // A sig-figs tag stops here: `round(2.5)` answers a plain 2, not
        // one dressed in the argument's decimal places.
        Expr::Num(_, Radix::Sig(_)) => Radix::Dec,
        Expr::Num(_, style) => *style,
        _ => Radix::Dec,
    };
    let rounded = |v: Num| Expr::Num(v, radix);
    match name {
        "sin" => trig(f64::sin),
        "cos" => trig(f64::cos),
        "tan" => trig(f64::tan),
        "sinh" => trig(f64::sinh),
        "cosh" => trig(f64::cosh),
        "tanh" => trig(f64::tanh),
        // The inverse functions return angles, so their results carry `rad`.
        "asin" => num_of(arg).map(|v| angle(Num::from_f64(v.to_f64().asin()))),
        "acos" => num_of(arg).map(|v| angle(Num::from_f64(v.to_f64().acos()))),
        "atan" => num_of(arg).map(|v| angle(Num::from_f64(v.to_f64().atan()))),
        // The monotone functions extend to an interval endpoint-wise.
        "exp" | "ln" | "log2" | "log10" if matches!(arg, Expr::Range(..)) => {
            monotone_interval(name, arg)
        }
        "exp" => num_of(arg).map(|v| float(v.to_f64().exp())),
        "ln" => num_of(arg).map(|v| float(v.to_f64().ln())),
        "log2" => num_of(arg).map(|v| float(v.to_f64().log2())),
        "log10" => num_of(arg).map(|v| float(v.to_f64().log10())),
        "sqrt" | "√" => complex_sqrt(arg).or_else(|| {
            Some(simplify(&Expr::Pow(
                Box::new(arg.clone()),
                Box::new(Expr::Num(Num::ratio(1, 2), Radix::Dec)),
            )))
        }),
        "round" => num_of(arg).map(|v| rounded(v.round())),
        "ceil" => num_of(arg).map(|v| rounded(v.ceil())),
        "floor" => num_of(arg).map(|v| rounded(v.floor())),
        "truncate" => num_of(arg).map(|v| rounded(v.truncate())),
        "sign" => num_of(arg).map(|v| Expr::Num(v.sign(), Radix::Dec)),
        _ => None,
    }
}

/// `min(1, 2, 3)` and `min([1, 2, 3])` mean the same thing.
fn spread(values: &[Expr]) -> Vec<Expr> {
    if values.len() == 1 {
        if let Expr::Matrix(rows) = &values[0] {
            return rows.iter().flatten().cloned().collect();
        }
    }
    values.to_vec()
}

fn flatten_matrix(expr: &Expr) -> Option<Vec<Expr>> {
    match expr {
        Expr::Matrix(rows) => Some(rows.iter().flatten().cloned().collect()),
        _ => None,
    }
}

fn binomial(n: i64, k: i64) -> Num {
    if k < 0 || k > n {
        return Num::zero();
    }
    let k = k.min(n - k);
    let mut result = Num::one();
    for i in 0..k {
        result = result.mul(&Num::from_i64(n - i));
        result = result.div(&Num::from_i64(i + 1));
    }
    result
}

// ---------------------------------------------------------------------------
// Complex parts
// ---------------------------------------------------------------------------

/// Splits an expression into its real and imaginary parts, treating `i` as an
/// ordinary symbol.
fn split_complex(expr: &Expr) -> (Expr, Expr) {
    let terms: Vec<Expr> = match expr {
        Expr::Add(terms) => terms.clone(),
        other => vec![other.clone()],
    };
    let mut real = Vec::new();
    let mut imaginary = Vec::new();
    for term in terms {
        let factors: Vec<Expr> = match &term {
            Expr::Mul(factors) => factors.clone(),
            other => vec![other.clone()],
        };
        let has_i = factors
            .iter()
            .any(|f| matches!(f, Expr::Var(name) if name == "i"));
        if has_i {
            let rest: Vec<Expr> = factors
                .into_iter()
                .filter(|f| !matches!(f, Expr::Var(name) if name == "i"))
                .collect();
            imaginary.push(Expr::mul(rest));
        } else {
            real.push(term);
        }
    }
    (
        simplify(&Expr::add(real)),
        simplify(&Expr::add(imaginary)),
    )
}

fn conjugate(expr: &Expr) -> Expr {
    let (real, imaginary) = split_complex(expr);
    if imaginary.is_zero() {
        // `conj(a)` on an unknown stays symbolic.
        if matches!(expr, Expr::Var(_)) {
            return Expr::Call("conj".to_string(), vec![Arg::positional(expr.clone())]);
        }
        return real;
    }
    simplify(&Expr::sub(
        real,
        Expr::mul(vec![imaginary, Expr::var("i")]),
    ))
}

fn real_part(expr: &Expr) -> Expr {
    split_complex(expr).0
}

fn imaginary_part(expr: &Expr) -> Expr {
    split_complex(expr).1
}

// ---------------------------------------------------------------------------
// Absolute value, norms, indexing
// ---------------------------------------------------------------------------

pub fn abs(expr: &Expr) -> Expr {
    match expr {
        Expr::Matrix(rows) => {
            if rows.len() > 1 && rows.len() == rows[0].len() {
                // Square matrix: the determinant.
                determinant(rows)
            } else {
                // Vector: the 2-norm.
                norm(expr, None)
            }
        }
        other => simplify(&Expr::Abs(Box::new(other.clone()))),
    }
}

pub fn norm(expr: &Expr, p: Option<&Expr>) -> Expr {
    let Some(items) = flatten_matrix(expr) else {
        return simplify(&Expr::Abs(Box::new(expr.clone())));
    };
    let order = p.and_then(num_of).unwrap_or(Num::from_i64(2));
    let powered: Vec<Expr> = items
        .iter()
        .map(|item| {
            Expr::Pow(
                Box::new(Expr::Abs(Box::new(item.clone()))),
                Box::new(Expr::Num(order.clone(), Radix::Dec)),
            )
        })
        .collect();
    let total = simplify(&Expr::add(powered));
    simplify(&Expr::Pow(
        Box::new(total),
        Box::new(Expr::Num(order.div(&order).div(&order), Radix::Dec)),
    ))
}

pub fn index(base: &Expr, indices: &[Expr]) -> Expr {
    let Expr::Matrix(rows) = base else {
        return Expr::Index(Box::new(base.clone()), indices.to_vec());
    };
    match indices {
        // A single index reads in column-major order, per the Reference:
        // for `[0, 1; 2, 3]`, `mat[1]` is `2`.
        [only] => {
            if let Some(range) = as_range(only) {
                let flat = column_major(rows);
                let picked: Vec<Expr> = range
                    .iter()
                    .filter_map(|i| flat.get(*i as usize).cloned())
                    .collect();
                return Expr::Matrix(vec![picked]);
            }
            let Some(i) = num_of(only).and_then(|n| n.to_i64()) else {
                return Expr::Index(Box::new(base.clone()), indices.to_vec());
            };
            column_major(rows)
                .get(i as usize)
                .cloned()
                .unwrap_or_else(|| Expr::Error(format!("index {i} out of range")))
        }
        [row_index, column_index] => {
            let rows_wanted = as_range(row_index)
                .or_else(|| num_of(row_index).and_then(|n| n.to_i64()).map(|i| vec![i]));
            let columns_wanted = as_range(column_index).or_else(|| {
                num_of(column_index)
                    .and_then(|n| n.to_i64())
                    .map(|i| vec![i])
            });
            let (Some(rows_wanted), Some(columns_wanted)) = (rows_wanted, columns_wanted) else {
                return Expr::Index(Box::new(base.clone()), indices.to_vec());
            };
            let picked: Vec<Vec<Expr>> = rows_wanted
                .iter()
                .filter_map(|r| rows.get(*r as usize))
                .map(|row| {
                    columns_wanted
                        .iter()
                        .filter_map(|c| row.get(*c as usize).cloned())
                        .collect()
                })
                .collect();
            if picked.len() == 1 && picked[0].len() == 1 {
                picked[0][0].clone()
            } else {
                Expr::Matrix(picked)
            }
        }
        _ => Expr::Index(Box::new(base.clone()), indices.to_vec()),
    }
}

fn column_major(rows: &[Vec<Expr>]) -> Vec<Expr> {
    transpose(rows).into_iter().flatten().collect()
}

fn as_range(expr: &Expr) -> Option<Vec<i64>> {
    match expr {
        Expr::Range(lo, hi) => {
            let lo = num_of(lo)?.to_i64()?;
            let hi = num_of(hi)?.to_i64()?;
            Some(if lo <= hi {
                (lo..=hi).collect()
            } else {
                (hi..=lo).rev().collect()
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Matrices
// ---------------------------------------------------------------------------

pub fn determinant(rows: &[Vec<Expr>]) -> Expr {
    let n = rows.len();
    if n == 0 {
        return Expr::num(0);
    }
    if n == 1 {
        return rows[0][0].clone();
    }
    if n == 2 {
        return simplify(&Expr::sub(
            Expr::mul(vec![rows[0][0].clone(), rows[1][1].clone()]),
            Expr::mul(vec![rows[0][1].clone(), rows[1][0].clone()]),
        ));
    }
    // Cofactor expansion along the first row. Fine for the small matrices a
    // symbolic inverse is useful on.
    let mut terms = Vec::new();
    for (c, cell) in rows[0].iter().enumerate() {
        let minor: Vec<Vec<Expr>> = rows[1..]
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .filter(|(j, _)| *j != c)
                    .map(|(_, v)| v.clone())
                    .collect()
            })
            .collect();
        let sign = if c % 2 == 0 { 1 } else { -1 };
        terms.push(Expr::mul(vec![
            Expr::num(sign),
            cell.clone(),
            determinant(&minor),
        ]));
    }
    simplify(&Expr::add(terms))
}

/// Gauss-Jordan elimination. Works symbolically because every arithmetic step
/// goes through `simplify` rather than through floating point.
pub fn matrix_inverse(rows: &[Vec<Expr>]) -> Expr {
    let n = rows.len();
    if n == 0 || rows.iter().any(|row| row.len() != n) {
        return Expr::Error("only square matrices can be inverted".to_string());
    }
    let mut work: Vec<Vec<Expr>> = rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            let mut extended = row.clone();
            for c in 0..n {
                extended.push(if r == c { Expr::num(1) } else { Expr::num(0) });
            }
            extended
        })
        .collect();

    for pivot in 0..n {
        // Prefer a row whose pivot is a nonzero number; fall back to any
        // structurally nonzero entry so symbolic matrices still work.
        let chosen = (pivot..n)
            .find(|r| {
                work[*r][pivot]
                    .as_num()
                    .map(|v| !v.is_zero())
                    .unwrap_or(false)
            })
            .or_else(|| (pivot..n).find(|r| !work[*r][pivot].is_zero()));
        let Some(chosen) = chosen else {
            return Expr::Error("matrix is singular".to_string());
        };
        work.swap(pivot, chosen);

        let divisor = work[pivot][pivot].clone();
        for c in 0..2 * n {
            work[pivot][c] = simplify(&Expr::div(work[pivot][c].clone(), divisor.clone()));
        }
        for r in 0..n {
            if r == pivot || work[r][pivot].is_zero() {
                continue;
            }
            let factor = work[r][pivot].clone();
            for c in 0..2 * n {
                let adjustment = Expr::mul(vec![factor.clone(), work[pivot][c].clone()]);
                work[r][c] = simplify(&Expr::sub(work[r][c].clone(), adjustment));
            }
        }
    }

    Expr::Matrix(
        work.into_iter()
            .map(|row| row[n..].to_vec())
            .collect(),
    )
}

/// Solves `a * x = b` for the vector `x`.
pub fn solve_linear_system(a: &[Vec<Expr>], b: &[Vec<Expr>]) -> Expr {
    match matrix_inverse(a) {
        Expr::Matrix(inverse) => match matmul(&inverse, b) {
            Ok(product) => Expr::Matrix(product),
            Err(message) => Expr::Error(message),
        },
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Time of day
// ---------------------------------------------------------------------------

fn time_of_day(expr: &Expr) -> Expr {
    // The argument arrives already expanded, so a time value is a multiple of
    // the base unit `s`; a bare number is taken as seconds.
    let seconds = num_of(expr)
        .or_else(|| num_of(&simplify(&Expr::div(expr.clone(), Expr::var("s")))));
    let Some(seconds) = seconds else {
        return Expr::Call("tod".to_string(), vec![Arg::positional(expr.clone())]);
    };
    let day = 24 * 60 * 60;
    let mut total = seconds.to_f64().round() as i64 % day;
    if total < 0 {
        total += day;
    }
    let (hours, minutes, secs) = (total / 3600, (total % 3600) / 60, total % 60);
    let mut parts = Vec::new();
    if hours != 0 {
        parts.push(Expr::mul(vec![Expr::num(hours), Expr::var("hr")]));
    }
    if minutes != 0 {
        parts.push(Expr::mul(vec![Expr::num(minutes), Expr::var("mins")]));
    }
    if secs != 0 {
        parts.push(Expr::mul(vec![Expr::num(secs), Expr::var("s")]));
    }
    if parts.is_empty() {
        return Expr::num(0);
    }
    Expr::add(parts)
}

// ---------------------------------------------------------------------------
// Lazy builtins
// ---------------------------------------------------------------------------

pub fn call_lazy(env: &Env, name: &str, args: &[Arg], ctx: &mut Ctx) -> Expr {
    match name {
        "sum" | "∑" => fold(env, args, ctx, Folding::Sum),
        "prod" | "∏" => fold(env, args, ctx, Folding::Product),
        "map" => map(env, args, ctx),
        "filter" => filter(env, args, ctx),
        "reduce" => reduce(env, args, ctx),
        "der" | "∂" => derivative(env, args, ctx),
        "jacobian" => jacobian(env, args, ctx),
        "taylor" => taylor(env, args, ctx),
        // `plot` is a display directive; the engine passes it through so a UI
        // layer can pick it up.
        "plot" => Expr::Call(
            "plot".to_string(),
            args.iter()
                .map(|a| Arg {
                    name: a.name.clone(),
                    value: a.value.clone(),
                })
                .collect(),
        ),
        // `solve(x)` isolates x from the surrounding document, so a solution
        // can be stored: `sol = solve(x coord)`.
        "solve" => match args.first().map(|a| &a.value) {
            Some(Expr::Var(target)) => crate::solve::solve_for(env, target)
                .unwrap_or_else(|| Expr::Error(format!("cannot solve for {target}"))),
            _ => Expr::Call("solve".to_string(), args.to_vec()),
        },
        _ => Expr::Call(name.to_string(), args.to_vec()),
    }
}

/// The items an iteration argument produces.
fn items_of(expr: &Expr) -> Option<Vec<Expr>> {
    match expr {
        Expr::Range(lo, hi) => {
            let lo = num_of(lo)?.to_i64()?;
            let hi = num_of(hi)?.to_i64()?;
            Some(
                if lo <= hi {
                    (lo..=hi).collect::<Vec<_>>()
                } else {
                    (hi..=lo).rev().collect::<Vec<_>>()
                }
                .into_iter()
                .map(Expr::num)
                .collect(),
            )
        }
        Expr::Matrix(rows) => Some(rows.iter().flatten().cloned().collect()),
        _ => None,
    }
}

/// Picks the variable an iteration binds when it was not named explicitly:
/// the first free variable of the body that the environment does not already
/// define. That is what lets `sum(x, data)` bind `x` while
/// `sum((age_i - mean)^2, ages)` binds `age_i` and leaves `mean` alone.
/// Builtin names are never binders, so `map(cos(n/8), 0..8)` binds `n`,
/// not `cos`.
fn implicit_binder(env: &Env, body: &Expr) -> Option<String> {
    body.free_vars()
        .into_iter()
        .find(|name| !env.is_defined(name) && !is_builtin(name))
}

/// Resolves an iteration body: a bare name that refers to a definition is
/// replaced by its body, so `sum(sq, v=1..5)` sums `v^2`.
fn body_of(env: &Env, expr: &Expr) -> (Expr, Option<Vec<String>>) {
    if let Expr::Var(name) = expr {
        if let Some(def) = env.get(name) {
            if !def.is_unit {
                return (def.body.clone(), Some(def.params()));
            }
        }
    }
    (expr.clone(), None)
}

/// Applies a per-item body, either as a function call or by substitution.
fn apply_item(env: &Env, source: &Expr, binder: &Option<String>, item: &Expr, ctx: &mut Ctx) -> Expr {
    // A bare name that is a builtin or a declared function is applied.
    if let Expr::Var(name) = source {
        let is_function = env
            .get(name)
            .map(|d| d.params.is_some())
            .unwrap_or_else(|| call_unary(name, &Expr::num(0)).is_some());
        if is_function {
            let mut inner = Ctx {
                expand_units: ctx.expand_units,
                ..Ctx::default()
            };
            return env.eval_in(
                &Expr::Call(name.clone(), vec![Arg::positional(item.clone())]),
                &mut inner,
            );
        }
    }
    let (body, _) = body_of(env, source);
    let mut inner = Ctx {
        locals: ctx.locals.clone(),
        active: ctx.active.clone(),
        expand_units: ctx.expand_units,
        in_prelude: ctx.in_prelude,
        depth: ctx.depth,
        calls: ctx.calls,
        pm: ctx.pm.clone(),
    };
    if let Some(binder) = binder {
        inner.locals.insert(binder.clone(), item.clone());
    }
    simplify(&env.eval_in(&body, &mut inner))
}

enum Folding {
    Sum,
    Product,
}

fn fold(env: &Env, args: &[Arg], ctx: &mut Ctx, kind: Folding) -> Expr {
    // `sum([1, 2, 3])` sums a matrix directly.
    if args.len() == 1 {
        let value = env.eval_in(&args[0].value, ctx);
        if let Some(items) = items_of(&value) {
            return match kind {
                Folding::Sum => simplify(&Expr::add(items)),
                Folding::Product => simplify(&Expr::mul(items)),
            };
        }
        return Expr::Call("sum".to_string(), args.to_vec());
    }
    if args.len() < 2 {
        return Expr::Call("sum".to_string(), args.to_vec());
    }

    let source = &args[0].value;
    let iteration = &args[1];
    let items_expr = env.eval_in(&iteration.value, ctx);
    let Some(items) = items_of(&items_expr) else {
        return Expr::Call("sum".to_string(), args.to_vec());
    };

    let (body, _) = body_of(env, source);
    let binder = iteration
        .name
        .clone()
        .or_else(|| implicit_binder(env, &body));

    let mapped: Vec<Expr> = items
        .iter()
        .map(|item| apply_item(env, source, &binder, item, ctx))
        .collect();
    match kind {
        Folding::Sum => simplify(&Expr::add(mapped)),
        Folding::Product => simplify(&Expr::mul(mapped)),
    }
}

fn map(env: &Env, args: &[Arg], ctx: &mut Ctx) -> Expr {
    if args.len() < 2 {
        return Expr::Call("map".to_string(), args.to_vec());
    }
    let source = &args[0].value;
    let (body, _) = body_of(env, source);

    // Several named iterables zip together:
    // `map(10*y + x, x = [1, 2], y = [3, 4]) => [31, 42]`
    let iterations: Vec<(&Arg, Vec<Expr>)> = args[1..]
        .iter()
        .filter_map(|arg| {
            let value = env.eval_in(&arg.value, &mut ctx.clone());
            items_of(&value).map(|items| (arg, items))
        })
        .collect();
    if iterations.len() != args.len() - 1 {
        return Expr::Call("map".to_string(), args.to_vec());
    }

    let length = iterations.iter().map(|(_, items)| items.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(length);
    for i in 0..length {
        if iterations.len() == 1 && iterations[0].0.name.is_none() {
            let binder = implicit_binder(env, &body);
            out.push(apply_item(env, source, &binder, &iterations[0].1[i], ctx));
            continue;
        }
        let mut inner = Ctx {
            locals: ctx.locals.clone(),
            active: ctx.active.clone(),
            expand_units: ctx.expand_units,
            in_prelude: ctx.in_prelude,
            depth: ctx.depth,
            calls: ctx.calls,
            pm: ctx.pm.clone(),
        };
        for (arg, items) in &iterations {
            let binder = arg
                .name
                .clone()
                .or_else(|| implicit_binder(env, &body))
                .unwrap_or_else(|| "x".to_string());
            inner.locals.insert(binder, items[i].clone());
        }
        out.push(simplify(&env.eval_in(&body, &mut inner)));
    }
    Expr::Matrix(vec![out])
}

fn filter(env: &Env, args: &[Arg], ctx: &mut Ctx) -> Expr {
    if args.len() != 2 {
        return Expr::Call("filter".to_string(), args.to_vec());
    }
    let predicate = &args[0].value;
    let value = env.eval_in(&args[1].value, ctx);
    let Some(items) = items_of(&value) else {
        return Expr::Call("filter".to_string(), args.to_vec());
    };
    let binder = args[1]
        .name
        .clone()
        .or_else(|| implicit_binder(env, predicate));
    let kept: Vec<Expr> = items
        .into_iter()
        .filter(|item| {
            matches!(
                crate::simplify::truth_of(&apply_item(env, predicate, &binder, item, ctx)),
                Some(true)
            )
        })
        .collect();
    Expr::Matrix(vec![kept])
}

fn reduce(env: &Env, args: &[Arg], ctx: &mut Ctx) -> Expr {
    if args.len() < 2 {
        return Expr::Call("reduce".to_string(), args.to_vec());
    }
    let source = &args[0].value;
    let value = env.eval_in(&args[1].value, ctx);
    let Some(items) = items_of(&value) else {
        return Expr::Call("reduce".to_string(), args.to_vec());
    };
    if items.is_empty() {
        return Expr::num(0);
    }

    let (body, params) = body_of(env, source);
    // The first two unbound names are the accumulator and the element, in
    // order of appearance: `reduce(acc + x + b, ...)` folds over `acc` and `x`
    // and leaves `b` free.
    let names: Vec<String> = params.unwrap_or_else(|| {
        body.free_vars()
            .into_iter()
            .filter(|n| !env.is_defined(n))
            .collect()
    });

    let mut iter = items.into_iter();
    let mut accumulator = match args.get(2) {
        Some(initial) => env.eval_in(&initial.value, ctx),
        None => iter.next().unwrap(),
    };

    // An undefined function name reduces symbolically: `reduce(g, [1, 2, 3])`
    // gives `g(g(1, 2), 3)`.
    // A bare name that is not a real function reduces symbolically:
    // `reduce(g, [1, 2, 3])` gives `g(g(1, 2), 3)`.
    let symbolic = matches!(source, Expr::Var(name)
        if (!env.is_defined(name) || env.is_unit_name(name)) && names.len() < 2);

    for item in iter {
        if symbolic {
            let Expr::Var(name) = source else { unreachable!() };
            accumulator = Expr::Call(
                name.clone(),
                vec![Arg::positional(accumulator), Arg::positional(item)],
            );
            continue;
        }
        let mut inner = Ctx {
            locals: ctx.locals.clone(),
            active: ctx.active.clone(),
            expand_units: ctx.expand_units,
            in_prelude: ctx.in_prelude,
            depth: ctx.depth,
            calls: ctx.calls,
            pm: ctx.pm.clone(),
        };
        if let Some(name) = names.first() {
            inner.locals.insert(name.clone(), accumulator.clone());
        }
        if let Some(name) = names.get(1) {
            inner.locals.insert(name.clone(), item.clone());
        }
        accumulator = simplify(&env.eval_in(&body, &mut inner));
    }
    accumulator
}

// ---------------------------------------------------------------------------
// Calculus
// ---------------------------------------------------------------------------

fn derivative(env: &Env, args: &[Arg], ctx: &mut Ctx) -> Expr {
    if args.is_empty() {
        return Expr::Call("der".to_string(), args.to_vec());
    }
    let (body, _) = body_of(env, &args[0].value);
    let body = {
        let mut inner = Ctx {
            expand_units: ctx.expand_units,
            ..Ctx::default()
        };
        // Expand definitions but keep the variable of differentiation free.
        env.eval_in(&body, &mut inner)
    };

    let variable = match args.get(1).map(|a| &a.value) {
        Some(Expr::Var(name)) => name.clone(),
        _ => match implicit_binder(env, &body).or_else(|| body.free_vars().into_iter().next()) {
            Some(name) => name,
            None => return Expr::num(0),
        },
    };
    let order = args
        .get(2)
        .and_then(|a| num_of(&env.eval_in(&a.value, ctx)))
        .and_then(|n| n.to_i64())
        .unwrap_or(1);

    let mut result = body;
    for _ in 0..order.max(0) {
        result = differentiate(&result, &variable);
    }
    simplify(&result)
}

/// Symbolic differentiation.
pub fn differentiate(expr: &Expr, variable: &str) -> Expr {
    match expr {
        Expr::Num(..) | Expr::Str(_) | Expr::Bool(_) => Expr::num(0),
        Expr::Var(name) => {
            if name == variable {
                Expr::num(1)
            } else {
                Expr::num(0)
            }
        }
        Expr::Add(terms) => simplify(&Expr::add(
            terms.iter().map(|t| differentiate(t, variable)).collect(),
        )),
        // Product rule, generalized to n factors.
        Expr::Mul(factors) => {
            let mut terms = Vec::new();
            for (i, factor) in factors.iter().enumerate() {
                let mut parts: Vec<Expr> = factors.clone();
                parts[i] = differentiate(factor, variable);
                let _ = factor;
                terms.push(Expr::mul(parts));
            }
            simplify(&Expr::add(terms))
        }
        Expr::Pow(base, exp) => {
            let base_prime = differentiate(base, variable);
            if !exp.mentions(variable) {
                // d/dx f^n = n f^(n-1) f'
                let reduced = simplify(&Expr::sub((**exp).clone(), Expr::num(1)));
                return simplify(&Expr::mul(vec![
                    (**exp).clone(),
                    Expr::Pow(base.clone(), Box::new(reduced)),
                    base_prime,
                ]));
            }
            // d/dx f^g = f^g * (g' ln f + g f'/f)
            let exp_prime = differentiate(exp, variable);
            let log_term = Expr::mul(vec![
                exp_prime,
                Expr::Call("ln".to_string(), vec![Arg::positional((**base).clone())]),
            ]);
            let ratio = Expr::mul(vec![
                (**exp).clone(),
                base_prime,
                Expr::Pow(base.clone(), Box::new(Expr::num(-1))),
            ]);
            simplify(&Expr::mul(vec![
                expr.clone(),
                Expr::add(vec![log_term, ratio]),
            ]))
        }
        // `der(3 + |a - 2|)` is piecewise, which the Reference shows as an
        // `if` expression.
        Expr::Abs(inner) => {
            let inner_prime = differentiate(inner, variable);
            simplify(&Expr::If(
                Box::new(Expr::Cmp(
                    CmpOp::Lt,
                    inner.clone(),
                    Box::new(Expr::num(0)),
                )),
                Box::new(Expr::neg(inner_prime.clone())),
                Box::new(inner_prime),
            ))
        }
        Expr::If(cond, then_branch, else_branch) => simplify(&Expr::If(
            cond.clone(),
            Box::new(differentiate(then_branch, variable)),
            Box::new(differentiate(else_branch, variable)),
        )),
        Expr::Call(name, args) if args.len() == 1 => {
            let inner = &args[0].value;
            if !inner.mentions(variable) {
                return Expr::num(0);
            }
            let inner_prime = differentiate(inner, variable);
            let outer = match name.as_str() {
                "sin" => Expr::Call("cos".to_string(), args.clone()),
                "cos" => Expr::neg(Expr::Call("sin".to_string(), args.clone())),
                "tan" => Expr::Pow(
                    Box::new(Expr::Call("cos".to_string(), args.clone())),
                    Box::new(Expr::num(-2)),
                ),
                "exp" => expr.clone(),
                "ln" => Expr::Pow(Box::new(inner.clone()), Box::new(Expr::num(-1))),
                "log10" | "log2" => Expr::mul(vec![
                    Expr::Pow(Box::new(inner.clone()), Box::new(Expr::num(-1))),
                    Expr::Pow(
                        Box::new(Expr::Call(
                            "ln".to_string(),
                            vec![Arg::positional(Expr::num(
                                if name == "log10" { 10 } else { 2 },
                            ))],
                        )),
                        Box::new(Expr::num(-1)),
                    ),
                ]),
                "sqrt" | "√" => Expr::mul(vec![
                    Expr::Num(Num::ratio(1, 2), Radix::Dec),
                    Expr::Pow(
                        Box::new(inner.clone()),
                        Box::new(Expr::Num(Num::ratio(-1, 2), Radix::Dec)),
                    ),
                ]),
                "sinh" => Expr::Call("cosh".to_string(), args.clone()),
                "cosh" => Expr::Call("sinh".to_string(), args.clone()),
                _ => {
                    return Expr::Call(
                        "der".to_string(),
                        vec![
                            Arg::positional(expr.clone()),
                            Arg::positional(Expr::var(variable)),
                        ],
                    )
                }
            };
            simplify(&Expr::mul(vec![outer, inner_prime]))
        }
        Expr::Matrix(rows) => Expr::Matrix(
            rows.iter()
                .map(|row| row.iter().map(|c| differentiate(c, variable)).collect())
                .collect(),
        ),
        _ => {
            if expr.mentions(variable) {
                Expr::Call(
                    "der".to_string(),
                    vec![
                        Arg::positional(expr.clone()),
                        Arg::positional(Expr::var(variable)),
                    ],
                )
            } else {
                Expr::num(0)
            }
        }
    }
}

fn jacobian(env: &Env, args: &[Arg], ctx: &mut Ctx) -> Expr {
    // Functions may be listed directly or wrapped in a matrix; trailing bare
    // variables name the differentiation variables.
    let mut functions = Vec::new();
    let mut variables = Vec::new();
    for arg in args {
        let (body, _) = body_of(env, &arg.value);
        match &body {
            Expr::Matrix(rows) => functions.extend(rows.iter().flatten().cloned()),
            Expr::Var(name) if env.get(name).is_none() && !functions.is_empty() => {
                variables.push(name.clone())
            }
            other => functions.push(other.clone()),
        }
    }
    let functions: Vec<Expr> = functions
        .iter()
        .map(|f| {
            let mut inner = Ctx {
                expand_units: ctx.expand_units,
                ..Ctx::default()
            };
            env.eval_in(f, &mut inner)
        })
        .collect();
    if variables.is_empty() {
        for function in &functions {
            for name in function.free_vars() {
                if !env.is_defined(&name) && !variables.contains(&name) {
                    variables.push(name);
                }
            }
        }
        variables.sort();
    }
    let rows: Vec<Vec<Expr>> = functions
        .iter()
        .map(|function| {
            variables
                .iter()
                .map(|variable| differentiate(function, variable))
                .collect()
        })
        .collect();
    Expr::Matrix(rows)
}

fn taylor(env: &Env, args: &[Arg], ctx: &mut Ctx) -> Expr {
    if args.len() < 2 {
        return Expr::Call("taylor".to_string(), args.to_vec());
    }
    let (body, _) = body_of(env, &args[0].value);
    let body = {
        let mut inner = Ctx {
            expand_units: ctx.expand_units,
            ..Ctx::default()
        };
        env.eval_in(&body, &mut inner)
    };
    // The second argument is `x = point`.
    let Some(variable) = args[1].name.clone() else {
        return Expr::Call("taylor".to_string(), args.to_vec());
    };
    let point = env.eval_in(&args[1].value, ctx);
    let order = args
        .get(2)
        .and_then(|a| num_of(&env.eval_in(&a.value, ctx)))
        .and_then(|n| n.to_i64())
        .unwrap_or(1);

    let mut terms = Vec::new();
    let mut derivative = body;
    let mut factorial = Num::one();
    for k in 0..=order.max(0) {
        if k > 0 {
            factorial = factorial.mul(&Num::from_i64(k));
        }
        let mut inner = Ctx::default();
        inner.locals.insert(variable.clone(), point.clone());
        let value = simplify(&env.eval_in(&derivative, &mut inner));
        let shift = Expr::sub(Expr::var(&variable), point.clone());
        terms.push(simplify(&Expr::mul(vec![
            Expr::Num(Num::one().div(&factorial), Radix::Dec),
            value,
            Expr::Pow(Box::new(shift), Box::new(Expr::num(k))),
        ])));
        derivative = differentiate(&derivative, &variable);
    }
    simplify(&Expr::add(terms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::render;
    use crate::parser::{parse_expr, parse_line};

    fn run(lines: &[&str]) -> String {
        let mut env = Env::with_prelude();
        let mut last = String::new();
        for line in lines {
            for statement in parse_line(line) {
                match statement.stmt {
                    Stmt::Define { name, params, body } => env.define(&name, params, body),
                    Stmt::Expr(expr) => last = render(&env.eval(&expr)),
                    _ => {}
                }
            }
        }
        last
    }

    fn e(src: &str) -> String {
        let env = Env::with_prelude();
        render(&env.eval(&parse_expr(src)))
    }

    #[test]
    fn elementary_functions() {
        assert_eq!(e("sin(pi/6)"), "0.5");
        assert_eq!(e("floor(8/3)"), "2");
        assert_eq!(e("ceil(8/3)"), "3");
        assert_eq!(e("log(1000, 2)"), "9.9658");
        assert_eq!(e("2^ceil(log(1000, 2))"), "1,024");
        assert_eq!(e("sqrt(9)"), "3");
        assert_eq!(e("log10(1000)"), "3");
        assert_eq!(e("round(2.5)"), "2");
        assert_eq!(e("sign(-100)"), "-1");
    }

    #[test]
    fn trigonometry_accepts_angles_with_units() {
        assert_eq!(e("cos(60°)"), "0.5");
        assert_eq!(e("cos(60 deg)"), "0.5");
        assert_eq!(e("cos(pi/3)"), "0.5");
        assert_eq!(e("atan2(10,10) in °"), "45°");
        assert_eq!(e("acos(0.5) in °"), "60°");
        assert_eq!(e("asin(0.5) in °"), "30°");
        assert_eq!(e("atan(-1) in °"), "-45°");
    }

    #[test]
    fn complex_helpers() {
        assert_eq!(e("sqrt(-10000)"), "100i");
        assert_eq!(e("conj(3i)"), "-3i");
        assert_eq!(e("conj(2 + 3i)"), "-3i + 2");
        assert_eq!(e("re(3 + 5i)"), "3");
        assert_eq!(e("im(3 + 5i)"), "5");
    }

    #[test]
    fn statistics_and_combinatorics() {
        assert_eq!(e("average(85, 92, 95)"), "90.6667");
        assert_eq!(e("mean([85, 92, 95])"), "90.6667");
        assert_eq!(e("choose(8, 3)"), "56");
        assert_eq!(e("nCr(52, 5)"), "2,598,960");
        assert_eq!(e("choose(8, -1)"), "0");
        assert_eq!(e("max(-11, 222, -1, 11)"), "222");
        assert_eq!(e("min([-1, 0, 2])"), "-1");
    }

    #[test]
    fn matrix_operations() {
        assert_eq!(e("[1, 2; 3, 4]^-1"), "[-2, 1; 1.5, -0.5]");
        assert_eq!(e("|[1, 2; 3, 4]|"), "-2");
        assert_eq!(e("inv([1, 2; 3, 4])"), "[-2, 1; 1.5, -0.5]");
        assert_eq!(e("|[a, b; c, d]|"), "a*d - b*c");
        assert_eq!(e("dot([1, 0, 0], [0, 1, 0])"), "0");
        assert_eq!(e("cross([1, 0, 0], [0, 1, 0])"), "[0, 0, 1]");
        assert_eq!(e("dot([a, b], [c, d])"), "a*c + b*d");
        assert_eq!(e("|[1, 2, 3]|"), "3.7417");
        assert_eq!(e("len([1,2])"), "2");
    }

    #[test]
    fn indexes_column_major() {
        let base = &["mat = [0, 1; 2, 3]"];
        assert_eq!(run(&[base[0], "mat[0,0]"]), "0");
        assert_eq!(run(&[base[0], "mat[1,1]"]), "3");
        assert_eq!(run(&[base[0], "mat[1,0]"]), "2");
        // Single-index reads run down the columns.
        assert_eq!(run(&[base[0], "mat[0]"]), "0");
        assert_eq!(run(&[base[0], "mat[1]"]), "2");
        assert_eq!(run(&[base[0], "mat[2]"]), "1");
    }

    #[test]
    fn map_and_reduce() {
        assert_eq!(e("map(cos, [0, pi/4, pi/3])"), "[1, 0.7071, 0.5]");
        assert_eq!(e("map(10*x, [0, 1, 500])"), "[0, 10, 5,000]");
        assert_eq!(e("map(2x, 0..3)"), "[0, 2, 4, 6]");
        // The binder is the free variable, never the builtin being called.
        assert_eq!(e("map(cos(n*pi), 0..2)"), "[1, -1, 1]");
        assert_eq!(e("map(10*y + x, x = [1, 2], y = [3, 4])"), "[31, 42]");
        assert_eq!(e("reduce(acc + x, [1, 2, 3])"), "6");
        assert_eq!(e("reduce(g, [1, 2, 3])"), "g(g(1, 2), 3)");
        assert_eq!(e("filter(x > 0, [-2, -1, 0, 1, 2])"), "[1, 2]");
    }

    #[test]
    fn calling_a_value_applies_its_result() {
        // A stored expression called positionally binds the result's
        // leftover variable: Calca's `t5 = taylor(f, ...)`, `t5(0.7)`.
        assert_eq!(run(&["f(x) = x^2", "t = taylor(f, x=0, 2)", "t(3)"]), "9");
        // A name the document defines is a value the body uses, not a
        // slot an argument may fill: the argument reaches `t`, not `gain`.
        assert_eq!(run(&["gain = 3", "g = gain * t", "g(5)"]), "15");
    }

    #[test]
    fn sums_and_products() {
        assert_eq!(e("sum(x*x, x=1..5)"), "55");
        assert_eq!(e("sum([1, 2, 3])"), "6");
        assert_eq!(e("prod(x, 1..3)"), "6");
        assert_eq!(e("prod(x, [1, 2, 3])"), "6");
        assert_eq!(e("prod(x^2, [1, 2, 3])"), "36");
        assert_eq!(run(&["data = [10, 20, 30]", "sum(x, data)"]), "60");
        assert_eq!(run(&["sq(v) = v^2", "sum(sq, v=1..5)"]), "55");
    }

    #[test]
    fn derivatives() {
        assert_eq!(e("der(x^2, x)"), "2 x");
        assert_eq!(e("∂(x^2, x)"), "2 x");
        let model = "fall(t) = 1/2*a*t^2 + v0*t + x0";
        assert_eq!(run(&[model, "der(fall, t)"]), "a*t + v0");
        assert_eq!(run(&[model, "der(fall, a)"]), "0.5 t^2");
        assert_eq!(run(&[model, "der(fall, t, 2)"]), "a");
    }

    #[test]
    fn time_of_day_decomposes_seconds() {
        assert_eq!(e("tod(7980)"), "2 hr + 13 mins");
        assert_eq!(e("tod(7980s)"), "2 hr + 13 mins");
        assert_eq!(e("tod(-7980)"), "21 hr + 47 mins");
        assert_eq!(e("tod(-2 hours)"), "22 hr");
    }
}
