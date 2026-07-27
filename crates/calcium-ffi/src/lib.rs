//! C ABI for the Calcium engine.
//!
//! Deliberately hand-written rather than generated. The interface is three
//! functions over `String -> String`, so a code generator would add a build
//! step, a version to keep in sync and a layer to debug through, in exchange
//! for marshalling that fits on one screen.
//!
//! Every returned pointer is owned by the caller and must be handed back to
//! [`calcium_string_free`]. Every function tolerates a null input and returns
//! null rather than trapping, because unwinding across the ABI is undefined.

use calcium_core::doc;
use std::ffi::{c_char, CStr, CString};

/// Reads a C string. Returns `None` for null or invalid UTF-8.
///
/// # Safety
/// `source` must be null or a valid, NUL-terminated C string.
unsafe fn borrow(source: *const c_char) -> Option<&'static str> {
    if source.is_null() {
        return None;
    }
    CStr::from_ptr(source).to_str().ok()
}

/// Hands a `String` to the caller as an owned C string.
fn release(text: String) -> *mut c_char {
    match CString::new(text) {
        Ok(owned) => owned.into_raw(),
        // An interior NUL cannot survive a C string; drop it rather than
        // truncating silently at the NUL.
        Err(_) => std::ptr::null_mut(),
    }
}

/// Guards the boundary: a panic must not unwind into Swift.
fn guarded(body: impl FnOnce() -> String + std::panic::UnwindSafe) -> *mut c_char {
    match std::panic::catch_unwind(body) {
        Ok(text) => release(text),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Evaluates a document and returns its answers as JSON:
/// `[{"line":0,"text":"4","error":false}, ...]`, where `line` is 0-based.
///
/// # Safety
/// `source` must be null or a valid, NUL-terminated UTF-8 C string. The result
/// must be freed with [`calcium_string_free`].
#[no_mangle]
pub unsafe extern "C" fn calcium_evaluate(source: *const c_char) -> *mut c_char {
    let Some(source) = borrow(source) else {
        return std::ptr::null_mut();
    };
    guarded(move || {
        let document = doc::evaluate(source);
        let mut json = String::from("[");
        for (i, answer) in document.answers.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str("{\"line\":");
            json.push_str(&answer.line.to_string());
            json.push_str(",\"text\":");
            write_json_string(&mut json, &answer.text);
            json.push_str(",\"error\":");
            json.push_str(if answer.is_error { "true" } else { "false" });
            json.push('}');
        }
        json.push(']');
        json
    })
}

/// Returns the document with every `=>` followed by its freshly computed
/// answer. This is what gets written to disk.
///
/// # Safety
/// As [`calcium_evaluate`].
#[no_mangle]
pub unsafe extern "C" fn calcium_rewrite(source: *const c_char) -> *mut c_char {
    let Some(source) = borrow(source) else {
        return std::ptr::null_mut();
    };
    guarded(move || doc::rewrite(source))
}

/// Returns the document with the answer after every `=>` removed. This is what
/// the editor holds while you type.
///
/// # Safety
/// As [`calcium_evaluate`].
#[no_mangle]
pub unsafe extern "C" fn calcium_strip_answers(source: *const c_char) -> *mut c_char {
    let Some(source) = borrow(source) else {
        return std::ptr::null_mut();
    };
    guarded(move || doc::strip_answers(source))
}

/// How each line reads, as JSON:
/// `[{"kind":"code","comment":12}, {"kind":"prose"}, ...]`, one entry per
/// source line, `comment` being the UTF-16 offset of a trailing `#`.
///
/// An editor needs this to colour prose and comments without re-deriving rules
/// the engine already has — and they have exceptions, so a private copy drifts.
///
/// # Safety
/// As [`calcium_evaluate`].
#[no_mangle]
pub unsafe extern "C" fn calcium_line_kinds(source: *const c_char) -> *mut c_char {
    let Some(source) = borrow(source) else {
        return std::ptr::null_mut();
    };
    guarded(move || {
        let mut json = String::from("[");
        for (i, line) in doc::line_info(source).iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str("{\"kind\":");
            json.push_str(match line.kind {
                doc::BlockKind::Heading => "\"heading\"",
                doc::BlockKind::Code => "\"code\"",
                doc::BlockKind::Prose => "\"prose\"",
            });
            if let Some(level) = line.heading_level {
                json.push_str(",\"level\":");
                json.push_str(&level.to_string());
            }
            if let Some(comment) = line.comment {
                json.push_str(",\"comment\":");
                json.push_str(&comment.to_string());
            }
            if let Some(query) = line.query {
                json.push_str(",\"query\":");
                json.push_str(&query.to_string());
            }
            if let Some((offset, length)) = line.redefines {
                json.push_str(&format!(",\"redefines\":[{offset},{length}]"));
            }
            json.push('}');
        }
        json.push(']');
        json
    })
}

