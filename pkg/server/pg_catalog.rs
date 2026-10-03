// Copyright 2026 AsterSQL.
//! PG-only DataGrip catalog probes backed by canonical schema/transaction data.
use crate::conn::{
    CancellationToken, ColumnInfo, ConnError, ConnResult, NativeType, PreparedMetadata,
    QueryResult, TiDBContext, Value,
};

pub(crate) const DATABASES_SQL: &str = r#"select N.oid::bigint as id,
       datname as name, D.description, datistemplate as is_template,
       datallowconn as allow_connections,
       pg_catalog.pg_get_userbyid(N.datdba) as "owner"
from pg_catalog.pg_database N
left join pg_catalog.pg_shdescription D on N.oid = D.objoid
order by case when datname = pg_catalog.current_database() then -1::bigint else N.oid::bigint end"#;
pub(crate) const TRANSACTIONS_SQL: &str = "select L.transactionid::varchar::bigint as transaction_id from pg_catalog.pg_locks L where L.transactionid is not null order by pg_catalog.age(L.transactionid) desc limit 1";

use crate::pg_catalog_query::{self, CastType, Expr, ParseResult, Select};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogQuery {
    pub(crate) select: Select,
    pub(crate) current_schema: Option<String>,
    pub(crate) public_first: bool,
}
impl CatalogQuery {
    pub(crate) fn parse(sql: &str) -> ParseResult<Option<Self>> {
        let Some(select) = pg_catalog_query::parse(sql)? else {
            return Ok(None);
        };
        let query = Self {
            select,
            current_schema: Some("public".into()),
            public_first: false,
        };
        query.validate()?;
        Ok(Some(query))
    }
    pub(crate) fn parse_session(
        sql: &str,
        context: &dyn TiDBContext,
        session: &crate::pg_session::PgSession,
    ) -> ParseResult<Option<Self>> {
        let mut shadowed = Vec::new();
        let public_first = session.public_precedes_catalog();
        let names = pg_catalog_query::implicit_relations(sql);
        if public_first && !names.is_empty() {
            let current = context
                .execute_query("SELECT DATABASE()", false, &CancellationToken::new())
                .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
            if let Some(Value::Text(database)) = current
                .first()
                .and_then(|r| r.rows.first())
                .and_then(|r| r.first())
            {
                let snapshot = context
                    .schema_snapshot()
                    .ok_or_else(|| ("0A000", "schema snapshot is unavailable".into()))?;
                let tables = snapshot
                    .SchemaTableInfos(&astersql_infoschema::CiString::new(database))
                    .map_err(|e| ("0A000", e.to_string()))?;
                shadowed.extend(
                    tables
                        .iter()
                        .filter(|t| names.contains(&t.name.original))
                        .map(|t| t.name.original.clone()),
                );
            }
        }
        let Some(select) = pg_catalog_query::parse_shadowed(sql, &shadowed)? else {
            return Ok(None);
        };
        let query = Self {
            select,
            current_schema: session.schema().map(str::to_owned),
            public_first,
        };
        query.validate()?;
        Ok(Some(query))
    }
    pub(crate) fn classify(sql: &str) -> Option<Self> {
        Self::parse(sql).ok().flatten()
    }
    fn validate(&self) -> ParseResult<()> {
        if !matches!(
            self.select.from.name.as_str(),
            "pg_class"
                | "pg_database"
                | "pg_locks"
                | "pg_namespace"
                | "pg_tablespace"
                | "pg_description"
                | "pg_shdescription"
        ) {
            return Err(("0A000", "catalog provider is not implemented yet".into()));
        }
        if let Some(join) = &self.select.join {
            if !matches!(
                (self.select.from.name.as_str(), join.relation.name.as_str()),
                ("pg_database", "pg_shdescription")
                    | ("pg_namespace", "pg_description")
                    | ("pg_tablespace", "pg_shdescription")
            ) || join.relation.alias == self.select.from.alias
            {
                return Err(("0A000", "unsupported catalog join".into()));
            }
            if self.expr_type(&join.on)?.0 != 1 {
                return Err(("0A000", "catalog JOIN must be a predicate".into()));
            }
        }
        for projection in &self.select.projections {
            if contains_age(&projection.expr) {
                return Err((
                    "0A000",
                    "native transaction age is only available for ordering".into(),
                ));
            }
            self.expr_type(&projection.expr)?;
        }
        if let Some(filter) = &self.select.filter {
            if contains_age(filter) {
                return Err((
                    "0A000",
                    "native transaction age is only available for ordering".into(),
                ));
            }
            if self.expr_type(filter)?.0 != 1 {
                return Err(("0A000", "catalog WHERE must be a predicate".into()));
            }
        }
        for order in &self.select.order {
            self.expr_type(self.order_expr(&order.expr)?)?;
        }
        Ok(())
    }
    fn order_expr<'a>(&'a self, expr: &'a Expr) -> ParseResult<&'a Expr> {
        if let Expr::Column(path) = expr {
            if let [name] = path.as_slice() {
                let mut projections = self
                    .select
                    .projections
                    .iter()
                    .filter(|projection| projection.name == *name);
                if let Some(projection) = projections.next() {
                    if projections.next().is_some() {
                        return Err(("0A000", "ambiguous catalog ORDER BY alias".into()));
                    }
                    return Ok(&projection.expr);
                }
            }
        }
        Ok(expr)
    }
    fn column(&self, path: &[String]) -> ParseResult<(usize, u8, usize)> {
        let name = path.last().unwrap().as_str();
        let qualifier = match path {
            [_] => None,
            [qualifier, _] => Some(qualifier.as_str()),
            [catalog, relation, _] if catalog == "pg_catalog" => Some(relation.as_str()),
            _ => return Err(("0A000", "unsupported column qualification".into())),
        };
        let main = &self.select.from;
        let belongs_main = qualifier.is_none_or(|q| q == main.alias);
        if belongs_main {
            let boolean = astersql_parser_mysql::r#type::IsBooleanFlag;
            let field = match (main.name.as_str(), name) {
                ("pg_class", "oid") => Some((0, crate::pg_oid::OID_TYPE, 0)),
                ("pg_class", "relname") => Some((1, 253, 0)),
                ("pg_class", "relnamespace") => Some((2, crate::pg_oid::OID_TYPE, 0)),
                ("pg_class", "relkind") => Some((3, 253, 0)),
                ("pg_database", "oid") => Some((0, crate::pg_oid::OID_TYPE, 0)),
                ("pg_database", "datname") => Some((1, 253, 0)),
                ("pg_database", "datistemplate") => Some((3, 1, boolean)),
                ("pg_database", "datallowconn") => Some((4, 1, boolean)),
                ("pg_database", "datdba") => Some((5, 8, 0)),
                ("pg_namespace", "oid") => Some((0, crate::pg_oid::OID_TYPE, 0)),
                ("pg_namespace", "nspname") => Some((1, 253, 0)),
                ("pg_namespace", "nspowner") => Some((5, 8, 0)),
                ("pg_namespace", "xmin") => Some((8, 8, 0)),
                ("pg_tablespace", "oid") => Some((0, crate::pg_oid::OID_TYPE, 0)),
                ("pg_tablespace", "spcname") => Some((1, 253, 0)),
                ("pg_tablespace", "spcowner") => Some((5, 8, 0)),
                // Optional ACL/options use the bounded catalog's nullable text
                // representation; no native tablespace rows currently exist.
                ("pg_tablespace", "spcacl") => Some((8, 253, 0)),
                ("pg_tablespace", "spcoptions") => Some((10, 253, 0)),
                ("pg_locks", "transactionid") => Some((0, 8, 0)),
                ("pg_description" | "pg_shdescription", _) => description_column(&main.name, name),
                _ => None,
            };
            if let Some(field) = field {
                return Ok(field);
            }
        }
        if let Some(join) = &self.select.join {
            if qualifier.is_none_or(|q| q == join.relation.alias) {
                if let Some(field) = description_column(&join.relation.name, name) {
                    return Ok(field);
                }
            }
        }
        Err((
            "0A000",
            format!("unsupported catalog column {}", path.join(".")),
        ))
    }
    fn expr_type(&self, expr: &Expr) -> ParseResult<(u8, usize)> {
        match expr {
            Expr::Column(path) => self.column(path).map(|(_, code, flags)| (code, flags)),
            Expr::Null | Expr::Text(_) => Ok((253, 0)),
            Expr::Integer(_) => Ok((8, 0)),
            Expr::Boolean(_) => Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag)),
            Expr::Cast(inner, target) => {
                let (code, _) = self.expr_type(inner)?;
                if *target == CastType::Bigint && code == 1 {
                    return Err((
                        "0A000",
                        "boolean to bigint catalog casts are unsupported".into(),
                    ));
                }
                Ok((
                    match target {
                        CastType::Bigint => 8,
                        CastType::Varchar => 253,
                        CastType::Oid => crate::pg_oid::OID_TYPE,
                        CastType::Regclass => crate::pg_oid::REGCLASS_TYPE,
                    },
                    0,
                ))
            }
            Expr::Call(path, args) => {
                let name = match path.as_slice() {
                    [name] => name.as_str(),
                    [catalog, name] if catalog == "pg_catalog" => name.as_str(),
                    _ => return Err(("0A000", "unsupported function qualification".into())),
                };
                for arg in args {
                    self.expr_type(arg)?;
                }
                match (name, args.as_slice()) {
                    ("current_database" | "current_catalog", [])
                        if self.select.from.name == "pg_database" =>
                    {
                        Ok((253, 0))
                    }
                    ("current_schema", []) if self.select.from.name == "pg_namespace" => {
                        Ok((253, 0))
                    }
                    ("pg_get_userbyid", [arg])
                        if matches!(arg, Expr::Null) || self.expr_type(arg)?.0 == 8 =>
                    {
                        Ok((253, 0))
                    }
                    ("age", [Expr::Column(path)])
                        if self.select.from.name == "pg_locks" && self.column(path)?.0 == 0 =>
                    {
                        Ok((8, 0))
                    }
                    _ => Err(("0A000", "unsupported catalog function or arguments".into())),
                }
            }
            Expr::Equal(left, right) | Expr::Compare(left, _, right) => {
                let left_type = self.expr_type(left)?.0;
                let right_type = self.expr_type(right)?.0;
                if !matches!(**left, Expr::Null)
                    && !matches!(**right, Expr::Null)
                    && left_type != right_type
                    && !(matches!(
                        left_type,
                        3 | 8 | crate::pg_oid::OID_TYPE | crate::pg_oid::REGCLASS_TYPE
                    ) && matches!(
                        right_type,
                        3 | 8 | crate::pg_oid::OID_TYPE | crate::pg_oid::REGCLASS_TYPE
                    ))
                {
                    return Err(("0A000", "incompatible catalog equality types".into()));
                }
                Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag))
            }
            Expr::And(left, right) | Expr::Or(left, right) => {
                if (self.expr_type(left)?.0 != 1 && !matches!(**left, Expr::Null))
                    || (self.expr_type(right)?.0 != 1 && !matches!(**right, Expr::Null))
                {
                    return Err((
                        "0A000",
                        "catalog logical operators require predicates".into(),
                    ));
                }
                Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag))
            }
            Expr::In(inner, values) => {
                for value in values {
                    self.expr_type(&Expr::Equal(inner.clone(), Box::new(value.clone())))?;
                }
                Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag))
            }
            Expr::Not(inner) => {
                if self.expr_type(inner)?.0 != 1 && !matches!(**inner, Expr::Null) {
                    return Err(("0A000", "catalog NOT requires a predicate".into()));
                }
                Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag))
            }
            Expr::NotNull(inner) | Expr::IsNull(inner) => {
                self.expr_type(inner)?;
                Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag))
            }
            Expr::Case { condition, yes, no } => {
                if self.expr_type(condition)?.0 != 1
                    || self.expr_type(yes)? != self.expr_type(no)?
                {
                    return Err(("0A000", "unsupported CASE types".into()));
                }
                self.expr_type(yes)
            }
        }
    }
    pub(crate) fn metadata(&self) -> PreparedMetadata {
        let fields = self
            .select
            .projections
            .iter()
            .map(|projection| {
                let (code, flags) = self
                    .expr_type(&projection.expr)
                    .expect("validated catalog expression");
                (projection.name.as_str(), code, flags)
            })
            .collect::<Vec<_>>();
        let columns = fields
            .iter()
            .map(|(name, code, flags)| ColumnInfo {
                schema: String::new(),
                table: String::new(),
                org_table: String::new(),
                name: (*name).into(),
                org_name: String::new(),
                charset: 45,
                column_length: 64,
                column_type: *code,
                flags: *flags as u16,
                decimals: 0,
                default_value: None,
            })
            .collect();
        let native_types = fields
            .into_iter()
            .map(|(_, code, flags)| NativeType {
                code,
                flags,
                length: 64,
                decimal: 0,
            })
            .collect();
        // Catalog statement identity belongs to the PG statement name, not an
        // engine prepared statement. No backend handle is allocated or closed.
        PreparedMetadata {
            statement_id: 0,
            parameter_count: 0,
            columns,
            native_types,
        }
    }
    pub(crate) fn execute(&self, context: &dyn TiDBContext) -> ConnResult<QueryResult> {
        let metadata = self.metadata();
        let snapshot = context.schema_snapshot();
        let mut database = String::new();
        let rows = match self.select.from.name.as_str() {
            "pg_class" | "pg_database" | "pg_namespace" => {
                let snapshot = snapshot
                    .as_ref()
                    .ok_or_else(|| ConnError::Session("schema snapshot is unavailable".into()))?;
                let current =
                    context.execute_query("SELECT DATABASE()", false, &CancellationToken::new())?;
                let selected_database = current
                    .first()
                    .and_then(|r| r.rows.first())
                    .and_then(|r| r.first());
                database = match selected_database {
                    Some(Value::Text(name)) => name.clone(),
                    _ => String::new(),
                };
                let schemas = snapshot.AllSchemas();
                if self.select.from.name == "pg_class" {
                    class_rows(snapshot.as_ref(), &database)?
                } else if self.select.from.name == "pg_namespace" {
                    // public is the current native database, never another database.
                    // pg_catalog is implicit even when absent from search_path.
                    let public = schemas
                        .iter()
                        .find(|s| s.name.lower == database.to_lowercase());
                    let mut rows = vec![namespace_row(11, "pg_catalog")];
                    if let Some(schema) = public {
                        rows.push(namespace_row(namespace_oid(schema.id)?, "public"));
                    }
                    rows
                } else {
                    // Native schema metadata has no shared-description rows. A
                    // LEFT JOIN to this empty relation keeps every schema and
                    // gives all right-side fields NULL, regardless of its predicate.
                    schemas
                        .into_iter()
                        .map(|schema| {
                            // Both projections refer to the same native database object.
                            let oid = namespace_oid(schema.id)?;
                            Ok(vec![
                                Value::Signed(oid),
                                Value::Text(schema.name.original.clone()),
                                // Native schemas have no PostgreSQL owner or description,
                                // and are neither template databases nor disabled databases.
                                Value::Null,
                                Value::Text("false".into()),
                                Value::Text("true".into()),
                                Value::Null,
                                Value::Null,
                                Value::Null,
                                // There is no PG XID source. Expose an explicitly
                                // nullable bigint state_number, never a native TSO.
                                Value::Null,
                                Value::Null,
                            ])
                        })
                        .collect::<ConnResult<Vec<_>>>()?
                }
            }
            // Native database/schema metadata has no comment source. These
            // relations are empty, not one NULL-description row per object.
            // The same column definitions serve direct queries and LEFT JOINs.
            "pg_description" | "pg_shdescription" => Vec::new(),
            // Native storage has no PostgreSQL tablespace source. Keep the
            // typed relation empty, without synthetic pg_default/pg_global
            // rows or exposing paths from the server host.
            "pg_tablespace" => Vec::new(),
            "pg_locks" => {
                // Native TSO start timestamps are monotonically ordered. They
                // are not PostgreSQL wraparound XIDs or fabricated lock rows.
                let results = context.execute_query(
                    "SELECT ID FROM information_schema.tidb_trx",
                    false,
                    &CancellationToken::new(),
                )?;
                let mut transactions = Vec::new();
                for result in &results {
                    for row in &result.rows {
                        let id = match row.first() {
                            Some(Value::Text(value)) => value.parse::<i64>().map_err(|e| {
                                ConnError::Session(format!("invalid native transaction ID: {e}"))
                            })?,
                            Some(Value::Signed(value)) => *value,
                            Some(Value::Unsigned(value)) => i64::try_from(*value).map_err(|e| {
                                ConnError::Session(format!(
                                    "native transaction ID exceeds bigint: {e}"
                                ))
                            })?,
                            Some(Value::Null) => continue,
                            _ => {
                                return Err(ConnError::Session(
                                    "missing native transaction ID".into(),
                                ));
                            }
                        };
                        transactions.push(vec![Value::Signed(id)]);
                    }
                }
                transactions
            }
            _ => unreachable!("validated catalog provider"),
        };
        let rows = self.project(rows, &database, snapshot.as_deref())?;
        Ok(QueryResult {
            columns: metadata.columns,
            native_types: metadata.native_types,
            rows,
            state: context.state(),
            response_lifecycle: None,
            result_set: None,
        })
    }
    fn evaluate(
        &self,
        expr: &Expr,
        row: &[Value],
        database: &str,
        snapshot: Option<&dyn astersql_infoschema::InfoSchema>,
    ) -> ConnResult<Value> {
        let evaluate = |expr: &Expr| self.evaluate(expr, row, database, snapshot);
        Ok(match expr {
            Expr::Column(path) => row[self.column(path).expect("validated column").0].clone(),
            Expr::Null => Value::Null,
            Expr::Integer(n) => Value::Signed(*n),
            Expr::Boolean(value) => Value::Text(value.to_string()),
            Expr::Text(s) => Value::Text(s.clone()),
            Expr::Cast(inner, target) => match (evaluate(inner)?, target) {
                (Value::Null, _) => Value::Null,
                (Value::Signed(n), CastType::Bigint) => Value::Signed(n),
                (Value::Signed(n), CastType::Oid | CastType::Regclass) => {
                    Value::Signed(i64::from(u32::try_from(n).map_err(|_| {
                        ConnError::Session("PG oid conversion out of range".into())
                    })?))
                }
                (Value::Text(s), CastType::Oid) => Value::Signed(i64::from(
                    u32::try_from(
                        s.trim()
                            .parse::<i128>()
                            .map_err(|_| ConnError::Session("invalid PG oid input".into()))?,
                    )
                    .map_err(|_| ConnError::Session("PG oid conversion out of range".into()))?,
                )),
                (Value::Text(s), CastType::Regclass) => {
                    let snapshot = snapshot.ok_or_else(|| {
                        ConnError::Session("schema snapshot is unavailable".into())
                    })?;
                    Value::Signed(i64::from(crate::pg_oid::resolve_with_path(
                        &s,
                        database,
                        snapshot,
                        self.public_first,
                    )?))
                }
                (Value::Signed(n), CastType::Varchar) => {
                    if self.expr_type(inner).expect("validated expression").0
                        == crate::pg_oid::REGCLASS_TYPE
                    {
                        let snapshot = snapshot.ok_or_else(|| {
                            ConnError::Session("schema snapshot is unavailable".into())
                        })?;
                        Value::Text(crate::pg_oid::display(
                            u32::try_from(n).map_err(|_| {
                                ConnError::Session("PG oid conversion out of range".into())
                            })?,
                            database,
                            snapshot,
                        )?)
                    } else {
                        Value::Text(n.to_string())
                    }
                }
                (Value::Text(s), CastType::Varchar) => Value::Text(s),
                (Value::Text(s), CastType::Bigint) => {
                    Value::Signed(s.parse().map_err(|_| {
                        ConnError::Session("catalog bigint conversion failed".into())
                    })?)
                }
                _ => return Err(ConnError::Session("unsupported catalog conversion".into())),
            },
            Expr::Call(path, args) => match path.last().unwrap().as_str() {
                "current_database" | "current_catalog" => Value::Text(database.into()),
                "current_schema" => self
                    .current_schema
                    .as_ref()
                    .map_or(Value::Null, |s| Value::Text(s.clone())),
                "pg_get_userbyid" => {
                    evaluate(&args[0])?;
                    Value::Null
                }
                // Preserve the native oldest-transaction meaning: smaller TSO
                // has greater age. This is ordering, not a fabricated PG XID.
                "age" => match evaluate(&args[0])? {
                    Value::Signed(n) => Value::Signed(n.checked_neg().ok_or_else(|| {
                        ConnError::Session(
                            "native transaction age ordering overflows bigint".into(),
                        )
                    })?),
                    _ => Value::Null,
                },
                _ => unreachable!("validated function"),
            },
            Expr::Equal(left, right) => match (evaluate(left)?, evaluate(right)?) {
                (Value::Null, _) | (_, Value::Null) => Value::Null,
                (left, right) => Value::Text((left == right).to_string()),
            },
            Expr::Compare(left, op, right) => match (evaluate(left)?, evaluate(right)?) {
                (Value::Null, _) | (_, Value::Null) => Value::Null,
                (left, right) => {
                    let order = match (left, right) {
                        (Value::Signed(a), Value::Signed(b)) => a.cmp(&b),
                        (Value::Text(a), Value::Text(b)) => a.cmp(&b),
                        _ => unreachable!("validated comparison types"),
                    };
                    use crate::pg_catalog_query::CompareOp::*;
                    Value::Text(
                        match op {
                            NotEqual => !order.is_eq(),
                            Less => order.is_lt(),
                            LessEqual => !order.is_gt(),
                            Greater => order.is_gt(),
                            GreaterEqual => !order.is_lt(),
                        }
                        .to_string(),
                    )
                }
            },
            Expr::In(inner, values) => {
                let value = evaluate(inner)?;
                let mut unknown = matches!(value, Value::Null);
                let mut found = false;
                for item in values {
                    let item = evaluate(item)?;
                    if matches!(item, Value::Null) {
                        unknown = true;
                    } else if !matches!(value, Value::Null) && value == item {
                        found = true;
                        break;
                    }
                }
                if found {
                    Value::Text("true".into())
                } else if unknown {
                    Value::Null
                } else {
                    Value::Text("false".into())
                }
            }
            Expr::Not(inner) => match evaluate(inner)? {
                Value::Null => Value::Null,
                Value::Text(value) => Value::Text((value != "true").to_string()),
                _ => unreachable!("validated predicate"),
            },
            Expr::IsNull(inner) => Value::Text(matches!(evaluate(inner)?, Value::Null).to_string()),
            Expr::Or(left, right) => match (evaluate(left)?, evaluate(right)?) {
                (Value::Text(left), _) if left == "true" => Value::Text("true".into()),
                (_, Value::Text(right)) if right == "true" => Value::Text("true".into()),
                (Value::Null, _) | (_, Value::Null) => Value::Null,
                _ => Value::Text("false".into()),
            },
            Expr::And(left, right) => match (evaluate(left)?, evaluate(right)?) {
                (Value::Text(left), _) if left == "false" => Value::Text("false".into()),
                (_, Value::Text(right)) if right == "false" => Value::Text("false".into()),
                (Value::Null, _) | (_, Value::Null) => Value::Null,
                _ => Value::Text("true".into()),
            },
            Expr::NotNull(inner) => {
                Value::Text((!matches!(evaluate(inner)?, Value::Null)).to_string())
            }
            Expr::Case { condition, yes, no } => {
                if matches!(evaluate(condition)?, Value::Text(s) if s == "true") {
                    evaluate(yes)?
                } else {
                    evaluate(no)?
                }
            }
        })
    }
    fn project(
        &self,
        rows: Vec<Vec<Value>>,
        database: &str,
        snapshot: Option<&dyn astersql_infoschema::InfoSchema>,
    ) -> ConnResult<Vec<Vec<Value>>> {
        let mut selected = Vec::new();
        for row in rows {
            if let Some(filter) = &self.select.filter {
                if !matches!(self.evaluate(filter, &row, database, snapshot)?, Value::Text(s) if s == "true")
                {
                    continue;
                }
            }
            let keys = self
                .select
                .order
                .iter()
                .map(|order| {
                    self.evaluate(
                        self.order_expr(&order.expr)
                            .expect("validated order expression"),
                        &row,
                        database,
                        snapshot,
                    )
                })
                .collect::<ConnResult<Vec<_>>>()?;
            selected.push((row, keys));
        }
        selected.sort_by(|(_, a), (_, b)| {
            for ((a, b), order) in a.iter().zip(b).zip(&self.select.order) {
                let comparison = match (a, b) {
                    (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
                    (Value::Null, _) => std::cmp::Ordering::Greater,
                    (_, Value::Null) => std::cmp::Ordering::Less,
                    (Value::Signed(a), Value::Signed(b)) => a.cmp(b),
                    (Value::Text(a), Value::Text(b)) => a.cmp(b),
                    _ => std::cmp::Ordering::Equal,
                };
                let comparison = if order.descending {
                    comparison.reverse()
                } else {
                    comparison
                };
                if !comparison.is_eq() {
                    return comparison;
                }
            }
            std::cmp::Ordering::Equal
        });
        selected
            .into_iter()
            .take(self.select.limit.unwrap_or(usize::MAX))
            .map(|(row, _)| {
                self.select
                    .projections
                    .iter()
                    .map(|projection| {
                        let value = self.evaluate(&projection.expr, &row, database, snapshot)?;
                        if self
                            .expr_type(&projection.expr)
                            .expect("validated expression")
                            .0
                            == crate::pg_oid::REGCLASS_TYPE
                        {
                            if let Value::Signed(oid) = value {
                                let snapshot = snapshot.ok_or_else(|| {
                                    ConnError::Session("schema snapshot is unavailable".into())
                                })?;
                                return Ok(Value::Text(crate::pg_oid::display(
                                    u32::try_from(oid).map_err(|_| {
                                        ConnError::Session("PG oid conversion out of range".into())
                                    })?,
                                    database,
                                    snapshot,
                                )?));
                            }
                        }
                        Ok(value)
                    })
                    .collect()
            })
            .collect()
    }
}

