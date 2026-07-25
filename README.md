# Calcium

A Markdown text editor where any line can be a calculation, recomputed as you
type. Write `2 + 2 =>` and the answer appears in the document text:

```
    distance   = 420 miles
    efficiency = 30 miles/gallon
    fuel price = 3.45 $/gallon

    fuel needed = distance / efficiency    => 14 gallon
    trip cost   = fuel needed * fuel price => $48.3
```

Change `efficiency` and every answer below it updates. The file is plain
Markdown, so it can be mailed, committed to git, or opened in any editor.

The engine is Rust; the macOS/iOS UI will be Swift on top of it. See
[Why Rust with a Swift UI](#why-rust-with-a-swift-ui) for where the split falls.

```
crates/calcium-core/    the engine — lexer, parser, simplifier, solver, units
crates/calcium-cli/     `calcium run` / `calcium check`
corpus/                 hand-written test documents
tests/golden.rs         runs the corpus as a regression test
```

## Status

96 unit tests, plus 261 end-to-end expectations in `corpus/`, all passing.

| Document | Expectations |
|---|---|
| `corpus/tour.calcium` | 69 — a readable introduction to the language |
| `corpus/reference.calcium` | 159 — systematic, one section per feature |
| `corpus/worked.calcium` | 33 — realistic documents end to end |

```bash
cargo test                                            # unit tests + the corpus
cargo run -p calcium-cli -- check corpus/*.calcium    # per-line report
cargo run -p calcium-cli -- run corpus/tour.calcium   # rewrite a document
```

Because a document carries its own answers, any document is a test: `check`
recomputes every `=>` and reports what disagrees. That works on your own files
too, not just the corpus.

The corpus is written by hand rather than generated from this engine — a corpus
blessed from its own output would pass by construction and prove nothing.
Numeric answers were derived independently with exact `Fraction` arithmetic in
Python; algebraic ones by working out the algebra. Writing them that way
immediately turned up five real bugs: recursive functions never reached their
base case, `0xFF + 1` refused to add, products of sums like `(2 + 3i)*(2 - 3i)`
were left unexpanded, `tod` dropped coefficients of 1, and `x^-1` printed
instead of `1/x`.

It also caught the engine being *right* where a first-pass float calculation was
wrong: the mortgage total in `worked.calcium` differs in the fourth decimal
between exact rationals and floats, and the engine has the exact one.

No UI yet. That is the next step.

## Architecture

Two observations shaped everything, and both remove machinery you would
otherwise expect to need.

**A unit is just a definition.** Units are defined in the language itself —
`ft = 3048 m / 10000`, `N = kg*m/s^2` — so dimensional analysis is not a
subsystem, it is ordinary symbolic algebra over symbols that happen to name
units. [`prelude.calcium`](crates/calcium-core/src/prelude.calcium) is an
ordinary document, parsed at startup like any other.

The one twist: prelude definitions are **opaque** in normal expressions and
expand only under an `in` conversion. That is what makes `4 days + 3 weeks` keep
two unlike terms instead of collapsing to seconds, while `in days` still
converts. Definitions a *document* makes always expand.

**Complex numbers are the same trick.** `i` is an ordinary symbol plus one
rewrite, `i^2 -> -1`. That single rule gives `i*i => -1`, `i^43 => -i` and
`(1 + 2i)*i => i - 2` from the machinery that already collects units.

The rest is conventional:

| Module | Role |
|---|---|
| [`num`](crates/calcium-core/src/num.rs) | Exact `BigRational`, falling back to `f64` only when an operation leaves the rationals |
| [`lexer`](crates/calcium-core/src/lexer.rs) | Tokens, including comma-grouped numbers and the Unicode operator set |
| [`parser`](crates/calcium-core/src/parser.rs) | Recursive descent |
| [`simplify`](crates/calcium-core/src/simplify.rs) | Canonical sum-of-products; collects like terms |
| [`eval`](crates/calcium-core/src/eval.rs) | Environment, substitution, `in` conversion |
| [`builtins`](crates/calcium-core/src/builtins.rs) | Standard library, including symbolic differentiation |
| [`solve`](crates/calcium-core/src/solve.rs) | Linear, quadratic, and matrix equations |
| [`format`](crates/calcium-core/src/format.rs) | Expressions back to source text |
| [`doc`](crates/calcium-core/src/doc.rs) | Blocks, prose-vs-code, writing answers back |

### Three things worth knowing before editing

**Definitions are stored unevaluated.** See the redefinition section of
`corpus/reference.calcium`: `scaled = base * 3` answers `30`, then `base` is
redefined and the *same* definition answers `60`. A definition is a closure over
names resolved at use site, never a value captured at definition site.

**The formatter is load-bearing, not cosmetic.** Answers are written *into* the
document as text, so whatever `format` prints is what the user edits and what
gets re-parsed. `render` must produce something `parse_expr` reads back to the
same tree; `tests/golden.rs` asserts this over every answer in the corpus.

**Two adjacent bare words are one identifier.** `mass of earth` is a single
name, which means a product of units needs an explicit `*`: `N = kg m/s^2` would
define `N` in terms of a variable literally named `kg m`.

### Grammar corners that are easy to get wrong

*Implicit multiplication binds tighter than explicit.* `2x/3y` is `(2x)/(3y)`,
but `2*x/3*y` is `((2*x)/3)*y`. This is a real precedence level.

*`in` is overloaded.* It is the conversion keyword in `100 ft in m` and the unit
*inches* in `5 ft + 4 in`. Resolved by lookahead: `in` converts only when
something that can begin an expression follows it.

*`|` is context-dependent.* Prefix it opens an absolute value, infix it is
bitwise or. `|foo|^2 + |bar|^2` needs the parser to know which position it is in.

## Known limitations

**`i` is the imaginary unit.** Anything using `i` for electric current will
misbehave, because solving `v = i*r` for `r` gives `v/i`, which reduces under
`i^2 = -1`. Any other name for current works.

**Single-letter unit collisions.** `[A, B, C]` picks up ampere, byte and
coulomb. Two distinct unit symbols also do not compare unequal, so `if x == B`
stays symbolic. Both are fixable; neither is fixed.

**A name cannot contain the word `in`,** which is reserved for conversions —
`items in cart` parses as a conversion of `items`.

**Culture-specific number input.** `@fr-FR` output is correct (`3 141,59`) but
the lexer only *reads* invariant-culture numbers, so `beta = 12 000,62` fails.

**Currency rates** are a fixed snapshot and belong behind a refresh mechanism
before anyone relies on them for money.

**Not implemented.** `plot(...)` passes through untouched for a UI layer to pick
up; `#?` (AI autocomplete) parses but does not resolve; heading-scoped
definitions are flat rather than scoped.

## Why Rust with a Swift UI

The core is one pure function — `evaluate(documentText) -> [LineResult]`. String
in, a flat array of answers out. No object graph crosses the boundary, no
callbacks, no shared mutable state. That is the best case for a language split,
and it is why the usual regret about Rust cores (a chatty, stateful interface)
does not apply.

Three reasons, in order of weight:

1. **The FFI surface is tiny and pure.** At document scale you could serialize
   the result as JSON and never measure the cost.
2. **Exact arithmetic.** A CAS needs exact rationals and bignums or
   simplification drifts. `num-rational` + `num-bigint` hand you this; in Swift
   you would be writing BigInt. This argument holds even without a Linux port.
3. **Portability compounds.** The same core gets Linux, WASM, a CLI, and
   Windows. Swift-on-Linux would buy only Linux, and SwiftUI does not cross the
   line either way — a Linux UI is a rewrite regardless.

## Next steps

- **Swift app.** `DocumentGroup` + TextKit 2, bridged with UniFFI. The engine
  already exposes the shape needed: `doc::evaluate` returns answers with line
  numbers, and `doc::rewrite` produces the updated buffer.
- **Decide where answers live.** Writing `=> 4` into the document text means the
  app rewrites the buffer on every keystroke, with real UX edges around undo
  coalescing and cursor preservation. The alternative is to render answers in a
  gutter and only materialize them into text on save or export. The first is
  truer to the format; the second is easier to get right. Worth picking
  deliberately before writing UI code.
- **Incremental evaluation.** `cargo run --release -p calcium-cli --example bench`
  reports the cost of re-evaluating a document from scratch: ~2.6 ms for the
  420-line reference, of which 0.3 ms is the prelude. Fine for ordinary
  documents, but it scales worse than linearly, and a file several times longer
  would need either per-block caching or evaluation off the main thread before
  it kept up with typing.
