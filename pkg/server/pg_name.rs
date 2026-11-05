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
    source_tokens(sql, false)
}
fn source_tokens(sql: &str, native: bool) -> ParseResult<Vec<Token>> {
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
        if b[i] == b'\'' || b[i] == b'"' || native && b[i] == b'`' {
            let delimiter = b[i];
            quoted = delimiter != b'\'';
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
                        let doubled = format!("{0}{0}", delimiter as char);
                        let n = sql[content..i].replace(&doubled, &(delimiter as char).to_string());
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

/// Native ViewInfo currently stores CREATE VIEW text. Extract its validated
/// SELECT using token spans so AS in quoted names, strings and comments is safe.
/// This native quoting mode is used only for stored source, never PG input.
pub(crate) fn stored_view_select(sql: &str) -> ParseResult<String> {
    let (statements, _) = astersql_parser::New()
        .ParseSQL(sql, &[])
        .map_err(|e| error("0A000", &format!("invalid stored view source: {e}")))?;
    let [statement] = statements.as_slice() else {
        return Err(error("0A000", "invalid stored view source statement count"));
    };
    if statement.as_any().is::<astersql_parser_ast::SelectStmt>()
        || statement.as_any().is::<astersql_parser_ast::SetOprStmt>()
    {
        return Ok(sql.trim().to_owned());
    }
    if !statement
        .as_any()
        .is::<astersql_parser_ast::CreateViewStmt>()
    {
        return Err(error("0A000", "stored view source is not a view SELECT"));
    }
    let tokens = source_tokens(sql, true)?;
    let view = tokens
        .iter()
        .position(|t| t.word("view"))
        .ok_or_else(|| error("0A000", "stored VIEW token is unavailable"))?;
    let start = tokens
        .iter()
        .skip(view + 1)
        .find(|t| t.word("as"))
        .ok_or_else(|| error("0A000", "stored view AS token is unavailable"))?
        .end;
    let mut end = tokens.len();
    if tokens.last().is_some_and(|t| t.symbol == Some(b';')) {
        end -= 1;
    }
    if end >= 4
        && tokens[end - 4].word("with")
        && (tokens[end - 3].word("cascaded") || tokens[end - 3].word("local"))
        && tokens[end - 2].word("check")
        && tokens[end - 1].word("option")
    {
        end -= 4;
    } else if end >= 3
        && tokens[end - 3].word("with")
        && tokens[end - 2].word("check")
        && tokens[end - 1].word("option")
    {
        end -= 3;
    }
    let end = tokens.get(end).map_or(sql.len(), |t| t.start);
    Ok(sql[start..end].trim().to_owned())
}
struct Resolver<'a> {
    tokens: Vec<Token>,
    sql: &'a str,
    database: &'a str,
    public: bool,
    context: Option<&'a dyn TiDBContext>,
    create: bool,
    edits: BTreeMap<usize, (usize, String)>,
    error_position: Option<usize>,
    snapshot: Option<astersql_infoschema::SchemaRef>,
}
impl Resolver<'_> {
    fn word(&self, i: usize, name: &str) -> bool {
        self.tokens.get(i).is_some_and(|t| t.word(name))
    }
    // DISTINCT FROM belongs to the comparison predicate, not a relation list.
    // Token words exclude comments and quoted identifiers.
    fn relation_from(&self, i: usize) -> bool {
        let predicate = i >= 2
            && self.word(i - 1, "distinct")
            && (self.word(i - 2, "is")
                || (i >= 3 && self.word(i - 2, "not") && self.word(i - 3, "is")));
        self.word(i, "from") && !predicate
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
        range_alias: bool,
    ) -> ParseResult<usize> {
        while i < end
            && (self.word(i, "if")
                || self.word(i, "not")
                || self.word(i, "exists")
                || self.word(i, "only")
                || self.word(i, "lateral"))
        {
            i += 1;
        }
        if self.symbol(i, b'(') {
            if !self.word(i + 1, "select")
                && !self.word(i + 1, "with")
                && !self.word(i + 1, "values")
            {
                let close = self.close(i, end)?;
                let bare = self.tokens[i + 1..close]
                    .iter()
                    .all(|token| token.name.is_some() || token.symbol == Some(b'.'));
                if bare {
                    self.error_position =
                        Some(self.sql[..self.tokens[close].start].chars().count() + 1);
                    return Err(error(
                        "42601",
                        "syntax error at or near closing relation parenthesis",
                    ));
                }
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
            // Function signatures are bound by the ordinary session FROM binder,
            // rather than looked up as public relations or rewritten as tables.
            if matches!(parts.as_slice(), [name] if matches!(name.as_str(), "generate_series" | "unnest"))
                || matches!(parts.as_slice(), [schema, name] if schema == "pg_catalog" && matches!(name.as_str(), "generate_series" | "unnest"))
            {
                return Ok(self.close(i, end)? + 1);
            }
            return Err(error("0A000", "native table functions are unsupported"));
        }
        if parts.len() > 3 {
            self.error_position = Some(self.sql[..self.tokens[start].start].chars().count() + 1);
            return Err(error(
                "42601",
                "improper qualified name (too many dotted names)",
            ));
        }
        if parts.len() == 3 && !self.database.is_empty() && parts[0] != self.database {
            self.error_position = Some(self.sql[..self.tokens[start].start].chars().count() + 1);
            return Err(error(
                "0A000",
                "cross-database references are not implemented",
            ));
        }
        let bound = self.context.and_then(|context| {
            let schema = match parts.as_slice() {
                [_] => None,
                [schema, _] => Some(schema.clone()),
                [_, schema, _] => Some(schema.clone()),
                _ => return None,
            };
            // System catalog queries are handled by the catalog provider.
            if schema.as_deref() == Some("pg_catalog") {
                return None;
            }
            let name = parts.last().unwrap().clone();
            let bound = context.pg_bind_relation(schema.clone(), name.clone(), self.create);
            // DROP IF EXISTS still binds the namespace but must not require the
            // relation to exist. Preserve that established utility behavior.
            if self.tokens.first().is_some_and(|t| t.word("drop"))
                && self
                    .tokens
                    .windows(2)
                    .any(|w| w[0].word("if") && w[1].word("exists"))
                && bound.as_ref().is_some_and(|r| {
                    r.as_ref()
                        .is_err_and(|e| crate::pg_conn::sqlstate(e) == "42P01")
                })
            {
                return context.pg_bind_relation(schema, name, true);
            }
            bound
        });
        self.create = false;
        let physical = if let Some(bound) = bound {
            Some(bound.map_err(|e| {
                self.error_position =
                    Some(self.sql[..self.tokens[start].start].chars().count() + 1);
                (crate::pg_conn::sqlstate(&e), e.to_string())
            })?)
        } else {
            match parts.as_slice() {
                [name] => {
                    if !self.public {
                        self.error_position =
                            Some(self.sql[..self.tokens[start].start].chars().count() + 1);
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
            None
        };
        if self.database.is_empty() {
            return Err(error("3D000", "no current database selected"));
        }
        // Earlier visible CTEs took the return above. Resolve physical names
        // exactly in the owning connection's snapshot before native folding,
        // and diagnose missing objects at the original token.
        if physical.is_none() && !allow_columns
            // Catalog relations are owned by the catalog parser/provider,
            // including invalid-parameter diagnostics before native fallback.
            && !(parts.len() == 1 && crate::pg_catalog_query::is_catalog_relation(&parts[0]))
            && let Some(snapshot) = &self.snapshot
            && snapshot
                .ModelTableInfoByExactName(
                    &astersql_infoschema::CiString::new(self.database),
                    parts.last().unwrap(),
                )
                .is_err()
        {
            self.error_position = Some(self.sql[..self.tokens[start].start].chars().count() + 1);
            return Err(error("42P01", "relation does not exist"));
        }
        let first = self.tokens[start].start;
        let last = self.tokens[i - 1].end;
        // Remove individual identifier edits swallowed by the relation span.
        self.edits
            .retain(|offset, _| *offset < first || *offset >= last);
        let alias = if range_alias
            && physical
                .as_ref()
                .is_some_and(|name| name != parts.last().unwrap())
            && !self.word(i, "as")
            && !self.tokens.get(i).is_some_and(|token| {
                token.name.is_some()
                    && ![
                        "where",
                        "join",
                        "inner",
                        "left",
                        "right",
                        "full",
                        "cross",
                        "on",
                        "using",
                        "order",
                        "group",
                        "having",
                        "limit",
                        "offset",
                        "union",
                        "for",
                        "returning",
                        "set",
                    ]
                    .iter()
                    .any(|word| token.word(word))
            }) {
            format!(" AS {}", quote(parts.last().unwrap()))
        } else {
            String::new()
        };
        self.edits.insert(
            first,
            // Keep the table segment's PG quotes. Re-quoting with backticks
            // destroys its source identity before the shared parser sees it.
            (
                last,
                format!(
                    "{}.{}{}",
                    quote(self.database),
                    &physical.as_ref().map_or_else(
                        || self.sql[self.tokens[i - 1].start..last].to_owned(),
                        |name| quote(name)
                    ),
                    alias,
                ),
            ),
        );
        Ok(i)
    }
    fn scope(&mut self, mut i: usize, end: usize, inherited: &HashSet<String>) -> ParseResult<()> {
        let mut ctes = inherited.clone();
        let with_start = i;
        if self.word(i, "with") {
            i += 1;
            let recursive = self.word(i, "recursive");
            if recursive {
                i += 1;
            }
            // Duplicate declarations belong to this WITH list, independently
            // of inherited names that may legally be shadowed. Check the list
            // before resolving bodies, as PostgreSQL does during CTE analysis.
            let mut declared = HashSet::new();
            let mut definitions = Vec::new();
            loop {
                let name = self
                    .tokens
                    .get(i)
                    .and_then(|t| t.name.clone())
                    .ok_or_else(|| error("42601", "expected CTE name"))?;
                if !declared.insert(name.clone()) {
                    self.error_position =
                        Some(self.sql[..self.tokens[i].start].chars().count() + 1);
                    return Err(error(
                        "42712",
                        &format!("WITH query name \"{name}\" specified more than once"),
                    ));
                }
                i += 1;
                if self.symbol(i, b'(') {
                    i = self.close(i, end)? + 1;
                }
                if self.word(i, "as")
                    && (self.word(i + 1, "materialized")
                        || self.word(i + 1, "not") && self.word(i + 2, "materialized"))
                {
                    return Err(error(
                        "0A000",
                        "explicit CTE materialization is unsupported",
                    ));
                }
                if !self.word(i, "as") || !self.symbol(i + 1, b'(') {
                    return Err(error("42601", "expected CTE query"));
                }
                i += 1;
                let close = self.close(i, end)?;
                definitions.push((name, i + 1, close));
                i = close + 1;
                if !self.symbol(i, b',') {
                    break;
                }
                i += 1;
            }
            if self.word(i, "search") || self.word(i, "cycle") {
                return Err(error("0A000", "CTE SEARCH/CYCLE is unsupported"));
            }
            if with_start != 0
                && definitions.iter().any(|(_, start, _)| {
                    ["insert", "update", "delete"]
                        .iter()
                        .any(|word| self.word(*start, word))
                })
            {
                self.error_position =
                    Some(self.sql[..self.tokens[with_start].start].chars().count() + 1);
                return Err(error(
                    "0A000",
                    "WITH clause containing a data-modifying statement must be at the top level",
                ));
            }
            if recursive {
                // Classify the direct DELETE self-reference before the general
                // recursive capability gate. A qualified name is a real table,
                // and the DELETE target itself is not a recursive CTE read.
                for (name, start, close) in &definitions {
                    if !self.word(*start, "delete") {
                        continue;
                    }
                    let mut next = *start + 2;
                    while next < *close {
                        let open = next;
                        next += 1;
                        if !self.symbol(open, b'(') {
                            continue;
                        }
                        let end = self.close(open, *close)?;
                        next = end + 1;
                        // Only a direct SELECT subquery is classified here;
                        // nested WITH scopes stay behind the capability gate.
                        if !self.symbol(open, b'(') || !self.word(open + 1, "select") {
                            continue;
                        }
                        if (open + 1..end).any(|token| self.symbol(token, b'(')) {
                            continue;
                        }
                        for cursor in open + 1..end {
                            if (self.relation_from(cursor) || self.word(cursor, "join"))
                                && self.tokens.get(cursor + 1).and_then(|t| t.name.as_ref())
                                    == Some(name)
                                && !self.symbol(cursor + 2, b'.')
                                && !self.symbol(cursor + 2, b'(')
                            {
                                self.error_position =
                                    Some(self.sql[..self.tokens[*start].start].chars().count() + 1);
                                return Err(error(
                                    "42P19",
                                    &format!(
                                        "recursive query \"{name}\" must not contain data-modifying statements"
                                    ),
                                ));
                            }
                        }
                    }
                }
                // Reject a direct seed-term self reference before the capability
                // gate. Only a top-level UNION seed is classified here: nested
                // query scopes need their own binder and remain behind the gate.
                for (name, start, close) in &definitions {
                    if !self.word(*start, "select") {
                        continue;
                    }
                    let mut cursor = *start;
                    let mut union = None;
                    while cursor < *close {
                        if self.symbol(cursor, b'(') {
                            cursor = self.close(cursor, *close)? + 1;
                            continue;
                        }
                        if self.word(cursor, "union") {
                            union = Some(cursor);
                            break;
                        }
                        cursor += 1;
                    }
                    if let Some(union) = union {
                        cursor = *start;
                        while cursor < union {
                            if self.symbol(cursor, b'(') {
                                cursor = self.close(cursor, union)? + 1;
                                continue;
                            }
                            if (self.relation_from(cursor) || self.word(cursor, "join"))
                                && self.tokens.get(cursor + 1).and_then(|t| t.name.as_ref())
                                    == Some(name)
                                && !self.symbol(cursor + 2, b'.')
                                && !self.symbol(cursor + 2, b'(')
                            {
                                self.error_position = Some(
                                    self.sql[..self.tokens[cursor + 1].start].chars().count() + 1,
                                );
                                return Err(error(
                                    "42P19",
                                    "recursive reference must not appear within its non-recursive term",
                                ));
                            }
                            cursor += 1;
                        }
                    }
                }
                return Err(error("0A000", "recursive PG CTEs are unsupported"));
            }
            for (name, start, close) in definitions {
                for token in start..close {
                    if self.word(token, "for")
                        && ["update", "share", "no", "key"]
                            .iter()
                            .any(|word| self.word(token + 1, word))
                    {
                        return Err(error("0A000", "row locking in PG CTEs is unsupported"));
                    }
                    if self.symbol(token + 1, b'(')
                        && [
                            "random",
                            "nextval",
                            "setval",
                            "currval",
                            "pg_sleep",
                            "clock_timestamp",
                            "timeofday",
                        ]
                        .iter()
                        .any(|word| self.word(token, word))
                    {
                        return Err(error(
                            "0A000",
                            "volatile PG CTE expressions are unsupported",
                        ));
                    }
                }
                self.scope(start, close, &ctes)?;
                ctes.insert(name);
            }
        }
        // A view's SELECT sources share the ordinary query namespace boundary.
        // Its declaration is still handled separately as a DDL target below.
        let select = self.word(i, "select")
            || self.word(i, "create")
                && (self.word(i + 1, "view")
                    || self.word(i + 1, "or")
                        && self.word(i + 2, "replace")
                        && self.word(i + 3, "view"));
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
        let view_rename = alter && self.word(i + 1, "view");
        let create_index = self.word(i, "create")
            && (self.word(i + 1, "index")
                || self.word(i + 1, "unique") && self.word(i + 2, "index"));
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
            if (select || delete || insert) && self.relation_from(i) || self.word(i, "join") {
                expect = true;
                list = true;
            } else if delete && self.word(i, "using") {
                // DELETE USING introduces another relation list, distinct from
                // the target following FROM. Resolve its names at the same
                // PostgreSQL relation boundary before the pre-write executor gate.
                expect = true;
                list = true;
            } else if insert && self.word(i, "into") || update && self.word(i, "update") {
                expect = true;
                list = update;
            } else if create_index && self.word(i, "on") {
                // PG indexes share their table's namespace with all relations.
                // Persist the declared name unchanged; never synthesize suffixes
                // for native table-local collisions.
                if let (Some(context), Some(snapshot)) = (self.context, &self.snapshot) {
                    let index_token = if self.word(1, "unique") { 2 } else { 1 };
                    let name_token = if self.word(index_token + 1, "if") {
                        index_token + 4
                    } else {
                        index_token + 1
                    };
                    if let Some(name) = self.tokens.get(name_token).and_then(|t| t.name.as_ref()) {
                        let first = self.tokens.get(i + 1).and_then(|t| t.name.clone());
                        let (schema, table) = if self.symbol(i + 2, b'.') {
                            (first, self.tokens.get(i + 3).and_then(|t| t.name.clone()))
                        } else {
                            (None, first)
                        };
                        if let Some(table) = table {
                            if let Some(bound) = context.pg_bind_relation(schema, table, false) {
                                let physical = bound
                                    .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
                                let database = astersql_infoschema::CiString::new(self.database);
                                let target = snapshot
                                    .ModelTableInfoByExactName(&database, &physical)
                                    .map_err(|e| ("42P01", e.to_string()))?;
                                let namespace =
                                    target.PostgreSQLNamespace.as_ref().map(|(id, _)| *id);
                                for table in snapshot
                                    .SchemaTableInfos(&database)
                                    .map_err(|e| ("0A000", e.to_string()))?
                                {
                                    let model = table.model_meta.as_ref().ok_or_else(|| {
                                        error("0A000", "relation metadata unavailable")
                                    })?;
                                    if model.State != astersql_meta_model::StatePublic
                                        || model.PostgreSQLNamespace.as_ref().map(|(id, _)| *id)
                                            != namespace
                                    {
                                        continue;
                                    }
                                    let relation = model
                                        .PostgreSQLNamespace
                                        .as_ref()
                                        .map_or(model.Name.O.as_str(), |(_, name)| name.as_str());
                                    if relation == name
                                        || crate::pg_catalog::catalog_indexes(model)
                                            .map_err(|e| {
                                                (crate::pg_conn::sqlstate(&e), e.to_string())
                                            })?
                                            .iter()
                                            .any(|idx| {
                                                idx.State == astersql_meta_model::StatePublic
                                                    && idx.Name.O == *name
                                            })
                                    {
                                        return Err(error("42P07", "relation already exists"));
                                    }
                                }
                            }
                        }
                    }
                }
                // The index target is a relation reference, using the same
                // public/current-database boundary as other PG DDL targets.
                expect = true;
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
                // A view rename keeps its existing namespace. Its bare new name
                // must not be bound in search_path's creation schema.
                if view_rename {
                    if let Some(token) = self.tokens.get(i + 2) {
                        if let Some(name) = &token.name {
                            self.edits.insert(
                                token.start,
                                (token.end, format!("`{}`", name.replace('`', "``"))),
                            );
                        }
                    }
                    i += 3;
                    continue;
                }
                // RENAME's destination is a declaration, never an existing relation.
                self.create = true;
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
                    select
                        && (self.relation_from(i) || self.word(i, "join") || self.symbol(i, b',')),
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
    rewrite_with_position(sql, database, public, &mut None)
}

fn rewrite_with_position(
    sql: &str,
    database: &str,
    public: bool,
    position: &mut Option<usize>,
) -> ParseResult<String> {
    rewrite_with_snapshot(sql, database, public, position, None)
}

fn rewrite_with_snapshot(
    sql: &str,
    database: &str,
    public: bool,
    position: &mut Option<usize>,
    snapshot: Option<astersql_infoschema::SchemaRef>,
) -> ParseResult<String> {
    rewrite_with_context(sql, database, public, position, snapshot, None)
}

fn rewrite_with_context<'a>(
    sql: &'a str,
    database: &'a str,
    public: bool,
    position: &mut Option<usize>,
    snapshot: Option<astersql_infoschema::SchemaRef>,
    context: Option<&'a dyn TiDBContext>,
) -> ParseResult<String> {
    *position = None;
    let allowed_aliases = alias_scope(sql, snapshot.clone(), database, position)?;
    let parts: BTreeMap<_, _> = identifier_parts(sql, &allowed_aliases)?
        .into_iter()
        .map(|part| (part.Span, part))
        .collect();
    let mut tokens = tokens(sql)?;
    // Join semantic identities back to source tokens by their original spans.
    // This consumes the AST metadata before relation/schema adaptation, without
    // guessing identifiers inside strings or comments.
    let mut edits = BTreeMap::new();
    for token in &mut tokens {
        if let Some(part) = parts.get(&(token.start, token.end)) {
            token.name = Some(part.Lookup.clone());
            token.quoted = part.Quoted;
            // The native parser keeps display spelling even for unquoted names.
            // Fold only parser-proven identifiers, never strings or comments.
            if !part.Quoted && sql[token.start..token.end] != part.Lookup {
                edits.insert(token.start, (token.end, part.Lookup.clone()));
            }
        }
    }
    let mut resolver = Resolver {
        snapshot,
        context,
        create: tokens.first().is_some_and(|t| t.word("create"))
            && tokens
                .iter()
                .take(4)
                .any(|t| t.word("table") || t.word("view")),
        tokens,
        sql,
        database,
        public,
        edits,
        error_position: None,
    };
    let resolved = resolver.scope(0, resolver.tokens.len(), &HashSet::new());
    *position = resolver.error_position;
    resolved?;
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

/// Consume the formal parser's original per-segment identity at the normal PG
/// name boundary. Until exact object binding is available, never silently bind
/// a quoted mixed-case reference through the backend's lowercase catalog.
fn identifier_parts(
    sql: &str,
    aliases: &HashSet<(usize, usize)>,
) -> ParseResult<Vec<astersql_parser_ast::PostgreSQLIdentifier>> {
    use astersql_parser_ast as ast;
    let Ok((statements, _)) =
        astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL).ParseSQL(sql, &[])
    else {
        return Ok(Vec::new()); // Lexical diagnostics remain owned by their existing path.
    };
    struct Gate {
        blocked: bool,
        parts: Vec<ast::PostgreSQLIdentifier>,
        declarations: HashSet<(usize, usize)>,
        exact_relations: bool,
        exact_columns: bool,
        tokens: Vec<Token>,
    }
    impl Gate {
        fn consume(&mut self, parts: &[ast::PostgreSQLIdentifier]) {
            self.blocked |= parts.iter().any(|part| {
                !self.declarations.contains(&part.Span)
                    && part.Quoted
                    && part.Lookup.bytes().any(|byte| byte.is_ascii_uppercase())
            });
            self.parts.extend_from_slice(parts);
        }
    }
    impl ast::Visitor for Gate {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            let fields = if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
                select.Fields.Fields.as_slice()
            } else if let Some(insert) = node.as_any().downcast_ref::<ast::InsertStmt>() {
                insert.Returning.as_slice()
            } else if let Some(update) = node.as_any().downcast_ref::<ast::UpdateStmt>() {
                update.Returning.as_slice()
            } else {
                &[]
            };
            for field in fields.iter().filter(|field| !field.AsName.O.is_empty()) {
                // OriginalText includes the projection's alias. Its last token
                // is the declaration, including the bare-alias form without AS.
                if let Some(token) = self.tokens.iter().rev().find(|token| {
                    token.start >= field.Offset
                        && token.end <= field.Offset + field.OriginalText.len()
                }) && !token.quoted
                    && token.name.as_deref() == Some(field.AsName.L.as_str())
                {
                    self.parts.push(ast::PostgreSQLIdentifier {
                        Original: field.AsName.O.clone(),
                        Decoded: field.AsName.O.clone(),
                        Lookup: field.AsName.L.clone(),
                        Quoted: false,
                        Span: (token.start, token.end),
                    });
                }
            }
            false
        }
        fn enter_column_name(&mut self, column: &ast::ColumnName) -> bool {
            // These consumers bind PostgreSQLParts.Lookup at the owning shared
            // session parse boundary, including write targets and declarations.
            // Other statements keep their existing capability gate.
            if self.exact_columns {
                self.parts.extend_from_slice(&column.PostgreSQLParts);
            } else {
                self.consume(&column.PostgreSQLParts);
            }
            false
        }
        fn enter_table_name(&mut self, table: &ast::TableName) -> bool {
            if !self.exact_relations {
                self.consume(&table.PostgreSQLParts);
                return false;
            }
            if table.PostgreSQLParts.len() == 3 {
                // Database identity is checked exactly by Resolver::relation,
                // independently of the backend's case-insensitive catalog.
                self.parts.push(table.PostgreSQLParts[0].clone());
                self.parts.extend_from_slice(&table.PostgreSQLParts[1..]);
            } else {
                self.parts.extend_from_slice(&table.PostgreSQLParts);
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut gate = Gate {
        blocked: false,
        parts: Vec::new(),
        declarations: aliases.clone(),
        exact_relations: false,
        exact_columns: false,
        tokens: tokens(sql)?,
    };
    for statement in statements {
        if let Some(create) = statement.as_any().downcast_ref::<ast::CreateTableStmt>()
            && create.TemporaryKeyword != ast::TemporaryKeyword::None
            && create
                .Table
                .PostgreSQLParts
                .iter()
                .any(|p| p.Quoted && p.Lookup.chars().any(|ch| ch.is_ascii_uppercase()))
        {
            return Err(error(
                "0A000",
                "case-distinct temporary relations require exact temporary namespace binding",
            ));
        }
        // Release the gate only for consumers that preserve physical identity.
        // Other DDL/DML operations retain their previous capability boundary.
        // CREATE INDEX targets pass through Resolver::relation's exact shared
        // model lookup too; retaining the old gate rejects quoted table names
        // in definitions returned by pg_get_indexdef.
        gate.exact_relations = statement.as_any().is::<ast::CreateIndexStmt>()
            || statement.as_any().is::<ast::SelectStmt>()
            || statement.as_any().is::<ast::SetOprStmt>()
            || statement.as_any().is::<ast::InsertStmt>()
            || statement.as_any().is::<ast::DropTableStmt>()
            || statement.as_any().is::<ast::UpdateStmt>();
        gate.exact_columns = statement.as_any().is::<ast::SelectStmt>()
            || statement.as_any().is::<ast::SetOprStmt>()
            || statement.as_any().is::<ast::InsertStmt>()
            || statement.as_any().is::<ast::UpdateStmt>()
            || statement.as_any().is::<ast::CreateTableStmt>();
        // Declarations are not references; CREATE TABLE persists their decoded
        // spelling and matching policy in the normal shared catalog.
        let declaration =
            if let Some(create) = statement.as_any().downcast_ref::<ast::CreateTableStmt>() {
                Some(&create.Table)
            } else if let Some(create) = statement.as_any().downcast_ref::<ast::CreateViewStmt>() {
                Some(&create.ViewName)
            } else {
                statement
                    .as_any()
                    .downcast_ref::<ast::CreateSequenceStmt>()
                    .map(|create| &create.Name)
            };
        gate.declarations = aliases
            .iter()
            .copied()
            .chain(
                declaration
                    .into_iter()
                    .flat_map(|table| &table.PostgreSQLParts)
                    .map(|part| part.Span),
            )
            .collect();
        statement.accept(&mut gate);
    }
    if gate.blocked {
        Err(error(
            "0A000",
            "quoted mixed-case identifiers require exact name binding support",
        ))
    } else {
        Ok(gate.parts)
    }
}

/// Bind qualified column references to their query's visible range names before
/// the case-insensitive native binder runs. Catalog facts come from the same
/// connection snapshot; this does not execute a projection to discover columns.
fn alias_scope(
    sql: &str,
    snapshot: Option<astersql_infoschema::SchemaRef>,
    database: &str,
    position: &mut Option<usize>,
) -> ParseResult<HashSet<(usize, usize)>> {
    use astersql_parser_ast::{self as ast, Node};
    let Ok((statements, _)) =
        astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL).ParseSQL(sql, &[])
    else {
        return Ok(HashSet::new());
    };
    #[derive(Clone)]
    struct Binding {
        name: String,
        columns: Option<HashSet<String>>,
        exact_range: bool,
    }
    struct Scope<'a> {
        tokens: Vec<Token>,
        snapshot: Option<astersql_infoschema::SchemaRef>,
        database: &'a str,
        scopes: Vec<Vec<Binding>>,
        ctes: Vec<BTreeMap<String, Option<HashSet<String>>>>,
        owned_with: HashSet<usize>,
        allowed: HashSet<(usize, usize)>,
        failure: Option<(&'static str, &'static str, usize)>,
    }
    impl Scope<'_> {
        fn output_names(node: &dyn ast::Node) -> Option<Vec<String>> {
            if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
                return select
                    .Fields
                    .Fields
                    .iter()
                    .map(|field| {
                        if !field.AsName.O.is_empty() {
                            Some(field.AsName.O.clone())
                        } else if let Some(ast::ExprNode {
                            Kind: ast::ExprKind::Column(column),
                            ..
                        }) = &field.Expr
                        {
                            Some(column.Name.O.clone())
                        } else {
                            // Unknown projections stay with the shared binder; do
                            // not infer column names from a sample execution.
                            None
                        }
                    })
                    .collect();
            }
            None
        }

        fn sources(&mut self, node: &ast::ResultSetNode, bindings: &mut Vec<Binding>) {
            match node {
                ast::ResultSetNode::Join(join) => {
                    for source in [join.Left.as_deref(), join.Right.as_deref()]
                        .into_iter()
                        .flatten()
                    {
                        self.sources(source, bindings);
                    }
                }
                ast::ResultSetNode::TableSource(source) => {
                    let parts = &source.Source.PostgreSQLParts;
                    let function_name = source.FunctionSource.as_ref().and_then(|call| match &call
                        .Kind
                    {
                        ast::ExprKind::Function { FnName, .. } => Some(FnName),
                        _ => None,
                    });
                    let name = if source.AsName.O.is_empty() && function_name.is_some() {
                        function_name.expect("function name").L.clone()
                    } else if source.AsName.O.is_empty() {
                        parts
                            .last()
                            .map(|p| p.Lookup.clone())
                            .unwrap_or_else(|| source.Source.Name.L.clone())
                    } else {
                        // Alias syntax has no AST span yet. Locate only the
                        // declaration immediately after this AST relation span;
                        // literals/comments are already excluded by the lexer.
                        let end = parts.last().map(|part| part.Span.1);
                        let alias = end.and_then(|end| {
                            self.tokens
                                .iter()
                                .filter(|t| t.start >= end)
                                .find(|t| !t.word("AS"))
                        });
                        alias
                            .and_then(|t| t.name.clone())
                            .unwrap_or_else(|| source.AsName.L.clone())
                    };
                    let cte = (parts.len() == 1)
                        .then(|| {
                            self.ctes
                                .iter()
                                .rev()
                                .find_map(|scope| scope.get(&parts[0].Lookup))
                        })
                        .flatten();
                    let columns = if let Some(columns) = cte {
                        columns.clone()
                    } else if source.QuerySource.is_none()
                        && (parts.len() == 1
                            || parts
                                .get(parts.len().saturating_sub(2))
                                .is_some_and(|p| p.Lookup == "public"))
                    {
                        self.snapshot
                            .as_ref()
                            .and_then(|schema| {
                                schema
                                    .ModelTableInfoByExactName(
                                        &astersql_infoschema::CiString::new(self.database),
                                        parts.last().map_or(source.Source.Name.L.as_str(), |p| {
                                            p.Lookup.as_str()
                                        }),
                                    )
                                    .ok()
                            })
                            .map(|table| {
                                table
                                    .Columns
                                    .iter()
                                    .filter(|c| !c.Hidden)
                                    .map(|c| c.Name.O.clone())
                                    .collect()
                            })
                    } else {
                        None
                    };
                    // Native range keys fold case. Refuse the one shape that
                    // would merge two distinct PG aliases before native binding.
                    let exact_range = source.AsName.L.is_empty() && source.Source.Name.L == name;
                    if bindings.iter().any(|binding| {
                        binding.name != name
                            && binding.name.to_lowercase() == name.to_lowercase()
                            && !(binding.exact_range && exact_range)
                    }) {
                        self.failure = Some((
                            "0A000",
                            "case-distinct aliases require exact range binding",
                            parts.last().map_or(0, |part| part.Span.0),
                        ));
                    }
                    bindings.push(Binding {
                        name,
                        columns,
                        exact_range,
                    });
                }
            }
        }
    }
    impl ast::Visitor for Scope<'_> {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            // SELECT owns WITH traversal below, so each definition is checked
            // before becoming visible to later definitions and the body.
            if let Some(with) = node.as_any().downcast_ref::<ast::WithClause>()
                && self
                    .owned_with
                    .contains(&(with as *const ast::WithClause as usize))
            {
                return true;
            }
            if let Some(insert) = node.as_any().downcast_ref::<ast::InsertStmt>() {
                // INSERT values cannot see either row. RETURNING sees only the
                // target. The proposed row exists exclusively in DO UPDATE's
                // RHS and predicate; it is never a physical relation/schema.
                self.scopes.push(Vec::new());
                for row in &insert.Lists {
                    for expr in row {
                        expr.accept(self);
                    }
                }
                if let Some(query) = &insert.Select {
                    query.accept(self);
                }
                self.scopes.pop();
                let mut target = Vec::new();
                if let Some(table) = &insert.Table {
                    for source in [
                        table.TableRefs.Left.as_deref(),
                        table.TableRefs.Right.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        self.sources(source, &mut target);
                    }
                }
                self.scopes.push(target.clone());
                for field in &insert.Returning {
                    if let Some(expr) = &field.Expr {
                        expr.accept(self);
                    }
                }
                if let Some(conflict) = &insert.PostgreSQLConflict {
                    if let Some(predicate) = &conflict.Predicate {
                        predicate.accept(self);
                    }
                    if conflict.Action == ast::PostgreSQLConflictAction::DoUpdate {
                        if let Some(binding) = target.first() {
                            self.scopes.last_mut().unwrap().push(Binding {
                                name: "excluded".into(),
                                exact_range: false,
                                columns: binding.columns.clone(),
                            });
                        }
                        for assignment in &conflict.UpdateAssignments {
                            assignment.Expr.accept(self);
                        }
                        if let Some(predicate) = &conflict.UpdatePredicate {
                            predicate.accept(self);
                        }
                    }
                }
                self.scopes.pop();
                return true;
            }
            if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
                self.ctes.push(BTreeMap::new());
                if let Some(with) = &select.With {
                    self.owned_with
                        .insert(&*with.borrow() as *const ast::WithClause as usize);
                    for cte in &with.borrow().CTEs {
                        cte.Query.accept(self);
                        let columns = Self::output_names(cte.Query.as_ref()).map(|mut names| {
                            for (name, alias) in names.iter_mut().zip(&cte.ColNameList) {
                                *name = alias.O.clone();
                            }
                            names.into_iter().collect()
                        });
                        self.ctes
                            .last_mut()
                            .unwrap()
                            .insert(cte.Name.O.clone(), columns);
                    }
                }
                // Only bare ORDER BY output references can bypass input-column
                // lookup. The source span cannot authorize WHERE references.
                for item in &select.OrderBy {
                    if let ast::ExprKind::Column(column) = &item.Expr.Kind
                        && let [part] = column.PostgreSQLParts.as_slice()
                        && select.Fields.Fields.iter().any(|field| {
                            !field.AsName.O.is_empty() && field.AsName.O == part.Lookup
                        })
                    {
                        self.allowed.insert(part.Span);
                    }
                }
                let mut bindings = Vec::new();
                if let Some(from) = &select.From {
                    for source in [
                        from.TableRefs.Left.as_deref(),
                        from.TableRefs.Right.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        self.sources(source, &mut bindings);
                    }
                }
                self.scopes.push(bindings);
            }
            if self.failure.is_none()
                && let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>()
                && let ast::ExprKind::Column(column) = &expr.Kind
                && column.PostgreSQLParts.len() == 1
                && !self.allowed.contains(&column.PostgreSQLParts[0].Span)
                && let Some(bindings) = self.scopes.last()
                && (!bindings.is_empty()
                    || (self.scopes.len() == 1 && column.PostgreSQLParts[0].Quoted))
                && bindings.iter().all(|b| b.columns.is_some())
            {
                let part = &column.PostgreSQLParts[0];
                let matches = bindings
                    .iter()
                    .filter(|b| {
                        b.columns
                            .as_ref()
                            .is_some_and(|cols| cols.contains(&part.Lookup))
                    })
                    .count();
                if matches > 1 {
                    self.failure = Some(("42702", "column reference is ambiguous", part.Span.0));
                } else if matches == 1 {
                    self.allowed.insert(part.Span);
                } else {
                    self.failure = Some((
                        "42703",
                        "column does not exist in the visible relation",
                        part.Span.0,
                    ));
                }
            }
            if self.failure.is_none()
                && let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>()
                && let ast::ExprKind::Column(column) = &expr.Kind
                && column.PostgreSQLParts.len() == 2
                && !self.scopes.is_empty()
            {
                let qualifier = &column.PostgreSQLParts[0];
                let visible = self
                    .scopes
                    .iter()
                    .rev()
                    .find(|scope| scope.iter().any(|binding| binding.name == qualifier.Lookup));
                let target = visible.and_then(|scope| {
                    scope
                        .iter()
                        .find(|binding| binding.name == qualifier.Lookup)
                });
                if visible.is_some_and(|scope| {
                    scope
                        .iter()
                        .filter(|binding| binding.name == qualifier.Lookup)
                        .count()
                        > 1
                }) {
                    self.failure =
                        Some(("42P09", "table reference is ambiguous", qualifier.Span.0));
                    return false;
                }
                if let Some(binding) = target {
                    if binding
                        .columns
                        .as_ref()
                        .is_some_and(|columns| !columns.contains(&column.PostgreSQLParts[1].Lookup))
                    {
                        self.failure = Some((
                            "42703",
                            "column does not exist in the visible relation",
                            qualifier.Span.0,
                        ));
                    } else {
                        // Only names validated against this visible binding may
                        // bypass the old mixed-case capability gate.
                        self.allowed.insert(qualifier.Span);
                        if binding.columns.is_some() {
                            self.allowed.insert(column.PostgreSQLParts[1].Span);
                        }
                    }
                } else {
                    self.failure = Some((
                        "42P01",
                        "missing FROM-clause entry for relation",
                        qualifier.Span.0,
                    ));
                }
            }
            false
        }
        fn leave(&mut self, node: &dyn ast::Node) -> bool {
            if node.as_any().is::<ast::SelectStmt>() {
                self.scopes.pop();
                self.ctes.pop();
            }
            true
        }
    }
    let mut scope = Scope {
        tokens: tokens(sql)?,
        snapshot,
        database,
        scopes: Vec::new(),
        ctes: Vec::new(),
        owned_with: HashSet::new(),
        allowed: HashSet::new(),
        failure: None,
    };
    for statement in statements {
        statement.accept(&mut scope);
    }
    if let Some((state, message, byte)) = scope.failure {
        *position = Some(sql[..byte].chars().count() + 1);
        return Err(error(state, message));
    }
    Ok(scope.allowed)
}

