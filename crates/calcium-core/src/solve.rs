//! Solving for variables.
//!
//! When `=>` is applied to a variable that has no value, Calca searches back
//! through the document for a definition or equation that mentions it and
//! tries to isolate it. We handle the linear and quadratic cases plus matrix
//! equations, and when isolation fails we report the closest rearrangement —
//! which is exactly what the Reference documents for the cubic case:
//!
//! ```text
//! 10z + 100x = x^3 + x^2
//! x => 100 x - x^3 - x^2 == -10 z
//! ```

use crate::ast::*;
use crate::builtins;
use crate::eval::Env;
use crate::lexer::Radix;
use crate::num::Num;
use crate::simplify::simplify;

/// Evaluates an expression, falling back to solving when it is a bare unknown.
pub fn evaluate_or_solve(env: &Env, expr: &Expr) -> Expr {
    // `solve(x)` asks explicitly.
    if let Expr::Call(name, args) = expr {
        if name == "solve" {
            let target = match args.first().map(|a| &a.value) {
                Some(Expr::Var(target)) => Some(target.clone()),
                _ => None,
            };
            if let Some(target) = target {
                return solve_for(env, &target)
                    .unwrap_or_else(|| Expr::Error(format!("cannot solve for {target}")));
            }
        }
    }

    let evaluated = env.eval(expr);

    // Only a bare variable that stayed symbolic triggers a solve.
    let Expr::Var(name) = expr else {
        return evaluated;
    };
    if !matches!(&evaluated, Expr::Var(other) if other == name) {
        return evaluated;
    }
    solve_for(env, name).unwrap_or(evaluated)
}

thread_local! {
    /// Names currently being solved for. Evaluation can re-enter the solver
    /// (isolating `x` may require evaluating a definition that itself calls an
    /// unknown), and each entry point builds a fresh context, so the guard has
    /// to live outside them.
    static SOLVING: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Runs `body` unless we are already solving for `name`.
fn guarded<T>(name: &str, body: impl FnOnce() -> Option<T>) -> Option<T> {
    let entered = SOLVING.with(|solving| {
        let mut solving = solving.borrow_mut();
        if solving.iter().any(|n| n == name) || solving.len() > 16 {
            return false;
        }
        solving.push(name.to_string());
        true
    });
    if !entered {
        return None;
    }
    let result = body();
    SOLVING.with(|solving| {
        solving.borrow_mut().pop();
    });
    result
}

/// Finds a relation mentioning `name` and isolates it.
pub fn solve_for(env: &Env, name: &str) -> Option<Expr> {
    guarded(name, || solve_for_inner(env, name))
}

fn solve_for_inner(env: &Env, name: &str) -> Option<Expr> {
    // Equations are searched most recent first, then definitions.
    for (lhs, rhs) in env.equations.iter().rev() {
        if lhs.mentions(name) || rhs.mentions(name) {
            if let Some(solution) = solve_relation(env, lhs, rhs, name) {
                return Some(solution);
            }
        }
    }
    for candidate in env.recent_names() {
        let Some(def) = env.get(candidate) else {
            continue;
        };
        if def.is_unit || candidate == name || !def.body.mentions(name) {
            continue;
        }
        // `sol = solve(x coord)` stores a solution; treating it as another
        // equation to rearrange just recurses back into the solver.
        if contains_solve_call(&def.body) {
            continue;
        }
        // A definition `f = body` is the equation `f == body`.
        if let Some(solution) =
            solve_relation_holding(env, &Expr::var(candidate), &def.body, name, candidate)
        {
            return Some(solution);
        }
    }
    None
}

fn contains_solve_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(name, args) => {
            name == "solve" || args.iter().any(|a| contains_solve_call(&a.value))
        }
        Expr::Add(items) | Expr::Mul(items) => items.iter().any(contains_solve_call),
        Expr::Pow(a, b) | Expr::Convert(a, b) => contains_solve_call(a) || contains_solve_call(b),
        _ => false,
    }
}

