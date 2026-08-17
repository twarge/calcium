//! Tokenizer.
//!
//! Two things here are unusual enough to call out:
//!
//! * `,` is both a thousands separator *inside* a number (`3,100`) and an
//!   argument separator (`[1, 2, 3]`). We disambiguate positionally.
//! * Identifiers can contain spaces (`mass of earth`). The lexer emits one
//!   `Word` per space-separated run and the *parser* decides how many to glue
//!   together, because only the parser knows which words are keywords.

use crate::num::Num;
use num_bigint::BigInt;

/// How a number was written, so `0xCC => 0xCC` can round-trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Radix {
    Dec,
    Hex,
    Oct,
    Bin,
    /// Decimal, written with a decimal point: the payload is the power of
    /// ten of the last typed digit's place, negated — `2.0` is `Sig(1)`,
    /// `2.` is `Sig(0)`, `1.5e-3` is `Sig(4)`. Formats exactly like `Dec`;
    /// under `@sigfigs` the evaluator reads it as an implied half-ULP
    /// uncertainty.
    Sig(i32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Num(Num, Radix),
    Word(String),
    Str(String),
    /// `@precision`, `@fr-FR`, ... — the whole thing including the `@`.
    Directive(String),

    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Bang,
    Amp,
    AmpAmp,
    Pipe,
    PipePipe,
    PipePipeSuffix,

    Eq,
    EqEq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,

    PlusEq,
    Arrow,
    DotDot,
    /// `±` — a value with an uncertainty.
    PlusMinus,

    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Semi,
    Colon,

    /// `#?` — the AI autocomplete request.
    HashQuestion,
    /// Input the lexer could not make sense of, carrying its complaint.
    ///
    /// Lexing never fails outright. A line holds a calculation *and* its
    /// answer, and the answer is whatever was there last — possibly nonsense
    /// the author is halfway through editing. Failing the line would report
    /// that nonsense as the calculation's error; emitting a token lets the
    /// parser discard everything past the `=>` as it already does.
    Invalid(String),
    Eof,
}