/// Lexical token spans per line, as JSON with compact keys — this is called
/// on every keystroke: `[[{"o":4,"l":5,"c":"def"}, ...], [], ...]`, offsets
/// and lengths in UTF-16, one array per source line, empty for prose.
///
/// # Safety
/// As [`calcium_evaluate`].
#[no_mangle]
pub unsafe extern "C" fn calcium_tokens(source: *const c_char) -> *mut c_char {
    let Some(source) = borrow(source) else {
        return std::ptr::null_mut();
    };
    guarded(move || {
        let mut json = String::from("[");
        for (i, line) in doc::tokens(source).iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push('[');
            for (j, span) in line.iter().enumerate() {
                if j > 0 {
                    json.push(',');
                }
                json.push_str(&format!(
                    "{{\"o\":{},\"l\":{},\"c\":\"{}\"}}",
                    span.offset,
                    span.length,
                    match span.class {
                        doc::TokenClass::Number => "num",
                        doc::TokenClass::Str => "str",
                        doc::TokenClass::Operator => "op",
                        doc::TokenClass::Keyword => "kw",
                        doc::TokenClass::Function => "fn",
                        doc::TokenClass::Definition => "def",
                        doc::TokenClass::Name => "name",
                        doc::TokenClass::Directive => "dir",
                    }
                ));
            }
            json.push(']');
        }
        json.push(']');
        json
    })
}

/// Completions for the caret: names usable at `line` (0-based) matching
/// `prefix`, as JSON `[{"name":"speed","value":"30 mph","doc":true}, ...]` —
/// document definitions first with current values, then prelude names.
/// Capped at 80 entries; a menu shows fewer.
///
/// # Safety
/// As [`calcium_evaluate`], for both string arguments.
#[no_mangle]
pub unsafe extern "C" fn calcium_completions(
    source: *const c_char,
    line: u32,
    prefix: *const c_char,
) -> *mut c_char {
    let Some(source) = borrow(source) else {
        return std::ptr::null_mut();
    };
    let Some(prefix) = borrow(prefix) else {
        return std::ptr::null_mut();
    };
    guarded(move || {
        let mut json = String::from("[");
        for (i, completion) in doc::completions(source, line as usize, prefix)
            .iter()
            .take(80)
            .enumerate()
        {
            if i > 0 {
                json.push(',');
            }
            json.push_str("{\"name\":");
            write_json_string(&mut json, &completion.name);
            json.push_str(",\"value\":");
            write_json_string(&mut json, &completion.value);
            json.push_str(",\"doc\":");
            json.push_str(if completion.from_document { "true" } else { "false" });
            json.push('}');
        }
        json.push(']');
        json
    })
}

/// Allocates `len` bytes for the caller to write into, used by the wasm
/// embedding: JavaScript cannot call `malloc`, so the module exports its own
/// way in. Pair with [`calcium_dealloc`]. The native apps never need this —
/// `withCString` hands memory across directly.
#[no_mangle]
pub extern "C" fn calcium_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::with_capacity(len.max(1));
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Releases a buffer from [`calcium_alloc`].
///
/// # Safety
/// `ptr` must come from `calcium_alloc(len)` with the same `len`, not yet
/// freed.
#[no_mangle]
pub unsafe extern "C" fn calcium_dealloc(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
}

/// Frees a string returned by this library. Ignores null.
///
/// # Safety
/// `text` must be null, or a pointer previously returned by one of the
/// functions above and not yet freed.
#[no_mangle]
pub unsafe extern "C" fn calcium_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(CString::from_raw(text));
    }
}