/// The native index model has no PG ordering/INCLUDE facts. Reject these
/// declarations before parsing/adaptation can discard them or write metadata.
pub(crate) fn check_index_options(sql: &str) -> ParseResult<()> {
    // Command classification also sees internally adapted native identifiers.
    let parts = source_tokens(sql, true)?;
    // No PG clustered storage or replica-identity subsystem is exposed.
    if parts.first().is_some_and(|p| p.word("cluster"))
        || parts.first().is_some_and(|p| p.word("alter"))
            && parts.get(1).is_some_and(|p| p.word("table"))
            && parts
                .windows(2)
                .any(|p| p[0].word("replica") && p[1].word("identity"))
    {
        return Err(error(
            "0A000",
            "PostgreSQL clustered storage and replica identity are unsupported",
        ));
    }
    let index = parts.first().is_some_and(|p| p.word("create"))
        && (parts.get(1).is_some_and(|p| p.word("index"))
            || parts.get(1).is_some_and(|p| p.word("unique"))
                && parts.get(2).is_some_and(|p| p.word("index")));
    if !index {
        return Ok(());
    }
    let index_token = if parts[1].word("unique") { 2 } else { 1 };
    if parts
        .get(index_token + 1)
        .is_some_and(|p| p.word("concurrently"))
    {
        return Err(error(
            "0A000",
            "PostgreSQL concurrent index construction is unsupported",
        ));
    }
    let Some(on) = parts.iter().position(|p| p.word("on")) else {
        return Ok(());
    };
    let Some(open) = parts
        .iter()
        .enumerate()
        .skip(on + 1)
        .find_map(|(i, p)| (p.symbol == Some(b'(')).then_some(i))
    else {
        return Ok(());
    };
    if let Some(method) = parts[on + 1..open].iter().position(|p| p.word("using")) {
        let method = on + 1 + method + 1;
        if !parts.get(method).is_some_and(|p| p.word("btree")) {
            return Err(error(
                "0A000",
                "PostgreSQL index method has no native equivalent",
            ));
        }
    }
    let mut depth = 0usize;
    for (i, part) in parts.iter().enumerate().skip(open) {
        match part.symbol {
            Some(b'(') => depth += 1,
            Some(b')') => depth = depth.saturating_sub(1),
            _ => {}
        }
        let ordering = depth == 1
            && (part.word("desc")
                && parts
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| p.name.is_some())
                || part.word("nulls")
                    && parts
                        .get(i + 1)
                        .is_some_and(|p| p.word("first") || p.word("last")));
        let include = depth == 0
            && part.word("include")
            && parts.get(i + 1).is_some_and(|p| p.symbol == Some(b'('));
        let null_policy = depth == 0
            && part.word("nulls")
            && parts.get(i + 1).is_some_and(|p| p.word("not"))
            && parts.get(i + 2).is_some_and(|p| p.word("distinct"));
        if ordering || include || null_policy {
            return Err(error(
                "0A000",
                "PostgreSQL index ordering, INCLUDE and NULL uniqueness require stored index facts",
            ));
        }
    }
    Ok(())
}

