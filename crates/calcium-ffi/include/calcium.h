// C ABI for the Calcium engine.
//
// Every returned pointer is owned by the caller and must be released with
// calcium_string_free. Every function returns NULL on a null argument, on
// invalid UTF-8, or if the engine panics — it never traps.
#ifndef CALCIUM_H
#define CALCIUM_H

#ifdef __cplusplus
extern "C" {
#endif

/// Answers as JSON: [{"line":0,"text":"4","error":false}, ...], line 0-based.
char *calcium_evaluate(const char *source);

/// The document with every `=>` followed by its freshly computed answer.
char *calcium_rewrite(const char *source);

/// The document with the answer after every `=>` removed.
char *calcium_strip_answers(const char *source);

/// The document as Typst markup, answers computed fresh.
char *calcium_typst(const char *source);

/// Sampled plots as JSON, positioned below their 0-based source line:
/// [{"line":3,"x":"t","series":[{"label":"sin(t)","swept":true,
///   "points":[[0,0], ...]}]}, ...]
char *calcium_plots(const char *source);

/// How each line reads: ["heading","code","prose", ...], one per source line.
char *calcium_line_kinds(const char *source);

/// Token spans per line, UTF-16 offsets, empty arrays for prose:
/// [[{"o":4,"l":5,"c":"def"}, ...], [], ...]
char *calcium_tokens(const char *source);

/// Completions at `line` (0-based) matching `prefix`:
/// [{"name":"speed","value":"30 mph","doc":true}, ...]
char *calcium_completions(const char *source, unsigned int line, const char *prefix);

/// Releases a string returned by any of the above. Ignores NULL.
void calcium_string_free(char *text);

#ifdef __cplusplus
}
#endif
#endif