/// Writes a JSON string literal, escaping what RFC 8259 requires.
fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below 0x20 must be escaped; the rest may go through as
            // UTF-8, which is what the answers are full of (°, Ω, µ, €).
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips a `&str` through the C ABI the way Swift will.
    fn through(f: unsafe extern "C" fn(*const c_char) -> *mut c_char, source: &str) -> String {
        let input = CString::new(source).unwrap();
        unsafe {
            let raw = f(input.as_ptr());
            assert!(!raw.is_null(), "boundary returned null");
            let text = CStr::from_ptr(raw).to_str().unwrap().to_string();
            calcium_string_free(raw);
            text
        }
    }

    #[test]
    fn evaluates_to_json() {
        let json = through(calcium_evaluate, "    2 + 2 =>\n    sqrt(9) =>");
        assert_eq!(
            json,
            "[{\"line\":0,\"text\":\"4\",\"error\":false},\
              {\"line\":1,\"text\":\"3\",\"error\":false}]"
        );
    }

    #[test]
    fn escapes_quotes_and_keeps_unicode() {
        // String results carry quotes; unit answers carry non-ASCII.
        let json = through(calcium_evaluate, "    grade = \"A\" =>\n    45 deg in ° =>");
        assert!(json.contains(r#"\"A\""#), "quotes not escaped: {json}");
        assert!(json.contains('°'), "unicode mangled: {json}");
    }

    #[test]
    fn reports_errors_as_a_flag_not_a_crash() {
        let json = through(calcium_evaluate, "    1 + 2 * =>");
        assert!(json.contains("\"error\":true"), "got {json}");
    }

    #[test]
    fn strips_and_rewrites_are_inverses() {
        let original = "    x = 2\n    x + 3 => 5";
        let stripped = through(calcium_strip_answers, original);
        assert_eq!(stripped, "    x = 2\n    x + 3 =>");
        assert_eq!(through(calcium_rewrite, &stripped), original);
    }

    #[test]
    fn null_input_returns_null_rather_than_trapping() {
        unsafe {
            assert!(calcium_evaluate(std::ptr::null()).is_null());
            assert!(calcium_rewrite(std::ptr::null()).is_null());
            assert!(calcium_strip_answers(std::ptr::null()).is_null());
            calcium_string_free(std::ptr::null_mut()); // must not crash
        }
    }

    #[test]
    fn reports_line_kinds() {
        // `T = 125 degC` also *redefines* the tesla, and the report says so.
        let json = through(calcium_line_kinds, "# Head\nT = 125 degC # note\nA sentence.");
        assert_eq!(
            json,
            "[{\"kind\":\"heading\",\"level\":1},\
              {\"kind\":\"code\",\"comment\":13,\"redefines\":[0,1]},\
              {\"kind\":\"prose\"}]"
        );
    }

    #[test]
    fn empty_document_is_an_empty_array() {
        assert_eq!(through(calcium_evaluate, ""), "[]");
    }

    #[test]
    fn reports_tokens() {
        let json = through(calcium_tokens, "Prose.\n    x = 2 =>");
        assert_eq!(
            json,
            "[[],\
              [{\"o\":4,\"l\":1,\"c\":\"def\"},\
               {\"o\":6,\"l\":1,\"c\":\"op\"},\
               {\"o\":8,\"l\":1,\"c\":\"num\"},\
               {\"o\":10,\"l\":2,\"c\":\"op\"}]]"
        );
    }

    #[test]
    fn reports_completions_with_values() {
        let source = CString::new("    speed = 30 mph\n").unwrap();
        let prefix = CString::new("sp").unwrap();
        let json = unsafe {
            let raw = calcium_completions(source.as_ptr(), 1, prefix.as_ptr());
            assert!(!raw.is_null());
            let text = CStr::from_ptr(raw).to_str().unwrap().to_string();
            calcium_string_free(raw);
            text
        };
        assert!(
            json.contains("{\"name\":\"speed\",\"value\":\"30 mph\",\"doc\":true}"),
            "got {json}"
        );
    }
}
