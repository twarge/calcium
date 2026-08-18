//! The environment and the evaluator.
//!
//! Two design decisions shape everything here.
//!
//! **Definitions are stored unevaluated.** A definition written near the top of
//! a document must pick up a redefinition of its inputs made further down — see
//! the redefinition section of `corpus/reference.calcium`. So a definition is a
//! closure over names resolved at use site, never a value captured at
//! definition site.
//!
//! **Units are ordinary definitions that happen to be opaque.** A prelude
//! definition stays symbolic in normal expressions — which is why
//! `4 days + 3 weeks` keeps two unlike terms instead of collapsing to seconds —
//! and expands only when an `in` conversion forces it. There is no separate
//! dimensional-analysis engine.

use crate::ast::*;
use crate::builtins;
use crate::format::render;
use crate::lexer::Radix;
use crate::num::{Num, NumFormat};
use crate::parser::parse_line;
use crate::simplify::simplify;
use std::collections::HashMap;

const PRELUDE: &str = include_str!("prelude.calcium");

/// SI magnitude prefixes, applied programmatically so the prelude does not
/// have to enumerate `PW`, `nW`, `µW` and friends.
///
/// Both the symbols and the spelled-out forms, so `nanosecond`, `kilogram` and
/// `microcentury` resolve as readily as `ns`, `kg` and `µW`. Symbols are tried
/// first because they are far commoner; a name that matches neither falls
/// through to being an ordinary free symbol.
const SI_PREFIXES: &[(&str, i32)] = &[
    ("Y", 24),
    ("Z", 21),
    ("E", 18),
    ("P", 15),
    ("T", 12),
    ("G", 9),
    ("M", 6),
    ("k", 3),
    ("h", 2),
    ("da", 1),
    ("d", -1),
    ("c", -2),
    ("m", -3),
    ("u", -6),
    ("μ", -6),
    ("µ", -6),
    ("n", -9),
    ("p", -12),
    ("f", -15),
    ("a", -18),
    ("z", -21),
    ("y", -24),
    // Spelled-out forms.
    ("yotta", 24),
    ("zetta", 21),
    ("exa", 18),
    ("peta", 15),
    ("tera", 12),
    ("giga", 9),
    ("mega", 6),
    ("kilo", 3),
    ("hecto", 2),
    ("deka", 1),
    ("deca", 1),
    ("deci", -1),
    ("centi", -2),
    ("milli", -3),
    ("micro", -6),
    ("nano", -9),
    ("pico", -12),
    ("femto", -15),
    ("atto", -18),
    ("zepto", -21),
    ("yocto", -24),
];

/// Units that measure from a different zero, as `(name, degree size in K,
/// zero point in K)` — a reading `x` is `x * size + zero` kelvin.
///
/// This is the one thing the "a unit is just a definition" idea cannot reach.
/// Everything else in the prelude is a scale factor, and conversion is
/// division; an offset has nowhere to live in a product. So the sizes stay in
/// the prelude, where they belong and where they make `J/degC` equal `J/K`,
/// and only the zero points are here, applied by `in` and nowhere else.
///
/// The consequence, and it is a real one: `25 degC` is a *size* of 25 degrees
/// everywhere except under a conversion. That keeps `T2 - T1` right and makes
/// `25 degC + 25 degC` meaningless-but-defined, which is the trade most
/// practical tools make.
const AFFINE_UNITS: &[(&str, (i64, i64), (i64, i64))] = &[
    ("K", (1, 1), (0, 1)),
    ("kelvin", (1, 1), (0, 1)),
    ("kelvins", (1, 1), (0, 1)),
    ("degC", (1, 1), (27315, 100)),
    ("celsius", (1, 1), (27315, 100)),
    ("degF", (5, 9), (229835, 900)),
    ("fahrenheit", (5, 9), (229835, 900)),
    ("degR", (5, 9), (0, 1)),
    ("rankine", (5, 9), (0, 1)),
];

fn affine_unit(name: &str) -> Option<(Num, Num)> {
    AFFINE_UNITS.iter().find(|(n, _, _)| *n == name).map(|(_, size, zero)| {
        (Num::ratio(size.0, size.1), Num::ratio(zero.0, zero.1))
    })
}

#[derive(Clone, Debug)]
pub struct Def {
    /// `None` means the parameter list is implied by the body's free
    /// variables, in order of first appearance.
    pub params: Option<Vec<String>>,
    pub body: Expr,
    /// Prelude definitions are opaque outside of `in` conversions.
    pub is_unit: bool,
    /// Whether this came from the prelude. A prelude body resolves its own
    /// references against the prelude, so a document defining `T` for
    /// temperature cannot reach inside `gauss = T/10000` and turn the tesla
    /// into 125 °C.
    pub from_prelude: bool,
}

impl Def {
    /// The effective parameter list.
    ///
    /// When a definition declares no parameters, they are derived from the
    /// variables on the right-hand side, in order of appearance. Standard
    /// library names are excluded: `color = round(f*0xFF)` takes `f` alone, or
    /// calling `color(1/3)` would bind `round`.
    pub fn params(&self) -> Vec<String> {
        self.params.clone().unwrap_or_else(|| {
            self.body
                .free_vars()
                .into_iter()
                .filter(|name| !crate::builtins::is_builtin(name))
                .collect()
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Env {
    defs: HashMap<String, Def>,
    /// The prelude as itself, untouched by document redefinitions. Bodies of
    /// prelude definitions resolve here first — lexical scoping for the unit
    /// table, dynamic for everything the document writes.
    prelude: std::sync::Arc<HashMap<String, Def>>,
    /// Definition order, so the solver can search backwards through history.
    order: Vec<String>,
    /// Names the document declared as units with `@unit`. Redefining one of
    /// these keeps it a unit — even a prelude name, which the declaration
    /// reclaims for the document.
    declared_units: std::collections::HashSet<String>,
    /// Relations whose left side was not a plain name, e.g. `12x + 13y = 163`.
    pub equations: Vec<(Expr, Expr)>,
    pub fmt: NumFormat,
    /// `@sigfigs`: decimal literals carry an implied half-ULP uncertainty —
    /// `2.0` means 2.00 ± 0.05 — propagated exactly like an explicit `±`,
    /// with results rounded where the uncertainty says the digits end.
    pub sigfigs: bool,
    /// Context-free values of definitions already derived this generation.
    cache: ValueCache,
}

/// Evaluation context: what is bound locally, what we are already inside of,
/// and whether unit definitions should expand.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub locals: HashMap<String, Expr>,
    /// Names currently being substituted. Guards against `r = r` and against
    /// mutually recursive definitions looping forever.
    pub active: Vec<String>,
    pub expand_units: bool,
    /// True while expanding the body of a prelude definition, where names
    /// resolve against the prelude before the document.
    pub in_prelude: bool,
    /// Expression nesting budget.
    pub depth: usize,
    /// How many function applications deep we are. Recursion is allowed — the
    /// Reference advertises it — so this is what actually bounds it.
    pub calls: usize,
    /// When present, every `±` encountered is replaced by an internal marker
    /// symbol and recorded here — the collection pass of error propagation.
    /// Shared, because sub-contexts (unit expansion inside a conversion)
    /// must feed the same table.
    pub pm: Option<std::rc::Rc<std::cell::RefCell<PmState>>>,
}

/// The `±` occurrences met while collecting: each becomes one independent
/// random variable, keyed by where it appeared, so a definition expanded
/// twice contributes one variable rather than two uncorrelated copies.
#[derive(Debug, Default)]
pub struct PmState {
    pub table: Vec<PmSource>,
    /// (owning definition, source form) -> marker index.
    keys: HashMap<(String, String), usize>,
}

/// One source of uncertainty, already evaluated.
#[derive(Clone, Debug)]
pub struct PmSource {
    pub centre: Expr,
    pub sigma: Expr,
    /// Implied by a literal's significant figures rather than written `±`.
    /// A result whose uncertainty is entirely implied displays as a plain
    /// number rounded to its significant digits, not as `a ± b`.
    pub implied: bool,
}

impl PmState {
    /// Registers one occurrence, or finds the marker it already has.
    fn intern(&mut self, key: (String, String), source: PmSource) -> usize {
        match self.keys.get(&key) {
            Some(&index) => index,
            None => {
                let index = self.table.len();
                self.table.push(source);
                self.keys.insert(key, index);
                index
            }
        }
    }
}

/// Internal marker symbols carry the one character no user name can start
/// with, since the lexer reads `±` as an operator.
fn marker_name(index: usize) -> String {
    format!("±{index}")
}

fn is_marker(name: &str) -> bool {
    name.starts_with('±')
}

/// Replaces every sig-figs number tag with plain decimal, recursively.
fn strip_sig(expr: &Expr) -> Expr {
    let strip = |e: &Expr| Box::new(strip_sig(e));
    match expr {
        Expr::Num(value, Radix::Sig(_)) => Expr::Num(value.clone(), Radix::Dec),
        Expr::Num(..) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(_) | Expr::AiQuery
        | Expr::Error(_) => expr.clone(),
        Expr::Add(items) => Expr::Add(items.iter().map(strip_sig).collect()),
        Expr::Mul(items) => Expr::Mul(items.iter().map(strip_sig).collect()),
        Expr::Pow(a, b) => Expr::Pow(strip(a), strip(b)),
        Expr::Range(a, b) => Expr::Range(strip(a), strip(b)),
        Expr::PlusMinus(a, b) => Expr::PlusMinus(strip(a), strip(b)),
        Expr::Convert(a, b) => Expr::Convert(strip(a), strip(b)),
        Expr::Relation(a, b) => Expr::Relation(strip(a), strip(b)),
        Expr::Mod(a, b) => Expr::Mod(strip(a), strip(b)),
        Expr::Cmp(op, a, b) => Expr::Cmp(*op, strip(a), strip(b)),
        Expr::Logic(op, a, b) => Expr::Logic(*op, strip(a), strip(b)),
        Expr::Bit(op, a, b) => Expr::Bit(*op, strip(a), strip(b)),
        Expr::Abs(a) => Expr::Abs(strip(a)),
        Expr::Not(a) => Expr::Not(strip(a)),
        Expr::Transpose(a) => Expr::Transpose(strip(a)),
        Expr::Norm(a, p) => Expr::Norm(strip(a), p.as_ref().map(|p| strip(p))),
        Expr::If(c, t, f) => Expr::If(strip(c), strip(t), strip(f)),
        Expr::Let(name, value, body) => Expr::Let(name.clone(), strip(value), strip(body)),
        Expr::Call(name, args) => Expr::Call(
            name.clone(),
            args.iter()
                .map(|a| Arg { name: a.name.clone(), value: strip_sig(&a.value) })
                .collect(),
        ),
        Expr::Index(base, indices) => {
            Expr::Index(strip(base), indices.iter().map(strip_sig).collect())
        }
        Expr::Matrix(rows) => Expr::Matrix(
            rows.iter()
                .map(|row| row.iter().map(strip_sig).collect())
                .collect(),
        ),
        Expr::Dict(entries) => Expr::Dict(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), strip_sig(v)))
                .collect(),
        ),
    }
}

