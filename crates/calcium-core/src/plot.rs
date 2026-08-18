//! Sampling `plot(...)` calls into series of points.
//!
//! The format follows Calca. `plot(sin(5t * 2pi))` plots an expression in
//! its free variable over the default range −1..1; a range as the last
//! argument (`plot(sin(t), 0..2pi)`) or a named binding
//! (`plot(x^2, x = 0..10)`) sets the domain; several arguments are several
//! series on one chart; an array of numbers plots against its index; and a
//! two-column matrix is (x, y) data. A range may sit behind a name or
//! carry a unit: `0..1.5s` binds each sample as a quantity of seconds.
//!
//! Arrays steer the sampling too, Calca's way: a trailing array of numbers
//! gives the exact x coordinates. Expressions before it are evaluated at
//! those points — `plot(sin(x), [0, pi/4, pi/2])` — and arrays before it
//! pair up with them element-wise as (x, y) data — `plot(ys, xs)`. And a
//! two-entry array in a swept variable is a parametric curve:
//! `plot([cos(t), sin(t)], 0..2pi)` draws (x(t), y(t)).
//!
//! The engine samples, the front-ends draw: a `Plot` is nothing but labeled
//! series of finite points, computed with the environment as it stood at
//! the plot's own line. A sample that fails to come out a real number — a
//! symbolic leftover, an error, a pole — is simply skipped, so `1/x` plots
//! as two branches and a typo plots as nothing.

use crate::ast::{Arg, Expr};
use crate::eval::{Ctx, Env};
use crate::lexer::Radix;
use crate::num::Num;

/// Points per expression series: enough for a smooth curve at any editor
/// width while keeping a document's work bounded.
pub const SAMPLES: usize = 256;

/// One `plot(...)` call, sampled and ready to draw.
#[derive(Debug, Clone)]
pub struct Plot {
    /// Source line the call sits on; the drawing belongs below it.
    pub line: usize,
    /// The variable swept along the x axis, when one exists.
    pub x_label: Option<String>,
    /// The unit the sweep carried, rendered — `s` for a `0..1.5s` domain —
    /// so the axis can read `t (s)`.
    pub x_unit: Option<String>,
    /// The unit an `in` conversion asked the series be expressed in,
    /// rendered — `pA` for `plot(i(t) in pA, ...)` — for the vertical axis.
    pub y_unit: Option<String>,
    pub series: Vec<Series>,
}

#[derive(Debug, Clone)]
pub struct Series {
    /// The argument as the document wrote it, for legends.
    pub label: String,
    pub points: Vec<(f64, f64)>,
    /// Whether the points came from sweeping an expression — a dense curve
    /// to draw as a line — rather than literal data worth marking.
    pub swept: bool,
}

