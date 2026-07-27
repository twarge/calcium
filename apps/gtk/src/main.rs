//! The Linux editor: GTK 4 over the same engine, linked as a crate — no C
//! ABI, no JSON, the document layer's own types.
//!
//! The editing model is the Mac editor's, ported: answers live in the text
//! after each `=>`, spliced in as you type with the caret held still by the
//! same three-case adjustment; styling — prose, headings, comments, token
//! colours, inline Markdown — reapplies on every change from the engine's
//! own reports. Return steps over answers and continues list markers;
//! Ctrl+/ toggles comments, Ctrl+] and Ctrl+[ indent and outdent; Ctrl+O,
//! Ctrl+S open and save, answers materialised on disk.
//!
//! Not yet ported: completions, value scrubbing, prose spelling, per-file
//! view state, and the distraction-free chrome.

use calcium_core::doc::{self, BlockKind, TokenClass};
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("com.twarge.calcium")
        .build();
    app.connect_activate(build_window);
    app.run()
}

struct Editor {
    buffer: gtk::TextBuffer,
    window: gtk::ApplicationWindow,
    /// True while our own splice edits run, so they are not mistaken for
    /// the user's and re-entered.
    splicing: Cell<bool>,
    /// One refresh per idle turn, however many change signals arrive.
    queued: Cell<bool>,
    /// The answer text last written per line, so deleting a `=>` takes its
    /// answer with it.
    last_answers: RefCell<HashMap<usize, String>>,
    /// Shared with the file-dialog callbacks, which outlive the call.
    file: Rc<RefCell<Option<std::path::PathBuf>>>,
}

fn build_window(app: &gtk::Application) {
    let buffer = gtk::TextBuffer::new(None);
    buffer.set_enable_undo(true);
    make_tags(&buffer);

    let view = gtk::TextView::with_buffer(&buffer);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_left_margin(16);
    view.set_right_margin(16);
    view.set_top_margin(14);
    view.set_bottom_margin(14);
    view.set_monospace(true);

    // Fira Code when installed; the family list falls back gracefully.
    let css = gtk::CssProvider::new();
    css.load_from_data("textview { font-family: \"Fira Code\", monospace; font-size: 12pt; }");
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let scroll = gtk::ScrolledWindow::builder().child(&view).build();

    let open_button = gtk::Button::from_icon_name("document-open-symbolic");
    open_button.set_tooltip_text(Some("Open (Ctrl+O)"));
    let save_button = gtk::Button::from_icon_name("document-save-symbolic");
    save_button.set_tooltip_text(Some("Save (Ctrl+S)"));
    let header = gtk::HeaderBar::new();
    header.pack_start(&open_button);
    header.pack_start(&save_button);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Calcium")
        .default_width(900)
        .default_height(620)
        .child(&scroll)
        .build();
    window.set_titlebar(Some(&header));

    let editor = Rc::new(Editor {
        buffer: buffer.clone(),
        window: window.clone(),
        splicing: Cell::new(false),
        queued: Cell::new(false),
        last_answers: RefCell::new(HashMap::new()),
        file: Rc::new(RefCell::new(None)),
    });

    let ed = editor.clone();
    buffer.connect_changed(move |_| {
        if ed.splicing.get() || ed.queued.replace(true) {
            return;
        }
        let ed = ed.clone();
        glib::idle_add_local_once(move || {
            ed.queued.set(false);
            ed.refresh();
        });
    });

    let ed = editor.clone();
    open_button.connect_clicked(move |_| ed.open());
    let ed = editor.clone();
    save_button.connect_clicked(move |_| ed.save());

    let keys = gtk::EventControllerKey::new();
    let ed = editor.clone();
    keys.connect_key_pressed(move |_, key, _, state| ed.key_pressed(key, state));
    view.add_controller(keys);

    buffer.set_text(STARTER);
    window.present();
    view.grab_focus();
}