/// A value cut off at its uncertainty's leading digit: 6.2199 with sigma
/// 0.17 rounds to 6.2. Requires both to be quantities of the same thing.
fn round_to_sigma(centre: &Expr, sigma: &Expr) -> Option<Expr> {
    let (v, v_rest) = crate::simplify::split_coefficient(centre);
    let (s, s_rest) = crate::simplify::split_coefficient(sigma);
    if render(&v_rest) != render(&s_rest) {
        return None;
    }
    let width = s.to_f64();
    if !width.is_finite() || width <= 0.0 {
        return None;
    }
    // The last meaningful place is the one whose half-ULP the uncertainty
    // amounts to: sigma 0.05 marks the tenths as the final digit, which is
    // exactly the classic sig-figs answer.
    let place = (2.0 * width).abs().log10().floor() as i32;
    let quantum = 10f64.powi(place);
    let rounded = Num::from_f64((v.to_f64() / quantum).round() * quantum);
    let style = if place < 0 {
        Radix::Sig(-place)
    } else {
        Radix::Dec
    };
    // Assembled by hand, not simplified: the simplifier treats the sig-figs
    // tag as arithmetic-inert and would strip the trailing zeros this
    // rounding exists to show.
    let number = Expr::Num(rounded, style);
    if v_rest.is_one() {
        return Some(number);
    }
    Some(Expr::mul(vec![number, v_rest]))
}

/// Whether a source of uncertainty appears anywhere in the expression: a
/// `±` always counts, and with sig-figs on, so does a decimal-pointed
/// literal.
fn mentions_uncertain(expr: &Expr, sigfigs: bool) -> bool {
    let mentions_pm = |e: &Expr| mentions_uncertain(e, sigfigs);
    match expr {
        Expr::PlusMinus(..) => true,
        Expr::Num(_, Radix::Sig(_)) => sigfigs,
        Expr::Num(..) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(_) | Expr::AiQuery
        | Expr::Error(_) => false,
        Expr::Add(items) | Expr::Mul(items) => items.iter().any(mentions_pm),
        Expr::Matrix(rows) => rows.iter().flatten().any(mentions_pm),
        Expr::Dict(entries) => entries.iter().any(|(_, v)| mentions_pm(v)),
        Expr::Call(_, args) => args.iter().any(|a| mentions_pm(&a.value)),
        Expr::Index(base, indices) => mentions_pm(base) || indices.iter().any(mentions_pm),
        Expr::Pow(a, b)
        | Expr::Range(a, b)
        | Expr::Cmp(_, a, b)
        | Expr::Logic(_, a, b)
        | Expr::Bit(_, a, b)
        | Expr::Mod(a, b)
        | Expr::Convert(a, b)
        | Expr::Relation(a, b) => mentions_pm(a) || mentions_pm(b),
        Expr::Abs(a) | Expr::Not(a) | Expr::Transpose(a) => mentions_pm(a),
        Expr::Norm(a, p) => mentions_pm(a) || p.as_deref().map(mentions_pm).unwrap_or(false),
        Expr::If(c, t, f) => mentions_pm(c) || mentions_pm(t) || mentions_pm(f),
        Expr::Let(_, value, body) => mentions_pm(value) || mentions_pm(body),
    }
}

const MAX_DEPTH: usize = 512;

/// How deep function application may recurse. The bound is the *stack*, not
/// patience: evaluation runs on whatever thread the embedding provides, and
/// an overflow aborts the whole process. macOS gives secondary threads
/// 512 KiB by default, where 200 levels of a release build already die; a
/// debug build fits under 100 levels in the 2 MiB of a test thread. Each
/// level is several evaluator frames, and they are not small. 64 holds
/// everywhere with room to spare, and is still far beyond what a document
/// plausibly recurses.
const MAX_CALLS: usize = 64;

/// Work budget for one answer: every `eval_in` call, `simplify` call, and
/// rendered node burns one unit, and a dry tank turns the answer into an
/// error instead of a frozen editor. `MAX_CALLS` bounds recursion *depth*, but a recursive
/// definition whose base case never resolves (`if nn <= 1 ...`, a typo for
/// `n`) stays within that bound while the symbolic result — and the cost of
/// re-simplifying it on every unwind — keeps growing. Fuel is what bounds
/// the total. The costliest corpus answer burns ~35k units; the runaway
/// cases burn this whole budget in a fraction of a second.
const FUEL_BUDGET: usize = 250_000;

/// The answer shown when the budget runs out.
pub(crate) const FUEL_ERROR: &str = "gave up: the computation kept growing";

thread_local! {
    /// Lives outside `Ctx` for the same reason as `solve::SOLVING`: the
    /// engine's own machinery builds fresh contexts mid-evaluation, and the
    /// budget has to survive them.
    static FUEL: std::cell::Cell<usize> = const { std::cell::Cell::new(FUEL_BUDGET) };
}

/// Refills the tank. The document loop calls this before every statement, so
/// one runaway answer cannot starve the rest of the document.
pub fn refuel() {
    FUEL.with(|fuel| fuel.set(FUEL_BUDGET));
}

/// Burns one unit; false once the tank is dry.
pub(crate) fn spend_fuel() -> bool {
    FUEL.with(|fuel| {
        let left = fuel.get();
        fuel.set(left.saturating_sub(1));
        left > 0
    })
}

