// Copyright 2026 AsterSQL.
//! Bounded PostgreSQL DDL adaptation for the PostgreSQL listener.

use crate::conn::TiDBContext;
use crate::pg_catalog_query::ParseResult;
use std::collections::BTreeMap;

const MAX_SQL_LENGTH: usize = 1 << 20;
const MAX_TOKENS: usize = 16_384;
const MAX_DEPTH: usize = 128;

#[derive(Clone)]
enum Kind {
    Word(String),
    Identifier(String),
    String,
    Symbol(u8),
}
#[derive(Clone)]
struct Token {
    start: usize,
    end: usize,
    kind: Kind,
}
impl Token {
    fn word(&self, expected: &str) -> bool {
        matches!(&self.kind, Kind::Word(word) if word.eq_ignore_ascii_case(expected))
    }
    fn symbol(&self, expected: u8) -> bool {
        matches!(self.kind, Kind::Symbol(symbol) if symbol == expected)
    }
    fn identifier(&self) -> bool {
        matches!(self.kind, Kind::Word(_) | Kind::Identifier(_))
    }
}

fn error(state: &'static str, message: &str) -> (&'static str, String) {
    (state, message.into())
}
fn quote(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}
fn tokens(sql: &str) -> ParseResult<Vec<Token>> {
    if sql.len() > MAX_SQL_LENGTH {
        return Err(error("54000", "PG SQL exceeds the 1 MiB adaptation limit"));
    }
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut depth = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"--") {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                if bytes[i..].starts_with(b"/*") {
                    return Err(error("0A000", "nested PG comments are unsupported"));
                }
                i += 1;
            }
            if i == bytes.len() {
                return Err(error("42601", "unterminated SQL comment"));
            }
            i += 2;
            continue;
        }
        let start = i;
        let kind = if bytes[i] == b'\'' || bytes[i] == b'"' || bytes[i] == b'`' {
            let delimiter = bytes[i];
            let identifier = delimiter != b'\'';
            i += 1;
            let content = i;
            loop {
                if i == bytes.len() {
                    return Err(error("42601", "unterminated SQL quote"));
                }
                if bytes[i] == delimiter {
                    if bytes.get(i + 1) == Some(&delimiter) {
                        i += 2;
                        continue;
                    }
                    let value = if identifier {
                        let doubled = format!("{0}{0}", delimiter as char);
                        Some(sql[content..i].replace(&doubled, &(delimiter as char).to_string()))
                    } else {
                        None
                    };
                    i += 1;
                    break match value {
                        Some(value) if value.is_empty() => {
                            return Err(error("42601", "empty quoted identifier"));
                        }
                        Some(value) => Kind::Identifier(value),
                        None => Kind::String,
                    };
                }
                if delimiter == b'\''
                    && start > 0
                    && matches!(bytes[start - 1], b'e' | b'E')
                    && (start == 1 || !bytes[start - 2].is_ascii_alphanumeric())
                    && bytes[i] == b'\\'
                    && i + 1 < bytes.len()
                {
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' || bytes[i] >= 128 {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || matches!(bytes[i], b'_' | b'$')
                    || bytes[i] >= 128)
            {
                i += 1;
            }
            Kind::Word(sql[start..i].to_lowercase())
        } else {
            let symbol = bytes[i];
            i += 1;
            match symbol {
                b'(' => {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        return Err(error("54001", "PG SQL nesting exceeds 128 levels"));
                    }
                }
                b')' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| error("42601", "unmatched SQL parenthesis"))?;
                }
                _ => {}
            }
            Kind::Symbol(symbol)
        };
        tokens.push(Token {
            start,
            end: i,
            kind,
        });
        if tokens.len() > MAX_TOKENS {
            return Err(error("54000", "PG SQL exceeds the token limit"));
        }
    }
    if depth != 0 {
        return Err(error("42601", "unclosed SQL parenthesis"));
    }
    Ok(tokens)
}

/// Trim parser-owned statement spans to their nonempty token boundaries.
/// StatementList can retain preceding empty fragments/comments in Text().
/// Reuse the SQL lexer so quoted semicolons remain part of their token.
pub(crate) fn query_statement_text(sql: &str) -> ParseResult<&str> {
    let tokens = tokens(sql)?;
    let mut content = tokens.iter().filter(|token| !token.symbol(b';'));
    let Some(first) = content.next() else {
        return Ok("");
    };
    let last = content.next_back().unwrap_or(first);
    Ok(&sql[first.start..last.end])
}

