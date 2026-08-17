//! Expression and statement trees.
//!
//! Subtraction and division are *not* represented. `a - b` is stored as
//! `Add[a, Mul[-1, b]]` and `a / b` as `Mul[a, Pow[b, -1]]`. That makes
//! `Add` and `Mul` flat, associative, commutative bags, which is what the
//! simplifier needs to collect like terms. The formatter reconstructs `-`
//! and `/` on the way out.

use crate::lexer::Radix;
use crate::num::Num;

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(Num, Radix),
    Str(String),
    Bool(bool),
    /// A name. Undefined names are legal and simply stay symbolic — that is
    /// how units, chemical elements and free algebraic variables all work.
    Var(String),

    Add(Vec<Expr>),
    Mul(Vec<Expr>),
    Pow(Box<Expr>, Box<Expr>),

    Call(String, Vec<Arg>),
    /// `f[i]` / `m[r, c]` — indexing, always 0-based.
    Index(Box<Expr>, Vec<Expr>),

    /// Rows of columns. A vector is a matrix with one row or one column.
    Matrix(Vec<Vec<Expr>>),
    /// `lo..hi`, inclusive. Doubles as an interval for interval arithmetic.
    Range(Box<Expr>, Box<Expr>),
    /// `value ± sigma` — a measured value with a one-sigma uncertainty,
    /// propagated through calculations by first-order error analysis.
    PlusMinus(Box<Expr>, Box<Expr>),
    Dict(Vec<(String, Expr)>),

    /// `|x|` — absolute value, vector length, or determinant depending on
    /// what `x` turns out to be.
    Abs(Box<Expr>),
    /// `||v||p` — the p-norm, defaulting to 2.
    Norm(Box<Expr>, Option<Box<Expr>>),
    /// `m^T`
    Transpose(Box<Expr>),

    Cmp(CmpOp, Box<Expr>, Box<Expr>),
    Logic(LogicOp, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Bit(BitOp, Box<Expr>, Box<Expr>),
    Mod(Box<Expr>, Box<Expr>),

    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Let(String, Box<Expr>, Box<Expr>),

    /// `value in unit` — re-express `value` in terms of `unit`.
    Convert(Box<Expr>, Box<Expr>),
    /// An unsolved relation, `lhs == rhs`, kept as a value so a failed solve
    /// can report how far it got.
    Relation(Box<Expr>, Box<Expr>),

    /// `#?` — an unresolved AI autocomplete request.
    AiQuery,
    /// A parse or evaluation failure, carried inline so one bad line does not
    /// stop the document.
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// `f(y=5)` passes `y` by name rather than position.
    pub name: Option<String>,
    pub value: Expr,
}

