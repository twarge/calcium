// Calcium in the browser: the same Rust engine the apps use, compiled to
// WebAssembly, speaking the same C-string ABI over linear memory. No
// wasm-bindgen — the interface is a few functions over strings, and the
// marshalling lives in engine.js, shared with the evaluation worker.
//
// The editing model is the apps' too, ported from the Swift coordinator:
// answers live in the text after each `=>`, written in when typing pauses,
// with the caret adjusted across every splice. Evaluation runs in a worker,
// as it runs off the main thread in the apps: a slow document costs late
// answers, never a frozen editor. See EditorView.swift for the reference
// implementation and the reasoning.

import { load } from "./engine.js";

const { call } = await load();
const lineInfo = (text) => JSON.parse(call("calcium_line_kinds", text) ?? "[]");
const tokenInfo = (text) => JSON.parse(call("calcium_tokens", text) ?? "[]");
// Where each answer sits in the live text — parsed, not evaluated, so it is
// cheap enough for every paint. The engine's parser decides what an arrow
// is: not one inside a string literal, and one per `;`-separated statement.
const answerSlots = (text) => JSON.parse(call("calcium_answer_slots", text) ?? "[]");

const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" });

// ---------------------------------------------------------------------------
// Splicing — the three-case caret adjustment from the apps. JavaScript string
// indices are UTF-16 code units, which is exactly what the engine reports.
// ---------------------------------------------------------------------------

function adjust(caret, editStart, editEnd, newLength) {
  if (caret <= editStart) return caret;
  if (caret <= editEnd) return editStart + Math.min(caret - editStart, newLength);
  return caret + newLength - (editEnd - editStart);
}

/** Writes each fresh answer into the range the engine gave for it — the
 *  text must be the text that was evaluated; returns [text, caret]. */
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
    if (answer.start > answer.end || answer.end > line.length) continue;
    const from = starts[answer.line] + answer.start;
    const to = starts[answer.line] + answer.end;
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
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&#39;");

// Markdown links on prose lines become real anchors — the backdrop sits
// above the textarea with pointer-events off, and the anchors alone opt
// back in, so link clicks land while every other click falls through to
// the caret. http(s) only; anything else stays plain text.
const LINK = /\[([^\]\n]+)\]\(([^)\s]+)\)/g;
function linkifyProse(line) {
  let html = "";
  let pos = 0;
  for (const m of line.matchAll(LINK)) {
    const url = m[2];
    if (!/^https?:\/\//i.test(url)) continue;
    html += escapeHTML(line.slice(pos, m.index));
    html += `<span class="dim">[</span>` +
      `<a href="${escapeHTML(url)}" target="_blank" rel="noopener">${escapeHTML(m[1])}</a>` +
      `<span class="dim">](${escapeHTML(url)})</span>`;
    pos = m.index + m[0].length;
  }
  html += escapeHTML(line.slice(pos));
  return html;
}

function renderBackdrop(text, answers, info, toks, slots) {
  // The live text's slots, each paired with the last evaluation's answer
  // for the same line and position on it: the value may be a beat stale,
  // the range never is.
  const slotsByLine = new Map();
  const seen = new Map();
  for (const slot of slots) {
    const nth = seen.get(slot.line) ?? 0;
    seen.set(slot.line, nth + 1);
    const answer = answers.filter((a) => a.line === slot.line)[nth];
    if (!answer) continue;
    if (!slotsByLine.has(slot.line)) slotsByLine.set(slot.line, []);
    slotsByLine.get(slot.line).push([slot.start, slot.end, answer.error ? "error" : "answer"]);
  }
  return text
    .split("\n")
    .map((line, i) => {
      const meta = info[i] ?? { kind: "code" };
      if (meta.kind === "prose") {
        return `<span class="prose">${linkifyProse(line)}</span>`;
      }
      if (meta.kind === "raw") {
        return `<span class="raw">${escapeHTML(line)}</span>`;
      }
      let cls = meta.kind === "heading" ? "heading" : "";

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
      for (const cut of slotsByLine.get(i) ?? []) {
        if (cut[0] < cut[1]) cuts.push(cut);
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
    editor.value, lastAnswers, lineInfo(editor.value), tokenInfo(editor.value),
    answerSlots(editor.value));
  // A trailing newline in a <textarea> renders one line shorter than the same
  // text in a <div>; pad so the columns stay aligned.
  if (editor.value.endsWith("\n")) backdrop.innerHTML += "\n";
}

/// The full pass — evaluate in the worker, then splice and repaint — after
/// typing pauses. Only the newest request's reply is honoured, and the splice
/// only lands if the text has not moved since the request: if it has, the
/// input handler has already scheduled the refresh that will supersede this.
let evalSeq = 0;
let evalText = "";

worker.onmessage = ({ data }) => {
  if (data.seq !== evalSeq) return;
  lastAnswers = data.answers;
  if (editor.value === evalText) {
    const [text, caret] = splice(editor.value, lastAnswers, editor.selectionStart);
    if (text !== editor.value) {
      editor.value = text;
      editor.setSelectionRange(caret, caret);
    }
  }
  paint();
};

function refresh() {
  evalText = editor.value;
  worker.postMessage({ seq: ++evalSeq, text: evalText });
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

Compute with uncertainty:

    current = 2±0.1 mA
    resistance = 10±2 Ω
    voltage = current * resistance in mV =>

_Unknown_ units **cancel**!

    burrito length = 1 ft / burrito
    burrito cost = 8 USD / burrito
    1 mile / burrito length * burrito cost in kEUR => 37.1307 kEUR

It is a symbolic calculator, too:

    y^2 - 5y + 6 = 0
    y =>

Plots — \`plot(sin(t), t = 0..2 s)\` — draw in the Mac and iOS apps.

Calcium is free and [open source](https://github.com/twarge/calcium). Feel free to open issues on [GitHub](https://github.com/twarge/calcium/issues).

`;
paint();
refresh();