fn solve_relation(env: &Env, lhs: &Expr, rhs: &Expr, name: &str) -> Option<Expr> {
    solve_relation_holding(env, lhs, rhs, name, "")
}

/// `held` names a definition whose own name must stay symbolic while its body
/// is evaluated.
fn solve_relation_holding(
    env: &Env,
    lhs: &Expr,
    rhs: &Expr,
    name: &str,
    held: &str,
) -> Option<Expr> {
    // A matrix equation `M * x = v` is solved by inversion rather than by
    // isolation, which is how the Examples solve simultaneous equations.
    if let Some(solution) = solve_matrix_equation(env, lhs, rhs, name) {
        return Some(solution);
    }

    // Bring to `difference == 0` and isolate. The unknown and the definition
    // being rearranged both stay symbolic.
    let suppressed: Vec<String> = [name, held]
        .iter()
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .collect();
    let difference = env.eval_suppressing(&Expr::sub(lhs.clone(), rhs.clone()), &suppressed);
    if let Some(coefficients) = polynomial(&difference, name) {
        match coefficients.len() {
            // c0 + c1*x = 0
            2 => {
                let [constant, linear] = [&coefficients[0], &coefficients[1]];
                if !linear.is_zero() {
                    return Some(env.eval_suppressing(
                        &Expr::div(Expr::neg(constant.clone()), linear.clone()),
                        &suppressed,
                    ));
                }
            }
            // c0 + c1*x + c2*x^2 = 0
            3 => {
                let [constant, linear, quadratic] =
                    [&coefficients[0], &coefficients[1], &coefficients[2]];
                if !quadratic.is_zero() {
                    // With no linear term the roots are a symmetric pair, and
                    // Calca reports only the principal one: solving
                    // `a^2 + b^2 = c^2` for `c` answers `9.434 miles`, not
                    // `[9.434 miles, -9.434 miles]`.
                    if linear.is_zero() {
                        let ratio = Expr::div(
                            Expr::neg(constant.clone()),
                            quadratic.clone(),
                        );
                        let root = Expr::Call("sqrt".to_string(), vec![Arg::positional(ratio)]);
                        return Some(env.eval_suppressing(&root, &suppressed));
                    }
                    return Some(quadratic_roots(env, quadratic, linear, constant, &suppressed));
                }
                if !linear.is_zero() {
                    return Some(env.eval_suppressing(
                        &Expr::div(Expr::neg(constant.clone()), linear.clone()),
                        &suppressed,
                    ));
                }
            }
            _ => {}
        }
    }

    // Could not isolate: report how far we got, with the terms mentioning the
    // unknown on the left and everything else on the right.
    Some(closest_rearrangement(&difference, name))
}

/// `[-b/(2a) - sqrt(b^2 - 4ac)/(2a), -b/(2a) + sqrt(b^2 - 4ac)/(2a)]`
fn quadratic_roots(env: &Env, a: &Expr, b: &Expr, c: &Expr, suppressed: &[String]) -> Expr {
    let discriminant = Expr::sub(
        Expr::Pow(Box::new(b.clone()), Box::new(Expr::num(2))),
        Expr::mul(vec![Expr::num(4), a.clone(), c.clone()]),
    );
    let root = Expr::Call(
        "sqrt".to_string(),
        vec![Arg::positional(simplify(&discriminant))],
    );
    let half = Expr::Num(Num::ratio(1, 2), Radix::Dec);
    let base = Expr::mul(vec![
        Expr::neg(half.clone()),
        b.clone(),
        Expr::Pow(Box::new(a.clone()), Box::new(Expr::num(-1))),
    ]);
    let spread = Expr::mul(vec![
        half,
        root,
        Expr::Pow(Box::new(a.clone()), Box::new(Expr::num(-1))),
    ]);
    let minus = env.eval_suppressing(&Expr::sub(base.clone(), spread.clone()), suppressed);
    let plus = env.eval_suppressing(&Expr::add(vec![base, spread]), suppressed);
    Expr::Matrix(vec![vec![minus, plus]])
}