impl Editor {
    // MARK: The core loop

    fn text(&self) -> String {
        self.buffer
            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), true)
            .to_string()
    }

    fn caret(&self) -> i32 {
        self.buffer
            .iter_at_mark(&self.buffer.get_insert())
            .offset()
    }

    /// Evaluate, splice the answers into the text, restyle. The engine
    /// ignores whatever already follows a `=>`, so the buffer goes over
    /// as-is.
    fn refresh(&self) {
        let text = self.text();
        let answers = doc::evaluate(&text).answers;

        // Work in character offsets, the buffer's own coordinate.
        let lines: Vec<(i32, String)> = line_table(&text);
        let mut edits: Vec<(i32, i32, String)> = Vec::new();

        // Deleting the `=>` takes its stale answer with it — the caret's
        // line only, where an arrow can just have been deleted.
        let caret = self.caret();
        let caret_line = lines
            .iter()
            .position(|(start, body)| {
                caret >= *start && caret <= start + body.chars().count() as i32
            });
        if let Some(index) = caret_line {
            if let Some(stale) = self.last_answers.borrow().get(&index) {
                let (start, body) = &lines[index];
                if !answers.iter().any(|a| a.line == index)
                    && !body.contains("=>")
                    && body.ends_with(stale.as_str())
                {
                    let chars = body.chars().count() as i32;
                    let stale_chars = stale.chars().count() as i32;
                    edits.push((start + chars - stale_chars, start + chars, String::new()));
                }
            }
        }

        for answer in &answers {
            let Some((start, body)) = lines.get(answer.line) else {
                continue;
            };
            let Some(arrow_byte) = body.find("=>") else {
                continue;
            };
            let after_arrow = start + body[..arrow_byte].chars().count() as i32 + 2;
            let line_end = start + body.chars().count() as i32;
            let replacement = if answer.text.is_empty() {
                String::new()
            } else {
                format!(" {}", answer.text)
            };
            let existing: String = body
                .chars()
                .skip((after_arrow - start) as usize)
                .collect();
            if existing != replacement {
                edits.push((after_arrow, line_end, replacement));
            }
        }
        *self.last_answers.borrow_mut() = answers
            .iter()
            .map(|a| {
                let text = if a.text.is_empty() {
                    String::new()
                } else {
                    format!(" {}", a.text)
                };
                (a.line, text)
            })
            .collect();

        let mut caret = self.caret();
        if !edits.is_empty() {
            self.splicing.set(true);
            // Answers are not the author's edits; keep them off the stack.
            self.buffer.begin_irreversible_action();
            edits.sort_by(|a, b| b.0.cmp(&a.0));
            for (from, to, replacement) in &edits {
                let mut start = self.buffer.iter_at_offset(*from);
                let mut end = self.buffer.iter_at_offset(*to);
                self.buffer.delete(&mut start, &mut end);
                let mut at = self.buffer.iter_at_offset(*from);
                self.buffer.insert(&mut at, replacement);
                caret = adjust(caret, *from, *to, replacement.chars().count() as i32);
            }
            self.buffer.end_irreversible_action();
            let total = self.buffer.char_count();
            self.buffer
                .place_cursor(&self.buffer.iter_at_offset(caret.clamp(0, total)));
            self.splicing.set(false);
        }

        self.restyle(&answers);
    }

    /// Tags only, never characters.
    fn restyle(&self, answers: &[doc::Answer]) {
        let text = self.text();
        self.buffer.remove_all_tags(&self.buffer.start_iter(), &self.buffer.end_iter());
        let info = doc::line_info(&text);
        let tokens = doc::tokens(&text);
        let answer_lines: HashMap<usize, bool> =
            answers.iter().map(|a| (a.line, a.is_error)).collect();

        for (index, line) in text.lines().enumerate() {
            let Some(meta) = info.get(index) else { continue };
            let chars = line.chars().count() as i32;
            match meta.kind {
                BlockKind::Heading => {
                    let tag = match meta.heading_level.unwrap_or(1) {
                        0 | 1 => "h1",
                        2 => "h2",
                        _ => "h3",
                    };
                    self.tag_line(tag, index, 0, chars, line);
                }
                BlockKind::Prose => {
                    self.tag_line("prose", index, 0, chars, line);
                    self.style_markdown(index, line);
                }
                BlockKind::Code => {
                    if let Some(spans) = tokens.get(index) {
                        for span in spans {
                            let name = match span.class {
                                TokenClass::Number => "num",
                                TokenClass::Str => "str",
                                TokenClass::Operator => "op",
                                TokenClass::Keyword | TokenClass::Directive => "kw",
                                TokenClass::Function => "fn",
                                TokenClass::Definition => "def",
                                TokenClass::Name => continue,
                            };
                            self.tag_utf16(name, index, span.offset, span.offset + span.length, line);
                        }
                    }
                    if let Some(comment) = meta.comment {
                        self.tag_utf16("comment", index, comment, utf16_len(line), line);
                    }
                    if let Some((at, len)) = meta.redefines {
                        self.tag_utf16("redef", index, at, at + len, line);
                    }
                    if let Some(is_error) = answer_lines.get(&index) {
                        if let Some(arrow) = line.find("=>") {
                            let from = line[..arrow].chars().count() as i32 + 2;
                            self.tag_line(
                                if *is_error { "err" } else { "answer" },
                                index, from, chars, line,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Inline Markdown on a prose line, the Mac rules: marks visible but
    /// stepped back, emphasis not reaching inside code spans.
    fn style_markdown(&self, index: usize, line: &str) {
        use std::sync::OnceLock;
        static CODE: OnceLock<regex::Regex> = OnceLock::new();
        static BOLD: OnceLock<regex::Regex> = OnceLock::new();
        static ITALIC: OnceLock<regex::Regex> = OnceLock::new();
        static LINK: OnceLock<regex::Regex> = OnceLock::new();
        static MARKER: OnceLock<regex::Regex> = OnceLock::new();
        let code = CODE.get_or_init(|| regex::Regex::new(r"`[^`\n]+`").unwrap());
        let bold = BOLD.get_or_init(|| regex::Regex::new(r"\*\*[^*\n]+\*\*").unwrap());
        let italic = ITALIC.get_or_init(|| regex::Regex::new(r"(^|[\s(])(_[^_\n]+_)($|[\s).,;:!?])").unwrap());
        let link = LINK.get_or_init(|| regex::Regex::new(r"\[[^\]\n]+\]\([^)\s]+\)").unwrap());
        let marker = MARKER.get_or_init(|| regex::Regex::new(r"^\s*(?:[-*>]|\d+\.)\s").unwrap());

        let to_chars = |byte: usize| line[..byte].chars().count() as i32;
        if let Some(m) = marker.find(line) {
            self.tag_line("dim", index, to_chars(m.start()), to_chars(m.end()), line);
        }
        let mut code_spans: Vec<(usize, usize)> = Vec::new();
        for m in code.find_iter(line) {
            code_spans.push((m.start(), m.end()));
            self.tag_line("mono", index, to_chars(m.start()), to_chars(m.end()), line);
            self.tag_line("dim", index, to_chars(m.start()), to_chars(m.start()) + 1, line);
            self.tag_line("dim", index, to_chars(m.end()) - 1, to_chars(m.end()), line);
        }
        let outside = |s: usize, e: usize| !code_spans.iter().any(|(cs, ce)| s < *ce && e > *cs);
        for m in bold.find_iter(line) {
            if !outside(m.start(), m.end()) {
                continue;
            }
            self.tag_line("bold", index, to_chars(m.start()), to_chars(m.end()), line);
            self.tag_line("dim", index, to_chars(m.start()), to_chars(m.start()) + 2, line);
            self.tag_line("dim", index, to_chars(m.end()) - 2, to_chars(m.end()), line);
        }
        for c in italic.captures_iter(line) {
            let m = c.get(2).unwrap();
            if !outside(m.start(), m.end()) {
                continue;
            }
            self.tag_line("italic", index, to_chars(m.start()), to_chars(m.end()), line);
            self.tag_line("dim", index, to_chars(m.start()), to_chars(m.start()) + 1, line);
            self.tag_line("dim", index, to_chars(m.end()) - 1, to_chars(m.end()), line);
        }
        for m in link.find_iter(line) {
            if outside(m.start(), m.end()) {
                self.tag_line("link", index, to_chars(m.start()), to_chars(m.end()), line);
            }
        }
    }

    fn tag_line(&self, tag: &str, line: usize, from: i32, to: i32, body: &str) {
        let chars = body.chars().count() as i32;
        let (from, to) = (from.clamp(0, chars), to.clamp(0, chars));
        if from >= to {
            return;
        }
        let Some(start) = self.buffer.iter_at_line_offset(line as i32, from) else { return };
        let Some(end) = self.buffer.iter_at_line_offset(line as i32, to) else { return };
        self.buffer.apply_tag_by_name(tag, &start, &end);
    }

    /// The engine reports UTF-16 offsets — what Apple's text views count.
    /// GTK counts characters; convert against the line's own text.
    fn tag_utf16(&self, tag: &str, line_index: usize, from: usize, to: usize, line: &str) {
        self.tag_line(
            tag,
            line_index,
            utf16_to_chars(line, from),
            utf16_to_chars(line, to),
            line,
        );
    }

    // MARK: Keys

    fn key_pressed(&self, key: gtk::gdk::Key, state: gtk::gdk::ModifierType) -> glib::Propagation {
        let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if ctrl {
            match key {
                gtk::gdk::Key::o => {
                    self.open();
                    return glib::Propagation::Stop;
                }
                gtk::gdk::Key::s => {
                    self.save();
                    return glib::Propagation::Stop;
                }
                gtk::gdk::Key::slash => {
                    self.transform_lines(toggled_comment);
                    return glib::Propagation::Stop;
                }
                gtk::gdk::Key::bracketright => {
                    self.transform_lines(|l| {
                        (!l.is_empty()).then(|| format!("    {l}"))
                    });
                    return glib::Propagation::Stop;
                }
                gtk::gdk::Key::bracketleft => {
                    self.transform_lines(outdented);
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }
        if key == gtk::gdk::Key::Return && !ctrl {
            return self.return_pressed();
        }
        glib::Propagation::Proceed
    }

    /// Return steps over an answer rather than through it, and continues
    /// Markdown list markers — the Mac editor's rules.
    fn return_pressed(&self) -> glib::Propagation {
        let text = self.text();
        let caret = self.caret();
        let lines = line_table(&text);
        let info = doc::line_info(&text);
        let Some(index) = lines
            .iter()
            .position(|(s, b)| caret >= *s && caret <= s + b.chars().count() as i32)
        else {
            return glib::Propagation::Proceed;
        };
        let (start, body) = &lines[index];

        // Over the answer: caret between `=>` and line end moves to the end
        // first, then the newline lands there.
        if let Some(arrow_byte) = body.find("=>") {
            let after_arrow = start + body[..arrow_byte].chars().count() as i32 + 2;
            let line_end = start + body.chars().count() as i32;
            if caret >= after_arrow && caret < line_end {
                self.buffer.place_cursor(&self.buffer.iter_at_offset(line_end));
                self.buffer
                    .insert_at_cursor("\n");
                return glib::Propagation::Stop;
            }
        }

        // List continuation on prose lines.
        if info.get(index).map(|m| m.kind) == Some(BlockKind::Prose) {
            let re = regex::Regex::new(r"^(\s*)([-*>]|\d+\.)( +)").unwrap();
            if let Some(c) = re.captures(body) {
                let marker_chars = body[..c.get(0).unwrap().end()].chars().count() as i32;
                if caret >= start + marker_chars {
                    if c.get(0).unwrap().end() == body.len() {
                        // Empty item: the marker goes, the newline does not.
                        let mut s = self.buffer.iter_at_offset(*start);
                        let mut e = self.buffer.iter_at_offset(start + marker_chars);
                        self.buffer.delete(&mut s, &mut e);
                        return glib::Propagation::Stop;
                    }
                    let bullet = c.get(2).unwrap().as_str();
                    let next = bullet
                        .strip_suffix('.')
                        .and_then(|n| n.parse::<u64>().ok())
                        .map(|n| format!("{}.", n + 1))
                        .unwrap_or_else(|| bullet.to_string());
                    let marker = format!("\n{}{}{}", &c[1], next, &c[3]);
                    self.buffer.insert_at_cursor(&marker);
                    return glib::Propagation::Stop;
                }
            }
        }
        glib::Propagation::Proceed
    }

    /// Applies a per-line rewrite to every line the selection touches.
    fn transform_lines(&self, transform: fn(&str) -> Option<String>) {
        let (from, to) = match self.buffer.selection_bounds() {
            Some((s, e)) => (s.offset(), e.offset()),
            None => (self.caret(), self.caret()),
        };
        let text = self.text();
        let lines = line_table(&text);
        let mut replacement: Vec<String> = Vec::new();
        let mut span: Option<(i32, i32)> = None;
        for (start, body) in &lines {
            let end = start + body.chars().count() as i32;
            if end < from || *start > to {
                continue;
            }
            span = Some(match span {
                None => (*start, end),
                Some((s, _)) => (s, end),
            });
            replacement.push(transform(body).unwrap_or_else(|| body.clone()));
        }
        let Some((span_start, span_end)) = span else { return };
        let replacement = replacement.join("\n");
        let mut s = self.buffer.iter_at_offset(span_start);
        let mut e = self.buffer.iter_at_offset(span_end);
        self.buffer.delete(&mut s, &mut e);
        let mut at = self.buffer.iter_at_offset(span_start);
        self.buffer.insert(&mut at, &replacement);
        self.buffer.select_range(
            &self.buffer.iter_at_offset(span_start),
            &self.buffer.iter_at_offset(span_start + replacement.chars().count() as i32),
        );
    }

    // MARK: Files

    fn open(&self) {
        let dialog = gtk::FileDialog::new();
        let window = self.window.clone();
        let buffer = self.buffer.clone();
        let this_file = self.file.clone();
        dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            let Ok(contents) = std::fs::read_to_string(&path) else { return };
            // The buffer holds the document answer-free; the refresh that
            // set_text triggers writes fresh answers straight back in.
            buffer.set_text(&doc::strip_answers(&contents));
            *this_file.borrow_mut() = Some(path);
        });
    }

    fn save(&self) {
        let existing = self.file.borrow().clone();
        if let Some(path) = existing {
            self.write_to(&path);
            return;
        }
        let dialog = gtk::FileDialog::new();
        dialog.set_initial_name(Some("Untitled.calcium"));
        let window = self.window.clone();
        let text = self.text();
        let this_file = self.file.clone();
        dialog.save(Some(&window), gio::Cancellable::NONE, move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            let _ = std::fs::write(&path, materialized(&text));
            *this_file.borrow_mut() = Some(path);
        });
    }

    fn write_to(&self, path: &std::path::Path) {
        let _ = std::fs::write(path, materialized(&self.text()));
    }
}

/// On disk a document carries its answers, and ends with a newline.
fn materialized(text: &str) -> String {
    let mut on_disk = doc::rewrite(text);
    if !on_disk.is_empty() && !on_disk.ends_with('\n') {
        on_disk.push('\n');
    }
    on_disk
}

/// Each line's starting character offset and body.
fn line_table(text: &str) -> Vec<(i32, String)> {
    let mut lines = Vec::new();
    let mut offset: i32 = 0;
    for line in text.split('\n') {
        lines.push((offset, line.to_string()));
        offset += line.chars().count() as i32 + 1;
    }
    lines
}

/// The caret across one splice: at or before the edit, unchanged — the
/// answer lands after it; inside, clamped — backspace reads as a step left;
/// past it, shifted.
fn adjust(caret: i32, from: i32, to: i32, new_len: i32) -> i32 {
    if caret <= from {
        caret
    } else if caret <= to {
        from + (caret - from).min(new_len)
    } else {
        caret + new_len - (to - from)
    }
}

fn utf16_len(line: &str) -> usize {
    line.encode_utf16().count()
}

/// UTF-16 offset (the engine's coordinate) to character offset (GTK's).
fn utf16_to_chars(line: &str, target: usize) -> i32 {
    let mut units = 0;
    for (count, c) in line.chars().enumerate() {
        if units >= target {
            return count as i32;
        }
        units += c.len_utf16();
    }
    line.chars().count() as i32
}

fn toggled_comment(line: &str) -> Option<String> {
    let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    if indent.is_empty() || indent.chars().count() == line.chars().count() {
        return None;
    }
    let rest = &line[indent.len()..];
    if let Some(stripped) = rest.strip_prefix("# ") {
        Some(format!("{indent}{stripped}"))
    } else if let Some(stripped) = rest.strip_prefix('#') {
        Some(format!("{indent}{stripped}"))
    } else {
        Some(format!("{indent}# {rest}"))
    }
}

fn outdented(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix('\t') {
        return Some(rest.to_string());
    }
    let spaces = line.chars().take_while(|c| *c == ' ').count().min(4);
    (spaces > 0).then(|| line[spaces..].to_string())
}

fn make_tags(buffer: &gtk::TextBuffer) {
    let tag = |name: &str| {
        let t = gtk::TextTag::new(Some(name));
        buffer.tag_table().add(&t);
        t
    };
    // Mid-tone colours that read on both light and dark themes.
    let prose = tag("prose");
    prose.set_foreground(Some("#8b8b90"));
    for (name, scale) in [("h1", 1.6), ("h2", 1.35), ("h3", 1.15)] {
        let t = tag(name);
        t.set_scale(scale);
        t.set_weight(700);
    }
    tag("comment").set_foreground(Some("#6d87a5"));
    tag("answer").set_foreground(Some("#8b8b90"));
    tag("err").set_foreground(Some("#e05252"));
    tag("num").set_foreground(Some("#3f8fe8"));
    tag("str").set_foreground(Some("#b08040"));
    tag("kw").set_foreground(Some("#a86ae0"));
    tag("fn").set_foreground(Some("#e0609f"));
    tag("def").set_foreground(Some("#2aa79a"));
    tag("op").set_foreground(Some("#8b8b90"));
    let redef = tag("redef");
    redef.set_underline(pango::Underline::Error);
    redef.set_underline_rgba(Some(&gtk::gdk::RGBA::new(0.9, 0.54, 0.0, 1.0)));
    tag("bold").set_weight(700);
    tag("italic").set_style(pango::Style::Italic);
    tag("mono").set_family(Some("Fira Code, monospace"));
    tag("dim").set_foreground(Some("#a0a0a4"));
    tag("link").set_foreground(Some("#3f8fe8"));
}

const STARTER: &str = r#"# Calcium

Write math expressions and use `=>` to see the answer.

    1 + 2 =>

Odd units!

    walking speed = 1 mph
    walking speed in furlongs/fortnight
        =>

_Unknown_ units **cancel**!

    burrito length = 1 ft / burrito
    burrito cost = 8 USD / burrito
    1 mile / burrito length * burrito cost in kEUR =>
"#;