pub(crate) fn adapt(
    sql: &str,
    context: &dyn TiDBContext,
    session: &mut crate::pg_session::PgSession,
) -> ParseResult<String> {
    crate::pg_sql::check_returning_aliases(sql)?;
    check_index_options(sql)?;
    // Native physical partitions have no independent PG relation identities.
    // Gate valid PG partition DDL before the native parser can misdiagnose it
    // as malformed MySQL partition syntax or create partial metadata.
    let parts = tokens(sql)?;
    if parts
        .first()
        .is_some_and(|t| t.word("create") || t.word("alter"))
        && parts.iter().take(4).any(|t| t.word("table"))
    {
        let mut depth = 0usize;
        for (index, part) in parts.iter().enumerate() {
            match part.symbol {
                Some(b'(') => depth += 1,
                Some(b')') => depth = depth.saturating_sub(1),
                _ => {}
            }
            if depth == 0
                && part.word("partition")
                && parts
                    .get(index + 1)
                    .is_some_and(|t| t.word("by") || t.word("of"))
            {
                return Err(error(
                    "0A000",
                    "PostgreSQL partitioned relations are unsupported",
                ));
            }
        }
    }
    let path = session
        .effective_explicit_path(context)
        .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
    // Reuse the existing VALUES-to-ROW source map before parsing. The AST
    // retains original semantic structure; only inserted ROW bytes are removed
    // when converting diagnostics back to the client's query.
    let (parse_sql, insertions) = crate::pg_sql::values_parameter_source(sql)?;
    let original_offset = |offset: usize| {
        offset.saturating_sub(
            insertions
                .iter()
                .enumerate()
                .filter(|(index, start)| **start + 4 * index < offset)
                .count()
                * 4,
        )
    };
    let parsed = astersql_parser::Parser::with_dialect(astersql_parser_ast::SqlDialect::PostgreSQL)
        .ParseSQL(&parse_sql, &[]);
    if let Err(error) = &parsed
        && error
            .downcast_ref::<astersql_parser::PostgreSQLCapabilityError>()
            .is_some()
    {
        return Err(("0A000", error.to_string()));
    }
    if let Err(error) = &parsed
        && let Some(window) = error.downcast_ref::<astersql_parser::PostgreSQLWindowError>()
    {
        session.relation_error_position = sql
            .get(..original_offset(window.offset))
            .map(|prefix| prefix.chars().count() + 1);
        return Err((window.state, window.message.clone()));
    }
    if let Ok((statements, _)) = parsed {
        if let Err(error) =
            astersql_session::runtime::ValidatePostgreSQLExclusiveFunctions(&statements)
            && let Some(function) = error.pg_function_error()
        {
            session.function_error_hint = function.hint;
            session.error_detail = function.detail;
            session.relation_error_position = function.position.and_then(|offset| {
                sql.get(..original_offset(offset))
                    .map(|prefix| prefix.chars().count() + 1)
            });
            return Err((function.state, function.message));
        }
    }
    // Queries without native relations must not execute an extra engine query.
    match rewrite_with_position(sql, "", true, &mut session.relation_error_position) {
        Err(("3D000", _)) => {}
        Err(("0A000", ref message))
            if message == "cross-database or non-public relations are unsupported" => {}
        Err(("0A000", message))
            if message == "quoted mixed-case identifiers require exact name binding support" => {}
        result => return result,
    }
    let public = path.iter().any(|name| name == "public");
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
    alias_scope(
        sql,
        context.schema_snapshot(),
        database,
        &mut session.relation_error_position,
    )?;
    let sql = rewrite_with_context(
        sql,
        database,
        public,
        &mut session.relation_error_position,
        context.schema_snapshot(),
        Some(context),
    )?;
    // PostgreSQL puts USING after the target relation; the native parser puts
    // it before ON. Omitting explicit btree preserves the native DDL default
    // (pkg/ddl/index.rs); check_index_options rejects other methods first.
    let mut sql = sql;
    let parts = source_tokens(&sql, true)?;
    let create_index = parts.first().is_some_and(|p| p.word("create"))
        && (parts.get(1).is_some_and(|p| p.word("index"))
            || parts.get(1).is_some_and(|p| p.word("unique"))
                && parts.get(2).is_some_and(|p| p.word("index")));
    if create_index {
        if let Some(on) = parts.iter().position(|p| p.word("on")) {
            if let Some(method) = parts
                .iter()
                .enumerate()
                .skip(on + 1)
                .take_while(|(_, p)| p.symbol != Some(b'('))
                .find_map(|(i, p)| p.word("using").then_some(i))
            {
                if parts.get(method + 1).is_some_and(|p| p.word("btree")) {
                    sql.replace_range(parts[method].start..parts[method + 1].end, "");
                }
            }
        }
    }
    validate_relations(&sql, database)?;
    Ok(sql)
}

