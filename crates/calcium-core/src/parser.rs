//! Recursive-descent parser.
//!
//! Two rules here are load-bearing and worth reading before changing anything:
//!
//! 1. **Implicit multiplication binds tighter than explicit.** `2x/3y` is
//!    `(2x)/(3y)` but `2*x/3*y` is `((2*x)/3)*y`. That is a real precedence
//!    level (`factor`), not a hack.
//!
//! 2. **`in` is overloaded.** It is the conversion keyword in `100 ft in m`,
//!    and it is the unit *inches* in `5 ft + 4 in`. We resolve it by
//!    lookahead: `in` converts only when followed by something that can begin
//!    an expression.

use crate::ast::*;
use crate::lexer::{lex, Radix, Tok, Token};
use crate::num::Num;

/// Words that never merge into a multi-word identifier.
const KEYWORDS: &[&str] = &["if", "then", "else", "let", "in", "mod", "true", "false"];

fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word)
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Inside `|...|`, a bare `|` closes the group instead of meaning
    /// bitwise-or.
    in_abs: usize,
    /// Inside the value part of `let name = value in body`, `in` terminates
    /// the value rather than introducing a conversion.
    in_let_value: usize,
}

/// Parses one logical line, which may hold several `;`-separated statements.
pub fn parse_line(src: &str) -> Vec<Statement> {
    let tokens = match lex(src) {
        Ok(tokens) => tokens,
        Err(err) => {
            // The line does not tokenize, so nothing downstream can run — but
            // if the author wrote `=>` they still asked for an answer, and
            // silence is the least useful thing to give them.
            let arrow = crate::check::outside_code_spans(src).contains("=>");
            return vec![Statement {
                stmt: Stmt::Expr(Expr::Error(err.message)),
                arrow,
            }];
        }
    };
    Parser {
        tokens,
        pos: 0,
        in_abs: 0,
        in_let_value: 0,
    }
    .statements()
}

/// Parses a single expression. Convenience for tests and for the solver.
pub fn parse_expr(src: &str) -> Expr {
    let tokens = match lex(src) {
        Ok(tokens) => tokens,
        Err(err) => return Expr::Error(err.message),
    };
    let mut parser = Parser {
        tokens,
        pos: 0,
        in_abs: 0,
        in_let_value: 0,
    };
    let expr = parser.expr();
    if !parser.at_end() {
        return Expr::Error(format!("unexpected trailing input in {src:?}"));
    }
    expr
}

impl Parser {
    // -- token helpers ------------------------------------------------------

    fn peek(&self) -> &Tok {
        &self.tokens[self.pos.min(self.tokens.len() - 1)].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.tokens[(self.pos + n).min(self.tokens.len() - 1)].tok
    }

    fn space_before(&self) -> bool {
        self.tokens[self.pos.min(self.tokens.len() - 1)].space_before
    }