fn column_type(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> ParseResult<Option<(usize, usize, &'static str)>> {
    let Some(column) = tokens.get(start) else {
        return Ok(None);
    };
    if !column.identifier() {
        return Err(error("42601", "expected CREATE TABLE column name"));
    }
    if [
        "constraint",
        "primary",
        "unique",
        "key",
        "index",
        "check",
        "foreign",
        "exclude",
        "like",
    ]
    .iter()
    .any(|word| column.word(word))
    {
        return Ok(None);
    }
    let Some(ty) = tokens.get(start + 1).filter(|token| start + 1 < end) else {
        return Err(error("42601", "expected CREATE TABLE column type"));
    };
    mapped_type(tokens, start + 1)
        .map(|mapping| mapping.map(|(last, native)| (ty.start, tokens[last].end, native)))
}

fn mapped_type(tokens: &[Token], start: usize) -> ParseResult<Option<(usize, &'static str)>> {
    let Some(ty) = tokens.get(start) else {
        return Err(error("42601", "expected PostgreSQL column type"));
    };
    if ty.word("serial") || ty.word("bigserial") {
        return Err(error(
            "0A000",
            "serial identity columns are unsupported until native identity metadata is available",
        ));
    }
    let (last, native) = if ty.word("smallint") || ty.word("int2") {
        (start, "SMALLINT")
    } else if ty.word("integer") || ty.word("int") {
        (start, "INT")
    } else if ty.word("bigint") {
        (start, "BIGINT")
    } else if ty.word("real") || ty.word("float4") {
        (start, "FLOAT")
    } else if ty.word("float8") {
        (start, "DOUBLE")
    } else if ty.word("double") && tokens.get(start + 1).is_some_and(|t| t.word("precision")) {
        (start + 1, "DOUBLE")
    } else if ty.word("numeric") || ty.word("decimal") {
        (start, "DECIMAL")
    } else if ty.word("boolean") {
        (start, "BOOLEAN")
    } else if ty.word("char") {
        (start, "CHAR")
    } else if ty.word("varchar") {
        (start, "VARCHAR")
    } else if ty.word("text") {
        (start, "TEXT")
    } else if ty.word("bytea") {
        (start, "BLOB")
    } else if ty.word("date") {
        (start, "DATE")
    } else if ty.word("time") {
        (start, "TIME")
    } else if ty.word("timestamp") {
        (start, "TIMESTAMP")
    } else {
        return Err(error(
            "0A000",
            "unsupported PostgreSQL CREATE TABLE column type",
        ));
    };
    Ok(Some((last, native)))
}

/// Recognize the PG18 row-image alias syntax before the shared parser, which
/// has no representation for it yet. Validate the remaining DML grammar so
/// malformed statements retain syntax diagnostics rather than a capability gate.
pub(crate) fn check_returning_aliases(sql: &str) -> ParseResult<()> {
    let ts = tokens(sql)?;
    for start in 0..ts.len().saturating_sub(2) {
        if !ts[start].word("returning")
            || !ts[start + 1].word("with")
            || !ts[start + 2].symbol(b'(')
        {
            continue;
        }
        let mut i = start + 3;
        let mut seen = std::collections::HashSet::new();
        loop {
            let Some(image) = ts.get(i) else {
                return Ok(());
            };
            let name = if image.word("old") {
                "old"
            } else if image.word("new") {
                "new"
            } else {
                return Ok(());
            };
            if !seen.insert(name)
                || !ts.get(i + 1).is_some_and(|t| t.word("as"))
                || !ts.get(i + 2).is_some_and(Token::identifier)
            {
                return Ok(());
            }
            i += 3;
            if ts.get(i).is_some_and(|t| t.symbol(b',')) {
                i += 1;
                continue;
            }
            if !ts.get(i).is_some_and(|t| t.symbol(b')')) {
                return Ok(());
            }
            break;
        }
        let stripped = format!("{} {}", &sql[..ts[start + 1].start], &sql[ts[i].end..]);
        if let Ok((statements, _)) =
            astersql_parser::Parser::with_dialect(astersql_parser_ast::SqlDialect::PostgreSQL)
                .ParseSQL(&stripped, &[])
            && statements.len() == 1
            && (statements[0]
                .as_any()
                .is::<astersql_parser_ast::InsertStmt>()
                || statements[0]
                    .as_any()
                    .is::<astersql_parser_ast::UpdateStmt>()
                || statements[0]
                    .as_any()
                    .is::<astersql_parser_ast::DeleteStmt>())
        {
            return Err(error(
                "0A000",
                "PostgreSQL RETURNING OLD/NEW aliases are not supported",
            ));
        }
    }
    Ok(())
}

pub(crate) fn adapt(sql: &str) -> ParseResult<String> {
    adapt_internal(sql, None, None)
}

pub(crate) fn adapt_with_context(sql: &str, context: &dyn TiDBContext) -> ParseResult<String> {
    adapt_internal(sql, Some(context), None)
}

/// Index relation lookup must use the connection's effective search_path.
pub(crate) fn adapt_with_session(
    sql: &str,
    context: &dyn TiDBContext,
    session: &crate::pg_session::PgSession,
) -> ParseResult<String> {
    adapt_internal(sql, Some(context), Some(session))
}

// PG omits ROW before each VALUES tuple. Reuse the row-list AST while
// retaining an exact map back to original parameter byte offsets.
fn values_row_insertions(tokens: &[Token]) -> Vec<usize> {
    let mut insertions = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        // INSERT VALUES already uses bare tuples in the shared grammar. Only
        // query-level VALUES (including derived tables and CTEs) needs ROW.
        if !token.word("values") || (index != 0 && !tokens[index - 1].symbol(b'(')) {
            continue;
        }
        let mut depth = 0usize;
        let mut row_start = true;
        for token in &tokens[index + 1..] {
            if depth == 0 && row_start && token.symbol(b'(') {
                insertions.push(token.start);
                row_start = false;
            }
            if token.symbol(b'(') {
                depth += 1;
            } else if token.symbol(b')') {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            } else if depth == 0 && token.symbol(b',') {
                row_start = true;
            } else if depth == 0 {
                break;
            }
        }
    }
    insertions.sort_unstable();
    insertions.dedup();
    insertions
}