/// Samples one `plot` call's arguments. `None` when nothing plottable
/// comes out — no arguments, an unreadable range, series that never
/// produce a number.
pub fn sample(env: &Env, args: &[Arg]) -> Option<Plot> {
    // A plot arrives with whatever fuel its statement has left, which deep
    // in a long document may be almost none. Sampling is its own work, on
    // its own budget: a fresh tank here for the ranges and labels, and a
    // fresh tank per sample in `value_at` — 256 small evaluations rather
    // than one huge one, so a pole still costs only its own point.
    crate::eval::refuel();
    // A named range argument binds the sweep variable and sets the domain:
    // `plot(x^2, x = 0..10)`.
    let mut binder: Option<String> = None;
    let mut domain: Option<(f64, f64)> = None;
    // The unit the domain carries: `0..1.5s` sweeps its variable in
    // seconds, so a formula written for quantities keeps its dimensions.
    let mut x_unit: Option<Expr> = None;
    let mut bodies: Vec<&Expr> = Vec::new();
    for arg in args {
        match (&arg.name, &arg.value) {
            (Some(name), Expr::Range(lo, hi)) => {
                binder = Some(name.clone());
                if let Some((ends, unit)) = range_with_unit(env, lo, hi) {
                    domain = Some(ends);
                    x_unit = unit;
                }
            }
            (Some(_), _) => return None,
            (None, _) => bodies.push(&arg.value),
        }
    }
    // Calca's form: the range as the last of several plain arguments —
    // written out, or held in a name: `range = -0.1..1.4`, `plot(f, range)`.
    if bodies.len() > 1 {
        // Unexpanded, so a range held in a name keeps the unit spelling
        // its author gave it.
        let trailing = bodies.last().and_then(|last| match env.eval(last) {
            Expr::Range(lo, hi) => range_with_unit(env, &lo, &hi),
            _ => None,
        });
        if let Some((ends, unit)) = trailing {
            domain = Some(ends);
            x_unit = unit;
            bodies.pop();
        }
    }
    let domain = domain.unwrap_or((-1.0, 1.0));
    if domain.0 == domain.1 {
        return None;
    }

    // Explicit x coordinates, Calca's other form: a trailing array names
    // the exact points. Expressions before it are evaluated at those
    // points — `plot(sin(x), [0, pi/4, pi/2])` — and arrays before it
    // pair up with them element-wise as (x, y) data — `plot(ys, xs)`.
    let mut sample_xs: Option<Vec<f64>> = None;
    let mut x_name: Option<String> = None;
    if bodies.len() > 1 {
        if let Some(xs) = bodies.last().and_then(|last| numeric_vector(env, last)) {
            if !xs.is_empty() {
                if let Some(Expr::Var(name)) = bodies.last() {
                    x_name = Some(name.clone());
                }
                sample_xs = Some(xs);
                bodies.pop();
            }
        }
    }

    // An `in` on a series names the vertical axis: `plot(i(t) in pA, ...)`
    // plots picoamps under the axis label `pA`. The first conversion sets
    // the axis, and every series scales by it — a base-unit sample divided
    // by the unit's own base value is exactly that sample expressed in it.
    let mut y_unit: Option<Expr> = None;
    let mut y_scale = 1.0;
    let bodies: Vec<&Expr> = bodies
        .into_iter()
        .map(|body| match body {
            Expr::Convert(inner, unit) => {
                if y_unit.is_none() {
                    if let Some(value) = numeric(env, &env.eval_expanded(unit))
                        .filter(|v| v.is_finite() && *v != 0.0)
                    {
                        y_unit = Some((**unit).clone());
                        y_scale = value;
                    }
                }
                inner.as_ref()
            }
            other => other,
        })
        .collect();

    let mut x_label: Option<String> = None;
    let mut series = Vec::new();
    for body in bodies {
        let label = crate::format::render(body);
        if let Some(xs) = &sample_xs {
            if let Some(ys) = numeric_vector(env, body) {
                series.push(Series {
                    label,
                    points: xs.iter().copied().zip(ys).collect(),
                    swept: false,
                });
                continue;
            }
        }
        // A bare name resolves to its definition: `plot(f)` plots a
        // function of its own parameter, `plot(data)` plots stored data.
        let (resolved, params) = resolve(env, body);
        // A two-entry array in a swept variable is a parametric curve:
        // the parameter runs over the domain, the entries are (x, y).
        if let Some((fx, fy)) = parametric_pair(&resolved) {
            let bound = binder
                .clone()
                .or_else(|| params.clone().into_iter().flatten().next())
                .or_else(|| free_variable(env, &resolved));
            if let Some(bound) = bound {
                let (ts, unit) = match &sample_xs {
                    Some(ts) => (ts.clone(), None),
                    None => (steps(domain), x_unit.as_ref()),
                };
                let points = pair_at(env, fx, fy, &bound, &ts, unit);
                if !points.is_empty() {
                    series.push(Series { label, points, swept: sample_xs.is_none() });
                    continue;
                }
            }
        }
        if let Some(points) = data_points(env, &resolved) {
            series.push(Series { label, points, swept: false });
            continue;
        }
        // A defined function sweeps its own parameter, so `plot(f, g)`
        // draws both even when their parameters are named differently.
        let bound = binder
            .clone()
            .or_else(|| params.into_iter().flatten().next())
            .or_else(|| x_label.clone())
            .or_else(|| free_variable(env, &resolved));
        let Some(bound) = bound else { continue };
        let (points, swept) = match &sample_xs {
            Some(xs) => (sweep_at(env, &resolved, &bound, xs, None), false),
            None => (
                sweep_at(env, &resolved, &bound, &steps(domain), x_unit.as_ref()),
                true,
            ),
        };
        if !points.is_empty() {
            x_label.get_or_insert(bound);
            series.push(Series { label, points, swept });
        }
    }
    if series.is_empty() {
        return None;
    }
    if y_scale != 1.0 {
        for series in &mut series {
            for point in &mut series.points {
                point.1 /= y_scale;
            }
        }
    }
    // The unit belongs on the axis only when the axis is the swept
    // variable itself — not explicit sample points, not zipped data.
    let x_unit = match (&x_label, &sample_xs) {
        (Some(_), None) => x_unit.as_ref().map(crate::format::render),
        _ => None,
    };
    Some(Plot {
        line: 0,
        x_label: x_label.or(x_name),
        x_unit,
        y_unit: y_unit.as_ref().map(crate::format::render),
        series,
    })
}