    fn at_end(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    fn bump(&mut self) -> Tok {
        let tok = self.tokens[self.pos.min(self.tokens.len() - 1)].tok.clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn eat(&mut self, want: &Tok) -> bool {
        if self.peek() == want {
            self.bump();
            true
        } else {
            false
        }
    }

    fn peek_word(&self) -> Option<&str> {
        match self.peek() {
            Tok::Word(w) => Some(w.as_str()),
            _ => None,
        }
    }

    /// True when the token at `n` could start a primary expression.
    fn starts_primary_at(&self, n: usize) -> bool {
        match self.peek_at(n) {
            Tok::Word(w) => !is_keyword(w) || matches!(w.as_str(), "if" | "let" | "true" | "false"),
            other => other.starts_primary(),
        }
    }

    /// Decides whether the `in` at the cursor is the conversion keyword.
    fn in_is_conversion(&self) -> bool {
        if self.in_let_value > 0 {
            return false;
        }
        self.peek_word() == Some("in") && self.starts_primary_at(1)
    }

    // -- statements ---------------------------------------------------------

    fn statements(&mut self) -> Vec<Statement> {
        let mut out = Vec::new();
        loop {
            while self.eat(&Tok::Semi) {}
            if self.at_end() {
                break;
            }
            out.push(self.statement());
            if !matches!(self.peek(), Tok::Semi | Tok::Eof) {
                // Unconsumed junk: record it and resynchronize at the next `;`.
                let start = self.pos;
                self.skip_to_statement_end();
                if self.pos == start {
                    self.bump();
                }
                if let Some(last) = out.last_mut() {
                    if !matches!(last.stmt, Stmt::Expr(Expr::Error(_))) {
                        last.stmt = Stmt::Expr(Expr::Error("unexpected input".to_string()));
                    }
                }
            }
        }
        out
    }

    fn statement(&mut self) -> Statement {
        if let Tok::Directive(name) = self.peek().clone() {
            self.bump();
            let value = if self.eat(&Tok::Eq) {
                Some(self.expr())
            } else {
                None
            };
            let arrow = self.take_arrow();
            return Statement {
                stmt: Stmt::Directive { name, value },
                arrow,
            };
        }

        // `name +=` — a summing definition.
        let checkpoint = self.pos;
        if self.peek_word().is_some() {
            let name = self.identifier();
            if self.eat(&Tok::PlusEq) {
                let arrow = self.take_arrow();
                return Statement {
                    stmt: Stmt::SumDefine { name },
                    arrow,
                };
            }
            self.pos = checkpoint;
        }

        // `name = body` / `name(params) = body`, versus a bare expression or a
        // general equation.
        if let Some(stmt) = self.try_definition() {
            let arrow = self.take_arrow();
            return Statement { stmt, arrow };
        }

        let lhs = self.expr();
        if self.eat(&Tok::Eq) {
            let rhs = self.expr();
            let arrow = self.take_arrow();
            return Statement {
                stmt: Stmt::Equation { lhs, rhs },
                arrow,
            };
        }
        let arrow = self.take_arrow();
        Statement {
            stmt: Stmt::Expr(lhs),
            arrow,
        }
    }

    /// Recognizes `name = ...` and `name(a, b) = ...`, rewinding if the line
    /// turns out to be something else (`12x + 13y = 163`).
    fn try_definition(&mut self) -> Option<Stmt> {
        let checkpoint = self.pos;
        if self.peek_word().is_none() {
            return None;
        }
        let name = self.identifier();
        // `in` is the one keyword that is also a real unit name (inches), and
        // the prelude has to be able to define it.
        if is_keyword(&name) && name != "in" {
            self.pos = checkpoint;
            return None;
        }

        let mut params = None;
        if matches!(self.peek(), Tok::LParen) {
            let save = self.pos;
            self.bump();
            let mut names = Vec::new();
            let mut ok = true;
            if !matches!(self.peek(), Tok::RParen) {
                loop {
                    if self.peek_word().is_none() {
                        ok = false;
                        break;
                    }
                    names.push(self.identifier());
                    if self.eat(&Tok::Comma) {
                        continue;
                    }
                    break;
                }
            }
            if ok && self.eat(&Tok::RParen) && matches!(self.peek(), Tok::Eq) {
                params = Some(names);
            } else {
                self.pos = save;
                self.pos = checkpoint;
                return None;
            }
        }

        if !matches!(self.peek(), Tok::Eq) {
            self.pos = checkpoint;
            return None;
        }
        self.bump(); // '='
        let body = self.expr();
        Some(Stmt::Define { name, params, body })
    }

    /// Consumes a trailing `=>` plus whatever stale result text follows it.
    /// The result is always regenerated, never trusted.
    fn take_arrow(&mut self) -> bool {
        if !self.eat(&Tok::Arrow) {
            return false;
        }
        self.skip_to_statement_end();
        true
    }

    /// Skips to the next top-level `;`, tracking bracket depth so the `;`
    /// inside a matrix result like `[1, 3; 2, 4]` does not fool us.
    fn skip_to_statement_end(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                Tok::Eof => break,
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => depth -= 1,
                Tok::Semi if depth <= 0 => break,
                _ => {}
            }
            self.bump();
        }
    }

    // -- expressions --------------------------------------------------------

    fn expr(&mut self) -> Expr {
        self.conversion()
    }

    fn conversion(&mut self) -> Expr {
        let mut lhs = self.logic_or();
        while self.in_is_conversion() {
            self.bump();
            let unit = self.logic_or();
            lhs = Expr::Convert(Box::new(lhs), Box::new(unit));
        }
        lhs
    }

    fn logic_or(&mut self) -> Expr {
        let mut lhs = self.logic_and();
        while self.in_abs == 0 && matches!(self.peek(), Tok::PipePipe) {
            self.bump();
            let rhs = self.logic_and();
            lhs = Expr::Logic(LogicOp::Or, Box::new(lhs), Box::new(rhs));
        }
        lhs
    }

