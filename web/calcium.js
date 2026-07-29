// Calcium in the browser: the same Rust engine the apps use, compiled to
// WebAssembly, speaking the same C-string ABI over linear memory. No
// wasm-bindgen — the interface is four functions over strings, and the
// marshalling below is the whole of it.
//
// The editing model is the apps' too, ported from the Swift coordinator:
// answers live in the text after each `=>`, written in when typing pauses,
// with the caret adjusted across every splice. See EditorView.swift for the
// reference implementation and the reasoning.

const wasm = await WebAssembly.instantiateStreaming(
  fetch(new URL("calcium_ffi.wasm", import.meta.url)));
const engine = wasm.instance.exports;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** Calls a `char* -> char*` export, owning both buffers correctly. */
function call(name, text) {
  const bytes = encoder.encode(text);
  const inPtr = engine.calcium_alloc(bytes.length + 1);
  new Uint8Array(engine.memory.buffer, inPtr, bytes.length).set(bytes);
  new Uint8Array(engine.memory.buffer)[inPtr + bytes.length] = 0;
  const outPtr = engine[name](inPtr);
  engine.calcium_dealloc(inPtr, bytes.length + 1);
  if (!outPtr) return null;
  // Re-acquire the buffer: it may have moved if memory grew during the call.
  const memory = new Uint8Array(engine.memory.buffer);
  let end = outPtr;
  while (memory[end] !== 0) end++;
  const result = decoder.decode(memory.subarray(outPtr, end));
  engine.calcium_string_free(outPtr);
  return result;
}

const evaluate = (text) => JSON.parse(call("calcium_evaluate", text) ?? "[]");
const lineInfo = (text) => JSON.parse(call("calcium_line_kinds", text) ?? "[]");
const tokenInfo = (text) => JSON.parse(call("calcium_tokens", text) ?? "[]");

// ---------------------------------------------------------------------------
// Splicing — the three-case caret adjustment from the apps. JavaScript string
// indices are UTF-16 code units, which is exactly what the engine reports.
// ---------------------------------------------------------------------------

function adjust(caret, editStart, editEnd, newLength) {
  if (caret <= editStart) return caret;
  if (caret <= editEnd) return editStart + Math.min(caret - editStart, newLength);
  return caret + newLength - (editEnd - editStart);
}

/** Writes fresh answers after each `=>`; returns [text, caret]. */
function splice(text, answers, caret) {
  const lines = text.split("\n");
  const starts = [];
  let at = 0;
  for (const line of lines) {
    starts.push(at);
    at += line.length + 1;
  }
  const edits = [];
  for (const answer of answers) {
    const line = lines[answer.line];
    if (line === undefined) continue;
    const arrow = line.indexOf("=>");
    if (arrow < 0) continue;
    const from = starts[answer.line] + arrow + 2;
    const to = starts[answer.line] + line.length;
    const replacement = answer.text ? " " + answer.text : "";
    if (text.slice(from, to) !== replacement) edits.push([from, to, replacement]);
  }
  edits.sort((a, b) => b[0] - a[0]);
  for (const [from, to, replacement] of edits) {
    text = text.slice(0, from) + replacement + text.slice(to);
    caret = adjust(caret, from, to, replacement.length);
  }
  return [text, caret];
}

// ---------------------------------------------------------------------------
// Rendering the backdrop
// ---------------------------------------------------------------------------

const escapeHTML = (s) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function renderBackdrop(text, answers, info, toks) {
  const answerByLine = new Map(answers.map((a) => [a.line, a]));
  return text
    .split("\n")
    .map((line, i) => {
      const meta = info[i] ?? { kind: "code" };
      let cls = meta.kind === "heading" ? "heading" : meta.kind === "prose" ? "prose" : "";

      // Split the line into optionally-styled segments, left to right.
      // Redefinition first, so on an equal start its underline wins the
      // first-wins rule below; token colours fill in around the other cuts —
      // the engine stops tokens at the arrow and the comment, so they never
      // collide with the answer or comment spans.
      const cuts = [];
      if (meta.redefines) cuts.push([meta.redefines[0], meta.redefines[0] + meta.redefines[1], "redef"]);
      for (const t of toks?.[i] ?? []) {
        if (t.c !== "name") cuts.push([t.o, t.o + t.l, "tok-" + t.c]);
      }
      const answer = answerByLine.get(i);
      if (answer) {
        const arrow = line.indexOf("=>");
        if (arrow >= 0 && arrow + 2 < line.length)
          cuts.push([arrow + 2, line.length, answer.error ? "error" : "answer"]);
      }
      if (meta.comment !== undefined && meta.comment !== null)
        cuts.push([meta.comment, line.length, "comment"]);
      cuts.sort((a, b) => a[0] - b[0]);

      let html = "";
      let pos = 0;
      for (const [from, to, klass] of cuts) {
        if (from < pos) continue; // overlapping (comment inside answer): first wins
        html += escapeHTML(line.slice(pos, from));
        html += `<span class="${klass}">${escapeHTML(line.slice(from, to))}</span>`;
        pos = to;
      }
      html += escapeHTML(line.slice(pos));
      return cls ? `<span class="${cls}">${html}</span>` : html;
    })
    .join("\n");
}

// ---------------------------------------------------------------------------
// The editor loop
// ---------------------------------------------------------------------------

const editor = document.getElementById("editor");
const backdrop = document.getElementById("backdrop");
document.getElementById("loading").remove();

let pending = null;
let lastAnswers = [];

/// Repaints the backdrop from the current text. This runs on *every* input,
/// synchronously: the textarea's own glyphs are transparent, so the backdrop
/// is the only place typed characters are visible — if it waited for the
/// debounce, typing would be invisible until the pause. Only the answer
/// *values* may be a beat stale here; their positions are recomputed against
/// the live text.
function paint() {
  backdrop.innerHTML = renderBackdrop(
    editor.value, lastAnswers, lineInfo(editor.value), tokenInfo(editor.value));
  // A trailing newline in a <textarea> renders one line shorter than the same
  // text in a <div>; pad so the columns stay aligned.
  if (editor.value.endsWith("\n")) backdrop.innerHTML += "\n";
}

/// The full pass — evaluate, splice, repaint — after typing pauses.
function refresh() {
  lastAnswers = evaluate(editor.value);
  const [text, caret] = splice(editor.value, lastAnswers, editor.selectionStart);
  if (text !== editor.value) {
    editor.value = text;
    editor.setSelectionRange(caret, caret);
  }
  paint();
}

editor.addEventListener("input", () => {
  paint();
  clearTimeout(pending);
  pending = setTimeout(refresh, 150);
});
editor.addEventListener("scroll", () => {
  backdrop.scrollTop = editor.scrollTop;
});

editor.value = `# Calcium

Calcium is a text editor that updates math results as you type. Use \`=>\` to compute expressions. (Try it on this web page—it's the same code as the app.)

    speed = 88 mph in km/hour =>
    fuel = 12 gallon
    range = fuel * 32 miles/gallon in km =>

Odd units!

    walking speed = 1 mph
    walking speed in furlongs/fortnight
        => 2,688 furlongs/fortnight

_Unknown_ units **cancel**!

    burrito length = 1 ft / burrito
    burrito cost = 8 USD / burrito
    1 mile / burrito length * burrito cost in kEUR => 37.1307 kEUR

It is a symbolic calculator, too:

    y^2 - 5y + 6 = 0
    y =>

Try it here!

Calcium is free and [open source](https://github.com/twarge/calcium). Feel free to open issues on GitHub.

`;
refresh();