// Keep every insertion four bytes long so parameter offsets stay reversible,
// including the legal no-whitespace spelling VALUES(...).
fn values_row_prefix(sql: &str, start: usize) -> String {
    if start > 0 && sql.as_bytes()[start - 1].is_ascii_alphanumeric() {
        " ROW".to_owned()
    } else {
        "ROW ".to_owned()
    }
}

pub(crate) fn values_parameter_source(sql: &str) -> ParseResult<(String, Vec<usize>)> {
    let insertions = values_row_insertions(&tokens(sql)?);
    let edits = insertions
        .iter()
        .map(|start| (*start, (*start, values_row_prefix(sql, *start))))
        .collect();
    Ok((apply(sql, edits)?, insertions))
}

// PostgreSQL standard string literals preserve backslashes. Translate only
// DEFAULT literals at the DDL boundary before the native parser sees them;
// returned definitions must survive an ALTER SET DEFAULT round trip.
fn default_literal_sql(sql: &str, tokens: &[Token]) -> ParseResult<String> {
    if !tokens
        .first()
        .is_some_and(|t| t.word("create") || t.word("alter"))
    {
        return Ok(sql.into());
    }
    let mut edits = BTreeMap::new();
    for (index, _) in tokens.iter().enumerate().filter(|(_, t)| t.word("default")) {
        let Some(next) = tokens.get(index + 1) else {
            continue;
        };
        let (literal, escape) = if matches!(next.kind, Kind::String) {
            (next, false)
        } else if next.word("e") {
            match tokens.get(index + 2) {
                Some(t) if matches!(t.kind, Kind::String) && next.end == t.start => (t, true),
                _ => continue,
            }
        } else {
            continue;
        };
        let value = &sql[literal.start..literal.end];
        if escape {
            let mut bytes = value.as_bytes().iter().copied();
            while let Some(byte) = bytes.next() {
                if byte == b'\\'
                    && !bytes.next().is_some_and(|b| {
                        matches!(b, b'\\' | b'\'' | b'n' | b'r' | b't' | b'b' | b'f')
                    })
                {
                    return Err(error("0A000", "unsupported DEFAULT escape string"));
                }
            }
            edits.insert(next.start, (literal.end, value.to_owned()));
        } else if value.contains('\\') {
            edits.insert(literal.start, (literal.end, value.replace('\\', "\\\\")));
        }
    }
    apply(sql, edits)
}