thread_local! {
    /// Whether the evaluation in flight consulted the ambient locals frame.
    /// A definition whose body did cannot be cached: the cache is keyed by
    /// name alone, and locals vary between references. Lives outside `Ctx`
    /// for the same reason as `FUEL`: the engine's own machinery builds
    /// fresh contexts mid-evaluation, and the flag has to survive them.
    /// `apply` discards its body's reads — its bindings come from arguments
    /// whose own evaluation already reported to the caller's frame.
    static FRAME_TAINT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Whether it hit a truncation no frame absorbs — depth, fuel, the
    /// recursion limit — leaving something other than the value the source
    /// denotes. Poisons caching all the way up.
    static HARD_TAINT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Non-zero while evaluating under artificially suppressed names — the
    /// solver's `eval_suppressing` — where a cached value could hand back a
    /// suppressed name already folded in. The cache stands aside entirely.
    static CACHE_OFF: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Names met while already being substituted, each tagged with the
    /// expansion frame the reference sat in. A base unit is `s = s`: its own
    /// body hits its own guard on every expansion, identically in every
    /// context, so a frame absorbs hits on its own name made directly inside
    /// it. What a frame cannot absorb — a hit on some *other* name, or on its
    /// own through an intermediate expansion, both the signature of a cycle —
    /// marks the value context-dependent, and it bubbles up uncached.
    static ACTIVE_HITS: std::cell::RefCell<Vec<(String, u64)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// The innermost expansion frame in flight, 0 when none.
    static FRAME_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static NEXT_FRAME_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One name's expansion underway: `eval_var` unfolding a definition body, or
/// `apply` running one against bindings.
struct ExpansionFrame {
    mark: usize,
    id: u64,
    parent: u64,
}

fn enter_frame() -> ExpansionFrame {
    let id = NEXT_FRAME_ID.with(|next| {
        let id = next.get() + 1;
        next.set(id);
        id
    });
    ExpansionFrame {
        mark: ACTIVE_HITS.with(|hits| hits.borrow().len()),
        id,
        parent: FRAME_ID.with(|frame| frame.replace(id)),
    }
}

/// Closes the frame, absorbing the hits that are its own name referencing
/// itself directly. True when nothing context-dependent remains: every other
/// hit is left for the frames above to fail on in turn.
fn exit_frame(frame: ExpansionFrame, name: &str) -> bool {
    FRAME_ID.with(|current| current.set(frame.parent));
    ACTIVE_HITS.with(|hits| {
        let mut hits = hits.borrow_mut();
        let mut tail = hits.split_off(frame.mark);
        tail.retain(|(hit, owner)| !(*owner == frame.id && hit == name));
        let clean = tail.is_empty();
        hits.append(&mut tail);
        clean
    })
}

/// A reference resolved symbolically because the name is being substituted
/// somewhere above: remembered against the frame it happened in.
fn record_active_hit(name: &str) {
    ACTIVE_HITS.with(|hits| {
        hits.borrow_mut()
            .push((name.to_string(), FRAME_ID.with(|frame| frame.get())));
    });
}

/// Clears the taint machinery between documents. Absorbed hits are removed as
/// frames close, but a few paths leave residue by design — hits on suppressed
/// or local names that no frame owns — and it should not outlive the
/// evaluation that made it.
pub(crate) fn reset_taints() {
    FRAME_TAINT.with(|taint| taint.set(false));
    HARD_TAINT.with(|taint| taint.set(false));
    ACTIVE_HITS.with(|hits| hits.borrow_mut().clear());
    FRAME_ID.with(|frame| frame.set(0));
}

/// Holds the cache aside for a dynamic extent, restoring on drop so a panic
/// unwinding through the FFI guard cannot leave it off for the thread.
struct CacheOffGuard;

impl CacheOffGuard {
    fn hold() -> CacheOffGuard {
        CACHE_OFF.with(|off| off.set(off.get() + 1));
        CacheOffGuard
    }
}

impl Drop for CacheOffGuard {
    fn drop(&mut self) {
        CACHE_OFF.with(|off| off.set(off.get().saturating_sub(1)));
    }
}

/// Evaluated definition bodies, so a name referenced two hundred times — an
/// arrow line per reference, a plot sample per reference — derives its chain
/// once. Keyed by name under each (expand_units, in_prelude) pair, since both
/// change what a body evaluates to. Only values whose derivation touched
/// nothing contextual are admitted; any redefinition clears the table.
#[derive(Debug, Default)]
struct ValueCache(std::sync::Mutex<[HashMap<String, Expr>; 4]>);

/// A clone is a fresh environment about to diverge; it starts cold rather
/// than share or copy answers.
impl Clone for ValueCache {
    fn clone(&self) -> ValueCache {
        ValueCache::default()
    }
}

impl ValueCache {
    fn slot(ctx: &Ctx) -> usize {
        (ctx.expand_units as usize) | ((ctx.in_prelude as usize) << 1)
    }

    fn get(&self, ctx: &Ctx, name: &str) -> Option<Expr> {
        self.0.lock().unwrap()[Self::slot(ctx)].get(name).cloned()
    }

    fn put(&self, ctx: &Ctx, name: &str, value: Expr) {
        self.0.lock().unwrap()[Self::slot(ctx)].insert(name.to_string(), value);
    }

    fn clear(&self) {
        for slot in self.0.lock().unwrap().iter_mut() {
            slot.clear();
        }
    }
}

/// The prelude, parsed once.
///
/// It is four hundred definitions and it does not change, but a document is
/// re-evaluated on every pause in typing — so parsing it each time was a
/// sixth of the cost of every keystroke, and grew with every unit added.
static PRELUDE_ENV: std::sync::OnceLock<Env> = std::sync::OnceLock::new();

impl Env {
    /// An environment preloaded with the unit and currency prelude.
    ///
    /// A clone of the shared one: the caller goes on to add the document's own
    /// definitions, so it needs a copy it can write to.
    pub fn with_prelude() -> Env {
        PRELUDE_ENV.get_or_init(Env::build_prelude).clone()
    }

    fn build_prelude() -> Env {
        let mut env = Env::default();
        // Populated below, then snapshotted into `prelude`.
        // Most of the prelude is units, which stay opaque outside an `in`
        // conversion. Constants are different — `pi` and the Boltzmann constant
        // have to fold into arithmetic wherever they appear — so the file
        // switches modes with a marker comment rather than the engine keeping a
        // list of exceptions.
        let mut defining_units = true;
        for line in PRELUDE.lines() {
            let trimmed = line.trim();
            match trimmed {
                "#!constants" => {
                    defining_units = false;
                    continue;
                }
                "#!units" => {
                    defining_units = true;
                    continue;
                }
                _ => {}
            }
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            for statement in parse_line(trimmed) {
                if let Stmt::Define { name, params, body } = statement.stmt {
                    // Prelude decimals are exact by definition — CODATA
                    // values, statute pounds — so the sig-figs tags their
                    // decimal points would otherwise carry are stripped.
                    let body = strip_sig(&body);
                    env.insert(
                        name,
                        Def { params, body, is_unit: defining_units, from_prelude: true },
                    );
                }
            }
        }
        // Hand the formatter the unit vocabulary so it can keep a meaningful
        // coefficient of 1.
        let units: std::collections::HashSet<String> = env
            .defs
            .iter()
            .filter(|(_, def)| def.is_unit)
            .map(|(name, _)| name.clone())
            .collect();
        env.fmt.units = std::sync::Arc::new(units);
        env.prelude = std::sync::Arc::new(env.defs.clone());
        env
    }

    pub fn insert(&mut self, name: String, def: Def) {
        // Any value in the cache may have folded in the old definition.
        self.cache.clear();
        if !self.defs.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.defs.insert(name, def);
    }

    pub fn define(&mut self, name: &str, params: Option<Vec<String>>, body: Expr) {
        // Redefining a document-declared unit keeps it a unit: after `@unit`
        // says firkin is one, `firkin = 90 lb` is a derived unit — the
        // document reclaiming the prelude's firkin — not a shadowing
        // variable. An undeclared prelude name shadows as ever: a document
        // that defines `T` means its T, not the tesla.
        let is_unit = self.declared_units.contains(name);
        self.insert(
            name.to_string(),
            Def {
                params,
                body,
                is_unit,
                from_prelude: false,
            },
        );
    }

    /// Marks a name as a unit, per the document's `@unit` directive. An
    /// already-defined name keeps its body and becomes a derived unit; an
    /// undefined one becomes a base unit in the prelude's own image:
    /// `burrito = burrito`. Declared units get no SI prefixes — prefix
    /// resolution consults the prelude alone.
    pub fn declare_unit(&mut self, name: &str) {
        self.cache.clear();
        self.declared_units.insert(name.to_string());
        match self.defs.get_mut(name) {
            Some(def) => def.is_unit = true,
            None => self.insert(
                name.to_string(),
                Def {
                    params: None,
                    body: Expr::var(name),
                    is_unit: true,
                    from_prelude: false,
                },
            ),
        }
        // The formatter's unit vocabulary keeps a meaningful coefficient of
        // one: a bare `burrito` answer prints as `1 burrito`.
        std::sync::Arc::make_mut(&mut self.fmt.units).insert(name.to_string());
    }

    /// Records a relation for the solver. The solution to any name can turn
    /// on it, so derived values are dropped along with it.
    pub fn push_equation(&mut self, lhs: Expr, rhs: Expr) {
        self.cache.clear();
        self.equations.push((lhs, rhs));
    }

    /// Forgets every derived value. Directives change how bodies evaluate —
    /// `@sigfigs`, number formats — without touching a definition, so the
    /// document loop calls this when it applies one.
    pub fn clear_value_cache(&self) {
        self.cache.clear();
    }

    pub fn get(&self, name: &str) -> Option<&Def> {
        self.defs.get(name)
    }

    pub fn is_defined(&self, name: &str) -> bool {
        self.defs.contains_key(name)
    }

    /// Definition names, most recent first. The solver walks this looking for
    /// an equation that mentions the unknown.
    pub fn recent_names(&self) -> impl Iterator<Item = &String> {
        self.order.iter().rev()
    }

    /// Every name the prelude defines, for completion menus.
    pub fn prelude_names(&self) -> impl Iterator<Item = &String> {
        self.prelude.keys()
    }

    /// A prelude name's definition, for completion menus: `pi` reads better
    /// beside its value, a unit beside its meaning.
    pub fn prelude_def(&self, name: &str) -> Option<&Def> {
        self.prelude.get(name)
    }

    /// Resolves a name that is not explicitly defined but is a prefixed SI
    /// unit, e.g. `nW` as `1e-9 * W`.
    fn resolve_si_prefix(&self, name: &str) -> Option<Expr> {
        for (prefix, power) in SI_PREFIXES {
            let Some(base) = name.strip_prefix(prefix) else {
                continue;
            };
            if base.is_empty() {
                continue;
            }
            let Some(def) = self.prelude.get(base) else {
                continue;
            };
            if !def.is_unit {
                continue;
            }
            let scale = Num::from_i64(10).pow(&Num::from_i64(*power as i64));
            return Some(Expr::mul(vec![
                Expr::Num(scale, Radix::Dec),
                Expr::var(base),
            ]));
        }
        None
    }

    /// Whether the document itself declared this name a unit with `@unit`.
    pub fn is_declared_unit(&self, name: &str) -> bool {
        self.declared_units.contains(name)
    }

    /// Whether a name refers to a unit, directly or via an SI prefix.
    pub fn is_unit_name(&self, name: &str) -> bool {
        if let Some(def) = self.defs.get(name) {
            return def.is_unit;
        }
        self.resolve_si_prefix(name).is_some()
    }

    /// Whether the prelude defines `name`, regardless of document shadowing.
    pub fn prelude_defines(&self, name: &str) -> bool {
        self.prelude.contains_key(name)
    }

    // -- evaluation ---------------------------------------------------------

    /// Evaluates an expression in ordinary (non-expanding) mode.
    /// First-order Gaussian error propagation for a top-level `=>`.
    ///
    /// Returns None when nothing reachable carries a `±`, so the caller
    /// keeps its ordinary path. Otherwise: every `±` occurrence becomes an
    /// independent random variable, the expression is evaluated symbolically
    /// over those variables — correlations through shared definitions
    /// survive, so `m - m` is exactly 0 — and the result is
    /// centre ± sqrt(sum of (∂f/∂xᵢ · σᵢ)²), the partials taken
    /// symbolically and evaluated at the centres.
    pub fn eval_uncertain(&self, expr: &Expr) -> Option<Expr> {
        if !self.reaches_pm(expr) {
            return None;
        }
        let state = std::rc::Rc::new(std::cell::RefCell::new(PmState::default()));
        let mut collect = Ctx {
            pm: Some(state.clone()),
            ..Ctx::default()
        };
        let symbolic = simplify(&self.eval_in(expr, &mut collect));
        let table: Vec<PmSource> = state.borrow().table.clone();
        if table.is_empty() {
            return None;
        }

        let centres: HashMap<String, Expr> = table
            .iter()
            .enumerate()
            .map(|(index, source)| (marker_name(index), source.centre.clone()))
            .collect();
        let at_centres = |e: &Expr| -> Expr {
            let mut ctx = Ctx {
                locals: centres.clone(),
                ..Ctx::default()
            };
            simplify(&self.eval_in(e, &mut ctx))
        };
        let centre = at_centres(&symbolic);

        let mut squares = Vec::new();
        let mut explicit = false;
        for (index, source) in table.iter().enumerate() {
            let slope = builtins::differentiate(&symbolic, &marker_name(index));
            let contribution =
                simplify(&Expr::mul(vec![at_centres(&slope), source.sigma.clone()]));
            if contribution.is_zero() {
                continue;
            }
            explicit |= !source.implied;
            squares.push(Expr::Pow(Box::new(contribution), Box::new(Expr::num(2))));
        }
        // Every derivative vanished: the uncertainty cancelled out exactly.
        if squares.is_empty() {
            return Some(centre);
        }
        let sigma = self.eval(&Expr::Pow(
            Box::new(Expr::add(squares)),
            Box::new(Expr::Num(Num::ratio(1, 2), Radix::Dec)),
        ));
        // Purely implied uncertainty — significant figures, no written `±` —
        // displays as a plain number cut where the digits stop meaning
        // anything, which is what sig-figs arithmetic is.
        if !explicit {
            if let Some(rounded) = round_to_sigma(&centre, &sigma) {
                return Some(rounded);
            }
        }
        Some(Expr::PlusMinus(Box::new(centre), Box::new(sigma)))
    }

    /// Whether the expression, or any definition reachable from it, carries
    /// a source of uncertainty — the cheap gate in front of the propagation
    /// machinery. Prelude definitions are skipped: their decimals are exact.
    fn reaches_pm(&self, expr: &Expr) -> bool {
        if mentions_uncertain(expr, self.sigfigs) {
            return true;
        }
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut work: Vec<String> = expr.free_vars();
        while let Some(name) = work.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(def) = self.get(&name) {
                if def.from_prelude {
                    continue;
                }
                if mentions_uncertain(&def.body, self.sigfigs) {
                    return true;
                }
                work.extend(def.body.free_vars());
            }
        }
        false
    }

    pub fn eval(&self, expr: &Expr) -> Expr {
        let mut ctx = Ctx::default();
        simplify(&self.eval_in(expr, &mut ctx))
    }

    /// Evaluates while holding some names symbolic.
    ///
    /// The solver needs this: to turn the definition `f = (9/5)c + 32` into an
    /// equation it must evaluate the body without `f` itself unfolding, or the
    /// difference `f - body` collapses to zero.
    pub fn eval_suppressing(&self, expr: &Expr, suppressed: &[String]) -> Expr {
        let mut ctx = Ctx {
            active: suppressed.to_vec(),
            ..Ctx::default()
        };
        // A cached value may hold a suppressed name already folded in, which
        // is exactly what suppression exists to prevent.
        let _held = CacheOffGuard::hold();
        let mark = ACTIVE_HITS.with(|hits| hits.borrow().len());
        let result = simplify(&self.eval_in(expr, &mut ctx));
        // Hits on the suppressed names have no frame to absorb them; they
        // still count against any evaluation enclosing this one.
        ACTIVE_HITS.with(|hits| {
            let mut hits = hits.borrow_mut();
            let mut tail = hits.split_off(mark);
            tail.retain(|(hit, _)| !suppressed.contains(hit));
            hits.append(&mut tail);
        });
        result
    }

    /// Evaluates with unit definitions expanded to base units.
    pub fn eval_expanded(&self, expr: &Expr) -> Expr {
        let mut ctx = Ctx {
            expand_units: true,
            ..Ctx::default()
        };
        simplify(&self.eval_in(expr, &mut ctx))
    }

    pub fn eval_in(&self, expr: &Expr, ctx: &mut Ctx) -> Expr {
        if ctx.depth > MAX_DEPTH {
            // Truncated, not the value the expression denotes: uncacheable.
            HARD_TAINT.with(|taint| taint.set(true));
            return expr.clone();
        }
        if !spend_fuel() {
            HARD_TAINT.with(|taint| taint.set(true));
            return Expr::Error(FUEL_ERROR.to_string());
        }
        ctx.depth += 1;
        let result = self.eval_inner(expr, ctx);
        ctx.depth -= 1;
        result
    }

    fn eval_inner(&self, expr: &Expr, ctx: &mut Ctx) -> Expr {
        match expr {
            // Under `@sigfigs`, a decimal-pointed literal met while
            // collecting carries an implied half-ULP uncertainty. Prelude
            // bodies are exempt: `lb = 0.45359237 kg` is exact by statute.
            Expr::Num(value, Radix::Sig(decimals))
                if self.sigfigs && !ctx.in_prelude && ctx.pm.is_some() =>
            {
                let pm = ctx.pm.clone().unwrap();
                let owner = ctx.active.last().cloned().unwrap_or_default();
                let key = (owner, format!("{}#{decimals}", render(expr)));
                let sigma = Num::ratio(1, 2)
                    .mul(&Num::from_i64(10).pow(&Num::from_i64(-(*decimals as i64))));
                let index = pm.borrow_mut().intern(
                    key,
                    PmSource {
                        centre: Expr::Num(value.clone(), Radix::Dec),
                        sigma: Expr::Num(sigma, Radix::Dec),
                        implied: true,
                    },
                );
                return Expr::var(marker_name(index));
            }
            Expr::Num(..) | Expr::Str(_) | Expr::Bool(_) | Expr::Error(_) | Expr::AiQuery => {
                expr.clone()
            }

            Expr::Var(name) => self.eval_var(name, ctx),
            Expr::Call(name, args) => self.eval_call(name, args, ctx),

            Expr::Add(terms) => {
                let evaluated: Vec<Expr> = terms.iter().map(|t| self.eval_in(t, ctx)).collect();
                simplify(&Expr::add(evaluated))
            }
            Expr::Mul(factors) => {
                let evaluated: Vec<Expr> = factors.iter().map(|f| self.eval_in(f, ctx)).collect();
                simplify(&Expr::mul(evaluated))
            }
            Expr::Pow(base, exp) => {
                let base = self.eval_in(base, ctx);
                let exp = self.eval_in(exp, ctx);
                // A matrix raised to -1 is its inverse.
                if let (Expr::Matrix(rows), Some(e)) = (&base, exp.as_num()) {
                    if e.eq_num(&Num::from_i64(-1)) {
                        return builtins::matrix_inverse(rows);
                    }
                }
                simplify(&Expr::Pow(Box::new(base), Box::new(exp)))
            }

            Expr::Convert(value, unit) => self.eval_convert(value, unit, ctx),

            Expr::Index(base, indices) => {
                let base = self.eval_in(base, ctx);
                let indices: Vec<Expr> = indices.iter().map(|i| self.eval_in(i, ctx)).collect();
                builtins::index(&base, &indices)
            }

            Expr::Matrix(rows) => Expr::Matrix(
                rows.iter()
                    .map(|row| row.iter().map(|c| self.eval_in(c, ctx)).collect())
                    .collect(),
            ),
            Expr::Range(lo, hi) => Expr::Range(
                Box::new(self.eval_in(lo, ctx)),
                Box::new(self.eval_in(hi, ctx)),
            ),
            Expr::PlusMinus(value, sigma) => {
                // The parts of a `±` are exact by declaration: `2.0 ± 0.5`
                // has a sigma of exactly 0.5, and the written uncertainty
                // supersedes any the literals would imply. Collection stops
                // at this node's boundary and resumes after.
                let collecting = ctx.pm.take();
                let value = self.eval_in(value, ctx);
                let sigma = self.eval_in(sigma, ctx);
                ctx.pm = collecting;
                if let Some(pm) = ctx.pm.clone() {
                    // Collecting: this occurrence becomes a marker symbol.
                    // The key ties repeated expansions of one definition to
                    // one random variable.
                    let owner = ctx.active.last().cloned().unwrap_or_default();
                    let key = (owner, crate::format::render(expr));
                    let index = pm.borrow_mut().intern(
                        key,
                        PmSource { centre: value, sigma, implied: false },
                    );
                    return Expr::var(marker_name(index));
                }
                Expr::PlusMinus(Box::new(value), Box::new(sigma))
            }
            Expr::Dict(entries) => Expr::Dict(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), self.eval_in(v, ctx)))
                    .collect(),
            ),

            Expr::Abs(inner) => {
                let inner = self.eval_in(inner, ctx);
                builtins::abs(&inner)
            }
            Expr::Norm(inner, p) => {
                let inner = self.eval_in(inner, ctx);
                let p = p.as_ref().map(|p| self.eval_in(p, ctx));
                builtins::norm(&inner, p.as_ref())
            }
            Expr::Transpose(inner) => {
                let inner = self.eval_in(inner, ctx);
                simplify(&Expr::Transpose(Box::new(inner)))
            }

            Expr::Cmp(op, a, b) => {
                let a = self.eval_in(a, ctx);
                let b = self.eval_in(b, ctx);
                simplify(&Expr::Cmp(*op, Box::new(a), Box::new(b)))
            }
            Expr::Logic(op, a, b) => {
                let a = self.eval_in(a, ctx);
                let b = self.eval_in(b, ctx);
                simplify(&Expr::Logic(*op, Box::new(a), Box::new(b)))
            }
            Expr::Bit(op, a, b) => {
                let a = self.eval_in(a, ctx);
                let b = self.eval_in(b, ctx);
                simplify(&Expr::Bit(*op, Box::new(a), Box::new(b)))
            }
            Expr::Not(inner) => {
                let inner = self.eval_in(inner, ctx);
                simplify(&Expr::Not(Box::new(inner)))
            }
            Expr::Mod(a, b) => {
                let a = self.eval_in(a, ctx);
                let b = self.eval_in(b, ctx);
                simplify(&Expr::Mod(Box::new(a), Box::new(b)))
            }

            Expr::If(cond, then_branch, else_branch) => {
                let cond = self.eval_in(cond, ctx);
                // Only evaluate the branch we take, so a recursive base case
                // can actually stop the recursion.
                match crate::simplify::truth_of(&cond) {
                    Some(true) => self.eval_in(then_branch, ctx),
                    Some(false) => self.eval_in(else_branch, ctx),
                    None => simplify(&Expr::If(
                        Box::new(cond),
                        Box::new(self.eval_in(then_branch, ctx)),
                        Box::new(self.eval_in(else_branch, ctx)),
                    )),
                }
            }

            Expr::Let(name, value, body) => {
                let value = self.eval_in(value, ctx);
                let shadowed = ctx.locals.insert(name.clone(), value);
                let result = self.eval_in(body, ctx);
                match shadowed {
                    Some(previous) => ctx.locals.insert(name.clone(), previous),
                    None => ctx.locals.remove(name),
                };
                result
            }

            Expr::Relation(a, b) => Expr::Relation(
                Box::new(self.eval_in(a, ctx)),
                Box::new(self.eval_in(b, ctx)),
            ),
        }
    }

    fn eval_var(&self, name: &str, ctx: &mut Ctx) -> Expr {
        if let Some(value) = ctx.locals.get(name) {
            // The result now depends on the frame, which varies between
            // references to whatever definition is being expanded above.
            FRAME_TAINT.with(|taint| taint.set(true));
            let value = value.clone();
            // Arguments were evaluated at the call site, where units stay
            // symbolic. Inside a conversion they have to come apart, so
            // re-evaluate rather than hand back the stored form.
            if ctx.expand_units && !ctx.active.iter().any(|n| n == name) {
                ctx.active.push(name.to_string());
                let expanded = self.eval_in(&value, ctx);
                ctx.active.pop();
                return expanded;
            }
            return value;
        }
        // Already substituting this name: leave it symbolic. This is what
        // makes `r = r` and `A => A` declare a free symbol rather than hang.
        if ctx.active.iter().any(|n| n == name) {
            record_active_hit(name);
            return Expr::var(name);
        }
        if matches!(name, "Infinity" | "∞") {
            return Expr::Num(Num::infinity(), Radix::Dec);
        }
        // Inside a prelude body the prelude resolves first, so a document's
        // `T = 125 degC` cannot reach inside `gauss = T/10000`.
        let def = if ctx.in_prelude {
            self.prelude.get(name).or_else(|| self.defs.get(name))
        } else {
            self.defs.get(name)
        };
        if let Some(def) = def {
            if def.is_unit && !ctx.expand_units {
                return Expr::var(name);
            }
            // Under `±` collection a body has to be walked for markers, and
            // under suppression a cached value could smuggle a suppressed
            // name back in — both stand the cache down.
            let cacheable = ctx.pm.is_none() && CACHE_OFF.with(|off| off.get()) == 0;
            if cacheable {
                if let Some(value) = self.cache.get(ctx, name) {
                    // A free variable of the value that is locally bound
                    // right now would have been substituted by a fresh
                    // evaluation; hand those back to the slow path.
                    if ctx.locals.is_empty()
                        || value
                            .free_vars()
                            .iter()
                            .all(|free| !ctx.locals.contains_key(free))
                    {
                        return value;
                    }
                }
            }
            let body = def.body.clone();
            let was_in_prelude = ctx.in_prelude;
            ctx.in_prelude = def.from_prelude;
            ctx.active.push(name.to_string());
            let expansion = enter_frame();
            let frame_before = FRAME_TAINT.with(|taint| taint.replace(false));
            let hard_before = HARD_TAINT.with(|taint| taint.replace(false));
            let result = self.eval_in(&body, ctx);
            let frame = FRAME_TAINT.with(|taint| taint.get());
            let hard = HARD_TAINT.with(|taint| taint.get());
            let cycle_free = exit_frame(expansion, name);
            ctx.active.pop();
            ctx.in_prelude = was_in_prelude;
            // Context-free and complete: every later reference may reuse it.
            if cacheable && !frame && !hard && cycle_free && !crate::doc::holds_error(&result) {
                self.cache.put(ctx, name, result.clone());
            }
            FRAME_TAINT.with(|taint| taint.set(frame_before || frame));
            HARD_TAINT.with(|taint| taint.set(hard_before || hard));
            return result;
        }
        if ctx.expand_units {
            if let Some(expansion) = self.resolve_si_prefix(name) {
                // A prefixed unit is a prelude expression: `fT` must reach the
                // tesla even when the document has its own `T`.
                let was_in_prelude = ctx.in_prelude;
                ctx.in_prelude = true;
                let result = self.eval_in(&expansion, ctx);
                ctx.in_prelude = was_in_prelude;
                return result;
            }
        }
        Expr::var(name)
    }

    fn eval_call(&self, name: &str, args: &[Arg], ctx: &mut Ctx) -> Expr {
        // A function passed by name: `H(data, p = pmax)` binds `p` to the
        // symbol `pmax`, and `p(ex)` inside the body must dispatch to it.
        if let Some(Expr::Var(target)) = ctx.locals.get(name).cloned() {
            if target != name && ctx.active.iter().any(|n| n == &target) {
                // The dispatch this would have made depends on what is
                // being substituted above.
                record_active_hit(&target);
            }
            if target != name && !ctx.active.iter().any(|n| n == &target) {
                let shadowed = ctx.locals.remove(name);
                let result = self.eval_call(&target, args, ctx);
                if let Some(previous) = shadowed {
                    ctx.locals.insert(name.to_string(), previous);
                }
                return result;
            }
        }

        // Higher-order builtins receive their arguments unevaluated, because
        // `sum(x*x, x=1..5)` binds `x` rather than reading an outer `x`.
        if builtins::is_lazy(name) {
            return builtins::call_lazy(self, name, args, ctx);
        }

        // Trigonometry and friends need base units, so `cos(60°)` can reduce
        // the degree symbol to radians before computing.
        let mut expanding;
        let arg_ctx: &mut Ctx = if builtins::expands_args(name) && !ctx.expand_units {
            expanding = ctx.clone();
            expanding.expand_units = true;
            &mut expanding
        } else {
            ctx
        };
        let evaluated: Vec<Arg> = args
            .iter()
            .map(|arg| Arg {
                name: arg.name.clone(),
                value: self.eval_in(&arg.value, arg_ctx),
            })
            .collect();
        let ctx = arg_ctx;

        // A user definition shadows a builtin of the same name.
        if let Some(def) = self.defs.get(name) {
            // A *call* may re-enter its own definition: that is recursion, and
            // the base case in an `if` is what stops it. Only a bare variable
            // reference is blocked by `active`, which is what makes `r = r`
            // declare a free symbol instead of hanging.
            let recursing = ctx.active.iter().any(|n| n == name);
            if recursing {
                // Whether the definition re-enters turns on what is active;
                // the frame that pushed the name absorbs this.
                record_active_hit(name);
            }
            if !recursing || (!evaluated.is_empty() && ctx.calls < MAX_CALLS) {
                let applied = self.apply(name, def, &evaluated, ctx);
                // A self-definition (`r = r`, which just declares a symbol)
                // applies to nothing. Fall through so the solver can find a
                // relation that actually determines it.
                let inert = matches!(&applied, Expr::Var(other) if other == name);
                if !inert {
                    return applied;
                }
            }
        }
        if let Some(result) = builtins::call(self, name, &evaluated, ctx) {
            return result;
        }

        // Calling an unknown name solves for it and applies the solution:
        // after `v = i * r`, `i(v = 200V, r = 100Ω)` answers `2 A`.
        if ctx.active.iter().any(|n| n == name) {
            record_active_hit(name);
        }
        if !ctx.active.iter().any(|n| n == name) {
            ctx.active.push(name.to_string());
            let solution = crate::solve::solve_for(self, name);
            ctx.active.pop();
            if let Some(body) = solution {
                if !matches!(body, Expr::Relation(..) | Expr::Error(_)) {
                    let def = Def {
                        params: None,
                        body,
                        is_unit: false,
                        from_prelude: false,
                    };
                    return self.apply(name, &def, &evaluated, ctx);
                }
            }
        }
        simplify(&Expr::Call(name.to_string(), evaluated))
    }

    /// Applies a definition to arguments.
    ///
    /// Named arguments may bind *any* free variable of the body, not only
    /// declared parameters: `f(90, mean=100, stddev=10)` works even though `f`
    /// declares only `x`.
    pub fn apply(&self, name: &str, def: &Def, args: &[Arg], ctx: &mut Ctx) -> Expr {
        let mut body = &def.body;
        let mut params = def.params();
        let evaluated_value;
        if def.params.is_none() {
            // Implicit parameters are the *undefined* names, in written
            // order: a name the document already defines is a value the
            // body uses, not a slot an argument may fill.
            params.retain(|param| !self.is_defined(param));
            // When nothing written is bindable, the call applies the
            // definition's result: `t5 = taylor(f, x=0.5, 5)` stores the
            // taylor call, and `t5(0.7)` binds the polynomial's leftover
            // variable, which only exists after evaluation.
            if params.is_empty() && args.iter().any(|a| a.name.is_none()) {
                let saved = std::mem::take(&mut ctx.locals);
                ctx.active.push(name.to_string());
                // A fresh, empty frame: reads of locals installed within it
                // are determined by the body, not by the caller's context.
                let expansion = enter_frame();
                let frame_before = FRAME_TAINT.with(|taint| taint.replace(false));
                evaluated_value = self.eval_in(&def.body, ctx);
                FRAME_TAINT.with(|taint| taint.set(frame_before));
                exit_frame(expansion, name);
                ctx.active.pop();
                ctx.locals = saved;
                params = evaluated_value
                    .free_vars()
                    .into_iter()
                    .filter(|n| !self.is_defined(n) && !crate::builtins::is_builtin(n))
                    .collect();
                body = &evaluated_value;
            }
        }
        let mut bindings: HashMap<String, Expr> = HashMap::new();
        let mut position = 0usize;
        for arg in args {
            match &arg.name {
                Some(label) => {
                    bindings.insert(label.clone(), arg.value.clone());
                }
                None => {
                    if let Some(param) = params.get(position) {
                        bindings.insert(param.clone(), arg.value.clone());
                    }
                    position += 1;
                }
            }
        }

        let saved = std::mem::take(&mut ctx.locals);
        ctx.locals = bindings;
        ctx.active.push(name.to_string());
        ctx.calls += 1;
        // The bindings replace the frame wholesale, so the body's reads of
        // them say nothing about the caller's context: the arguments were
        // evaluated at the call site, where their reads already counted.
        let expansion = enter_frame();
        let frame_before = FRAME_TAINT.with(|taint| taint.replace(false));
        let result = if ctx.calls > MAX_CALLS {
            HARD_TAINT.with(|taint| taint.set(true));
            Expr::Error(format!("{name} recursed too deeply"))
        } else {
            self.eval_in(body, ctx)
        };
        FRAME_TAINT.with(|taint| taint.set(frame_before));
        exit_frame(expansion, name);
        ctx.calls -= 1;
        ctx.active.pop();
        ctx.locals = saved;
        simplify(&result)
    }

    /// `value in unit`.
    ///
    /// Both sides expand to base units so the quotient can cancel, but the
    /// *displayed* unit is the expression as the author wrote it — which is
    /// why `2 ton in sacks` answers in `sacks` and not in kilograms.
    fn eval_convert(&self, value: &Expr, unit: &Expr, ctx: &mut Ctx) -> Expr {
        // Temperature first: between two units that measure from different
        // zeros the conversion is affine, which the division below cannot
        // express.
        if let Some(result) = self.convert_temperature(value, unit, ctx) {
            return result;
        }

        // `in hex`, `in binary`, `in base 8` change the display radix instead.
        if let Some(radix) = radix_of(unit) {
            let evaluated = self.eval_in(value, ctx);
            return match evaluated.as_num().and_then(|n| n.to_bigint()) {
                Some(_) => match &evaluated {
                    Expr::Num(v, _) => Expr::Num(v.clone(), radix),
                    _ => evaluated,
                },
                None => evaluated,
            };
        }

        let mut expanding = Ctx {
            locals: ctx.locals.clone(),
            active: ctx.active.clone(),
            expand_units: true,
            in_prelude: ctx.in_prelude,
            depth: ctx.depth,
            calls: ctx.calls,
            pm: ctx.pm.clone(),
        };
        let expanded_value = simplify(&self.eval_in(value, &mut expanding));
        let expanded_unit = simplify(&self.eval_in(unit, &mut expanding));

        let quotient = simplify(&Expr::div(expanded_value.clone(), expanded_unit.clone()));

        // How the unit should be spelled in the answer. At the top level we
        // keep the author's spelling (`2 ton in sacks` answers in sacks);
        // nested inside an expansion we must hand back base units, or an outer
        // conversion sees a mix of expanded and unexpanded terms.
        let display_unit = |env: &Env, ctx: &mut Ctx| {
            if ctx.expand_units {
                expanded_unit.clone()
            } else {
                strip_unit_scale(&env.eval_in(unit, ctx), unit)
            }
        };

        // A clean conversion leaves a pure number behind.
        if quotient.as_num().is_some() {
            let unit = display_unit(self, ctx);
            return simplify(&Expr::mul(vec![quotient, unit]));
        }

        // An interval converts endpoint-wise: the quotient of a measured
        // range by the unit is a numeric range, and it takes the unit the
        // author asked for just as a number would.
        if let Expr::Range(lo, hi) = &quotient {
            if lo.as_num().is_some() && hi.as_num().is_some() {
                let unit = display_unit(self, ctx);
                return simplify(&Expr::mul(vec![quotient.clone(), unit]));
            }
        }

        // While collecting uncertainty the value carries marker symbols. A
        // quotient whose only symbols are markers is a clean number for
        // every draw of those variables, so it converts like one.
        if ctx.pm.is_some() && quotient.as_num().is_none() {
            let vars = quotient.free_vars();
            if !vars.is_empty() && vars.iter().all(|v| is_marker(v)) {
                let unit = display_unit(self, ctx);
                return simplify(&Expr::mul(vec![quotient.clone(), unit]));
            }
        }

        // A dimensionless value converted to a unit just takes that unit. The
        // A basal-metabolic-rate style formula relies on this: its body divides every
        // term by its unit and then asks for the total `in kcal/day`.
        if expanded_value.as_num().is_some() && !expanded_value.is_zero() {
            let unit = display_unit(self, ctx);
            return simplify(&Expr::mul(vec![expanded_value, unit]));
        }

        // A matrix or per-element conversion: scale each cell.
        if let Expr::Matrix(rows) = &expanded_value {
            let scaled: Vec<Vec<Expr>> = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| simplify(&Expr::div(cell.clone(), expanded_unit.clone())))
                        .collect()
                })
                .collect();
            if scaled.iter().flatten().all(|c| c.as_num().is_some()) {
                let unit = display_unit(self, ctx);
                return simplify(&Expr::mul(vec![Expr::Matrix(scaled), unit]));
            }
        }

        // Not convertible — report what is left over, which is more useful
        // than a bare "error".
        Expr::Error(format!(
            "cannot convert {} to {}",
            render(&expanded_value),
            render(&expanded_unit)
        ))
    }
}