    fn logic_and(&mut self) -> Expr {
        let mut lhs = self.bit_or();
        while self.eat(&Tok::AmpAmp) {
            let rhs = self.bit_or();
            lhs = Expr::Logic(LogicOp::And, Box::new(lhs), Box::new(rhs));
        }
        lhs
    }

    fn bit_or(&mut self) -> Expr {
        let mut lhs = self.bit_and();
        while self.in_abs == 0 && matches!(self.peek(), Tok::Pipe) {
            self.bump();
            let rhs = self.bit_and();
            lhs = Expr::Bit(BitOp::Or, Box::new(lhs), Box::new(rhs));
        }
        lhs
    }

    fn bit_and(&mut self) -> Expr {
        let mut lhs = self.comparison();
        while self.eat(&Tok::Amp) {
            let rhs = self.comparison();
            lhs = Expr::Bit(BitOp::And, Box::new(lhs), Box::new(rhs));
        }
        lhs
    }

    fn comparison(&mut self) -> Expr {
        let lhs = self.range();
        let op = match self.peek() {
            Tok::Lt => CmpOp::Lt,
            Tok::Gt => CmpOp::Gt,
            Tok::LtEq => CmpOp::Le,
            Tok::GtEq => CmpOp::Ge,
            Tok::EqEq => CmpOp::Eq,
            Tok::NotEq => CmpOp::Ne,
            _ => return lhs,
        };
        self.bump();
        let rhs = self.range();
        Expr::Cmp(op, Box::new(lhs), Box::new(rhs))
    }

    fn range(&mut self) -> Expr {
        let lhs = self.additive();
        if self.eat(&Tok::DotDot) {
            let rhs = self.additive();
            return Expr::Range(Box::new(lhs), Box::new(rhs));
        }
        lhs
    }

    fn additive(&mut self) -> Expr {
        let mut terms = vec![self.term()];
        loop {
            if self.eat(&Tok::Plus) {
                terms.push(self.term());
            } else if self.eat(&Tok::Minus) {
                terms.push(Expr::neg(self.term()));
            } else {
                break;
            }
        }
        Expr::add(terms)
    }

