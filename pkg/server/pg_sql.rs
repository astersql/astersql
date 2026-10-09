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
                if delimiter == b'\'' && bytes[i] == b'\\' && i + 1 < bytes.len() {
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
    let (last, native) = if ty.word("smallint") {
        (start, "SMALLINT")
    } else if ty.word("integer") || ty.word("int") {
        (start, "INT")
    } else if ty.word("bigint") {
        (start, "BIGINT")
    } else if ty.word("real") {
        (start, "FLOAT")
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

pub(crate) fn adapt(sql: &str) -> ParseResult<String> {
    adapt_internal(sql, None)
}

pub(crate) fn adapt_with_context(sql: &str, context: &dyn TiDBContext) -> ParseResult<String> {
    adapt_internal(sql, Some(context))
}

fn adapt_internal(sql: &str, context: Option<&dyn TiDBContext>) -> ParseResult<String> {
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
    if first.word("alter") && tokens.get(1).is_some_and(|token| token.word("table")) {
        adapt_alter(sql, &tokens, edits, context)
    } else if !first.word("create") {
        return apply(sql, edits);
    } else {
        adapt_create(sql, &tokens, edits)
    }
}

fn adapt_create(
    sql: &str,
    tokens: &[Token],
    mut edits: BTreeMap<usize, (usize, String)>,
) -> ParseResult<String> {
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
        format!(" {}", column.GetTypeDesc())
    } else {
        String::new()
    };
    let is_not_null =
        not_null.unwrap_or_else(|| astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()));
    if is_not_null {
        suffix.push_str(" NOT NULL");
    }
    if let Some(override_default) = default_override {
        if let Some(default) = override_default {
            suffix.push_str(" DEFAULT ");
            suffix.push_str(&default);
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