impl Env {
    /// `25 degC in K`, and the rest of the affine conversions.
    ///
    /// Returns `None` unless *both* sides name a temperature scale, so
    /// everything else falls through to ordinary division.
    fn convert_temperature(&self, value: &Expr, unit: &Expr, ctx: &mut Ctx) -> Option<Expr> {
        let Expr::Var(target) = unit else { return None };
        let (to_size, to_zero) = affine_unit(target)?;

        // Which scale the value is written in. Found from the expression, not
        // from its value: `0 degC` evaluates to plain `0`, because multiplying
        // a unit by zero quite reasonably discards it, and by then there is no
        // temperature left to convert.
        let from = self.temperature_scale_of(value, 0)?;
        let (from_size, from_zero) = affine_unit(&from)?;

        // The reading is then whatever the value measures in those degrees.
        let reading = simplify(&self.eval_in(
            &Expr::div(value.clone(), Expr::var(&from)),
            ctx,
        ));
        let reading = reading.as_num()?;

        // To kelvin, then back out into the target scale.
        let kelvin = reading.mul(&from_size).add(&from_zero);
        let converted = kelvin.sub(&to_zero).div(&to_size);
        Some(simplify(&Expr::mul(vec![
            Expr::Num(converted, Radix::Dec),
            Expr::var(target),
        ])))
    }