/// Reject foreign schemas on TableName nodes exposed by the native visitor.
/// Token handling remains necessary for nodes not visited by the native AST.
pub(crate) fn validate_relations(sql: &str, database: &str) -> ParseResult<()> {
    use astersql_parser_ast as ast;
    // Validate original PostgreSQL relation nodes without moving syntax errors
    // ahead of the extended protocol's OID checks. The command gate still rejects
    // syntax errors before execution; this guard only checks successfully parsed ASTs.
    let Ok((statements, _)) =
        astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL).ParseSQL(sql, &[])
    else {
        return Ok(());
    };
    struct Guard<'a> {
        database: &'a str,
        foreign: bool,
    }
    impl ast::Visitor for Guard<'_> {
        fn enter(&mut self, _: &dyn ast::Node) -> bool {
            false
        }
        fn enter_table_name(&mut self, table: &ast::TableName) -> bool {
            // TableName is a value type: the generic Node hook cannot see it.
            // Guard the original PG database segment before the native schema.
            let parts = &table.PostgreSQLParts;
            self.foreign |= if parts.len() == 3 {
                parts[0].Lookup != self.database || parts[1].Lookup != "public"
            } else {
                !table.Schema.L.is_empty() && table.Schema.L != self.database.to_lowercase()
            };
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

/// A missing relation in DROP IF EXISTS is a successful utility with a NOTICE.
/// Probe the same authoritative binder before execution; callers emit only when
/// the existing executor succeeds, so failed/denied commands never gain notices.
pub(crate) fn drop_if_exists_notices(sql: &str, context: &dyn TiDBContext) -> Vec<(u8, Vec<u8>)> {
    use astersql_parser_ast as ast;
    let Ok((nodes, _)) =
        astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL).ParseSQL(sql, &[])
    else {
        return vec![];
    };
    if nodes.len() != 1 {
        return vec![];
    }
    let Some(drop) = nodes[0].as_any().downcast_ref::<ast::DropTableStmt>() else {
        return vec![];
    };
    if !drop.IfExists || drop.IsView {
        return vec![];
    }
    drop.Tables
        .iter()
        .filter_map(|table| {
            let schema = (!table.Schema.O.is_empty()).then(|| table.Schema.O.clone());
            let missing = context
                .pg_bind_relation(schema, table.Name.O.clone(), false)
                .is_some_and(|bound| bound.is_err_and(|e| crate::pg_conn::sqlstate(&e) == "42P01"));
            if !missing {
                return None;
            }
            let mut body = b"SNOTICE\0VNOTICE\0C00000\0M".to_vec();
            body.extend_from_slice(
                format!("table \"{}\" does not exist, skipping", table.Name.O).as_bytes(),
            );
            body.extend_from_slice(&[0, 0]);
            Some((b'N', body))
        })
        .collect()
}