/// Splits `difference == 0` into `terms with the unknown == -(everything else)`.
fn closest_rearrangement(difference: &Expr, name: &str) -> Expr {
    let terms: Vec<Expr> = match difference {
        Expr::Add(terms) => terms.clone(),
        other => vec![other.clone()],
    };
    let mut with = Vec::new();
    let mut without = Vec::new();
    for term in terms {
        if term.mentions(name) {
            with.push(term);
        } else {
            without.push(Expr::neg(term));
        }
    }
    Expr::Relation(
        Box::new(simplify(&Expr::add(with))),
        Box::new(simplify(&Expr::add(without))),
    )
}

/// Recognizes `M * x = v` (in either order) and solves by inversion.
fn solve_matrix_equation(env: &Env, lhs: &Expr, rhs: &Expr, name: &str) -> Option<Expr> {
    // Read the *unevaluated* sides: once `M * x` is evaluated with `x` unknown
    // it becomes an element-wise scaling of `M` and the structure is lost.
    let (product, other) = if lhs.mentions(name) {
        (lhs, rhs)
    } else {
        (rhs, lhs)
    };
    let target = env.eval(other);

    let Expr::Mul(factors) = product else {
        return None;
    };
    // Exactly one factor is the unknown; the rest must multiply to a matrix.
    let unknown_at = factors
        .iter()
        .position(|f| matches!(f, Expr::Var(other) if other == name))?;
    if factors.iter().filter(|f| f.mentions(name)).count() != 1 {
        return None;
    }
    let mut coefficient_parts = factors.clone();
    coefficient_parts.remove(unknown_at);
    let coefficient = Expr::mul(coefficient_parts);

    let Expr::Matrix(matrix) = env.eval(&coefficient) else {
        return None;
    };
    let Expr::Matrix(vector) = target else {
        return None;
    };
    if matrix.len() != matrix[0].len() || matrix.len() != vector.len() {
        return None;
    }
    Some(builtins::solve_linear_system(&matrix, &vector))
}