    /// The temperature scale an expression is written in, looking through names
    /// to the definitions behind them.
    fn temperature_scale_of(&self, expr: &Expr, depth: usize) -> Option<String> {
        if depth > 8 {
            return None;
        }
        match expr {
            Expr::Var(name) => {
                if affine_unit(name).is_some() {
                    return Some(name.clone());
                }
                let def = self.defs.get(name)?;
                self.temperature_scale_of(&def.body, depth + 1)
            }
            Expr::Mul(items) | Expr::Add(items) => items
                .iter()
                .find_map(|item| self.temperature_scale_of(item, depth + 1)),
            _ => None,
        }
    }
}

/// Keeps the author's spelling of a unit for display. If evaluating the unit
/// expression produced a scaled form (because it named a document definition),
/// fall back to the literal text the author wrote.
fn strip_unit_scale(evaluated: &Expr, written: &Expr) -> Expr {
    // A numeric factor in the evaluated form means the name carried a scale
    // (`sacks = 25 kg`); the quotient has already absorbed it, so print the
    // name. Otherwise the evaluated form is a pure product of units and is
    // safe — and preferred, since a document that writes `N = kg*m/s^2`
    // expects to see `kg*m/s^2` back.
    if contains_number(evaluated) || matches!(evaluated, Expr::Error(_)) {
        written.clone()
    } else {
        evaluated.clone()
    }
}

