// Copyright 2026 AsterSQL.
//! PG-only relation resolution. Token spans preserve literals/comments; query
//! scopes distinguish CTE references from native relations. The native parser
//! still validates the resulting statement and owns object lookup/authorization.
use crate::conn::{CancellationToken, TiDBContext, Value};
use crate::pg_catalog_query::ParseResult;
use std::collections::{BTreeMap, HashSet};

struct Token {
    start: usize,
    end: usize,
    name: Option<String>,
    quoted: bool,
    symbol: Option<u8>,
}
impl Token {
    fn word(&self, word: &str) -> bool {
        !self.quoted
            && self
                .name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(word))
    }
}
fn error(code: &'static str, message: &str) -> (&'static str, String) {
    (code, message.into())
}
fn quote(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}
fn tokens(sql: &str) -> ParseResult<Vec<Token>> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
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
            if b[i..].starts_with(b"/*!") || b[i..].starts_with(b"/*T!") {
                return Err(error("0A000", "executable MySQL comments are unsupported"));
            }
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth != 0 {
                if b[i..].starts_with(b"/*") {
                    return Err(error(
                        "0A000",
                        "nested PG comments are unsupported by the native parser",
                    ));
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth != 0 {
                return Err(error("42601", "unterminated SQL comment"));
            }
            continue;
        }
        let start = i;
        let mut name = None;
        let mut quoted = false;
        let mut symbol = None;
        if b[i] == b'\'' || b[i] == b'"' {
            let delimiter = b[i];
            quoted = delimiter == b'"';
            i += 1;
            let content = i;
            loop {
                if i == b.len() {
                    return Err(error("42601", "unterminated SQL quote"));
                }
                if b[i] == delimiter {
                    if b.get(i + 1) == Some(&delimiter) {
                        i += 2;
                        continue;
                    }
                    if quoted {
                        let n = sql[content..i].replace("\"\"", "\"");
                        if n.is_empty() {
                            return Err(error("42601", "empty quoted identifier"));
                        }
                        name = Some(n);
                    }
                    i += 1;
                    break;
                }
                // Keep the existing native single-quoted escape behavior.
                if !quoted && b[i] == b'\\' && i + 1 < b.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if b[i] == b'`'
            || b[i] == b'#'
            || b[i] == b'$' && !b.get(i + 1).is_some_and(u8::is_ascii_digit)
        {
            return Err(error("0A000", "unsupported PG SQL quoting"));
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' || b[i] >= 128 {
            i += 1;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 128)
            {
                i += 1;
            }
            name = Some(sql[start..i].to_lowercase());
        } else {
            symbol = Some(b[i]);
            i += 1;
        }
        out.push(Token {
            start,
            end: i,
            name,
            quoted,
            symbol,
        });
    }
    Ok(out)
}
struct Resolver<'a> {
    tokens: Vec<Token>,
    database: &'a str,
    public: bool,
    edits: BTreeMap<usize, (usize, String)>,
}
impl Resolver<'_> {
    fn word(&self, i: usize, name: &str) -> bool {
        self.tokens.get(i).is_some_and(|t| t.word(name))
    }
    fn symbol(&self, i: usize, c: u8) -> bool {
        self.tokens.get(i).is_some_and(|t| t.symbol == Some(c))
    }
    fn close(&self, start: usize, end: usize) -> ParseResult<usize> {
        let mut depth = 0;
        for i in start..end {
            if self.symbol(i, b'(') {
                depth += 1;
            }
            if self.symbol(i, b')') {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
        }
        Err(error("42601", "unclosed SQL parenthesis"))
    }
    fn relation(
        &mut self,
        mut i: usize,
        end: usize,
        ctes: &HashSet<String>,
        allow_columns: bool,
    ) -> ParseResult<usize> {
        while i < end
            && (self.word(i, "if")
                || self.word(i, "not")
                || self.word(i, "exists")
                || self.word(i, "only"))
        {
            i += 1;
        }
        if self.symbol(i, b'(') {
            if !self.word(i + 1, "select") && !self.word(i + 1, "with") {
                return Err(error(
                    "0A000",
                    "parenthesized relation groups are unsupported",
                ));
            }
            return Ok(i);
        }
        let start = i;
        let mut parts = Vec::new();
        loop {
            let name = self
                .tokens
                .get(i)
                .and_then(|t| t.name.clone())
                .ok_or_else(|| error("42601", "expected relation identifier"))?;
            parts.push(name);
            i += 1;
            if !self.symbol(i, b'.') {
                break;
            }
            i += 1;
        }
        if parts.len() == 1 && ctes.contains(&parts[0]) {
            return Ok(i);
        }
        if self.symbol(i, b'(') && !allow_columns {
            return Err(error("0A000", "native table functions are unsupported"));
        }
        let name = match parts.as_slice() {
            [name] => {
                if !self.public {
                    return Err(error("42P01", "relation is not in the PG search path"));
                }
                name
            }
            [schema, name] if schema == "public" => name,
            [db, schema, name]
                if schema == "public" && (self.database.is_empty() || db == self.database) =>
            {
                name
            }
            _ => {
                return Err(error(
                    "0A000",
                    "cross-database or non-public relations are unsupported",
                ));
            }
        };
        if self.database.is_empty() {
            return Err(error("3D000", "no current database selected"));
        }
        let first = self.tokens[start].start;
        let last = self.tokens[i - 1].end;
        // Remove individual identifier edits swallowed by the relation span.
        self.edits
            .retain(|offset, _| *offset < first || *offset >= last);
        self.edits.insert(
            first,
            (last, format!("{}.{}", quote(self.database), quote(name))),
        );
        Ok(i)
    }
    fn scope(&mut self, mut i: usize, end: usize, inherited: &HashSet<String>) -> ParseResult<()> {
        let mut ctes = inherited.clone();
        if self.word(i, "with") {
            i += 1;
            if self.word(i, "recursive") {
                return Err(error("0A000", "recursive PG CTEs are unsupported"));
            }
            loop {
                let name = self
                    .tokens
                    .get(i)
                    .and_then(|t| t.name.clone())
                    .ok_or_else(|| error("42601", "expected CTE name"))?;
                i += 1;
                if self.symbol(i, b'(') {
                    i = self.close(i, end)? + 1;
                }
                if !self.word(i, "as") || !self.symbol(i + 1, b'(') {
                    return Err(error("42601", "expected CTE query"));
                }
                i += 1;
                let close = self.close(i, end)?;
                self.scope(i + 1, close, &ctes)?;
                ctes.insert(name);
                i = close + 1;
                if !self.symbol(i, b',') {
                    break;
                }
                i += 1;
            }
        }
        let select = self.word(i, "select");
        let insert = self.word(i, "insert");
        let update = self.word(i, "update");
        let delete = self.word(i, "delete");
        let drop = self.word(i, "drop");
        if insert && !self.word(i + 1, "into") {
            return Err(error("0A000", "PG INSERT requires INTO"));
        }
        if self.word(i, "truncate") && !self.word(i + 1, "table") {
            return Err(error("0A000", "use TRUNCATE TABLE"));
        }
        let ddl = self.word(i, "create")
            || self.word(i, "alter")
            || self.word(i, "drop")
            || self.word(i, "truncate");
        let alter = self.word(i, "alter");
        let mut list = false;
        let mut table_ddl = false;
        let mut expect = false;
        while i < end {
            if self.symbol(i, b'(') {
                let close = self.close(i, end)?;
                self.scope(i + 1, close, &ctes)?;
                i = close + 1;
                continue;
            }
            if self.word(i, "select") {
                list = false;
            }
            if [
                "where",
                "group",
                "having",
                "order",
                "limit",
                "union",
                "set",
                "values",
                "returning",
            ]
            .iter()
            .any(|w| self.word(i, w))
            {
                list = false;
            }
            if (select || delete || insert) && self.word(i, "from") || self.word(i, "join") {
                expect = true;
                list = true;
            } else if insert && self.word(i, "into") || update && self.word(i, "update") {
                expect = true;
                list = update;
            } else if ddl
                && (self.word(i, "table") || self.word(i, "view"))
                && (!table_ddl || alter)
            {
                expect = true;
                table_ddl = true;
                list = drop;
            } else if self.word(i, "references") || table_ddl && self.word(i, "like") {
                expect = true;
            } else if alter && table_ddl && self.word(i, "rename") && self.word(i + 1, "to") {
                i += 1;
                expect = true;
            } else if list && self.symbol(i, b',') {
                expect = true;
            }
            if expect {
                i = self.relation(
                    i + 1,
                    end,
                    &ctes,
                    ddl || insert && self.word(i, "into") || self.word(i, "references"),
                )?;
                expect = false;
            } else {
                i += 1;
            }
        }
        Ok(())
    }
}