// All description fields are NULL-extended for an unmatched LEFT JOIN.
// Their row slots also define direct access to the empty description providers.
fn description_column(relation: &str, name: &str) -> Option<(usize, u8, usize)> {
    match (relation, name) {
        ("pg_description" | "pg_shdescription", "description") => Some((2, 253, 0)),
        ("pg_description" | "pg_shdescription", "objoid") => Some((6, 8, 0)),
        ("pg_description" | "pg_shdescription", "classoid") => Some((7, 8, 0)),
        ("pg_description", "objsubid") => Some((9, 3, 0)),
        _ => None,
    }
}

fn contains_age(expr: &Expr) -> bool {
    match expr {
        Expr::Call(path, args) => {
            path.last().is_some_and(|name| name == "age") || args.iter().any(contains_age)
        }
        Expr::Cast(inner, _) | Expr::NotNull(inner) | Expr::IsNull(inner) | Expr::Not(inner) => {
            contains_age(inner)
        }
        Expr::In(inner, values) => contains_age(inner) || values.iter().any(contains_age),
        Expr::Equal(left, right)
        | Expr::Compare(left, _, right)
        | Expr::And(left, right)
        | Expr::Or(left, right) => contains_age(left) || contains_age(right),
        Expr::Case { condition, yes, no } => {
            contains_age(condition) || contains_age(yes) || contains_age(no)
        }
        _ => false,
    }
}