impl Tok {
    /// Whether this token can begin a primary expression. Used to resolve the
    /// `in` ambiguity (conversion keyword vs. the unit "inches").
    pub fn starts_primary(&self) -> bool {
        matches!(
            self,
            Tok::Num(..)
                | Tok::Word(_)
                | Tok::Str(_)
                | Tok::LParen
                | Tok::LBracket
                | Tok::LBrace
                | Tok::Minus
                | Tok::Bang
                | Tok::Pipe
        )
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    /// Byte offsets into the source line, for error underlining.
    pub start: usize,
    pub end: usize,
    /// Whether whitespace separated this token from the previous one. The
    /// parser needs this: `2x` and `2 x` are the same, but `a b` is one
    /// identifier while `a` `(b)` is a call.
    pub space_before: bool,
}

/// Characters that read as part of a bare word. Deliberately generous: unit
/// and currency symbols (`Ω`, `µ`, `$`, `€`) behave exactly like identifiers,
/// since in Calca a unit *is* just a definition.
fn is_word_start(c: char) -> bool {
    c.is_alphabetic()
        || c == '_'
        || matches!(c, '$' | '¥' | '€' | '£' | '₹' | '₽' | '₩' | '¢' | '°' | '∞' | '√' | '∑' | '∏' | '∂' | 'π')
}

fn is_word_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn lex(src: &str) -> Vec<Token> {
    Lexer::new(src).run()
}

/// Where the first `#?` autocomplete request sits, as a UTF-16 offset, or
/// `None` if the line has none.
pub fn query_start(src: &str) -> Option<usize> {
    lex(src)
        .iter()
        .find(|t| t.tok == Tok::HashQuestion)
        .map(|t| src[..t.start].encode_utf16().count())
}

/// Where a trailing `#` comment begins, as a UTF-16 offset into the line, or
/// `None` if there is not one.
///
/// Answered by running the lexer, not by scanning for a `#`: the rule has
/// exceptions — `#?` is an autocomplete request, and a `#` inside a string is
/// just a character — and an editor colouring comments needs the same answer
/// the engine acts on.
pub fn comment_start(src: &str) -> Option<usize> {
    // UTF-16, because that is what a text view counts in.
    Lexer::new(src)
        .run_inner()
        .1
        .map(|byte| src[..byte].encode_utf16().count())
}

struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    pos: usize,
    out: Vec<Token>,
    /// Byte offset of the `#` that ended scanning, if one did.
    comment: Option<usize>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer {
            src,
            chars: src.char_indices().collect(),
            pos: 0,
            out: Vec::new(),
            comment: None,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|(_, c)| *c)
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).map(|(_, c)| *c)
    }

    fn offset(&self) -> usize {
        self.chars
            .get(self.pos)
            .map(|(i, _)| *i)
            .unwrap_or(self.src.len())
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn run(self) -> Vec<Token> {
        self.run_inner().0
    }

    fn run_inner(mut self) -> (Vec<Token>, Option<usize>) {
        let mut space_before = false;
        loop {
            // Skip whitespace, remembering that we saw some.
            while matches!(self.peek(), Some(c) if c.is_whitespace()) {
                self.bump();
                space_before = true;
            }
            let start = self.offset();
            let Some(c) = self.peek() else { break };

            // `#?` is an AI query; any other `#` starts a trailing comment.
            if c == '#' {
                if self.peek_at(1) == Some('?') {
                    self.pos += 2;
                    self.push(Tok::HashQuestion, start, space_before);
                    space_before = false;
                    continue;
                }
                self.comment = Some(start);
                break; // comment runs to end of line
            }

            let tok = if c.is_ascii_digit() || (c == '.' && matches!(self.peek_at(1), Some(d) if d.is_ascii_digit())) {
                self.lex_number()
            } else if c == '"' {
                self.lex_string()
            } else if is_word_start(c) {
                self.lex_word()
            } else {
                self.lex_punct()
            };

            self.push(tok, start, space_before);
            space_before = false;
        }
        let end = self.src.len();
        self.out.push(Token {
            tok: Tok::Eof,
            start: end,
            end,
            space_before,
        });
        (self.out, self.comment)
    }

    fn push(&mut self, tok: Tok, start: usize, space_before: bool) {
        let end = self.offset();
        self.out.push(Token {
            tok,
            start,
            end,
            space_before,
        });
    }

    fn lex_word(&mut self) -> Tok {
        let start = self.pos;
        // Standalone symbols (currency, degree, operators-as-words) are each a
        // word of their own rather than merging with following letters, so
        // `$350` and `60°` split correctly.
        let c = self.peek().unwrap();
        if !(c.is_alphabetic() || c == '_') {
            self.bump();
            return Tok::Word(self.slice(start));
        }
        while matches!(self.peek(), Some(c) if is_word_continue(c)) {
            self.bump();
        }
        Tok::Word(self.slice(start))
    }

    fn slice(&self, from_index: usize) -> String {
        let start = self.chars[from_index].0;
        let end = self.offset();
        self.src[start..end].to_string()
    }

    fn lex_string(&mut self) -> Tok {
        self.bump(); // opening quote
        let mut value = String::new();
        loop {
            match self.bump() {
                None => return Tok::Invalid("unterminated string".to_string()),
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some(c) => value.push(c),
                    None => {}
                },
                Some(c) => value.push(c),
            }
        }
        Tok::Str(value)
    }

    fn lex_number(&mut self) -> Tok {

        // Radix-prefixed integers: 0xFF, 0b1010, 0o777. Underscores allowed.
        if self.peek() == Some('0') {
            if let Some(marker) = self.peek_at(1) {
                let radix = match marker {
                    'x' | 'X' => Some((16u32, Radix::Hex)),
                    'b' | 'B' => Some((2, Radix::Bin)),
                    'o' | 'O' => Some((8, Radix::Oct)),
                    _ => None,
                };
                if let Some((base, style)) = radix {
                    self.pos += 2;
                    let mut digits = String::new();
                    while let Some(c) = self.peek() {
                        if c == '_' {
                            self.bump();
                        } else if c.is_digit(base) {
                            digits.push(c);
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    if digits.is_empty() {
                        return Tok::Invalid(format!("expected digits after 0{marker}"));
                    }
                    let Some(v) = BigInt::parse_bytes(digits.as_bytes(), base) else {
                        return Tok::Invalid("invalid number".to_string());
                    };
                    return Tok::Num(Num::from_bigint(v), style);
                }
            }
        }

        let mut mantissa = String::new();
        self.take_grouped_digits(&mut mantissa);

        let mut scale = 0i64; // digits after the decimal point
        let mut has_point = false;
        if self.peek() == Some('.') && matches!(self.peek_at(1), Some(d) if d.is_ascii_digit()) {
            has_point = true;
            self.bump();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    mantissa.push(c);
                    scale += 1;
                    self.bump();
                } else if c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
        } else if self.peek() == Some('.') && self.peek_at(1) != Some('.') {
            // A trailing point — `2.` — marks the ones place as the last
            // significant digit. Two points would be a range.
            has_point = true;
            self.bump();
        }

        let mut exponent = 0i64;
        if matches!(self.peek(), Some('e') | Some('E')) {
            // Only an exponent if digits (or a sign then digits) follow;
            // otherwise this `e` belongs to a following identifier, as in
            // `2e` meaning `2 * e` (Euler's number).
            let mut look = 1;
            if matches!(self.peek_at(look), Some('+') | Some('-')) {
                look += 1;
            }
            if matches!(self.peek_at(look), Some(d) if d.is_ascii_digit()) {
                self.bump();
                let negative = match self.peek() {
                    Some('+') => {
                        self.bump();
                        false
                    }
                    Some('-') => {
                        self.bump();
                        true
                    }
                    _ => false,
                };
                let mut digits = String::new();
                while matches!(self.peek(), Some(d) if d.is_ascii_digit()) {
                    digits.push(self.bump().unwrap());
                }
                exponent = digits.parse::<i64>().unwrap_or(0);
                if negative {
                    exponent = -exponent;
                }
            }
        }

        let Some(integer) = BigInt::parse_bytes(mantissa.as_bytes(), 10) else {
            return Tok::Invalid("invalid number".to_string());
        };
        let mut value = Num::from_bigint(integer);
        let power = exponent - scale;
        if power != 0 {
            let ten = Num::from_i64(10);
            value = value.mul(&ten.pow(&Num::from_i64(power)));
        }
        let style = if has_point {
            Radix::Sig((scale - exponent).clamp(i32::MIN as i64, i32::MAX as i64) as i32)
        } else {
            Radix::Dec
        };
        Tok::Num(value, style)
    }

    /// Consumes digits, absorbing `,` only where it is unambiguously a
    /// thousands separator: immediately followed by exactly three digits.
    /// `3,100` is one number; `[1,2,3]` and `f(x, 500)` are not.
    fn take_grouped_digits(&mut self, out: &mut String) {
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                out.push(c);
                self.bump();
            } else if c == '_' {
                self.bump();
            } else if c == ',' {
                let three_digits = (1..=3).all(|n| matches!(self.peek_at(n), Some(d) if d.is_ascii_digit()));
                let then_not_digit = !matches!(self.peek_at(4), Some(d) if d.is_ascii_digit());
                if three_digits && then_not_digit {
                    self.bump(); // the comma
                    for _ in 0..3 {
                        out.push(self.bump().unwrap());
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    fn lex_punct(&mut self) -> Tok {
        let c = self.bump().unwrap();
        let tok = match c {
            '+' => {
                if self.peek() == Some('=') {
                    self.bump();
                    Tok::PlusEq
                } else {
                    Tok::Plus
                }
            }
            '-' => Tok::Minus,
            '*' => {
                if self.peek() == Some('*') {
                    self.bump();
                    Tok::Caret
                } else {
                    Tok::Star
                }
            }
            '×' | '⋅' | '·' => Tok::Star,
            '/' => Tok::Slash,
            '÷' => Tok::Slash,
            '^' => Tok::Caret,
            // `%` is not modulo (that spells `mod`); it is the percent *unit*,
            // defined as 1/100 in the prelude. That makes `80% * 2000` and
            // `21/45 in %` fall out of ordinary unit handling.
            '%' => Tok::Word("%".to_string()),
            '@' => {
                let text_start = self.pos - 1;
                while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_' || c == '-') {
                    self.bump();
                }
                Tok::Directive(self.slice(text_start))
            }
            '=' => {
                if self.peek() == Some('>') {
                    self.bump();
                    Tok::Arrow
                } else if self.peek() == Some('=') {
                    self.bump();
                    Tok::EqEq
                } else {
                    Tok::Eq
                }
            }
            '⇒' => Tok::Arrow,
            '!' => {
                if self.peek() == Some('=') {
                    self.bump();
                    Tok::NotEq
                } else {
                    Tok::Bang
                }
            }
            '¬' => Tok::Bang,
            '≠' => Tok::NotEq,
            '<' => {
                if self.peek() == Some('=') {
                    self.bump();
                    Tok::LtEq
                } else {
                    Tok::Lt
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.bump();
                    Tok::GtEq
                } else {
                    Tok::Gt
                }
            }
            '≤' => Tok::LtEq,
            '≥' => Tok::GtEq,
            '&' => {
                if self.peek() == Some('&') {
                    self.bump();
                    Tok::AmpAmp
                } else {
                    Tok::Amp
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.bump();
                    Tok::PipePipe
                } else {
                    Tok::Pipe
                }
            }
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            '±' => Tok::PlusMinus,
            ',' => Tok::Comma,
            ';' => Tok::Semi,
            ':' => Tok::Colon,
            '.' => {
                if self.peek() == Some('.') {
                    self.bump();
                    if self.peek() == Some('.') {
                        self.bump();
                    }
                    Tok::DotDot
                } else {
                    return Tok::Invalid("unexpected '.'".to_string());
                }
            }
            other => return Tok::Invalid(format!("unexpected character '{other}'")),
        };
        tok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src)
            .into_iter()
            .map(|t| t.tok)
            .filter(|t| *t != Tok::Eof)
            .collect()
    }

    fn nums(src: &str) -> Vec<String> {
        toks(src)
            .into_iter()
            .filter_map(|t| match t {
                Tok::Num(n, _) => Some(n.to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reads_grouped_numbers_but_not_list_commas() {
        assert_eq!(nums("3,100"), vec!["3,100"]);
        assert_eq!(nums("$829,475"), vec!["829,475"]);
        assert_eq!(nums("1,000,000"), vec!["1,000,000"]);
        // A comma followed by a space, or by the wrong digit count, separates.
        assert_eq!(nums("[1, 2, 3]"), vec!["1", "2", "3"]);
        assert_eq!(nums("[1,2,3]"), vec!["1", "2", "3"]);
        assert_eq!(nums("f(x, 500)"), vec!["500"]);
    }

    #[test]
    fn reads_decimals_and_exponents_exactly() {
        assert_eq!(nums("3.14159"), vec!["3.1416"]); // display rounds; value is exact
        assert_eq!(nums("3.14e3"), vec!["3,140"]);
        assert_eq!(nums("3.14e-3"), vec!["0.0031"]);
        assert_eq!(nums("5.972e24"), vec!["5.972e24"]);
        assert_eq!(nums("6.67384e-11"), vec!["6.6738e-11"]);
    }

    #[test]
    fn a_trailing_e_is_eulers_number_not_an_exponent() {
        // `2e` must lex as 2 then the identifier `e`.
        assert_eq!(toks("2e"), vec![Tok::Num(Num::from_i64(2), Radix::Dec), Tok::Word("e".into())]);
    }

    #[test]
    fn reads_radix_literals() {
        assert!(matches!(toks("0xFFFF_FFFF")[0], Tok::Num(_, Radix::Hex)));
        assert!(matches!(toks("0b1111_1111")[0], Tok::Num(_, Radix::Bin)));
        assert_eq!(nums("0xFF"), vec!["255"]);
        assert_eq!(nums("0b101"), vec!["5"]);
        assert_eq!(nums("0o310"), vec!["200"]);
    }

    #[test]
    fn splits_words_for_the_parser_to_rejoin() {
        assert_eq!(
            toks("mass of earth"),
            vec![
                Tok::Word("mass".into()),
                Tok::Word("of".into()),
                Tok::Word("earth".into())
            ]
        );
    }

    #[test]
    fn symbols_are_their_own_words() {
        // `$350` must not lex as one identifier, and `60°` must split.
        assert_eq!(
            toks("$350"),
            vec![Tok::Word("$".into()), Tok::Num(Num::from_i64(350), Radix::Dec)]
        );
        assert_eq!(
            toks("60°"),
            vec![Tok::Num(Num::from_i64(60), Radix::Dec), Tok::Word("°".into())]
        );
    }

    #[test]
    fn records_whitespace_so_the_parser_can_tell_calls_from_products() {
        let ts = lex("f(x)");
        assert!(!ts[1].space_before);
        let ts = lex("2 (3 + x)");
        assert!(ts[1].space_before);
    }

    #[test]
    fn handles_unicode_operators() {
        assert_eq!(toks("2 × 3"), toks("2 * 3"));
        assert_eq!(toks("2 ÷ 3"), toks("2 / 3"));
        assert_eq!(toks("3 ≤ 3"), toks("3 <= 3"));
        assert_eq!(toks("3 ≠ 3"), toks("3 != 3"));
        assert_eq!(toks("¬true"), toks("!true"));
        assert_eq!(toks("2 ** 3"), toks("2 ^ 3"));
    }

    #[test]
    fn finds_where_a_comment_starts() {
        assert_eq!(comment_start("I = 3/2 # nuclear spin"), Some(8));
        assert_eq!(comment_start("I = 3/2"), None);
        // `#?` is an autocomplete request, not a comment.
        assert_eq!(comment_start("mass = #?"), None);
        // A `#` inside a string is just a character.
        assert_eq!(comment_start("label = \"a # b\""), None);
        assert_eq!(comment_start("label = \"a # b\" # real"), Some(16));
        // Counted in UTF-16, which is what a text view uses.
        assert_eq!(comment_start("γ = 1 # note"), Some(6));
    }

    #[test]
    fn strips_trailing_comments_but_keeps_ai_queries() {
        assert_eq!(nums("5.972e24 kg #googled"), vec!["5.972e24"]);
        assert_eq!(toks("x = #?").last().unwrap(), &Tok::HashQuestion);
    }

    #[test]
    fn reads_ranges_and_arrows() {
        assert_eq!(
            toks("0..3"),
            vec![
                Tok::Num(Num::zero(), Radix::Dec),
                Tok::DotDot,
                Tok::Num(Num::from_i64(3), Radix::Dec)
            ]
        );
        assert!(toks("x => 4").contains(&Tok::Arrow));
        assert!(toks("expenses +=").contains(&Tok::PlusEq));
    }

    #[test]
    fn reads_strings() {
        assert_eq!(toks("\"Overweight\""), vec![Tok::Str("Overweight".into())]);
    }
}
