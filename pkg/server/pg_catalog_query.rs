// Copyright 2026 AsterSQL.
//! The deliberately bounded PostgreSQL catalog SELECT grammar.
//! Parsing builds expressions, never freezes catalog rows at Parse time.

pub(crate) type ParseResult<T> = Result<T, (&'static str, String)>;
fn syntax(message: &str) -> (&'static str, String) {
    ("42601", message.into())
}
fn unsupported(message: &str) -> (&'static str, String) {
    ("0A000", message.into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Quoted(String),
    String(String),
    Number(i64),
    Symbol(char),
    Cast,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Expr {
    Column(Vec<String>),
    Null,
    Integer(i64),
    Text(String),
    Call(Vec<String>, Vec<Expr>),
    Cast(Box<Expr>, CastType),
    Equal(Box<Expr>, Box<Expr>),
    NotNull(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Case {
        condition: Box<Expr>,
        yes: Box<Expr>,
        no: Box<Expr>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastType {
    Bigint,
    Varchar,
    Oid,
    Regclass,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Projection {
    pub(crate) expr: Expr,
    pub(crate) name: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Relation {
    pub(crate) name: String,
    pub(crate) alias: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Join {
    pub(crate) relation: Relation,
    pub(crate) on: Expr,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ordering {
    pub(crate) expr: Expr,
    pub(crate) descending: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Select {
    pub(crate) projections: Vec<Projection>,
    pub(crate) from: Relation,
    pub(crate) join: Option<Join>,
    pub(crate) filter: Option<Expr>,
    pub(crate) order: Vec<Ordering>,
    pub(crate) limit: Option<usize>,
}

fn lex(sql: &str) -> ParseResult<Vec<Token>> {
    let b = sql.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"--") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth != 0 {
                return Err(syntax("unterminated comment"));
            }
            continue;
        }
        if matches!(b[i], b'\'' | b'"') {
            let quote = b[i];
            i += 1;
            let mut value = String::new();
            let mut start = i;
            loop {
                if i == b.len() {
                    return Err(syntax("unterminated quoted token"));
                }
                if b[i] == quote {
                    value.push_str(&sql[start..i]);
                    i += 1;
                    if b.get(i) == Some(&quote) {
                        value.push(quote as char);
                        i += 1;
                        start = i;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            out.push(if quote == b'"' {
                Token::Quoted(value)
            } else {
                Token::String(value)
            });
            continue;
        }
        if b[i..].starts_with(b"::") {
            out.push(Token::Cast);
            i += 2;
            continue;
        }
        if b[i].is_ascii_alphabetic() || b[i] == b'_' || b[i] >= 128 {
            let start = i;
            i += 1;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'$') || b[i] >= 128)
            {
                i += 1;
            }
            out.push(Token::Word(sql[start..i].to_lowercase()));
            continue;
        }
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push(Token::Number(
                sql[start..i]
                    .parse()
                    .map_err(|_| syntax("integer is out of range"))?,
            ));
            continue;
        }
        if b[i].is_ascii() {
            out.push(Token::Symbol(b[i] as char));
            i += 1;
        } else {
            return Err(syntax("invalid token"));
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    casts: usize,
}
impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }
    fn word(&mut self, word: &str) -> bool {
        if self.peek() == Some(&Token::Word(word.into())) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn symbol(&mut self, c: char) -> bool {
        if self.peek() == Some(&Token::Symbol(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn require_word(&mut self, word: &str) -> ParseResult<()> {
        if self.word(word) {
            Ok(())
        } else {
            Err(syntax(&format!("expected {word}")))
        }
    }
    fn require_symbol(&mut self, c: char) -> ParseResult<()> {
        if self.symbol(c) {
            Ok(())
        } else {
            Err(syntax(&format!("expected {c}")))
        }
    }
    fn identifier(&mut self) -> ParseResult<String> {
        match self.peek().cloned() {
            Some(Token::Word(s) | Token::Quoted(s)) => {
                self.pos += 1;
                Ok(s)
            }
            _ => Err(syntax("expected identifier")),
        }
    }
    fn path(&mut self) -> ParseResult<Vec<String>> {
        let mut path = vec![self.identifier()?];
        while self.symbol('.') {
            path.push(self.identifier()?);
        }
        Ok(path)
    }
    fn expr(&mut self) -> ParseResult<Expr> {
        let mut expr = self.comparison()?;
        let mut terms = 1;
        while self.word("and") {
            terms += 1;
            if terms > 64 {
                return Err(unsupported("too many catalog predicate terms"));
            }
            expr = Expr::And(Box::new(expr), Box::new(self.comparison()?));
        }
        Ok(expr)
    }
    fn comparison(&mut self) -> ParseResult<Expr> {
        if self.depth == 64 {
            return Err(unsupported("catalog expression nesting is too deep"));
        }
        self.depth += 1;
        let result = self.expr_inner();
        self.depth -= 1;
        result
    }
    fn expr_inner(&mut self) -> ParseResult<Expr> {
        let mut expr = if self.word("null") {
            Expr::Null
        } else if self.word("current_catalog") {
            Expr::Call(vec!["current_catalog".into()], vec![])
        } else if self.word("case") {
            self.require_word("when")?;
            let condition = self.expr()?;
            self.require_word("then")?;
            let yes = self.expr()?;
            self.require_word("else")?;
            let no = self.expr()?;
            self.require_word("end")?;
            Expr::Case {
                condition: Box::new(condition),
                yes: Box::new(yes),
                no: Box::new(no),
            }
        } else if self.symbol('(') {
            let expr = self.expr()?;
            self.require_symbol(')')?;
            expr
        } else if self.symbol('-') {
            match self.peek().cloned() {
                Some(Token::Number(n)) => {
                    self.pos += 1;
                    Expr::Integer(-n)
                }
                _ => return Err(syntax("expected integer after minus")),
            }
        } else {
            match self.peek().cloned() {
                Some(Token::Number(n)) => {
                    self.pos += 1;
                    Expr::Integer(n)
                }
                Some(Token::String(s)) => {
                    self.pos += 1;
                    Expr::Text(s)
                }
                Some(Token::Word(_) | Token::Quoted(_)) => {
                    let path = self.path()?;
                    if self.symbol('(') {
                        let mut args = Vec::new();
                        if !self.symbol(')') {
                            loop {
                                args.push(self.expr()?);
                                if !self.symbol(',') {
                                    break;
                                }
                            }
                            self.require_symbol(')')?;
                        }
                        Expr::Call(path, args)
                    } else {
                        Expr::Column(path)
                    }
                }
                Some(Token::Symbol('*')) => {
                    return Err(unsupported("wildcard catalog projections are unsupported"));
                }
                _ => return Err(syntax("expected expression")),
            }
        };
        let mut casts = 0;
        while self.peek() == Some(&Token::Cast) {
            casts += 1;
            self.casts += 1;
            if casts > 64 || self.casts > 128 {
                return Err(unsupported("too many catalog casts"));
            }
            self.pos += 1;
            let target = self.identifier()?;
            let target = match target.as_str() {
                "bigint" => CastType::Bigint,
                "varchar" => CastType::Varchar,
                "oid" => CastType::Oid,
                "regclass" => CastType::Regclass,
                _ => return Err(unsupported("unsupported catalog cast")),
            };
            expr = Expr::Cast(Box::new(expr), target);
        }
        if self.symbol('=') {
            expr = Expr::Equal(Box::new(expr), Box::new(self.comparison()?));
        } else if self.word("is") {
            if self.word("null") {
                return Err(unsupported("IS NULL catalog predicates are unsupported"));
            }
            self.require_word("not")?;
            self.require_word("null")?;
            expr = Expr::NotNull(Box::new(expr));
        }
        if matches!(
            self.peek(),
            Some(Token::Symbol(
                '+' | '-' | '/' | '*' | '<' | '>' | '!' | '|' | '&'
            ))
        ) {
            return Err(unsupported("unsupported catalog operator"));
        }
        Ok(expr)
    }
    fn relation(&mut self) -> ParseResult<Relation> {
        let path = self.path()?;
        if path.len() != 2 || path[0] != "pg_catalog" {
            return Err(unsupported(
                "catalog relations must be qualified by pg_catalog",
            ));
        }
        let name = path[1].clone();
        if !matches!(
            name.as_str(),
            "pg_database"
                | "pg_locks"
                | "pg_namespace"
                | "pg_tablespace"
                | "pg_description"
                | "pg_shdescription"
        ) {
            return Err((
                "42P01",
                format!("relation pg_catalog.{name} does not exist"),
            ));
        }
        let alias = if self.word("as") || self.is_alias() {
            self.identifier()?
        } else {
            name.clone()
        };
        Ok(Relation { name, alias })
    }
    fn is_alias(&self) -> bool {
        match self.peek() {
            Some(Token::Quoted(_)) => true,
            Some(Token::Word(s)) => !matches!(
                s.as_str(),
                "from"
                    | "left"
                    | "join"
                    | "on"
                    | "where"
                    | "order"
                    | "limit"
                    | "group"
                    | "having"
                    | "union"
                    | "offset"
                    | "inner"
                    | "right"
                    | "full"
                    | "cross"
                    | "asc"
                    | "desc"
                    | "end"
                    | "then"
                    | "else"
            ),
            _ => false,
        }
    }
    fn select(&mut self) -> ParseResult<Select> {
        self.require_word("select")?;
        if self.word("distinct") {
            return Err(unsupported("DISTINCT catalog queries are unsupported"));
        }
        let mut projections = Vec::new();
        loop {
            let expr = self.expr()?;
            let name = if self.word("as") || self.is_alias() {
                self.identifier()?
            } else {
                label(&expr)
            };
            projections.push(Projection { expr, name });
            if !self.symbol(',') {
                break;
            }
        }
        self.require_word("from")?;
        let from = self.relation()?;
        let join = if self.word("left") {
            self.word("outer");
            self.require_word("join")?;
            let relation = self.relation()?;
            self.require_word("on")?;
            let on = self.expr()?;
            if !join_predicate(&on) {
                return Err(unsupported("only equality catalog joins are supported"));
            }
            Some(Join { relation, on })
        } else {
            None
        };
        let filter = if self.word("where") {
            Some(self.expr()?)
        } else {
            None
        };
        let mut order = Vec::new();
        if self.word("order") {
            self.require_word("by")?;
            loop {
                let expr = self.expr()?;
                let descending = self.word("desc");
                if !descending {
                    self.word("asc");
                }
                order.push(Ordering { expr, descending });
                if !self.symbol(',') {
                    break;
                }
            }
        }
        let limit = if self.word("limit") {
            match self.peek().cloned() {
                Some(Token::Number(n)) => {
                    self.pos += 1;
                    Some(usize::try_from(n).map_err(|_| syntax("invalid LIMIT"))?)
                }
                _ => return Err(syntax("expected nonnegative LIMIT")),
            }
        } else {
            None
        };
        self.symbol(';');
        if self.peek().is_some() {
            return Err(unsupported(
                "unsupported catalog clause or multiple statements",
            ));
        }
        Ok(Select {
            projections,
            from,
            join,
            filter,
            order,
            limit,
        })
    }
}
fn join_predicate(expr: &Expr) -> bool {
    match expr {
        Expr::Equal(..) => true,
        Expr::And(left, right) => join_predicate(left) && join_predicate(right),
        _ => false,
    }
}
fn label(expr: &Expr) -> String {
    match expr {
        Expr::Column(path) | Expr::Call(path, _) => path.last().unwrap().clone(),
        Expr::Cast(expr, _) => label(expr),
        _ => "?column?".into(),
    }
}

/// A tokenized relation reference, not catalog text inside strings/comments,
/// determines ownership. Ordinary engine SQL is untouched.
pub(crate) fn parse(sql: &str) -> ParseResult<Option<Select>> {
    let tokens = match lex(sql) {
        Ok(tokens) => tokens,
        Err(error) => {
            // Preserve engine ownership for lexical errors outside catalog SQL.
            if sql.to_ascii_lowercase().contains("pg_catalog.") {
                return Err(error);
            }
            return Ok(None);
        }
    };
    let catalog = tokens.windows(4).any(|w| {
        matches!(&w[0], Token::Word(s) if matches!(s.as_str(), "from" | "join"))
            && matches!(&w[1], Token::Word(s) | Token::Quoted(s) if s == "pg_catalog")
            && w[2] == Token::Symbol('.')
    });
    if !catalog {
        return Ok(None);
    }
    if !matches!(tokens.first(), Some(Token::Word(s)) if s == "select") {
        return Err(unsupported("only catalog SELECT is supported"));
    }
    Parser {
        tokens,
        pos: 0,
        depth: 0,
        casts: 0,
    }
    .select()
    .map(Some)
}