/// A bare name that refers to a definition is replaced by its body, the
/// way `sum(sq, v=1..5)` sums `v^2`. Units stay themselves.
fn resolve(env: &Env, expr: &Expr) -> (Expr, Option<Vec<String>>) {
    if let Expr::Var(name) = expr {
        if let Some(def) = env.get(name) {
            if !def.is_unit {
                return (def.body.clone(), Some(def.params()));
            }
        }
    }
    (expr.clone(), None)
}

/// The first free variable the environment does not define — the same rule
/// `sum(x, data)` uses to pick its binder. Builtin names are never swept.
fn free_variable(env: &Env, body: &Expr) -> Option<String> {
    body.free_vars()
        .into_iter()
        .find(|name| !env.is_defined(name) && !crate::builtins::is_builtin(name))
}

/// A range's endpoints as plain numbers, with the unit they carry:
/// `0..1.5s` is 0 to 1.5 swept in seconds. Endpoints that disagree about
/// their unit refuse — there is no one unit to sweep in.
fn range_with_unit(env: &Env, lo: &Expr, hi: &Expr) -> Option<((f64, f64), Option<Expr>)> {
    // The axis wears the unit the author wrote: `-500 Hz .. 500 Hz` sweeps
    // −500..500 in `Hz`, not the ±500 `1/s` it expands to. Endpoints are
    // read unexpanded first; only when their spellings disagree — `500 Hz`
    // against `0.5 kHz` — does the sweep fall back to base units, where
    // everything agrees or nothing does.
    if let (Some((lo_n, lo_unit)), Some((hi_n, hi_unit))) =
        (quantity_of(env, lo, false), quantity_of(env, hi, false))
    {
        match (lo_unit, hi_unit) {
            (None, unit) | (unit, None) => return Some(((lo_n, hi_n), unit)),
            (Some(a), Some(b)) if a == b => return Some(((lo_n, hi_n), Some(a))),
            _ => {}
        }
    }
    let (lo, lo_unit) = quantity_of(env, lo, true)?;
    let (hi, hi_unit) = quantity_of(env, hi, true)?;
    let unit = match (lo_unit, hi_unit) {
        (None, unit) | (unit, None) => unit,
        (Some(a), Some(b)) if a == b => Some(a),
        _ => return None,
    };
    Some(((lo, hi), unit))
}

/// An endpoint as its coefficient and the unit factors it carries:
/// `1.5s` is 1.5 against `s`, a bare `2` is 2 against nothing. Read
/// unexpanded, the unit is the author's own spelling — `Hz`, `kHz` —
/// and expanded it is base units.
fn quantity_of(env: &Env, expr: &Expr, expanded: bool) -> Option<(f64, Option<Expr>)> {
    let value = if expanded {
        env.eval_expanded(expr)
    } else {
        env.eval(expr)
    };
    let coefficient = numeric(env, &value).filter(|v| v.is_finite())?;
    let unit = match &value {
        Expr::Mul(factors) => {
            let units: Vec<Expr> = factors
                .iter()
                .filter(|f| !matches!(f, Expr::Num(..)))
                .cloned()
                .collect();
            match units.len() {
                0 => None,
                1 => Some(units.into_iter().next().unwrap()),
                _ => Some(Expr::Mul(units)),
            }
        }
        _ => None,
    };
    Some((coefficient, unit))
}