impl Arg {
    pub fn positional(value: Expr) -> Arg {
        Arg { name: None, value }
    }
    pub fn named(name: impl Into<String>, value: Expr) -> Arg {
        Arg {
            name: Some(name.into()),
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicOp {
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitOp {
    And,
    Or,
}

impl Expr {
    pub fn num(v: i64) -> Expr {
        Expr::Num(Num::from_i64(v), Radix::Dec)
    }

    pub fn var(name: impl Into<String>) -> Expr {
        Expr::Var(name.into())
    }

    /// Builds a product, flattening nested `Mul`s so the bag stays flat.
    /// Everything downstream — the simplifier, the formatter — assumes this.
    pub fn mul(factors: Vec<Expr>) -> Expr {
        let mut flat = Vec::with_capacity(factors.len());
        for factor in factors {
            match factor {
                Expr::Mul(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        match flat.len() {
            0 => Expr::num(1),
            1 => flat.pop().unwrap(),
            _ => Expr::Mul(flat),
        }
    }

    /// Builds a sum, flattening nested `Add`s.
    pub fn add(terms: Vec<Expr>) -> Expr {
        let mut flat = Vec::with_capacity(terms.len());
        for term in terms {
            match term {
                Expr::Add(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        match flat.len() {
            0 => Expr::num(0),
            1 => flat.pop().unwrap(),
            _ => Expr::Add(flat),
        }
    }

    pub fn neg(e: Expr) -> Expr {
        // Fold the sign into a literal so `a^-3` stays a simple exponent
        // rather than becoming `a^(-1*3)`.
        if let Expr::Num(value, radix) = &e {
            return Expr::Num(value.neg(), *radix);
        }
        Expr::mul(vec![Expr::num(-1), e])
    }

    pub fn sub(a: Expr, b: Expr) -> Expr {
        Expr::add(vec![a, Expr::neg(b)])
    }

    pub fn div(a: Expr, b: Expr) -> Expr {
        Expr::mul(vec![a, Expr::Pow(Box::new(b), Box::new(Expr::num(-1)))])
    }

    pub fn as_num(&self) -> Option<&Num> {
        match self {
            Expr::Num(n, _) => Some(n),
            _ => None,
        }
    }

    pub fn is_zero(&self) -> bool {
        matches!(self, Expr::Num(n, _) if n.is_zero())
    }

    pub fn is_one(&self) -> bool {
        matches!(self, Expr::Num(n, _) if n.is_one())
    }

    /// Every distinct free name in the tree, in order of first appearance.
    /// Calca uses exactly this order to build the implicit parameter list of a
    /// definition that does not declare one.
    pub fn free_vars(&self) -> Vec<String> {
        let mut found = Vec::new();
        self.collect_vars(&mut found, &mut Vec::new());
        found
    }

    fn collect_vars(&self, found: &mut Vec<String>, bound: &mut Vec<String>) {
        let note = |name: &String, found: &mut Vec<String>| {
            if !bound.contains(name) && !found.contains(name) {
                found.push(name.clone());
            }
        };
        match self {
            Expr::Var(name) => note(name, found),
            Expr::Num(..) | Expr::Str(_) | Expr::Bool(_) | Expr::AiQuery | Expr::Error(_) => {}
            Expr::Add(items) | Expr::Mul(items) => {
                for item in items {
                    item.collect_vars(found, bound);
                }
            }
            Expr::Matrix(rows) => {
                for row in rows {
                    for cell in row {
                        cell.collect_vars(found, bound);
                    }
                }
            }
            Expr::Dict(entries) => {
                for (_, value) in entries {
                    value.collect_vars(found, bound);
                }
            }
            Expr::Call(name, args) => {
                note(name, found);
                for arg in args {
                    arg.value.collect_vars(found, bound);
                }
            }
            Expr::Index(base, indices) => {
                base.collect_vars(found, bound);
                for index in indices {
                    index.collect_vars(found, bound);
                }
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
                a.collect_vars(found, bound);
                b.collect_vars(found, bound);
            }
            Expr::Abs(a) | Expr::Not(a) | Expr::Transpose(a) => a.collect_vars(found, bound),
            Expr::Norm(a, p) => {
                a.collect_vars(found, bound);
                if let Some(p) = p {
                    p.collect_vars(found, bound);
                }
            }
            Expr::If(c, t, f) => {
                c.collect_vars(found, bound);
                t.collect_vars(found, bound);
                f.collect_vars(found, bound);
            }
            Expr::Let(name, value, body) => {
                value.collect_vars(found, bound);
                bound.push(name.clone());
                body.collect_vars(found, bound);
                bound.pop();
            }
        }
    }

    /// Whether `name` occurs free anywhere. Drives the solver's search for a
    /// definition that mentions the unknown.
    pub fn mentions(&self, name: &str) -> bool {
        self.free_vars().iter().any(|v| v == name)
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// `name = body`, or `name(params) = body`.
    ///
    /// A definition is stored *unevaluated*. This is essential: redefining an
    /// input further down the document has to change the answer of a definition
    /// written above it. Definitions are closures over names resolved at use
    /// site, not values captured at definition site.
    Define {
        name: String,
        params: Option<Vec<String>>,
        body: Expr,
    },
    /// `name +=` — sums the indented definitions that follow it.
    SumDefine { name: String },
    /// A relation whose left side is not a plain name, e.g. `12x + 13y = 163`.
    /// Stored so a later `x =>` can solve it.
    Equation { lhs: Expr, rhs: Expr },
    /// A bare expression.
    Expr(Expr),
    /// `@precision = 8`, `@group = false`, `@fr-FR`.
    Directive { name: String, value: Option<Expr> },
}

/// One statement plus whether the author asked for an answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub stmt: Stmt,
    /// True when the source had `=>`. Only these produce output.
    pub arrow: bool,
}