fn adapt_internal(
    sql: &str,
    context: Option<&dyn TiDBContext>,
    session: Option<&crate::pg_session::PgSession>,
) -> ParseResult<String> {
    let normalized = default_literal_sql(sql, &tokens(sql)?)?;
    let sql = normalized.as_str();
    let tokens = tokens(sql)?;
    let Some(first) = tokens.first() else {
        return Ok(sql.to_owned());
    };
    let mut edits = BTreeMap::new();
    for token in &tokens {
        if let Kind::Identifier(name) = &token.kind {
            edits.insert(token.start, (token.end, quote(name)));
        }
    }
    for start in values_row_insertions(&tokens) {
        edits.insert(start, (start, values_row_prefix(sql, start)));
    }
    if (first.word("alter") || first.word("drop"))
        && tokens.get(1).is_some_and(|token| token.word("index"))
    {
        let path = match (session, context) {
            (Some(session), Some(context)) => session
                .effective_explicit_path(context)
                .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?,
            _ => vec!["pg_catalog".into(), "public".into()],
        };
        return adapt_index(&tokens, context, &path);
    }
    if first.word("rename") {
        return Err(error("0A000", "RENAME TABLE is not a PostgreSQL command"));
    }
    if first.word("alter") && tokens.get(1).is_some_and(|t| t.word("view")) {
        return adapt_relation_rename(&tokens, context);
    }
    if first.word("alter") && tokens.get(1).is_some_and(|token| token.word("table")) {
        let (_, _, action) = alter_table_parts(&tokens)?;
        if tokens.get(action).is_some_and(|t| t.word("rename"))
            && tokens.get(action + 1).is_some_and(|t| t.word("to"))
        {
            return adapt_relation_rename(&tokens, context);
        }
        adapt_alter(sql, &tokens, edits, context)
    } else if !first.word("create") {
        return apply(sql, edits);
    } else {
        adapt_create(sql, &tokens, edits)
    }
}

// pg_name resolves the source to its storage database and leaves the new name
// unqualified. Both table and view renames retain the source namespace.
fn adapt_relation_rename(
    tokens: &[Token],
    context: Option<&dyn TiDBContext>,
) -> ParseResult<String> {
    let context =
        context.ok_or_else(|| error("0A000", "relation rename requires catalog context"))?;
    let (database, source, action) = alter_table_parts(tokens)?;
    let end = tokens.len() - usize::from(tokens.last().is_some_and(|t| t.symbol(b';')));
    if !tokens.get(action).is_some_and(|t| t.word("rename"))
        || !tokens.get(action + 1).is_some_and(|t| t.word("to"))
    {
        return Err(error("0A000", "only ALTER relation RENAME TO is supported"));
    }
    let dest = action + 2;
    let target = if dest + 1 == end {
        token_name(&tokens[dest])
    } else {
        None
    }
    .ok_or_else(|| error("42601", "expected ALTER relation name RENAME TO name"))?;
    let database =
        database.ok_or_else(|| error("3D000", "relation storage database is unavailable"))?;
    let snapshot = context
        .schema_snapshot()
        .ok_or_else(|| error("0A000", "schema snapshot is unavailable"))?;
    let model = snapshot
        .ModelTableInfoByExactName(&astersql_infoschema::CiString::new(&database), &source)
        .map_err(|_| error("42P01", "relation does not exist"))?;
    let view = tokens.get(1).is_some_and(|t| t.word("view"));
    if view != model.View.is_some() || model.Sequence.is_some() {
        return Err(error("42809", "relation has the wrong kind for RENAME"));
    }
    let namespaces = context
        .pg_namespace_facts()
        .transpose()
        .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?
        .unwrap_or_default();
    if !namespaces.iter().any(|fact| {
        model
            .PostgreSQLNamespace
            .as_ref()
            .map_or(fact.name == "public", |(id, _)| fact.id == *id)
            && fact.can_create
    }) {
        return Err(error("42501", "permission denied for relation schema"));
    }
    let identity = context
        .user_identity()
        .map_err(|_| error("XX000", "cannot read authenticated role"))?;
    let actor = identity.split('@').next().unwrap_or("");
    if actor != "root" {
        let facts = context
            .pg_database_facts(None)
            .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
        if model
            .PostgreSQLOwner
            .and_then(|id| facts.first()?.roles.get(&id))
            .map(String::as_str)
            != Some(actor)
        {
            return Err(error("42501", "must be owner of relation"));
        }
    }
    // An unqualified RENAME destination stays in the source namespace even when
    // search_path's creation schema is different.
    let target = if let Some((id, _)) = model.PostgreSQLNamespace.as_ref() {
        let logical = target
            .strip_prefix(&format!("__astersql_pg_{id}_"))
            .unwrap_or(target);
        format!("__astersql_pg_{id}_{logical}")
    } else {
        target.to_owned()
    };
    if snapshot
        .ModelTableInfoByExactName(&astersql_infoschema::CiString::new(&database), &target)
        .is_ok()
    {
        return Err(error("42P07", "relation already exists"));
    }
    Ok(if view {
        format!(
            "RENAME TABLE {}.{} TO {}.{}",
            quote(&database),
            quote(&source),
            quote(&database),
            quote(&target)
        )
    } else {
        format!(
            "ALTER TABLE {}.{} RENAME TO {}.{}",
            quote(&database),
            quote(&source),
            quote(&database),
            quote(&target)
        )
    })
}