/// The plain number inside a fully evaluated expression. A quantity —
/// a number against unit symbols — yields its coefficient, so unit-laden
/// series plot in base units. Any other leftover symbol — a free
/// variable, the imaginary `i` — means the sample is not a real number.
fn numeric(env: &Env, expr: &Expr) -> Option<f64> {
    if let Some(value) = expr.as_num() {
        return Some(value.to_f64());
    }
    if let Expr::Mul(factors) = expr {
        let mut coefficient: Option<f64> = None;
        for factor in factors {
            match factor {
                Expr::Num(value, _) if coefficient.is_none() => {
                    coefficient = Some(value.to_f64());
                }
                Expr::Var(name) if env.is_unit_name(name) => {}
                Expr::Pow(base, exp) if exp.as_num().is_some() => match &**base {
                    Expr::Var(name) if env.is_unit_name(name) => {}
                    _ => return None,
                },
                _ => return None,
            }
        }
        return coefficient.or(Some(1.0));
    }
    None
}

/// An expression that evaluates to a matrix, as drawable points: a row or
/// column of numbers plots against its index, and a two-column matrix is
/// (x, y) pairs.
fn data_points(env: &Env, expr: &Expr) -> Option<Vec<(f64, f64)>> {
    let Expr::Matrix(rows) = env.eval_expanded(expr) else {
        return None;
    };
    let ys: Option<Vec<&Expr>> = if rows.len() == 1 {
        Some(rows[0].iter().collect())
    } else if rows.iter().all(|r| r.len() == 1) {
        Some(rows.iter().map(|r| &r[0]).collect())
    } else {
        None
    };
    if let Some(ys) = ys {
        return Some(
            ys.iter()
                .enumerate()
                .filter_map(|(i, y)| {
                    Some((i as f64, numeric(env, y).filter(|v| v.is_finite())?))
                })
                .collect(),
        );
    }
    if rows.iter().all(|r| r.len() == 2) {
        return Some(
            rows.iter()
                .filter_map(|r| {
                    let x = numeric(env, &r[0]).filter(|v| v.is_finite())?;
                    let y = numeric(env, &r[1]).filter(|v| v.is_finite())?;
                    Some((x, y))
                })
                .collect(),
        );
    }
    None
}

/// An argument that evaluates to a row or column of plain numbers — a
/// literal array, or the result of `map(...)` — as that list of numbers.
/// Anything symbolic anywhere in it means this is not a vector of data.
fn numeric_vector(env: &Env, expr: &Expr) -> Option<Vec<f64>> {
    let Expr::Matrix(rows) = env.eval_expanded(expr) else {
        return None;
    };
    let entries: Vec<&Expr> = if rows.len() == 1 {
        rows[0].iter().collect()
    } else if rows.iter().all(|row| row.len() == 1) {
        rows.iter().map(|row| &row[0]).collect()
    } else {
        return None;
    };
    entries
        .iter()
        .map(|entry| numeric(env, entry).filter(|v| v.is_finite()))
        .collect()
}

/// A two-entry array — one row of two, or two rows of one — as its
/// (x, y) expressions, the shape a parametric curve is written in.
fn parametric_pair(expr: &Expr) -> Option<(&Expr, &Expr)> {
    let Expr::Matrix(rows) = expr else {
        return None;
    };
    match rows.as_slice() {
        [row] if row.len() == 2 => Some((&row[0], &row[1])),
        [a, b] if a.len() == 1 && b.len() == 1 => Some((&a[0], &b[0])),
        _ => None,
    }
}

/// The evenly spaced sample positions across a domain.
fn steps((lo, hi): (f64, f64)) -> Vec<f64> {
    (0..SAMPLES)
        .map(|index| lo + (hi - lo) * index as f64 / (SAMPLES - 1) as f64)
        .collect()
}

/// One sample: the expression with `bound` set to `x` — carrying the
/// domain's unit, so `0..1.5s` binds `0.37 s`, not `0.37` — as a finite
/// number.
fn value_at(env: &Env, body: &Expr, bound: &str, x: f64, unit: Option<&Expr>) -> Option<f64> {
    // Each sample evaluates on a full tank; see `sample`.
    crate::eval::refuel();
    let mut ctx = Ctx {
        expand_units: true,
        ..Ctx::default()
    };
    let sample = Expr::Num(Num::from_f64(x), Radix::Dec);
    let sample = match unit {
        Some(unit) => Expr::mul(vec![sample, unit.clone()]),
        None => sample,
    };
    ctx.locals.insert(bound.to_string(), sample);
    let value = crate::simplify::simplify(&env.eval_in(body, &mut ctx));
    numeric(env, &value).filter(|v| v.is_finite())
}