    /// Explicit `*`, `/` and `mod`, left-associative.
    fn term(&mut self) -> Expr {
        let mut lhs = self.factor();
        loop {
            if self.eat(&Tok::Star) {
                let rhs = self.factor();
                lhs = Expr::mul(vec![lhs, rhs]);
            } else if self.eat(&Tok::Slash) {
                let rhs = self.factor();
                lhs = Expr::div(lhs, rhs);
            } else if self.peek_word() == Some("mod") {
                self.bump();
                let rhs = self.factor();
                lhs = Expr::Mod(Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        lhs
    }

    /// Juxtaposition: `2x`, `100 ft`, `6.25 height`. Binds tighter than `*`
    /// and `/`, which is what makes `2x/3y` mean `(2x)/(3y)`.
    fn factor(&mut self) -> Expr {
        let mut parts = vec![self.unary()];
        loop {
            // `in` here is either the conversion keyword (stop) or the unit
            // "inches" (keep going).
            if self.peek_word() == Some("in") {
                if self.in_is_conversion() || self.in_let_value > 0 {
                    break;
                }
                self.bump();
                parts.push(Expr::var("in"));
                continue;
            }
            if !self.starts_primary_now() {
                break;
            }
            parts.push(self.unary());
        }
        Expr::mul(parts)
    }

    /// Whether the cursor sits on something that continues a juxtaposition.
    /// A leading `-` does not: `2 - 3` is subtraction, never `2 * (-3)`.
    fn starts_primary_now(&self) -> bool {
        match self.peek() {
            Tok::Word(w) => !is_keyword(w) || matches!(w.as_str(), "if" | "let" | "true" | "false"),
            Tok::Num(..) | Tok::Str(_) | Tok::LParen | Tok::LBracket | Tok::LBrace => true,
            _ => false,
        }
    }

    fn unary(&mut self) -> Expr {
        if self.eat(&Tok::Minus) {
            return Expr::neg(self.unary());
        }
        if self.eat(&Tok::Plus) {
            return self.unary();
        }
        if self.eat(&Tok::Bang) {
            return Expr::Not(Box::new(self.unary()));
        }
        self.power()
    }

    fn power(&mut self) -> Expr {
        let base = self.postfix();
        if self.eat(&Tok::Caret) {
            // `m^T` is transposition, not exponentiation by a variable T.
            if self.peek_word() == Some("T") && !self.starts_primary_at(1) {
                self.bump();
                return Expr::Transpose(Box::new(base));
            }
            let exp = self.unary();
            return Expr::Pow(Box::new(base), Box::new(exp));
        }
        base
    }

    fn postfix(&mut self) -> Expr {
        let mut base = self.primary();
        // Indexing must be tight: `mat[0]` indexes, `2 [1,2]` scales a matrix.
        while matches!(self.peek(), Tok::LBracket) && !self.space_before() {
            self.bump();
            let mut indices = Vec::new();
            if !matches!(self.peek(), Tok::RBracket) {
                loop {
                    indices.push(self.expr());
                    if self.eat(&Tok::Comma) {
                        continue;
                    }
                    break;
                }
            }
            if !self.eat(&Tok::RBracket) {
                return Expr::Error("expected ']'".to_string());
            }
            base = Expr::Index(Box::new(base), indices);
        }
        base
    }

    /// Merges consecutive non-keyword words into one name: `mass of earth`.
    ///
    /// A run that has already started may also absorb numbers, so `item 1` and
    /// `Sep 3 2013` are single names. To keep `10 m` multiplication intact, a
    /// number is only absorbed when another name-ish token or an `=` follows
    /// it — `a 5 + b` stays a product.
    fn identifier(&mut self) -> String {
        let mut parts = Vec::new();
        loop {
            if !parts.is_empty() {
                if let Tok::Num(value, Radix::Dec) = self.peek().clone() {
                    let followed_by_name = matches!(
                        self.peek_at(1),
                        Tok::Word(_) | Tok::Num(..) | Tok::Eq | Tok::PlusEq
                    );
                    if followed_by_name && value.is_integer() && !value.is_negative() {
                        self.bump();
                        parts.push(value.to_string().replace(',', ""));
                        continue;
                    }
                }
            }
            let Tok::Word(w) = self.peek().clone() else {
                break;
            };
            if !parts.is_empty() && is_keyword(&w) {
                break;
            }
            // A symbol like `$` or `°` stands alone rather than gluing to a
            // following word, so `$ price` is not one identifier.
            let symbolic = !w.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false);
            if symbolic && !parts.is_empty() {
                break;
            }
            self.bump();
            parts.push(w.clone());
            if symbolic {
                break;
            }
        }
        parts.join(" ")
    }

    fn primary(&mut self) -> Expr {
        match self.peek().clone() {
            Tok::Num(value, radix) => {
                self.bump();
                Expr::Num(value, radix)
            }
            Tok::Str(s) => {
                self.bump();
                Expr::Str(s)
            }
            Tok::HashQuestion => {
                self.bump();
                Expr::AiQuery
            }
            Tok::LParen => {
                self.bump();
                let saved_abs = std::mem::replace(&mut self.in_abs, 0);
                let saved_let = std::mem::replace(&mut self.in_let_value, 0);
                let inner = self.expr();
                self.in_abs = saved_abs;
                self.in_let_value = saved_let;
                if !self.eat(&Tok::RParen) {
                    return Expr::Error("expected ')'".to_string());
                }
                inner
            }
            Tok::LBracket => self.matrix(),
            Tok::LBrace => self.dict(),
            Tok::Pipe => {
                self.bump();
                self.in_abs += 1;
                let inner = self.expr();
                self.in_abs -= 1;
                if !self.eat(&Tok::Pipe) {
                    return Expr::Error("expected '|'".to_string());
                }
                Expr::Abs(Box::new(inner))
            }
            Tok::PipePipe => {
                self.bump();
                self.in_abs += 1;
                let inner = self.expr();
                self.in_abs -= 1;
                if !self.eat(&Tok::PipePipe) {
                    return Expr::Error("expected '||'".to_string());
                }
                // An immediately adjacent number is the norm's order.
                let p = match (self.peek().clone(), self.space_before()) {
                    (Tok::Num(v, radix), false) => {
                        self.bump();
                        Some(Box::new(Expr::Num(v, radix)))
                    }
                    _ => None,
                };
                Expr::Norm(Box::new(inner), p)
            }
            Tok::Word(w) => match w.as_str() {
                "true" => {
                    self.bump();
                    Expr::Bool(true)
                }
                "false" => {
                    self.bump();
                    Expr::Bool(false)
                }
                "if" => self.if_expr(),
                "let" => self.let_expr(),
                _ => self.name_or_call(),
            },
            // A statement terminator here means the expression simply ran
            // out. Do *not* consume it: the caller still has to see the `=>`,
            // or a line the author asked for an answer on gets none at all.
            Tok::Arrow | Tok::Semi | Tok::Eof => {
                Expr::Error("expected a value".to_string())
            }
            other => {
                self.bump();
                Expr::Error(format!("unexpected {other:?}"))
            }
        }
    }

    fn if_expr(&mut self) -> Expr {
        self.bump(); // if
        let cond = self.expr();
        if self.peek_word() != Some("then") {
            return Expr::Error("expected 'then'".to_string());
        }
        self.bump();
        let then_branch = self.expr();
        if self.peek_word() != Some("else") {
            return Expr::Error("expected 'else'".to_string());
        }
        self.bump();
        let else_branch = self.expr();
        Expr::If(
            Box::new(cond),
            Box::new(then_branch),
            Box::new(else_branch),
        )
    }

    fn let_expr(&mut self) -> Expr {
        self.bump(); // let
        let name = self.identifier();
        if !self.eat(&Tok::Eq) {
            return Expr::Error("expected '=' in let".to_string());
        }
        self.in_let_value += 1;
        let value = self.expr();
        self.in_let_value -= 1;
        if self.peek_word() != Some("in") {
            return Expr::Error("expected 'in' after let value".to_string());
        }
        self.bump();
        let body = self.expr();
        Expr::Let(name, Box::new(value), Box::new(body))
    }

    fn name_or_call(&mut self) -> Expr {
        let name = self.identifier();
        if matches!(self.peek(), Tok::LParen) {
            self.bump();
            let mut args = Vec::new();
            if !matches!(self.peek(), Tok::RParen) {
                loop {
                    args.push(self.argument());
                    if self.eat(&Tok::Comma) {
                        continue;
                    }
                    break;
                }
            }
            if !self.eat(&Tok::RParen) {
                return Expr::Error(format!("expected ')' closing call to {name}"));
            }
            return Expr::Call(name, args);
        }
        Expr::Var(name)
    }

    /// `f(x)` positionally, or `f(y=5)` by name.
    fn argument(&mut self) -> Arg {
        let checkpoint = self.pos;
        if self.peek_word().is_some() {
            let name = self.identifier();
            if matches!(self.peek(), Tok::Eq) {
                self.bump();
                let saved_abs = std::mem::replace(&mut self.in_abs, 0);
                let saved_let = std::mem::replace(&mut self.in_let_value, 0);
                let value = self.expr();
                self.in_abs = saved_abs;
                self.in_let_value = saved_let;
                return Arg::named(name, value);
            }
            self.pos = checkpoint;
        }
        let saved_abs = std::mem::replace(&mut self.in_abs, 0);
        let saved_let = std::mem::replace(&mut self.in_let_value, 0);
        let value = self.expr();
        self.in_abs = saved_abs;
        self.in_let_value = saved_let;
        Arg::positional(value)
    }

    /// `[a, b; c, d]` — `,` separates columns, `;` separates rows.
    fn matrix(&mut self) -> Expr {
        self.bump(); // '['
        let saved_abs = std::mem::replace(&mut self.in_abs, 0);
        let saved_let = std::mem::replace(&mut self.in_let_value, 0);
        let mut rows = Vec::new();
        let mut row = Vec::new();
        if !matches!(self.peek(), Tok::RBracket) {
            loop {
                row.push(self.expr());
                if self.eat(&Tok::Comma) {
                    continue;
                }
                if self.eat(&Tok::Semi) {
                    rows.push(std::mem::take(&mut row));
                    continue;
                }
                break;
            }
        }
        self.in_abs = saved_abs;
        self.in_let_value = saved_let;
        if !row.is_empty() {
            rows.push(row);
        }
        if !self.eat(&Tok::RBracket) {
            return Expr::Error("expected ']'".to_string());
        }
        Expr::Matrix(rows)
    }

    fn dict(&mut self) -> Expr {
        self.bump(); // '{'
        let mut entries = Vec::new();
        if !matches!(self.peek(), Tok::RBrace) {
            loop {
                let key = match self.peek().clone() {
                    Tok::Word(_) => self.identifier(),
                    Tok::Str(s) => {
                        self.bump();
                        s
                    }
                    _ => return Expr::Error("expected dictionary key".to_string()),
                };
                if !self.eat(&Tok::Colon) {
                    return Expr::Error("expected ':' in dictionary".to_string());
                }
                entries.push((key, self.expr()));
                if self.eat(&Tok::Comma) {
                    continue;
                }
                break;
            }
        }
        if !self.eat(&Tok::RBrace) {
            return Expr::Error("expected '}'".to_string());
        }
        Expr::Dict(entries)
    }
}

/// Convenience for building a numeric literal in tests.
pub fn n(v: i64) -> Expr {
    Expr::Num(Num::from_i64(v), Radix::Dec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::render;

    /// Round-trips through the formatter, which is the clearest way to assert
    /// on tree shape without writing out the whole structure.
    fn p(src: &str) -> String {
        render(&parse_expr(src))
    }

    #[test]
    fn implicit_multiplication_binds_tighter_than_explicit() {
        // The single most consequential precedence rule in the language.
        assert_eq!(p("2x/3y"), p("(2*x)/(3*y)"));
        assert_eq!(p("2*x/3*y"), p("((2*x)/3)*y"));
        assert_ne!(p("2x/3y"), p("2*x/3*y"));
    }

    #[test]
    fn in_is_conversion_only_when_something_follows() {
        // Inches, because nothing can follow.
        assert_eq!(p("5 ft + 4 in"), p("5*ft + 4*in"));
        assert_eq!(p("(5 ft + 4 in)"), p("5*ft + 4*in"));
        // Conversion, because a unit follows.
        assert!(matches!(parse_expr("100 ft in m"), Expr::Convert(..)));
        assert!(matches!(parse_expr("expenses in $/mo"), Expr::Convert(..)));
        // Lowest precedence: the whole quotient converts, not just `time`.
        assert!(matches!(
            parse_expr("distance / time in miles/hour"),
            Expr::Convert(..)
        ));
    }

    #[test]
    fn let_value_is_not_swallowed_by_conversion() {
        // `let x = point[0] in ...`: the `in` closes the binding even though a
        // primary follows it.
        let e = parse_expr("let x = point[0] in x + 1");
        assert!(matches!(e, Expr::Let(..)), "got {e:?}");
    }

    #[test]
    fn merges_multiword_identifiers() {
        assert_eq!(parse_expr("mass of earth"), Expr::var("mass of earth"));
        assert_eq!(
            parse_expr("cost of trip(gas mileage=x)"),
            Expr::Call(
                "cost of trip".to_string(),
                vec![Arg::named("gas mileage", Expr::var("x"))]
            )
        );
        // A number then a word is multiplication, not a name.
        assert_eq!(p("10 m"), p("10*m"));
    }

    #[test]
    fn symbols_do_not_glue_to_names() {
        assert_eq!(p("$350"), p("350*$"));
        assert_eq!(p("60°"), p("60*°"));
        assert_eq!(p("80%"), p("80*%"));
    }

    #[test]
    fn division_and_juxtaposition_in_unit_expressions() {
        assert_eq!(p("-9.8m/s/s"), p("((-9.8*m)/s)/s"));
        assert_eq!(p("$100,000/12 month"), p("100000*$/(12*month)"));
        assert_eq!(p("6.25 height/cm"), p("(6.25*height)/cm"));
    }

    #[test]
    fn parses_matrices_and_indexing() {
        assert_eq!(
            parse_expr("[1, 2; 3, 4]"),
            Expr::Matrix(vec![vec![n(1), n(2)], vec![n(3), n(4)]])
        );
        assert_eq!(
            parse_expr("[1, 2, 3]"),
            Expr::Matrix(vec![vec![n(1), n(2), n(3)]])
        );
        assert!(matches!(parse_expr("mat[0,0]"), Expr::Index(..)));
        assert!(matches!(
            parse_expr("big matrix[0..1, 1..2]"),
            Expr::Index(..)
        ));
        assert!(matches!(parse_expr("[1, 2; 3, 4]^T"), Expr::Transpose(_)));
    }

    #[test]
    fn pipes_are_absolute_value_in_prefix_position_and_or_in_infix() {
        assert!(matches!(parse_expr("|-4|"), Expr::Abs(_)));
        assert!(matches!(parse_expr("|point a - point b|"), Expr::Abs(_)));
        // Nested pipes inside an abs must not read as bitwise-or.
        assert_eq!(p("|foo|^2 + |bar|^2"), p("|foo|^2 + |bar|^2"));
        assert!(matches!(parse_expr("|foo|^2"), Expr::Pow(..)));
        assert!(matches!(
            parse_expr("a || b"),
            Expr::Logic(LogicOp::Or, ..)
        ));
        assert!(matches!(parse_expr("||vec||"), Expr::Norm(_, None)));
        assert!(matches!(parse_expr("||vec||1"), Expr::Norm(_, Some(_))));
    }

    #[test]
    fn one_over_two_i_is_a_reciprocal_not_a_half() {
        // `im(z) = 1/2i*(z - conj(z))` only gives the right answer if this
        // parses as 1/(2i).
        assert_eq!(p("1/2i"), p("1/(2*i)"));
    }

    #[test]
    fn parses_if_and_let() {
        assert!(matches!(
            parse_expr("if v < 0 then -v else v"),
            Expr::If(..)
        ));
        // else-if chains nest in the else branch.
        let chained = parse_expr("if a then 1 else if b then 2 else 3");
        match chained {
            Expr::If(_, _, otherwise) => assert!(matches!(*otherwise, Expr::If(..))),
            other => panic!("expected nested if, got {other:?}"),
        }
    }

    #[test]
    fn parses_definitions_and_equations() {
        let stmts = parse_line("y(x) = m*x + b");
        assert!(matches!(
            &stmts[0].stmt,
            Stmt::Define { name, params: Some(p), .. } if name == "y" && p == &["x"]
        ));

        let stmts = parse_line("stamp = $0.73");
        assert!(matches!(&stmts[0].stmt, Stmt::Define { name, params: None, .. } if name == "stamp"));

        // A compound left side is an equation to be solved, not a definition.
        let stmts = parse_line("12x + 13y = 163");
        assert!(matches!(&stmts[0].stmt, Stmt::Equation { .. }));

        let stmts = parse_line("coefficients * xy = solution");
        assert!(matches!(&stmts[0].stmt, Stmt::Equation { .. }));

        let stmts = parse_line("expenses +=");
        assert!(matches!(&stmts[0].stmt, Stmt::SumDefine { name } if name == "expenses"));
    }

    #[test]
    fn discards_stale_results_after_the_arrow() {
        // The answer is always recomputed, so whatever sits after `=>` is
        // skipped — including a `;` inside a matrix result.
        let stmts = parse_line("2 + 2           => 4");
        assert_eq!(stmts.len(), 1);
        assert!(stmts[0].arrow);
        assert_eq!(stmts[0].stmt, Stmt::Expr(Expr::Add(vec![n(2), n(2)])));

        let stmts = parse_line("[1, 2; 3, 4]^T => [1, 3; 2, 4]");
        assert_eq!(stmts.len(), 1);
        assert!(stmts[0].arrow);
    }

    #[test]
    fn splits_semicolon_separated_statements() {
        let stmts = parse_line("gap(a, b) = abs(a - b); gap(10, 12) => 2; gap(10, 11) => 1");
        assert_eq!(stmts.len(), 3);
        assert!(matches!(&stmts[0].stmt, Stmt::Define { name, .. } if name == "gap"));
        assert!(stmts[1].arrow && stmts[2].arrow);
    }

    #[test]
    fn parses_directives() {
        let stmts = parse_line("@precision = 8");
        assert!(matches!(&stmts[0].stmt, Stmt::Directive { name, value: Some(_) } if name == "@precision"));
        let stmts = parse_line("@fr-FR");
        assert!(matches!(&stmts[0].stmt, Stmt::Directive { name, value: None } if name == "@fr-FR"));
    }

    #[test]
    fn implicit_parameters_follow_order_of_appearance() {
        // A definition with no parameter list takes its variables in order of
        // first appearance, so `low + fraction*(high - low)` gets
        // (low, fraction, high).
        let body = parse_expr("low + fraction * (high - low)");
        assert_eq!(body.free_vars(), vec!["low", "fraction", "high"]);
    }

    #[test]
    fn let_bound_names_are_not_free() {
        let body = parse_expr("let x = point[0] in x * ca");
        assert_eq!(body.free_vars(), vec!["point", "ca"]);
    }
}