// Resolve a PG schema relation back to its table-local native index. The
// resulting ordinary DDL keeps authorization and durable publication in the
// existing session/DDL path, rather than mutating a catalog projection.
fn adapt_index(
    tokens: &[Token],
    context: Option<&dyn TiDBContext>,
    path: &[String],
) -> ParseResult<String> {
    let context = context.ok_or_else(|| error("0A000", "index DDL requires catalog context"))?;
    if tokens.get(2).is_some_and(|t| t.word("if")) {
        return Err(error("0A000", "conditional index DDL is unsupported"));
    }
    let (schema, name, action) = alter_table_parts(tokens)?;
    let end = tokens.len() - usize::from(tokens.last().is_some_and(|t| t.symbol(b';')));
    let rename = tokens[0].word("alter");
    if rename {
        if action + 3 != end
            || !tokens[action].word("rename")
            || !tokens[action + 1].word("to")
            || !tokens[action + 2].identifier()
        {
            return Err(error("42601", "expected ALTER INDEX name RENAME TO name"));
        }
    } else if action != end {
        return Err(error("0A000", "unsupported DROP INDEX clause"));
    }
    let result = context
        .execute_query(
            "SELECT DATABASE()",
            false,
            &crate::conn::CancellationToken::new(),
        )
        .map_err(|_| error("XX000", "cannot read current database"))?;
    let database = match result
        .first()
        .and_then(|r| r.rows.first())
        .and_then(|r| r.first())
    {
        Some(crate::conn::Value::Text(db)) => db,
        _ => return Err(error("3D000", "no current database for index DDL")),
    };
    let snapshot = context
        .schema_snapshot()
        .ok_or_else(|| error("0A000", "schema snapshot is unavailable"))?;
    let facts = context
        .pg_namespace_facts()
        .transpose()
        .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?
        .unwrap_or_default();
    let qualified = schema.as_ref().map_or_else(
        || format!("\"{}\"", name.replace('"', "\"\"")),
        |schema| {
            format!(
                "\"{}\".\"{}\"",
                schema.replace('"', "\"\""),
                name.replace('"', "\"\"")
            )
        },
    );
    let oid = crate::pg_oid::resolve_in_path(&qualified, database, snapshot.as_ref(), path, &facts)
        .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
    if rename {
        let rows = crate::pg_catalog::class_rows(snapshot.as_ref(), database)
            .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
        let namespace = rows
            .iter()
            .find(|r| r[0] == crate::conn::Value::Signed(i64::from(oid)))
            .map(|r| &r[2]);
        if facts.iter().any(|fact| {
            let ns = if fact.name == "pg_catalog" {
                Some(11)
            } else {
                crate::pg_oid::namespace_oid(fact.id).ok()
            };
            namespace
                == ns
                    .map(|ns| crate::conn::Value::Signed(i64::from(ns)))
                    .as_ref()
                && !fact.can_create
        }) {
            return Err(error("42501", "permission denied for index schema"));
        }
        let destination =
            crate::conn::Value::Text(token_name(&tokens[action + 2]).unwrap().to_owned());
        if rows
            .iter()
            .any(|r| Some(&r[2]) == namespace && r[1] == destination)
        {
            return Err(error("42P07", "relation already exists"));
        }
    }
    let tables = snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new(database))
        .map_err(|_| error("XX000", "cannot read index owner table"))?;
    for table in tables {
        let Some(model) = &table.model_meta else {
            continue;
        };
        for index in crate::pg_catalog::catalog_indexes(model)
            .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?
        {
            if crate::pg_oid::index_oid(model.ID, index.ID).ok() != Some(oid) {
                continue;
            }
            let identity = context
                .user_identity()
                .map_err(|_| error("XX000", "cannot read authenticated role"))?;
            let actor = identity
                .rsplit_once('@')
                .map_or(identity.as_str(), |(user, _)| user);
            if actor != "root" {
                let roles = context
                    .pg_database_facts(None)
                    .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
                let owner = model
                    .PostgreSQLOwner
                    .and_then(|id| roles.first()?.roles.get(&id));
                if owner.map(String::as_str) != Some(actor) {
                    return Err(error("42501", "must be owner of index"));
                }
            }
            // Constraint-owned indexes require constraint DDL, which this narrow
            // standalone-index lifecycle path does not implement.
            if index.Primary || index.PostgreSQLConstraintName.is_some() {
                return Err(error("0A000", "constraint-owned index DDL is unsupported"));
            }
            // The target was resolved in this database above. Native index DDL
            // takes the storage table key, not the logical PG schema name.
            let table = quote(&model.Name.O);
            return Ok(if rename {
                format!(
                    "ALTER TABLE {} RENAME INDEX {} TO {}",
                    table,
                    quote(&index.Name.O),
                    quote(token_name(&tokens[action + 2]).unwrap())
                )
            } else {
                format!("DROP INDEX {} ON {}", quote(&index.Name.O), table)
            });
        }
    }
    Err(error("42809", "relation is not an index"))
}

