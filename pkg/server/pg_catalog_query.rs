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
pub(crate) enum Token {
    Word(String),
    Quoted(String),
    String(String),
    Number(i64),
    Parameter(usize),
    Symbol(char),
    Cast,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Expr {
    Column(Vec<String>),
    Null,
    Parameter(usize),
    Integer(i64),
    Boolean(bool),
    Text(String),
    Call(Vec<String>, Vec<Expr>),
    Cast(Box<Expr>, CastType),
    Equal(Box<Expr>, Box<Expr>),
    Compare(Box<Expr>, CompareOp, Box<Expr>),
    In(Box<Expr>, Vec<Expr>),
    Any(Box<Expr>, Box<Expr>),
    ArrayUnnest {
        input: Box<Expr>,
        projection: Box<Expr>,
    },
    InSubquery(Box<Expr>, Box<Select>),
    ScalarSubquery(Box<Select>),
    ArrayAgg(Box<Expr>, Vec<Ordering>),
    TypedLiteral(Box<Expr>, u8, usize),
    Not(Box<Expr>),
    IsNull(Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    NotNull(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Case {
        condition: Box<Expr>,
        yes: Box<Expr>,
        no: Box<Expr>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompareOp {
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastType {
    InternalChar,
    Bigint,
    Varchar,
    Oid,
    Regclass,
    OperatorName,
    IntArray,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Projection {
    pub(crate) expr: Expr,
    pub(crate) name: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Relation {
    pub(crate) cte_id: Option<usize>,
    pub(crate) name: String,
    pub(crate) alias: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Join {
    pub(crate) left: bool,
    pub(crate) natural: bool,
    pub(crate) relation: Relation,
    pub(crate) on: Expr,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ordering {
    pub(crate) expr: Expr,
    pub(crate) descending: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cte {
    pub(crate) id: usize,
    pub(crate) query: Select,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Select {
    pub(crate) ctes: Vec<Cte>,
    pub(crate) distinct: bool,
    pub(crate) unions: Vec<(bool, Select)>,
    pub(crate) projections: Vec<Projection>,
    pub(crate) from: Relation,
    pub(crate) joins: Vec<Join>,
    pub(crate) filter: Option<Expr>,
    pub(crate) order: Vec<Ordering>,
    pub(crate) limit: Option<usize>,
}

pub(crate) fn lex(sql: &str) -> ParseResult<Vec<Token>> {
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
        if b[i] == b'$' {
            i += 1;
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let index = sql[start..i]
                .parse::<usize>()
                .map_err(|_| ("42P02", "invalid catalog parameter index".into()))?;
            if !(1..=32767).contains(&index) {
                return Err(("42P02", "catalog parameter index out of range".into()));
            }
            if b.get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                return Err(syntax("parameter marker is part of an identifier"));
            }
            out.push(Token::Parameter(index - 1));
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
    predicates: usize,
    query_depth: usize,
    next_cte: usize,
    cte_scope: Vec<(String, usize)>,
    shadowed: Vec<String>,
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
    fn predicate_budget(&mut self) -> ParseResult<()> {
        self.predicates += 1;
        if self.predicates > 128 {
            return Err(unsupported("too many catalog predicate terms"));
        }
        Ok(())
    }
    // SQL precedence: scalar/cast, comparison, NOT, AND, then OR.
    fn expr(&mut self) -> ParseResult<Expr> {
        let mut expr = self.conjunction()?;
        while self.word("or") {
            self.predicate_budget()?;
            expr = Expr::Or(Box::new(expr), Box::new(self.conjunction()?));
        }
        Ok(expr)
    }
    fn conjunction(&mut self) -> ParseResult<Expr> {
        let mut expr = self.negation()?;
        while self.word("and") {
            self.predicate_budget()?;
            expr = Expr::And(Box::new(expr), Box::new(self.negation()?));
        }
        Ok(expr)
    }
    fn negation(&mut self) -> ParseResult<Expr> {
        if self.word("not") {
            self.predicate_budget()?;
            if self.depth == 64 {
                return Err(unsupported("catalog expression nesting is too deep"));
            }
            self.depth += 1;
            let result = self.negation();
            self.depth -= 1;
            return result.map(|expr| Expr::Not(Box::new(expr)));
        }
        self.comparison()
    }
    // RetrieveColumns joins (attrelid, attnum) to (adrelid, adnum).
    // A two-field row equality is exactly the SQL conjunction of the two
    // scalar equalities, including false/unknown precedence. Keep other row
    // operators and widths outside this bounded grammar.
    fn tuple_start(&self) -> bool {
        if self.peek() != Some(&Token::Symbol('(')) {
            return false;
        }
        let mut depth = 0usize;
        let mut comma = false;
        for token in &self.tokens[self.pos..] {
            match token {
                Token::Symbol('(') => depth += 1,
                Token::Symbol(')') => {
                    depth -= 1;
                    if depth == 0 {
                        return comma;
                    }
                }
                Token::Symbol(',') if depth == 1 => comma = true,
                _ => {}
            }
        }
        false
    }
    fn tuple_pair(&mut self) -> ParseResult<(Expr, Expr)> {
        self.require_symbol('(')?;
        let first = self.atom()?;
        if !self.symbol(',') {
            return Err(unsupported("catalog row equality requires two fields"));
        }
        let second = self.atom()?;
        if !self.symbol(')') {
            return Err(unsupported("catalog row equality requires two fields"));
        }
        Ok((first, second))
    }
    fn tuple_equality(&mut self) -> ParseResult<Expr> {
        if self.depth == 64 {
            return Err(unsupported("catalog expression nesting is too deep"));
        }
        self.depth += 1;
        let result = (|| {
            let (left_a, left_b) = self.tuple_pair()?;
            if !self.symbol('=') {
                return Err(unsupported("catalog row comparison supports only equality"));
            }
            let (right_a, right_b) = self.tuple_pair()?;
            for _ in 0..3 {
                self.predicate_budget()?;
            }
            Ok(Expr::And(
                Box::new(Expr::Equal(Box::new(left_a), Box::new(right_a))),
                Box::new(Expr::Equal(Box::new(left_b), Box::new(right_b))),
            ))
        })();
        self.depth -= 1;
        result
    }
    fn comparison(&mut self) -> ParseResult<Expr> {
        if self.tuple_start() {
            return self.tuple_equality();
        }
        let mut expr = self.atom()?;
        if self.symbol('=') {
            self.predicate_budget()?;
            expr = if self.word("any") {
                self.require_symbol('(')?;
                let input = self.expr()?;
                self.require_symbol(')')?;
                Expr::Any(Box::new(expr), Box::new(input))
            } else {
                Expr::Equal(Box::new(expr), Box::new(self.atom()?))
            };
        } else if self.symbol('<') {
            self.predicate_budget()?;
            let op = if self.symbol('>') {
                CompareOp::NotEqual
            } else if self.symbol('=') {
                CompareOp::LessEqual
            } else {
                CompareOp::Less
            };
            expr = Expr::Compare(Box::new(expr), op, Box::new(self.atom()?));
        } else if self.symbol('>') {
            self.predicate_budget()?;
            let op = if self.symbol('=') {
                CompareOp::GreaterEqual
            } else {
                CompareOp::Greater
            };
            expr = Expr::Compare(Box::new(expr), op, Box::new(self.atom()?));
        } else if self.symbol('!') {
            self.require_symbol('=')?;
            self.predicate_budget()?;
            expr = Expr::Compare(Box::new(expr), CompareOp::NotEqual, Box::new(self.atom()?));
        } else if self.word("is") {
            self.predicate_budget()?;
            let not = self.word("not");
            self.require_word("null")?;
            expr = if not {
                Expr::NotNull(Box::new(expr))
            } else {
                Expr::IsNull(Box::new(expr))
            };
        } else {
            let not = self.word("not");
            if not || self.word("in") {
                if not {
                    self.require_word("in")?;
                }
                self.predicate_budget()?;
                self.require_symbol('(')?;
                if self.symbol(')') {
                    return Err(syntax("IN requires a nonempty constant list"));
                }
                if matches!(self.peek(), Some(Token::Word(w)) if w == "select" || w == "with") {
                    let query = self.query()?;
                    self.require_symbol(')')?;
                    expr = Expr::InSubquery(Box::new(expr), Box::new(query));
                } else {
                    let mut values = Vec::new();
                    loop {
                        let value = self.atom()?;
                        if !constant(&value) {
                            return Err(unsupported("catalog IN requires constants"));
                        }
                        values.push(value);
                        if values.len() > 128 {
                            return Err(unsupported("too many catalog IN values"));
                        }
                        if !self.symbol(',') {
                            break;
                        }
                    }
                    self.require_symbol(')')?;
                    expr = Expr::In(Box::new(expr), values);
                }
                if not {
                    expr = Expr::Not(Box::new(expr));
                }
            }
        }
        if matches!(
            self.peek(),
            Some(Token::Symbol(
                '+' | '-' | '/' | '*' | '<' | '>' | '!' | '|' | '&' | '='
            ))
        ) || self.peek() == Some(&Token::Word("like".into()))
        {
            return Err(unsupported("unsupported catalog operator"));
        }
        Ok(expr)
    }
    fn atom(&mut self) -> ParseResult<Expr> {
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
        } else if self.word("true") {
            Expr::Boolean(true)
        } else if self.word("false") {
            Expr::Boolean(false)
        } else if self.word("current_catalog") {
            Expr::Call(vec!["current_catalog".into()], vec![])
        } else if self.word("array") {
            self.require_symbol('(')?;
            self.require_word("select")?;
            let projection = self.expr()?;
            self.require_word("from")?;
            let function = self.path()?;
            if !matches!(function.as_slice(), [name] if name == "unnest")
                && !matches!(function.as_slice(), [catalog, name] if catalog == "pg_catalog" && name == "unnest")
            {
                return Err(unsupported("catalog ARRAY requires an unnest source"));
            }
            self.require_symbol('(')?;
            let input = self.expr()?;
            self.require_symbol(')')?;
            self.require_symbol(')')?;
            Expr::ArrayUnnest {
                input: Box::new(input),
                projection: Box::new(projection),
            }
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
            let expr = if matches!(self.peek(), Some(Token::Word(w)) if w == "select" || w == "with")
            {
                Expr::ScalarSubquery(Box::new(self.query()?))
            } else {
                self.expr()?
            };
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
                        if path.last().is_some_and(|n| n == "array_agg") {
                            let value = self.expr()?;
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
                            self.require_symbol(')')?;
                            Expr::ArrayAgg(Box::new(value), order)
                        } else {
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
                        }
                    } else {
                        Expr::Column(path)
                    }
                }
                Some(Token::Parameter(index)) => {
                    self.pos += 1;
                    Expr::Parameter(index)
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
            let quoted = matches!(self.peek(), Some(Token::Quoted(_)));
            let target = self.identifier()?;
            let target = match target.as_str() {
                "char" if quoted => CastType::InternalChar,
                "bigint" => CastType::Bigint,
                "varchar" => CastType::Varchar,
                "oid" => CastType::Oid,
                "regclass" => CastType::Regclass,
                // Keep the supported name conversion atomic: a standalone
                // regoper result would require an operator catalog and wire type.
                "regoper" => {
                    if self.peek() != Some(&Token::Cast) {
                        return Err(unsupported("regoper requires a varchar name conversion"));
                    }
                    self.pos += 1;
                    if self.identifier()? != "varchar" {
                        return Err(unsupported("unsupported regoper conversion"));
                    }
                    self.casts += 1;
                    if self.casts > 128 {
                        return Err(unsupported("too many catalog casts"));
                    }
                    CastType::OperatorName
                }
                "int" | "integer" => {
                    self.require_symbol('[')?;
                    self.require_symbol(']')?;
                    CastType::IntArray
                }
                _ => return Err(unsupported("unsupported catalog cast")),
            };
            expr = Expr::Cast(Box::new(expr), target);
        }
        Ok(expr)
    }
    fn relation(&mut self) -> ParseResult<Relation> {
        let path = self.path()?;
        let name = match path.as_slice() {
            [name] => name.clone(),
            [catalog, name] if catalog == "pg_catalog" => name.clone(),
            _ => return Err(unsupported("unsupported catalog relation qualification")),
        };
        let cte_id = if path.len() == 1 {
            self.cte_scope
                .iter()
                .rev()
                .find(|(n, _)| n == &name)
                .map(|(_, id)| *id)
        } else {
            None
        };
        if cte_id.is_none() && path.len() == 1 && self.shadowed.contains(&name) {
            return Err(unsupported(
                "mixed public and catalog relations are unsupported",
            ));
        }
        if cte_id.is_none() && !is_catalog_relation(&name) {
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
        Ok(Relation {
            name,
            alias,
            cte_id,
        })
    }
    fn is_alias(&self) -> bool {
        match self.peek() {
            Some(Token::Quoted(_)) => true,
            Some(Token::Word(s)) => !matches!(
                s.as_str(),
                "from"
                    | "natural"
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
    // Query nesting and total definitions are bounded independently of scalar depth.
    // Unique IDs keep a nested name shadow distinct from its outer materialization.
    fn query(&mut self) -> ParseResult<Select> {
        if self.query_depth >= 8 {
            return Err(unsupported("catalog query nesting exceeds eight levels"));
        }
        self.query_depth += 1;
        let scope = self.cte_scope.len();
        let result = self.query_inner();
        self.cte_scope.truncate(scope);
        self.query_depth -= 1;
        result
    }
    fn query_inner(&mut self) -> ParseResult<Select> {
        let mut ctes = Vec::new();
        let mut names = std::collections::HashSet::new();
        if self.word("with") {
            if self.word("recursive") {
                return Err(unsupported("recursive catalog CTEs are unsupported"));
            }
            loop {
                let name = self.identifier()?;
                if !names.insert(name.clone()) {
                    return Err(("42712", "duplicate catalog CTE name".into()));
                }
                if self.next_cte >= 16 {
                    return Err(unsupported("catalog queries support at most sixteen CTEs"));
                }
                let id = self.next_cte;
                self.next_cte += 1;
                let mut columns = Vec::new();
                if self.symbol('(') {
                    loop {
                        columns.push(self.identifier()?);
                        if !self.symbol(',') {
                            break;
                        }
                    }
                    self.require_symbol(')')?;
                }
                self.require_word("as")?;
                self.require_symbol('(')?;
                if !matches!(self.peek(), Some(Token::Word(w)) if w == "select" || w == "with") {
                    return Err(unsupported("only read-only catalog CTEs are supported"));
                }
                // Nonrecursive definitions see earlier CTEs and outer scopes only.
                let mut query = self.query()?;
                self.require_symbol(')')?;
                if columns.len() > query.projections.len() {
                    return Err(syntax("too many CTE column aliases"));
                }
                for (projection, alias) in query.projections.iter_mut().zip(columns) {
                    projection.name = alias;
                }
                self.cte_scope.push((name, id));
                ctes.push(Cte { id, query });
                if !self.symbol(',') {
                    break;
                }
            }
        }
        if !matches!(self.peek(), Some(Token::Word(w)) if w == "select") {
            return Err(unsupported("only read-only catalog SELECT is supported"));
        }
        let mut select = self.select()?;
        while self.word("union") {
            if select.unions.len() >= 16 {
                return Err(unsupported(
                    "catalog queries support at most sixteen UNION arms",
                ));
            }
            let all = self.word("all");
            if !all {
                self.word("distinct");
            }
            if !select.order.is_empty() || select.limit.is_some() {
                return Err(syntax("ORDER BY/LIMIT must follow the UNION"));
            }
            let mut arm = self.select()?;
            select.order = std::mem::take(&mut arm.order);
            select.limit = arm.limit.take();
            select.unions.push((all, arm));
        }
        select.ctes = ctes;
        Ok(select)
    }
    fn select(&mut self) -> ParseResult<Select> {
        self.require_word("select")?;
        let distinct = self.word("distinct");
        let mut projections = Vec::new();
        loop {
            let expr = if self.symbol('*') {
                Expr::Column(vec!["*".into()])
            } else {
                self.expr()?
            };
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
        let mut joins = Vec::new();
        loop {
            let natural = self.word("natural");
            let left = if self.word("left") {
                self.word("outer");
                self.require_word("join")?;
                true
            } else if self.word("inner") {
                self.require_word("join")?;
                false
            } else if self.word("join") {
                false
            } else {
                break;
            };
            if joins.len() >= 8 {
                return Err(unsupported("catalog queries support at most eight joins"));
            }
            let relation = self.relation()?;
            let on = if natural {
                Expr::Boolean(true)
            } else {
                self.require_word("on")?;
                self.expr()?
            };
            joins.push(Join {
                left,
                natural,
                relation,
                on,
            });
        }
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
        Ok(Select {
            ctes: Vec::new(),
            distinct,
            unions: Vec::new(),
            projections,
            from,
            joins,
            filter,
            order,
            limit,
        })
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
    parse_shadowed(sql, &[])
}

pub(crate) fn is_catalog_relation(name: &str) -> bool {
    name == "pg_opclass"
        || name == "pg_locks"
        || crate::pg_oid::SYSTEM_RELATIONS
            .iter()
            .any(|(n, _)| *n == name)
}

// Track FROM lists at each nesting level so a mixed comma join cannot fall
// through to the native engine. Projection commas are not relation separators.
fn implicit_positions(tokens: &[Token]) -> Vec<(usize, String)> {
    let mut from = vec![false];
    let mut found = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let relation = match token {
            Token::Word(w) if matches!(w.as_str(), "from" | "join") => {
                *from.last_mut().unwrap() = true;
                true
            }
            Token::Word(w)
                if matches!(
                    w.as_str(),
                    "where" | "on" | "group" | "having" | "order" | "limit" | "union"
                ) =>
            {
                *from.last_mut().unwrap() = false;
                false
            }
            Token::Symbol(',') => *from.last().unwrap(),
            Token::Symbol('(') => {
                from.push(false);
                false
            }
            Token::Symbol(')') => {
                if from.len() > 1 {
                    from.pop();
                }
                false
            }
            _ => false,
        };
        if relation && tokens.get(i + 2) != Some(&Token::Symbol('.')) {
            if let Some(Token::Word(name) | Token::Quoted(name)) = tokens.get(i + 1) {
                if is_catalog_relation(name) {
                    found.push((i + 1, name.clone()));
                }
            }
        }
    }
    found
}

pub(crate) fn implicit_relations(sql: &str) -> Vec<String> {
    lex(sql)
        .map(|tokens| {
            implicit_positions(&tokens)
                .into_iter()
                .map(|(_, n)| n)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn parse_shadowed(sql: &str, shadowed: &[String]) -> ParseResult<Option<Select>> {
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
    // Determine ownership without rewriting tokens: lexical CTE scope must
    // take precedence over a same-named public relation during parsing.
    let catalog = tokens.windows(4).any(|w| {
        matches!(&w[0], Token::Word(s) if matches!(s.as_str(), "from" | "join"))
            && matches!(&w[1], Token::Word(s) | Token::Quoted(s) if s == "pg_catalog")
            && w[2] == Token::Symbol('.')
    }) || tokens.windows(3).any(|w| {
        matches!(&w[0], Token::Word(s) | Token::Quoted(s) if s == "pg_catalog")
            && w[1] == Token::Symbol('.')
            && matches!(&w[2], Token::Word(s) | Token::Quoted(s) if is_catalog_relation(s))
    });
    let implicit_catalog = implicit_positions(&tokens)
        .iter()
        .any(|(_, name)| !shadowed.contains(name));
    if !catalog && !implicit_catalog {
        return Ok(None);
    }
    if !matches!(tokens.first(), Some(Token::Word(s)) if s == "select" || s == "with") {
        return Err(unsupported("only catalog SELECT is supported"));
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        depth: 0,
        casts: 0,
        predicates: 0,
        query_depth: 0,
        next_cte: 0,
        cte_scope: Vec::new(),
        shadowed: shadowed.to_vec(),
    };
    let query = parser.query()?;
    parser.symbol(';');
    if parser.peek().is_some() {
        return Err(unsupported(
            "unsupported catalog clause or multiple statements",
        ));
    }
    Ok(Some(query))
}

fn constant(expr: &Expr) -> bool {
    match expr {
        Expr::Null | Expr::Integer(_) | Expr::Text(_) | Expr::Boolean(_) | Expr::Parameter(_) => {
            true
        }
        Expr::Cast(inner, _) => constant(inner),
        _ => false,
    }
}
