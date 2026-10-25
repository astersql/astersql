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
    edits: BTreeMap<usize, (usize, String)>,
    error_position: Option<usize>,
    snapshot: Option<astersql_infoschema::SchemaRef>,
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
        if self.database.is_empty() {
            return Err(error("3D000", "no current database selected"));
        }
        // Earlier visible CTEs took the return above. Resolve physical names
        // exactly in the owning connection's snapshot before native folding,
        // and diagnose missing objects at the original token.
        if !allow_columns
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
        self.edits.insert(
            first,
            // Keep the table segment's PG quotes. Re-quoting with backticks
            // destroys its source identity before the shared parser sees it.
            (
                last,
                format!(
                    "{}.{}",
                    quote(self.database),
                    &self.sql[self.tokens[i - 1].start..last]
                ),
            ),
        );
        Ok(i)
    }
    fn scope(&mut self, mut i: usize, end: usize, inherited: &HashSet<String>) -> ParseResult<()> {
        let mut ctes = inherited.clone();
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
            if recursive {
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
                            if (self.word(cursor, "from") || self.word(cursor, "join"))
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
                self.scope(start, close, &ctes)?;
                ctes.insert(name);
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
            } else if delete && self.word(i, "using") {
                // DELETE USING introduces another relation list, distinct from
                // the target following FROM. Resolve its names at the same
                // PostgreSQL relation boundary before the pre-write executor gate.
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
    for token in &mut tokens {
        if let Some(part) = parts.get(&(token.start, token.end)) {
            token.name = Some(part.Lookup.clone());
            token.quoted = part.Quoted;
        }
    }
    let mut resolver = Resolver {
        snapshot,
        tokens,
        sql,
        database,
        public,
        edits: BTreeMap::new(),
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
            let parts = if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>() {
                if let ast::ExprKind::Column(column) = &expr.Kind {
                    Some(&column.PostgreSQLParts)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(parts) = parts {
                self.consume(parts);
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
        gate.exact_relations = statement.as_any().is::<ast::SelectStmt>()
            || statement.as_any().is::<ast::SetOprStmt>()
            || statement.as_any().is::<ast::InsertStmt>()
            || statement.as_any().is::<ast::DropTableStmt>();
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
        allowed: HashSet<(usize, usize)>,
        failure: Option<(&'static str, &'static str, usize)>,
    }
    impl Scope<'_> {
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
                    let name = if source.AsName.O.is_empty() {
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
                    let columns = if source.QuerySource.is_none()
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
            }
            true
        }
    }
    let mut scope = Scope {
        tokens: tokens(sql)?,
        snapshot,
        database,
        scopes: Vec::new(),
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

pub(crate) fn adapt(
    sql: &str,
    context: &dyn TiDBContext,
    session: &mut crate::pg_session::PgSession,
) -> ParseResult<String> {
    // Queries without native relations must not execute an extra engine query.
    match rewrite_with_position(sql, "", true, &mut session.relation_error_position) {
        Err(("3D000", _)) => {}
        Err(("0A000", message))
            if message == "quoted mixed-case identifiers require exact name binding support" => {}
        result => return result,
    }
    let path = session
        .effective_explicit_path(context)
        .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
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
    let sql = rewrite_with_snapshot(
        sql,
        database,
        public,
        &mut session.relation_error_position,
        context.schema_snapshot(),
    )?;
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