fn adapt_create(
    sql: &str,
    tokens: &[Token],
    mut edits: BTreeMap<usize, (usize, String)>,
) -> ParseResult<String> {
    // No durable domain/composite/enum identities exist yet. Reject before
    // native parsing or metadata writes rather than presenting them as base types.
    if tokens
        .get(1)
        .is_some_and(|t| t.word("domain") || t.word("type"))
    {
        return Err(error(
            "0A000",
            "PostgreSQL user-defined types are not supported",
        ));
    }
    let mut table = 1;
    if tokens
        .get(table)
        .is_some_and(|t| t.word("temporary") || t.word("temp"))
    {
        table += 1;
    }
    if !tokens.get(table).is_some_and(|t| t.word("table")) {
        return apply(sql, edits);
    }
    let open = tokens
        .iter()
        .enumerate()
        .skip(table + 1)
        .find_map(|(index, token)| token.symbol(b'(').then_some(index))
        .ok_or_else(|| error("42601", "CREATE TABLE requires a column list"))?;
    let mut depth = 1usize;
    let mut segment = open + 1;
    for index in open + 1..tokens.len() {
        if tokens[index].symbol(b'(') {
            depth += 1;
        } else if tokens[index].symbol(b')') {
            depth -= 1;
            if depth == 0 {
                if let Some((start, end, native)) = column_type(&tokens, segment, index)? {
                    edits.insert(start, (end, native.into()));
                }
                break;
            }
        } else if depth == 1 && tokens[index].symbol(b',') {
            if let Some((start, end, native)) = column_type(&tokens, segment, index)? {
                edits.insert(start, (end, native.into()));
            }
            segment = index + 1;
        }
    }
    apply(sql, edits)
}

fn token_name(token: &Token) -> Option<&str> {
    match &token.kind {
        Kind::Word(name) | Kind::Identifier(name) => Some(name),
        _ => None,
    }
}

fn alter_table_parts(tokens: &[Token]) -> ParseResult<(Option<String>, String, usize)> {
    let mut index = 2;
    if tokens.get(index).is_some_and(|token| token.word("if"))
        && tokens
            .get(index + 1)
            .is_some_and(|token| token.word("exists"))
    {
        index += 2;
    }
    let first = tokens
        .get(index)
        .and_then(token_name)
        .ok_or_else(|| error("42601", "expected ALTER TABLE relation name"))?
        .to_owned();
    if tokens
        .get(index + 1)
        .is_some_and(|token| token.symbol(b'.'))
    {
        let table = tokens
            .get(index + 2)
            .and_then(token_name)
            .ok_or_else(|| error("42601", "expected ALTER TABLE relation name"))?
            .to_owned();
        Ok((Some(first), table, index + 3))
    } else {
        Ok((None, first, index + 1))
    }
}

