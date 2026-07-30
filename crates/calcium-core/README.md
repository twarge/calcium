# calcium-core

The engine of [Calcium](https://github.com/twarge/calcium): Markdown
documents in which any line can be a calculation. Write `2 + 2 =>` and the
answer belongs after the arrow; change a definition and every dependent
answer follows.

- **Units are algebra.** `88 mph in km/hour`, `40 miles/gallon in
  furlongs/hogshead`, units you define yourself (`giraffes = 900 kg`), and
  unknown units that cancel dimensionally.
- **Exact arithmetic** on rationals, with complex numbers, radix literals,
  and affine temperature conversions (`350 degF in degC`).
- **Symbolic solving**: `330 m = 1/2 * g * t^2` then `t =>` answers in
  seconds.
- **Editor services**: line classification, token spans, and completions
  with current values, all reported by the same lexer and evaluator that
  compute the answers — an editor built on them cannot drift from the
  engine.

```rust
use calcium_core::doc;

let source = "    speed = 88 mph in km/hour =>";
let answers = doc::evaluate(source).answers;
assert_eq!(answers[0].text, "141.6223 km/hour");

// The document with answers written in after each `=>`:
let on_disk = doc::rewrite(source);
```

The interface is strings in, structures out; there is no state to hold
between calls. The macOS/iOS apps consume it over a C ABI, the web demo
over WebAssembly, and the GTK app links this crate directly.

Licensed Apache-2.0.