/// Samples an expression at each position, with `bound` set to it.
fn sweep_at(
    env: &Env,
    body: &Expr,
    bound: &str,
    xs: &[f64],
    unit: Option<&Expr>,
) -> Vec<(f64, f64)> {
    xs.iter()
        .filter_map(|&x| Some((x, value_at(env, body, bound, x, unit)?)))
        .collect()
}

/// Samples a parametric pair at each parameter value; a sample where
/// either coordinate refuses to be a real number is skipped whole.
fn pair_at(
    env: &Env,
    fx: &Expr,
    fy: &Expr,
    bound: &str,
    ts: &[f64],
    unit: Option<&Expr>,
) -> Vec<(f64, f64)> {
    ts.iter()
        .filter_map(|&t| {
            Some((
                value_at(env, fx, bound, t, unit)?,
                value_at(env, fy, bound, t, unit)?,
            ))
        })
        .collect()
}

/// A sampled coordinate as compact text, shared by the JSON boundary and
/// the Typst export: six significant digits, scientific only at the
/// extremes, always a valid JSON and Typst number.
pub fn format_point(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let magnitude = value.abs();
    if (1e-4..1e9).contains(&magnitude) {
        let text = format!("{value:.6}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        text.to_string()
    } else {
        format!("{value:.5e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_expr;

    fn plot_args(source: &str) -> Vec<Arg> {
        match parse_expr(source) {
            Expr::Call(name, args) if name == "plot" => args,
            other => panic!("not a plot call: {other:?}"),
        }
    }

    fn sample_source(defs: &str, call: &str) -> Option<Plot> {
        let mut env = Env::with_prelude();
        crate::doc::evaluate_in(defs, &mut env);
        crate::eval::refuel();
        sample(&env, &plot_args(call))
    }

    #[test]
    fn an_expression_plots_over_the_default_range() {
        let plot = sample_source("", "plot(t^2)").unwrap();
        assert_eq!(plot.x_label.as_deref(), Some("t"));
        assert_eq!(plot.series.len(), 1);
        assert_eq!(plot.series[0].label, "t^2");
        let points = &plot.series[0].points;
        assert_eq!(points.len(), SAMPLES);
        assert_eq!(points.first().unwrap().0, -1.0);
        assert_eq!(points.last().unwrap().0, 1.0);
        assert!((points.first().unwrap().1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_named_range_binds_the_variable_and_the_domain() {
        let plot = sample_source("", "plot(x^2, x = 0..10)").unwrap();
        assert_eq!(plot.x_label.as_deref(), Some("x"));
        let points = &plot.series[0].points;
        assert_eq!(points.first().unwrap().0, 0.0);
        assert_eq!(points.last().unwrap().0, 10.0);
        assert!((points.last().unwrap().1 - 100.0).abs() < 1e-6);
    }

    #[test]
    fn a_trailing_range_is_the_domain_calca_style() {
        let plot = sample_source("", "plot(sin(t), 0..2pi)").unwrap();
        let points = &plot.series[0].points;
        assert_eq!(points.first().unwrap().0, 0.0);
        assert!((points.last().unwrap().0 - 2.0 * std::f64::consts::PI).abs() < 1e-9);
    }

    #[test]
    fn several_arguments_are_several_series() {
        let plot = sample_source("", "plot(sin(t), cos(t), 0..1)").unwrap();
        assert_eq!(plot.series.len(), 2);
        assert_eq!(plot.series[0].label, "sin(t)");
        assert_eq!(plot.series[1].label, "cos(t)");
    }

    #[test]
    fn an_array_plots_against_its_index() {
        let plot = sample_source("", "plot([3, 1, 4, 1, 5])").unwrap();
        assert!(plot.x_label.is_none());
        let points = &plot.series[0].points;
        assert_eq!(points.len(), 5);
        assert_eq!(points[0], (0.0, 3.0));
        assert_eq!(points[4], (4.0, 5.0));
    }

    #[test]
    fn a_two_column_matrix_is_xy_data() {
        let plot = sample_source("", "plot([0, 1; 2, 4; 3, 9])").unwrap();
        let points = &plot.series[0].points;
        assert_eq!(points, &[(0.0, 1.0), (2.0, 4.0), (3.0, 9.0)]);
    }

    #[test]
    fn a_trailing_array_pins_the_sample_points() {
        let plot = sample_source("", "plot(sin(x), [0, pi/4, 2pi/4, 3pi/4, pi])").unwrap();
        assert_eq!(plot.x_label.as_deref(), Some("x"));
        assert_eq!(plot.series.len(), 1);
        let points = &plot.series[0].points;
        assert!(!plot.series[0].swept);
        assert_eq!(points.len(), 5);
        assert!((points[0].1 - 0.0).abs() < 1e-9);
        assert!((points[2].0 - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert!((points[2].1 - 1.0).abs() < 1e-9);
        assert!(points[4].1.abs() < 1e-9);
    }

    #[test]
    fn a_trailing_vector_pairs_arrays_into_xy_data() {
        // Calca's parametric data form: `plot(ys, xs)`, x coordinates last.
        let defs = "    xs = map(cos(n*2pi/8), 0..8)\n    ys = map(sin(n*2pi/8), 0..8)\n";
        let plot = sample_source(defs, "plot(ys, xs)").unwrap();
        assert_eq!(plot.x_label.as_deref(), Some("xs"));
        assert_eq!(plot.series.len(), 1);
        assert_eq!(plot.series[0].label, "ys");
        assert!(!plot.series[0].swept);
        let points = &plot.series[0].points;
        assert_eq!(points.len(), 9);
        assert!((points[0].0 - 1.0).abs() < 1e-9);
        assert!(points[0].1.abs() < 1e-9);
        assert!((points[2].0).abs() < 1e-9);
        assert!((points[2].1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_named_range_sets_the_domain() {
        let plot = sample_source("    span = 0..10\n", "plot(x^2, span)").unwrap();
        let points = &plot.series[0].points;
        assert_eq!(points.first().unwrap().0, 0.0);
        assert_eq!(points.last().unwrap().0, 10.0);
    }

    #[test]
    fn functions_with_different_parameters_plot_together() {
        let defs = "    f(x) = x^2\n    g(z) = 400z^3\n";
        let plot = sample_source(defs, "plot(f, g)").unwrap();
        assert_eq!(plot.series.len(), 2);
        assert_eq!(plot.x_label.as_deref(), Some("x"));
        assert!((plot.series[0].points.last().unwrap().1 - 1.0).abs() < 1e-9);
        assert!((plot.series[1].points.last().unwrap().1 - 400.0).abs() < 1e-6);
    }

    #[test]
    fn an_expression_pair_is_a_parametric_curve() {
        let plot = sample_source("", "plot([cos(t), sin(t)], 0..2pi)").unwrap();
        assert!(plot.x_label.is_none());
        assert_eq!(plot.series.len(), 1);
        assert!(plot.series[0].swept);
        let points = &plot.series[0].points;
        assert_eq!(points.len(), SAMPLES);
        assert!((points.first().unwrap().0 - 1.0).abs() < 1e-9);
        // The curve comes back around: x reaches both ends of the circle.
        assert!(points.iter().any(|(x, _)| *x < -0.99));
        assert!((points.last().unwrap().0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_numeric_pair_is_still_data_not_parametric() {
        let plot = sample_source("", "plot([1, 2])").unwrap();
        assert_eq!(plot.series[0].points, vec![(0.0, 1.0), (1.0, 2.0)]);
    }

    #[test]
    fn a_defined_function_plots_its_own_parameter() {
        let plot = sample_source("    f(u) = u^3\n", "plot(f)").unwrap();
        assert_eq!(plot.x_label.as_deref(), Some("u"));
        let points = &plot.series[0].points;
        assert!((points.last().unwrap().1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn document_definitions_feed_the_sweep() {
        let plot = sample_source("    gain = 3\n", "plot(gain*t, 0..1)").unwrap();
        assert!((plot.series[0].points.last().unwrap().1 - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_unit_carrying_range_sweeps_quantities() {
        // Calca's thrown ball: the formula wants `t` as a time, and the
        // range says so. Every sample binds `t = x·s`, not a bare number.
        let defs = "    height of ball(t) = -9.8m/s^2 * t^2 + 30mi/hour * t in ft\n";
        let plot = sample_source(defs, "plot(height of ball(t), 0..1.5s)").unwrap();
        assert_eq!(plot.x_label.as_deref(), Some("t"));
        assert_eq!(plot.x_unit.as_deref(), Some("s"));
        let points = &plot.series[0].points;
        assert_eq!(points.len(), SAMPLES);
        assert_eq!(points.first().unwrap().0, 0.0);
        assert_eq!(points.last().unwrap().0, 1.5);
        // The ball rises and comes back down: a real arc, not one dead point.
        assert!(points.iter().any(|(_, y)| *y > 1.0));
        assert!(points.last().unwrap().1 < points.iter().fold(0.0f64, |m, p| m.max(p.1)));
    }

    #[test]
    fn unit_laden_series_plot_in_base_units() {
        let plot = sample_source("", "plot(t * 2 mA, 0..1)").unwrap();
        // 2 mA is 0.002 A: the coefficient in base units.
        assert!((plot.series[0].points.last().unwrap().1 - 0.002).abs() < 1e-9);
    }

    #[test]
    fn poles_become_gaps_not_points() {
        // The first sample sits exactly on the pole of 1/t.
        let plot = sample_source("", "plot(1/t, 0..1)").unwrap();
        let points = &plot.series[0].points;
        assert!(points.len() < SAMPLES);
        assert!(points.iter().all(|(_, y)| y.is_finite()));
    }

    #[test]
    fn complex_samples_are_gaps_not_coefficients() {
        // sqrt of a negative is i-something: not a point, not its magnitude.
        let plot = sample_source("", "plot(sqrt(t))").unwrap();
        let points = &plot.series[0].points;
        assert!(points.iter().all(|(x, _)| *x >= 0.0), "got: {points:?}");
    }

    #[test]
    fn nothing_plottable_is_no_plot() {
        assert!(sample_source("", "plot()").is_none());
        assert!(sample_source("", "plot(t, 5..5)").is_none());
        assert!(sample_source("", "plot(\"words\")").is_none());
    }

    #[test]
    fn an_unknown_name_sweeps_as_itself() {
        // A free name — multi-word names included — is its own axis.
        let plot = sample_source("", "plot(half life)").unwrap();
        assert_eq!(plot.x_label.as_deref(), Some("half life"));
        assert_eq!(plot.series[0].points.last().unwrap().1, 1.0);
    }

    #[test]
    fn the_axis_wears_the_unit_the_author_wrote() {
        // Hz expands to 1/s inside the engine, but the axis shows what
        // the document says: −500..500 in hertz.
        let plot = sample_source("", "plot(f * 2, f = -500 Hz .. 500 Hz)").unwrap();
        assert_eq!(plot.x_unit.as_deref(), Some("Hz"));
        let points = &plot.series[0].points;
        assert_eq!(points.first().unwrap().0, -500.0);
        assert_eq!(points.last().unwrap().0, 500.0);
    }

    #[test]
    fn an_in_conversion_names_and_scales_the_vertical_axis() {
        let plot = sample_source("", "plot(t * 2 mA in mA, 0..1)").unwrap();
        assert_eq!(plot.y_unit.as_deref(), Some("mA"));
        // 2 mA at t = 1, expressed in mA: 2, not the 0.002 A of base units.
        assert!((plot.series[0].points.last().unwrap().1 - 2.0).abs() < 1e-9);
        // The axis carries the unit, so the legend does not repeat it.
        assert!(!plot.series[0].label.contains(" in "), "got {:?}", plot.series[0].label);
    }

    #[test]
    fn a_spent_tank_still_draws_whole_curves() {
        // A plot deep in a long document arrives after its statement's
        // budget is mostly gone. Sampling refuels for itself, so the curve
        // spans the whole domain instead of stopping mid-sweep.
        let mut env = Env::with_prelude();
        crate::doc::evaluate_in("    f(x) = x^2\n", &mut env);
        while crate::eval::spend_fuel() {}
        let plot = sample(&env, &plot_args("plot(f(x), x = 0..1)")).unwrap();
        let points = &plot.series[0].points;
        assert_eq!(points.len(), SAMPLES);
        assert_eq!(points.first().unwrap().0, 0.0);
        assert_eq!(points.last().unwrap().0, 1.0);
    }

    #[test]
    fn points_format_compactly() {
        assert_eq!(format_point(0.0), "0");
        assert_eq!(format_point(0.5), "0.5");
        assert_eq!(format_point(-0.099833416646), "-0.099833");
        assert_eq!(format_point(2.5e-19), "2.50000e-19");
        assert_eq!(format_point(3.0), "3");
    }
}