fn adapt_alter(
    sql: &str,
    tokens: &[Token],
    mut edits: BTreeMap<usize, (usize, String)>,
    context: Option<&dyn TiDBContext>,
) -> ParseResult<String> {
    let (schema, table, action) = alter_table_parts(tokens)?;
    let mut depth = 0usize;
    for token in tokens.iter().skip(action) {
        if token.symbol(b'(') {
            depth += 1;
        } else if token.symbol(b')') {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token.symbol(b',') {
            return Err(error(
                "0A000",
                "multi-action PostgreSQL ALTER TABLE is unsupported",
            ));
        }
    }
    let Some(operation) = tokens.get(action) else {
        return Err(error("42601", "expected ALTER TABLE action"));
    };
    if operation.word("add") {
        let mut column = action + 1;
        if tokens.get(column).is_some_and(|token| token.word("column")) {
            column += 1;
        }
        if tokens.get(column).is_some_and(|token| token.word("if")) {
            column += 3;
        }
        if let Some((start, end, native)) = column_type(tokens, column, tokens.len())? {
            edits.insert(start, (end, native.into()));
        }
        return apply(sql, edits);
    }
    if operation.word("rename") {
        if tokens.get(action + 1).is_some_and(|token| token.word("to")) {
            return apply(sql, edits);
        }
        if !tokens
            .get(action + 1)
            .is_some_and(|token| token.word("column"))
        {
            return Err(error("0A000", "only PostgreSQL RENAME COLUMN is supported"));
        }
        return apply(sql, edits);
    }
    if operation.word("drop") {
        if tokens
            .iter()
            .skip(action + 1)
            .any(|token| token.word("cascade"))
        {
            return Err(error(
                "0A000",
                "PostgreSQL DROP COLUMN CASCADE is unsupported",
            ));
        }
        return apply(sql, edits);
    }
    if !operation.word("alter")
        || !tokens
            .get(action + 1)
            .is_some_and(|token| token.word("column"))
    {
        return Err(error("0A000", "unsupported PostgreSQL ALTER TABLE action"));
    }
    let column_index = action + 2;
    let column = tokens
        .get(column_index)
        .and_then(token_name)
        .ok_or_else(|| error("42601", "expected ALTER COLUMN name"))?;
    let clause = action + 3;
    if tokens.get(clause).is_some_and(|token| token.word("type")) {
        if tokens
            .iter()
            .skip(clause + 1)
            .any(|token| token.word("using"))
        {
            return Err(error(
                "0A000",
                "PostgreSQL ALTER COLUMN USING is unsupported",
            ));
        }
        let Some((last, native)) = mapped_type(tokens, clause + 1)? else {
            return Err(error("0A000", "unsupported PostgreSQL ALTER COLUMN type"));
        };
        edits.insert(tokens[action].start, (tokens[action].end, "MODIFY".into()));
        edits.insert(
            tokens[clause].start,
            (tokens[clause + 1].start, String::new()),
        );
        edits.insert(tokens[clause + 1].start, (tokens[last].end, native.into()));
        if let Some(context) = context {
            let suffix = existing_column_suffix(
                context,
                schema.as_deref(),
                &table,
                column,
                None,
                false,
                None,
            )?;
            let end = tokens
                .iter()
                .skip(clause + 1)
                .rev()
                .find(|token| !token.symbol(b';'))
                .map_or(tokens[last].end, |token| token.end);
            edits.insert(end, (end, suffix));
        }
        return apply(sql, edits);
    }
    let set = tokens.get(clause).is_some_and(|token| token.word("set"));
    let drop = tokens.get(clause).is_some_and(|token| token.word("drop"));
    if (set || drop)
        && tokens
            .get(clause + 1)
            .is_some_and(|token| token.word("default"))
    {
        if let Some(context) = context {
            let default = if set {
                let value = tokens
                    .get(clause + 2)
                    .ok_or_else(|| error("42601", "SET DEFAULT requires an expression"))?;
                let end = tokens
                    .iter()
                    .rev()
                    .find(|token| !token.symbol(b';'))
                    .map_or(value.end, |token| token.end);
                Some(sql[value.start..end].to_owned())
            } else {
                None
            };
            let definition = existing_column_suffix(
                context,
                schema.as_deref(),
                &table,
                column,
                None,
                true,
                Some(default),
            )?;
            let end = tokens
                .iter()
                .rev()
                .find(|token| !token.symbol(b';'))
                .map_or(tokens[clause + 1].end, |token| token.end);
            edits.insert(
                tokens[action].start,
                (
                    end,
                    format!("MODIFY COLUMN {}{}", quote(column), definition),
                ),
            );
        }
        return apply(sql, edits);
    }
    if (set || drop)
        && tokens
            .get(clause + 1)
            .is_some_and(|token| token.word("not"))
        && tokens
            .get(clause + 2)
            .is_some_and(|token| token.word("null"))
    {
        let context = context
            .ok_or_else(|| error("0A000", "ALTER COLUMN nullability requires catalog context"))?;
        let definition = existing_column_suffix(
            context,
            schema.as_deref(),
            &table,
            column,
            Some(set),
            true,
            None,
        )?;
        edits.insert(
            tokens[action].start,
            (
                tokens[clause + 2].end,
                format!("MODIFY COLUMN {}{}", quote(column), definition),
            ),
        );
        return apply(sql, edits);
    }
    Err(error("0A000", "unsupported PostgreSQL ALTER COLUMN clause"))
}

fn existing_column_suffix(
    context: &dyn TiDBContext,
    schema: Option<&str>,
    table: &str,
    column: &str,
    not_null: Option<bool>,
    include_type: bool,
    default_override: Option<Option<String>>,
) -> ParseResult<String> {
    use astersql_infoschema::CiString;
    use astersql_meta_model::DefaultValue;
    let snapshot = context
        .schema_snapshot()
        .ok_or_else(|| error("0A000", "schema snapshot is unavailable"))?;
    let database = match schema {
        Some(schema) => schema.to_owned(),
        None => {
            let result = context
                .execute_query(
                    "SELECT DATABASE()",
                    false,
                    &crate::conn::CancellationToken::new(),
                )
                .map_err(|error| ("XX000", error.to_string()))?;
            match result
                .first()
                .and_then(|result| result.rows.first())
                .and_then(|row| row.first())
            {
                Some(crate::conn::Value::Text(database)) => database.clone(),
                _ => return Err(error("3D000", "no current database for ALTER TABLE")),
            }
        }
    };
    let relation = snapshot
        .TableByName(&CiString::new(database), &CiString::new(table))
        .map_err(|error| ("42P01", error.to_string()))?;
    let model = relation
        .Meta()
        .model_meta
        .as_ref()
        .ok_or_else(|| error("XX000", "complete table metadata is unavailable"))?;
    let column = model
        .Columns
        .iter()
        .find(|candidate| candidate.Name.L == column.to_lowercase())
        .ok_or_else(|| error("42703", "ALTER COLUMN target does not exist"))?;
    if column.IsGenerated() {
        return Err(error("0A000", "generated ALTER COLUMN is unsupported"));
    }
    let mut suffix = if include_type {
        if astersql_parser_mysql::r#type::HasIsBooleanFlag(column.GetFlag()) {
            " BOOLEAN".into()
        } else {
            format!(" {}", column.GetTypeDesc())
        }
    } else {
        String::new()
    };
    let is_not_null =
        not_null.unwrap_or_else(|| astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()));
    if is_not_null {
        suffix.push_str(" NOT NULL");
    } else if not_null == Some(false) {
        // Explicit NULL distinguishes DROP NOT NULL from a type-only ALTER,
        // whose empty options deliberately retain existing column constraints.
        suffix.push_str(" NULL");
    }
    if let Some(override_default) = default_override {
        if let Some(default) = override_default {
            suffix.push_str(" DEFAULT ");
            let parts = tokens(&default)?;
            let cast = match column.FieldType.PostgreSQLTypeOid() {
                Some(25) => "text",
                Some(1043) => "character varying",
                Some(1042) => "bpchar",
                _ => "",
            };
            // A deparsed literal cast to this very column type is already
            // enforced by assignment. Remove only that redundant string cast;
            // arbitrary expressions and cross-type casts keep their gate.
            let literal = parts.first().filter(|t| matches!(t.kind, Kind::String));
            if let Some(literal) = literal.filter(|literal| {
                !cast.is_empty() && default[literal.end..].trim() == format!("::{cast}")
            }) {
                suffix.push_str(&default[..literal.end]);
            } else {
                suffix.push_str(&default);
            }
        } else {
            // Keep an explicit option so the native MODIFY path does not
            // mistake DROP DEFAULT for a type-only change and retain it.
            // PostgreSQL normalizes DEFAULT NULL to no default object.
            suffix.push_str(" DEFAULT NULL");
        }
        return Ok(suffix);
    }
    if let Some(default) = column.GetDefaultValue() {
        let rendered = match default {
            DefaultValue::Bool(value) => value.to_string(),
            DefaultValue::Int(value) => value.to_string(),
            DefaultValue::Uint(value) => value.to_string(),
            DefaultValue::Float(value) if value.is_finite() => value.to_string(),
            DefaultValue::Float(_) => {
                return Err(error("0A000", "non-finite column default is unsupported"));
            }
            DefaultValue::String(value) => {
                let value = String::from_utf8(value)
                    .map_err(|_| error("0A000", "binary column default is unsupported"))?;
                if column.DefaultIsExpr || value.to_uppercase().starts_with("CURRENT_TIMESTAMP") {
                    value
                } else {
                    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
                }
            }
        };
        suffix.push_str(" DEFAULT ");
        suffix.push_str(&rendered);
    }
    Ok(suffix)
}

fn apply(sql: &str, edits: BTreeMap<usize, (usize, String)>) -> ParseResult<String> {
    let mut output = String::with_capacity(sql.len());
    let mut previous = 0;
    for (start, (end, replacement)) in edits {
        if start < previous {
            continue;
        }
        output.push_str(&sql[previous..start]);
        output.push_str(&replacement);
        previous = end;
    }
    output.push_str(&sql[previous..]);
    Ok(output)
}