pub(crate) fn rewrite(sql: &str, database: &str, public: bool) -> ParseResult<String> {
    let tokens = tokens(sql)?;
    let mut edits = BTreeMap::new();
    for token in &tokens {
        if token.quoted {
            edits.insert(
                token.start,
                (token.end, quote(token.name.as_deref().unwrap())),
            );
        }
    }
    let mut resolver = Resolver {
        tokens,
        database,
        public,
        edits,
    };
    resolver.scope(0, resolver.tokens.len(), &HashSet::new())?;
    let mut out = String::with_capacity(sql.len());
    let mut previous = 0;
    for (start, (end, text)) in resolver.edits {
        out.push_str(&sql[previous..start]);
        out.push_str(&text);
        previous = end;
    }
    out.push_str(&sql[previous..]);
    Ok(out)
}

pub(crate) fn adapt(
    sql: &str,
    context: &dyn TiDBContext,
    session: &crate::pg_session::PgSession,
) -> ParseResult<String> {
    // Queries without native relations must not execute an extra engine query.
    match rewrite(sql, "", session.has_public()) {
        Err(("3D000", _)) => {}
        result => return result,
    }
    let current = context
        .execute_query("SELECT DATABASE()", false, &CancellationToken::new())
        .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
    let database = current
        .first()
        .and_then(|r| r.rows.first())
        .and_then(|r| r.first());
    let database = match database {
        Some(Value::Text(name)) => name.as_str(),
        _ => "",
    };
    let sql = rewrite(sql, database, session.has_public())?;
    validate_relations(&sql, database)?;
    Ok(sql)
}

