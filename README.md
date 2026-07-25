# Calcium

A Markdown text editor where any line can be a calculation, recomputed as you
type. Write `2 + 2 =>` and the answer appears beside it — and is saved into the
file:

```
    distance   = 420 miles
    efficiency = 30 miles/gallon
    fuel price = 3.45 $/gallon

    fuel needed = distance / efficiency    => 14 gallon
    trip cost   = fuel needed * fuel price => $48.3
```

Change `efficiency` and every answer below it updates. The file is plain
Markdown, so it can be mailed, committed to git, or opened in any editor.

A Rust engine with a Swift document app on top. See
[Why Rust with a Swift UI](#why-rust-with-a-swift-ui) for where the split falls.

```
crates/calcium-core/    the engine — lexer, parser, simplifier, solver, units
crates/calcium-ffi/     C ABI: three functions over String -> String
crates/calcium-cli/     `calcium run` / `calcium check`
apps/Calcium/           the macOS app
corpus/                 hand-written test documents
tests/golden.rs         runs the corpus as a regression test
```

## Status

105 unit tests, plus 261 end-to-end expectations in `corpus/`, all passing.

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

## The app

```bash
./apps/build.sh            # debug
./apps/build.sh release
```

A document-based macOS app: `DocumentGroup` and an `NSTextView`, with the find
bar, Find menu and replace-all that TextEdit has.

**Answers live in the text**, written in after each `=>` once typing pauses.
Type `1+2=>` and the answer appears *after* the caret, which stays where you
left it — so Return carries on to the next line, and typing carries on where
you were.

**Nothing is locked.** The caret goes anywhere, including past the answer, and
any edit is allowed. Backspacing into an answer deletes a character that is
immediately written back, so the visible effect is simply that the caret steps
left. Protecting the answer would take more machinery and read worse; letting it
be overwritten and restored gets the same result for free. Delete the `=>` and
its answer goes with it.

What that does demand is exact bookkeeping of the insertion point across a
splice. The boundaries in `adjust(_:for:)` are the whole design:

| Caret relative to the splice | Result |
|---|---|
| at or before it | unchanged — this is why the answer lands *after* the caret |
| inside, up to and including its far end | offset kept, clamped to the new length — this is why backspace reads as a step left |
| past it | shifted by the change in length |

Four things that are easy to get wrong, each fixed because it broke in use:

- **Splices bypass the undo stack**, and the text view keeps its *own* undo
  manager. Sharing the window's means sharing it with SwiftUI's document
  binding, which this editor writes to on every keystroke; the two interleave
  and Cmd-Z silently does nothing.
- **Anything reasoning about where an answer sits reads the live text**, never a
  cached range. Cached ranges are one refresh out of date while you type.
- **Recomputation waits for a pause in typing.** Rewriting the buffer between
  two keystrokes disturbs the text view's input handling and characters vanish.
- **Every automatic substitution is off**, including the system's
  double-space-inserts-a-period, which has no per-view property and needs an
  app-domain default. In a prose editor a smart quote is a nicety; here it turns
  `3 +  =>` into `3 +. =>` and a string literal into a syntax error.

A line that cannot be computed still answers, in red, with the reason.

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

1. **The FFI surface is tiny and pure.** It came out at three functions —
   evaluate, rewrite, strip — hand-written in
   [`calcium-ffi`](crates/calcium-ffi/src/lib.rs) rather than generated, because
   a code generator would have added a build step, a version to keep in sync and
   a layer to debug through, in exchange for marshalling that fits on one
   screen. Answers cross as JSON and the cost is not measurable at document
   scale.
2. **Exact arithmetic.** A CAS needs exact rationals and bignums or
   simplification drifts. `num-rational` + `num-bigint` hand you this; in Swift
   you would be writing BigInt. This argument holds even without a Linux port.
3. **Portability compounds.** The same core gets Linux, WASM, a CLI, and
   Windows. Swift-on-Linux would buy only Linux, and SwiftUI does not cross the
   line either way — a Linux UI is a rewrite regardless.

## Next steps

- **iOS.** The engine already builds for `aarch64-apple-ios`; the document model
  is shared and only the editor view is AppKit.
- **The typing pause.** Answers appear a beat after you stop typing rather than
  during. That delay is what keeps the splice out of the text view's input
  handling; narrowing it means making the splice interruptible instead.
- **Incremental evaluation.** `cargo run --release -p calcium-cli --example bench`
  reports the cost of re-evaluating a document from scratch: ~2.6 ms for the
  420-line reference, of which 0.3 ms is the prelude. Fine for ordinary
  documents, but it scales worse than linearly, and a file several times longer
  would need either per-block caching or evaluation off the main thread before
  it kept up with typing.
