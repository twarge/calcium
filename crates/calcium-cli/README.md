# calcium-cli

The command line for [Calcium](https://github.com/twarge/calcium):
Markdown documents in which any line can be a calculation. Write
`2 + 2 =>` and the answer belongs after the arrow.

## Install

```bash
cargo install calcium-cli
```

installs the `calcium` binary.

## Use

```bash
calcium run doc.calcium      # print the document with every => answered
calcium check doc.calcium    # recompute answers, report what disagrees
calcium kinds doc.calcium    # how each line reads: heading, prose, code
```

`run` writes fresh answers after each `=>` and prints the result — pipe
it back to the file to update a document in place. `check` makes any
document a test: because a Calcium file carries its own answers, `check`
recomputes every one and reports lines whose stored answer disagrees,
with a pass count at the end. `kinds` shows the engine's reading of each
line, useful when a line you meant as a calculation is being taken as
prose (indent it) or vice versa.

A document, briefly:

```
    fuel = 12 gallon
    range = fuel * 32 miles/gallon in km => 617.9881 km

    walking speed = 1 mph
    walking speed in furlongs/fortnight => 2,688 furlongs/fortnight
```

Units are algebra, arithmetic is exact, equations solve symbolically.
The engine is [calcium-core](https://crates.io/crates/calcium-core);
the macOS/iOS apps and the web demo are in the
[repository](https://github.com/twarge/calcium). Apache-2.0.