fn contains_number(expr: &Expr) -> bool {
    match expr {
        Expr::Num(..) => true,
        Expr::Mul(items) | Expr::Add(items) => items.iter().any(contains_number),
        Expr::Pow(a, _) => contains_number(a),
        _ => false,
    }
}

/// Recognizes `in hex`, `in binary`, `in base 8` and friends.
fn radix_of(unit: &Expr) -> Option<Radix> {
    let name = match unit {
        Expr::Var(name) => name.clone(),
        Expr::Mul(factors) => {
            // `base 2` parses as `base * 2`.
            if factors.len() == 2 {
                if let (Expr::Var(word), Expr::Num(value, _)) = (&factors[0], &factors[1]) {
                    if word == "base" {
                        return match value.to_i64() {
                            Some(2) => Some(Radix::Bin),
                            Some(8) => Some(Radix::Oct),
                            Some(10) => Some(Radix::Dec),
                            Some(16) => Some(Radix::Hex),
                            _ => None,
                        };
                    }
                }
            }
            return None;
        }
        _ => return None,
    };
    match name.as_str() {
        "bin" | "binary" => Some(Radix::Bin),
        "oct" | "octal" => Some(Radix::Oct),
        "dec" | "decimal" => Some(Radix::Dec),
        "hex" | "hexadecimal" => Some(Radix::Hex),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_expr;

    fn env() -> Env {
        Env::with_prelude()
    }

    fn e(env: &Env, src: &str) -> String {
        render(&env.eval(&parse_expr(src)))
    }

    fn run(lines: &[&str]) -> (Env, String) {
        let mut env = Env::with_prelude();
        let mut last = String::new();
        for line in lines {
            for statement in parse_line(line) {
                match statement.stmt {
                    Stmt::Define { name, params, body } => env.define(&name, params, body),
                    Stmt::Expr(expr) => last = render(&env.eval(&expr)),
                    Stmt::Equation { lhs, rhs } => env.push_equation(lhs, rhs),
                    _ => {}
                }
            }
        }
        (env, last)
    }

    #[test]
    fn substitutes_definitions() {
        let (_, out) = run(&["stamp = $0.73", "budget = $50", "budget/stamp"]);
        assert_eq!(out, "68.4932");
    }

    #[test]
    fn definitions_are_lazy_so_later_edits_flow_through() {
        // The behaviour that forces definitions to stay unevaluated: redefining
        // an input changes an answer defined before it.
        let (_, before) = run(&["rate = $10", "total = 3 * rate", "total"]);
        assert_eq!(before, "$30");
        let (_, after) = run(&["rate = $10", "total = 3 * rate", "rate = $20", "total"]);
        assert_eq!(after, "$60");
    }

    #[test]
    fn self_reference_declares_a_free_symbol() {
        // `r = r` and `A => A` must not hang.
        let (_, out) = run(&["r = r", "r"]);
        assert_eq!(out, "r");
        let (_, out) = run(&["v = i * r", "r = r", "v"]);
        assert_eq!(out, "i*r");
    }

    #[test]
    fn calls_functions_positionally_and_by_name() {
        let (_, out) = run(&["energy(m, v) = m*v^2/2", "energy(4, 3)"]);
        assert_eq!(out, "18");
        let (_, out) = run(&["energy(m, v) = m*v^2/2", "energy(v=3, m=4)"]);
        assert_eq!(out, "18");
    }

    #[test]
    fn definitions_become_functions_with_implicit_parameters() {
        let lerp = "mix = low + fraction * (high - low)";
        let (_, out) = run(&[lerp, "mix(fraction = 0)"]);
        assert_eq!(out, "low");
        let (_, out) = run(&[lerp, "mix(fraction = 1)"]);
        assert_eq!(out, "high");
        let (_, out) = run(&[lerp, "mix(fraction = 0.8)"]);
        assert_eq!(out, "0.8 high + 0.2 low");
    }

    #[test]
    fn named_arguments_may_bind_any_free_variable() {
        let (_, out) = run(&["y(x) = m*x + b", "y(10, m=2, b=103)"]);
        assert_eq!(out, "123");
        // Terms are sorted by key with the constant last, so a result is the
        // same every time it is recomputed.
        let (_, out) = run(&["y(x) = m*x + b", "y(10)"]);
        assert_eq!(out, "b + 10 m");
    }

    #[test]
    fn units_stay_symbolic_until_a_conversion_forces_them() {
        let env = env();
        // Unlike terms do not collapse into seconds.
        assert_eq!(e(&env, "4 weeks + 6 days"), "6 days + 4 weeks");
        // But a conversion expands everything.
        assert_eq!(e(&env, "100 ft in m"), "30.48 m");
        assert_eq!(e(&env, "100 yards in m"), "91.44 m");
        assert_eq!(e(&env, "6 tablespoons in cups"), "0.375 cups");
    }

    #[test]
    fn uncertainty_propagates_by_first_order_analysis() {
        let (env, _) = run(&["m = 2 ± 1", "a = 10 ± 0.3", "b = 7 ± 0.4"]);
        let u = |src: &str| render(&env.eval_uncertain(&parse_expr(src)).unwrap());
        assert_eq!(u("m"), "2 ± 1");
        assert_eq!(u("m + 3"), "5 ± 1");
        // First-order, not interval: sigma is 2·m·σ, and the centre is exact.
        assert_eq!(u("m^2"), "4 ± 4");
        // Independent errors add in quadrature: sqrt(0.3² + 0.4²) = 0.5.
        assert_eq!(u("a + b"), "17 ± 0.5");
        assert_eq!(u("a*b"), "70 ± 4.5");
        // Correlation through a shared variable cancels exactly.
        assert_eq!(u("m - m"), "0");
        // And partially: a/(a+b) is not the independent-quotient formula.
        assert_eq!(u("a/(a + b)"), "0.588 ± 0.016");
    }

    #[test]
    fn uncertainty_carries_units_and_conversions() {
        // No parentheses needed: `±` fuses number-to-number before the
        // unit applies, so the sigma is in millamps too.
        let (env, _) = run(&["current = 50 ± 1 mA", "R = 2 kΩ"]);
        let u = |src: &str| render(&env.eval_uncertain(&parse_expr(src)).unwrap());
        assert_eq!(u("current"), "(50 ± 1) mA");
        assert_eq!(u("current in A"), "(0.05 ± 0.001) A");
        assert_eq!(u("current * R in V"), "(100 ± 2) V");
    }

    #[test]
    fn plus_minus_binds_tighter_than_multiplication_and_powers() {
        let env = env();
        let u = |src: &str| render(&env.eval_uncertain(&parse_expr(src)).unwrap());
        assert_eq!(u("4 * 2 ± 0.1"), "8 ± 0.4");
        assert_eq!(u("2 ± 0.1 mm"), "(2 ± 0.1) mm");
        assert_eq!(u("2 ± 0.1 ^ 2"), "4 ± 0.4");
    }

    #[test]
    fn unfused_plus_minus_reads_like_addition() {
        let (env, _) = run(&["x = 12", "dx = 0.25"]);
        let u = |src: &str| render(&env.eval_uncertain(&parse_expr(src)).unwrap());
        assert_eq!(u("x ± dx"), "12 ± 0.25");
        // Like `+`, the product to the left is the centre and the term to
        // the right is the sigma.
        assert_eq!(u("2*x ± dx"), "24 ± 0.25");
        assert_eq!(u("x ± dx/2"), "12 ± 0.13");
    }

    #[test]
    fn certain_expressions_take_the_ordinary_path() {
        let (env, _) = run(&["x = 4"]);
        assert!(env.eval_uncertain(&parse_expr("x + 1")).is_none());
        assert!(env.eval_uncertain(&parse_expr("100 ft in m")).is_none());
    }

    #[test]
    fn intervals_convert_scale_and_pass_through_monotone_functions() {
        let env = env();
        // Endpoint-wise conversion, scaling, and inversion.
        assert_eq!(e(&env, "(100..200) cm in m"), "(1..2)*m");
        assert_eq!(e(&env, "(1..3) * 30 mA"), "(30..90)*mA");
        assert_eq!(e(&env, "(2..4)^-1"), "0.25..0.5");
        // Monotone functions act on the endpoints; sqrt keeps exactness.
        assert_eq!(e(&env, "sqrt(1..4)"), "1..2");
        assert_eq!(e(&env, "log10(10..1000)"), "1..3");
        // A negative-reaching interval has no real square root; stay put.
        assert_eq!(e(&env, "sqrt(-4..4)"), "sqrt(-4..4)");
    }

    #[test]
    fn converts_compound_units() {
        let env = env();
        assert_eq!(e(&env, "12 V * 2 A in kW"), "0.024 kW");
        assert_eq!(e(&env, "42 mph in kmph"), "67.5924 kmph");
        let (_, out) = run(&[
            "distance = 3.1 miles",
            "time = 27 minutes",
            "distance / time in miles/hour",
        ]);
        assert_eq!(out, "6.8889 miles/hour");
    }

    #[test]
    fn applies_si_prefixes_without_enumerating_them() {
        let env = env();
        assert_eq!(e(&env, "123,456.789012 W in kW"), "123.4568 kW");
        assert_eq!(e(&env, "123,456.789012 W in MW"), "0.1235 MW");
        assert_eq!(e(&env, "123,456.789012 W in mW"), "123,456,789.012 mW");
        assert_eq!(e(&env, "123,456.789012 W in µW"), "123,456,789,012 µW");
    }

    #[test]
    fn user_defined_units_work_like_built_in_ones() {
        let (_, out) = run(&["sacks = 25 kg", "1 tonne in sacks"]);
        assert_eq!(out, "40 sacks");
    }

    #[test]
    fn converts_to_percent_and_radix() {
        let env = env();
        assert_eq!(e(&env, "21/45 in %"), "46.6667%");
        assert_eq!(e(&env, "200 in hex"), "0xC8");
        assert_eq!(e(&env, "200 in octal"), "0o310");
        assert_eq!(e(&env, "200 in binary"), "0b11001000");
    }

    #[test]
    fn tracks_units_through_a_physics_model() {
        let (_, out) = run(&[
            "x(t) = 1/2 * a * t^2 + v0*t + x0",
            "a = -9.8m/s/s",
            "v0 = 100m/s",
            "x0 = 490m",
            "x(10s)",
        ]);
        assert_eq!(out, "1,000 m");
    }

    #[test]
    fn spreadsheet_style_accumulation() {
        let (_, out) = run(&[
            "job total = callout + (parts + labour) * hours",
            "hours   = 6",
            "callout = $80",
            "parts   = $45",
            "labour  = $65",
            "job total",
        ]);
        assert_eq!(out, "$740");
    }

    #[test]
    fn currency_conversion() {
        let env = env();
        assert!(e(&env, "$20 in eur").ends_with("eur") || e(&env, "$20 in eur").ends_with('€'));
        let (_, out) = run(&["$33 in ¥"]);
        assert!(out.contains('¥'), "expected a yen result, got {out}");
    }

    #[test]
    fn a_recursion_whose_guard_cannot_resolve_runs_out_of_fuel() {
        // `nn` for `n` — one typo'd character in the base case, so the guard
        // never resolves and *both* recursive calls expand. MAX_CALLS bounds
        // the depth, but a two-branch recursion doubles per level: the
        // symbolic result is exponential, and only the fuel budget stands
        // between a typo and a frozen editor. This test finishing at all is
        // the regression check.
        let source = "fib(n) = if nn < 2 then n else fib(n - 1) + fib(n - 2)\nfib(30) =>";
        let document = crate::doc::evaluate(source);
        assert_eq!(document.answers.len(), 1);
        assert!(document.answers[0].is_error, "got {:?}", document.answers[0]);
        assert_eq!(document.answers[0].text, FUEL_ERROR);
    }
}