/// PG-only checked namespace mapping shared with all later directory providers.
pub(crate) fn namespace_oid(id: i64) -> ConnResult<i64> {
    crate::pg_oid::namespace_oid(id).map(i64::from)
}

fn namespace_row(oid: i64, name: &str) -> Vec<Value> {
    vec![
        Value::Signed(oid),
        Value::Text(name.into()),
        Value::Null,
        Value::Text("false".into()),
        Value::Text("true".into()),
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
    ]
}

fn class_row(oid: u32, name: &str, namespace: u32, kind: &str) -> Vec<Value> {
    vec![
        Value::Signed(i64::from(oid)),
        Value::Text(name.into()),
        Value::Signed(i64::from(namespace)),
        Value::Text(kind.into()),
    ]
}

// One execution snapshot supplies identities and full model metadata. Native
// auto-increment columns are not sequences; only actual Sequence objects are S.
fn class_rows(
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
) -> ConnResult<Vec<Vec<Value>>> {
    let mut rows: Vec<_> = crate::pg_oid::SYSTEM_RELATIONS
        .iter()
        .map(|(name, oid)| class_row(*oid, name, 11, "r"))
        .collect();
    let Some(schema) = snapshot
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == database.to_lowercase())
    else {
        return Ok(rows);
    };
    let namespace = crate::pg_oid::namespace_oid(schema.id)?;
    for table in snapshot
        .SchemaTableInfos(&schema.name)
        .map_err(|e| ConnError::Session(e.to_string()))?
    {
        let model = table
            .model_meta
            .as_ref()
            .ok_or(ConnError::UnsupportedCommand(0))?;
        if model.State != astersql_meta_model::StatePublic {
            continue;
        }
        let partitioned = model.GetPartitionInfo().is_some();
        let kind = if model.View.is_some() {
            "v"
        } else if model.Sequence.is_some() {
            "S"
        } else if partitioned {
            "p"
        } else {
            "r"
        };
        rows.push(class_row(
            crate::pg_oid::table_oid(model.ID)?,
            &model.Name.O,
            namespace,
            kind,
        ));
        for index in &model.Indices {
            if index.State == astersql_meta_model::StatePublic {
                rows.push(class_row(
                    crate::pg_oid::index_oid(model.ID, index.ID)?,
                    &index.Name.O,
                    namespace,
                    if partitioned { "I" } else { "i" },
                ));
            }
        }
    }
    Ok(rows)
}