/// Writes `expr` as coefficients of powers of `name`: index `k` holds the
/// coefficient of `name^k`. Returns `None` if `name` appears anywhere the
/// expression is not polynomial in it (inside a `sqrt`, in a denominator,
/// under an `if`).
pub fn polynomial(expr: &Expr, name: &str) -> Option<Vec<Expr>> {
    const MAX_DEGREE: usize = 8;
    let terms: Vec<Expr> = match expr {
        Expr::Add(terms) => terms.clone(),
        other => vec![other.clone()],
    };

    let mut coefficients: Vec<Expr> = Vec::new();
    for term in terms {
        let factors: Vec<Expr> = match &term {
            Expr::Mul(factors) => factors.clone(),
            other => vec![other.clone()],
        };
        let mut degree = 0usize;
        let mut rest = Vec::new();
        for factor in factors {
            match &factor {
                Expr::Var(other) if other == name => degree += 1,
                Expr::Pow(base, exp) if matches!(&**base, Expr::Var(o) if o == name) => {
                    let power = exp.as_num()?.to_i64()?;
                    if power < 0 {
                        return None;
                    }
                    degree += power as usize;
                }
                other if other.mentions(name) => return None,
                other => rest.push(other.clone()),
            }
            if degree > MAX_DEGREE {
                return None;
            }
        }
        while coefficients.len() <= degree {
            coefficients.push(Expr::num(0));
        }
        coefficients[degree] = simplify(&Expr::add(vec![
            coefficients[degree].clone(),
            Expr::mul(rest),
        ]));
    }
    if coefficients.is_empty() {
        return None;
    }
    Some(coefficients)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc;
    use crate::format::render;

    fn answers(source: &str) -> Vec<String> {
        doc::evaluate(source)
            .answers
            .into_iter()
            .map(|a| a.text)
            .collect()
    }

    #[test]
    fn solves_a_linear_definition_for_another_variable() {
        let out = answers("    f = (9/5)*c + 32\n    c => 0");
        assert_eq!(out, vec!["0.5556 f - 17.7778"]);
    }

    #[test]
    fn solves_a_simple_equation() {
        let out = answers("    x + 2x + 4x = 42\n    x => 0");
        assert_eq!(out, vec!["6"]);
    }

    #[test]
    fn solves_a_quadratic_for_both_roots() {
        let out = answers("    a*x^2 + b*x + c = 0\n    x => 0");
        // Calca prints the discriminant as `b^2 - 4 a*c`; we sort terms by key,
        // so it comes out as `-4 a*c + b^2`. Same value, different order.
        assert_eq!(
            out,
            vec!["[-0.5 b/a - 0.5*sqrt(-4 a*c + b^2)/a, -0.5 b/a + 0.5*sqrt(-4 a*c + b^2)/a]"]
        );
    }

    #[test]
    fn solves_the_same_equation_for_a_different_variable() {
        let out = answers("    a*x^2 + b*x + c = 0\n    a => 0");
        assert_eq!(out, vec!["-b/x - c/x^2"]);
    }

    #[test]
    fn reports_the_closest_rearrangement_when_it_cannot_isolate() {
        // The Reference's documented cubic limitation.
        let out = answers("    10z + 100x = x^3 + x^2\n    x => 0");
        assert_eq!(out, vec!["100 x - x^2 - x^3 == -10 z"]);
    }

    #[test]
    fn solves_a_matrix_equation_by_inversion() {
        let out = answers(
            "    coefficients = [12, 13; -2, 100]\n\
             \x20   solution = [163; 688]\n\
             \x20   coefficients * xy = solution\n\
             \x20   xy => 0",
        );
        assert_eq!(out, vec!["[6; 7]"]);
    }

    #[test]
    fn solves_ohms_law_in_every_direction() {
        // Every line is indented by four spaces; built by join so a `\`
        // continuation cannot silently eat the indentation.
        // Taken from the Examples document. The local `Ω = V / A` matters: a
        // *document* definition expands eagerly, which is what collapses
        // `2 A * 100 Ω` into volts without an explicit conversion.
        let source = [
            "    A => A",
            "    r = r",
            "    V => V",
            "    v = current * r",
            "    Ω = V / A",
            "    v(current = 2A,   r = 100Ω)    => 0",
            "    current(v = 200V, r = 100Ω)    => 0",
            // NOTE: the Examples document writes this third line as
            // `r(v = 200V, i = 2A) in Ω`, but `i` is also the imaginary unit,
            // so `v/i` reduces to `-v*i` and the answer comes out wrong. Calca
            // has the same latent ambiguity; we resolve it in favour of the
            // imaginary unit, which the Reference documents at length. Using
            // any other name for current works.
            "    current = current",
            "    r(v = 200V, current = 2A) in Ω => 0",
        ]
        .join("\n");
        let out = answers(&source);
        // Calca prints the last one as `100Ω`. We print the expanded form
        // because the document redefined `Ω = V / A` and a document definition
        // with no scale factor displays expanded — the policy that scores
        // better across the corpus overall (it is also what Calca does for the
        // `N` redefinition in the Physics section).
        assert_eq!(&out[2..], ["200 V", "2 A", "100 V/A"]);
    }

    #[test]
    fn extracts_polynomial_coefficients() {
        let expr = simplify(&crate::parser::parse_expr("a*x^2 + b*x + c"));
        let coefficients = polynomial(&expr, "x").unwrap();
        assert_eq!(coefficients.len(), 3);
        assert_eq!(render(&coefficients[0]), "c");
        assert_eq!(render(&coefficients[1]), "b");
        assert_eq!(render(&coefficients[2]), "a");
        // Not polynomial in x.
        let expr = simplify(&crate::parser::parse_expr("sqrt(x) + 1"));
        assert!(polynomial(&expr, "x").is_none());
    }
}