/// Reject foreign schemas on TableName nodes exposed by the native visitor.
/// Token handling remains necessary for nodes not visited by the native AST.
fn validate_relations(sql: &str, database: &str) -> ParseResult<()> {
    use astersql_parser_ast as ast;
    // Validate relation nodes using native markers, without moving syntax errors
    // ahead of the extended protocol's OID checks. The command gate still rejects
    // syntax errors before execution; this guard only checks successfully parsed ASTs.
    let native = crate::pg_extended::markers(sql)
        .map(|(sql, _)| sql)
        .unwrap_or_else(|_| sql.to_owned());
    let Ok((statements, _)) = astersql_parser::New().ParseSQL(&native, &[]) else {
        return Ok(());
    };
    struct Guard<'a> {
        database: &'a str,
        foreign: bool,
    }
    impl ast::Visitor for Guard<'_> {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(table) = node.as_any().downcast_ref::<ast::TableName>() {
                if !table.Schema.L.is_empty() && table.Schema.L != self.database.to_lowercase() {
                    self.foreign = true;
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut guard = Guard {
        database,
        foreign: false,
    };
    for statement in statements {
        statement.accept(&mut guard);
    }
    if guard.foreign {
        return Err(error(
            "0A000",
            "unsupported relation outside the current database",
        ));
    }
    Ok(())
}
