// Copyright 2026 AsterSQL.
//! PG-only DataGrip catalog probes backed by canonical schema/transaction data.
use crate::conn::{
    CancellationToken, ColumnInfo, ConnError, ConnResult, NativeType, PreparedMetadata,
    QueryResult, TiDBContext, Value,
};

// Fixed provider slots preserve the existing catalog column layout. Joined
// relations get separate slots, including empty providers after LEFT JOIN.
const CATALOG_ROW_WIDTH: usize = 24;
const MAX_CATALOG_ROWS: usize = 16_384;
const MAX_CATALOG_JOIN_WORK: usize = 100_000;
type CteColumns = std::collections::HashMap<usize, Vec<(String, u8, usize)>>;
type CatalogRows = std::sync::Arc<Vec<Vec<Value>>>;

// Shared across every CTE and subquery in one Execute. Provider rows, native
// identity resolution and regclass conversions all use this single snapshot.
struct Execution<'a> {
    context: &'a dyn TiDBContext,
    cancel: &'a CancellationToken,
    snapshot: Option<astersql_infoschema::SchemaRef>,
    database: String,
    providers: std::cell::RefCell<std::collections::HashMap<String, CatalogRows>>,
    ctes: std::cell::RefCell<std::collections::HashMap<usize, CatalogRows>>,
    materialized: std::cell::Cell<usize>,
    work: std::cell::Cell<usize>,
}
impl Execution<'_> {
    fn comparison(&self) -> ConnResult<()> {
        check_catalog_cancel(self.cancel)?;
        let work = self.work.get() + 1;
        if work > MAX_CATALOG_JOIN_WORK {
            return Err(catalog_work_limit());
        }
        self.work.set(work);
        Ok(())
    }
    fn materialize(&self, rows: Vec<Vec<Value>>) -> ConnResult<CatalogRows> {
        check_catalog_cancel(self.cancel)?;
        self.materialized.set(self.materialized.get() + rows.len());
        if self.materialized.get() > MAX_CATALOG_ROWS {
            return Err(catalog_row_limit());
        }
        Ok(std::sync::Arc::new(rows))
    }
}
fn catalog_row_limit() -> ConnError {
    ConnError::Session("PG catalog row limit exceeded (16384 rows)".into())
}
fn catalog_work_limit() -> ConnError {
    ConnError::Session("PG catalog join work limit exceeded (100000 comparisons)".into())
}
fn check_catalog_cancel(cancel: &CancellationToken) -> ConnResult<()> {
    if cancel.is_cancelled() {
        Err(ConnError::Session("Query execution was interrupted".into()))
    } else {
        Ok(())
    }
}

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
    cte_columns: CteColumns,
    parameter_oids: Vec<u32>,
    parameter_values: Vec<Expr>,
}
impl CatalogQuery {
    pub(crate) fn parse(sql: &str) -> ParseResult<Option<Self>> {
        let Some(select) = pg_catalog_query::parse(sql)? else {
            return Ok(None);
        };
        let mut query = Self {
            select,
            current_schema: Some("public".into()),
            public_first: false,
            cte_columns: CteColumns::new(),
            parameter_oids: Vec::new(),
            parameter_values: Vec::new(),
        };
        query.parameters(&[])?;
        query.bind()?;
        Ok(Some(query))
    }
    pub(crate) fn parse_session(
        sql: &str,
        context: &dyn TiDBContext,
        session: &crate::pg_session::PgSession,
    ) -> ParseResult<Option<Self>> {
        let query = Self::parse_session_with_types(sql, context, session, &[])?;
        if query
            .as_ref()
            .is_some_and(|query| !query.parameter_oids.is_empty())
        {
            return Err(("42P02", "catalog parameters require Parse and Bind".into()));
        }
        Ok(query)
    }
    pub(crate) fn parse_session_with_types(
        sql: &str,
        context: &dyn TiDBContext,
        session: &crate::pg_session::PgSession,
        oids: &[u32],
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
        let mut query = Self {
            select,
            current_schema: session.schema().map(str::to_owned),
            public_first,
            cte_columns: CteColumns::new(),
            parameter_oids: Vec::new(),
            parameter_values: Vec::new(),
        };
        query.parameters(oids)?;
        query.bind()?;
        Ok(Some(query))
    }
    pub(crate) fn classify(sql: &str) -> Option<Self> {
        Self::parse(sql).ok().flatten()
    }
    fn nested(&self, select: Select) -> Self {
        Self {
            select,
            current_schema: self.current_schema.clone(),
            public_first: self.public_first,
            cte_columns: self.cte_columns.clone(),
            parameter_oids: self.parameter_oids.clone(),
            parameter_values: self.parameter_values.clone(),
        }
    }
    fn correlated(&self, select: &Select, row: Option<&[Value]>) -> ParseResult<Self> {
        let mut query = self.nested(select.clone());
        let local = query.clone();
        visit_select_exprs(&mut query.select, &mut |expr| {
            if let Expr::Column(path) = expr {
                // Local aliases shadow outer ones; unqualified names resolve
                // locally first. Only unresolved names can be correlated.
                if local.column(path).is_ok() {
                    return Ok(());
                }
                if path.len() > 1 && local.relations().any(|r| r.alias == path[path.len() - 2]) {
                    return Ok(());
                }
                let (slot, code, flags) = self.column(path)?;
                let value = row.map_or(Expr::Null, |row| literal(&row[slot], code));
                *expr = Expr::TypedLiteral(Box::new(value), code, flags);
            }
            Ok(())
        })?;
        Ok(query)
    }
    // Parameters are indexed across the entire statement, including CTEs and
    // subqueries. Only a direct ::oid cast infers an otherwise unknown type.
    fn parameters(&mut self, supplied: &[u32]) -> ParseResult<()> {
        let mut select = self.select.clone();
        let mut count = 0;
        visit_all_select_exprs(&mut select, &mut |expr| {
            if let Expr::Parameter(index) = expr {
                count = count.max(*index + 1);
            }
            Ok::<_, (&'static str, String)>(())
        })?;
        if supplied.len() > count {
            return Err(("08P01", "too many parameter OIDs".into()));
        }
        self.parameter_oids = supplied.to_vec();
        self.parameter_oids.resize(count, 0);
        visit_all_select_exprs(&mut select, &mut |expr| {
            if let Expr::Cast(inner, CastType::Oid) = expr {
                if let Expr::Parameter(index) = **inner {
                    if self.parameter_oids[index] == 0 {
                        self.parameter_oids[index] = 26;
                    }
                }
            }
            Ok::<_, (&'static str, String)>(())
        })?;
        for oid in &self.parameter_oids {
            catalog_parameter_type(*oid)?;
        }
        Ok(())
    }
    pub(crate) fn parameter_oids(&self) -> &[u32] {
        &self.parameter_oids
    }
    pub(crate) fn bind_values(&mut self, values: Vec<Expr>) -> ParseResult<()> {
        if values.len() != self.parameter_oids.len() {
            return Err(("08P01", "catalog parameter count mismatch".into()));
        }
        self.parameter_values = values;
        Ok(())
    }
    fn bind(&mut self) -> ParseResult<()> {
        for cte in self.select.ctes.clone() {
            if cte.query.projections.len() > 11 {
                return Err((
                    "0A000",
                    "catalog CTEs support at most eleven columns".into(),
                ));
            }
            let mut query = self.nested(cte.query);
            query.bind()?;
            self.cte_columns.extend(query.cte_columns.clone());
            let fields = query
                .select
                .projections
                .iter()
                .map(|p| {
                    query
                        .expr_type(&p.expr)
                        .map(|(code, flags)| (p.name.clone(), code, flags))
                })
                .collect::<ParseResult<Vec<_>>>()?;
            self.cte_columns.insert(cte.id, fields);
        }
        // Bind subqueries independently: outer relation aliases never leak in.
        let mut select = self.select.clone();
        visit_select_exprs(&mut select, &mut |expr| {
            if let Expr::InSubquery(_, select) = expr {
                let mut query = self.nested((**select).clone());
                query.bind()?;
                self.cte_columns.extend(query.cte_columns);
            }
            Ok(())
        })?;
        self.validate()
    }
    fn validate(&self) -> ParseResult<()> {
        let mut aliases = std::collections::HashSet::new();
        for relation in self.relations() {
            if relation.cte_id.is_none()
                && !matches!(
                    relation.name.as_str(),
                    "pg_class"
                        | "pg_attribute"
                        | "pg_type"
                        | "pg_attrdef"
                        | "pg_index"
                        | "pg_constraint"
                        | "pg_proc"
                        | "pg_language"
                        | "pg_depend"
                        | "pg_database"
                        | "pg_locks"
                        | "pg_namespace"
                        | "pg_tablespace"
                        | "pg_description"
                        | "pg_shdescription"
                        | "pg_inherits"
                        | "pg_opclass"
                )
            {
                return Err(("0A000", "catalog provider is not implemented yet".into()));
            }
            if !aliases.insert(&relation.alias) {
                return Err(("42712", "duplicate catalog relation alias".into()));
            }
        }
        for (index, join) in self.select.joins.iter().enumerate() {
            // ON binds only to the accumulated left side and this right side.
            // Later aliases must not accidentally read uninitialized row slots.
            let mut scope = self.clone();
            scope.select.joins.truncate(index + 1);
            if contains_age(&join.on) || scope.expr_type(&join.on)?.0 != 1 {
                return Err((
                    "0A000",
                    "catalog JOIN must be a predicate without age".into(),
                ));
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
        if self
            .select
            .projections
            .iter()
            .any(|p| has_aggregate(&p.expr))
        {
            for p in &self.select.projections {
                if ungrouped_column(&p.expr) {
                    return Err((
                        "42803",
                        "catalog aggregate contains an ungrouped column".into(),
                    ));
                }
            }
        }
        Ok(())
    }
    fn order_expr<'a>(&'a self, expr: &'a Expr) -> ParseResult<&'a Expr> {
        if let Expr::Integer(position) = expr {
            return usize::try_from(*position)
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|n| self.select.projections.get(n))
                .map(|p| &p.expr)
                .ok_or_else(|| {
                    (
                        "42P10",
                        "catalog ORDER BY position is not in select list".into(),
                    )
                });
        }
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
        let mut found = None;
        for (index, relation) in self.relations().enumerate() {
            if (path.len() == 3 && relation.cte_id.is_some())
                || qualifier.is_some_and(|q| q != relation.alias)
            {
                continue;
            }
            let boolean = astersql_parser_mysql::r#type::IsBooleanFlag;
            let field = if let Some(id) = relation.cte_id {
                let fields = &self.cte_columns[&id];
                let mut matches = fields.iter().enumerate().filter(|(_, (n, _, _))| n == name);
                let field = matches
                    .next()
                    .map(|(slot, (_, code, flags))| (slot, *code, *flags));
                if matches.next().is_some() {
                    return Err(("42702", "ambiguous CTE column".into()));
                }
                field
            } else {
                match (relation.name.as_str(), name) {
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
                    ("pg_description" | "pg_shdescription", _) => {
                        description_column(&relation.name, name)
                    }
                    _ => column_catalog_field(&relation.name, name),
                }
            };
            if let Some((slot, code, flags)) = field {
                if found.is_some() {
                    return Err(("42702", "ambiguous catalog column".into()));
                }
                found = Some((index * CATALOG_ROW_WIDTH + slot, code, flags));
            }
        }
        if let Some(field) = found {
            return Ok(field);
        }
        Err((
            "0A000",
            format!("unsupported catalog column {}", path.join(".")),
        ))
    }
    fn expr_type(&self, expr: &Expr) -> ParseResult<(u8, usize)> {
        match expr {
            Expr::Column(path) => self.column(path).map(|(_, code, flags)| (code, flags)),
            Expr::Parameter(index) => catalog_parameter_type(self.parameter_oids[*index]),
            Expr::Null | Expr::Text(_) => Ok((253, 0)),
            Expr::TypedLiteral(_, code, flags) => Ok((*code, *flags)),
            Expr::ScalarSubquery(select) => {
                let mut query = self.correlated(select, None)?;
                query.bind()?;
                if query.select.projections.len() != 1 {
                    return Err(("42601", "scalar subquery requires one column".into()));
                }
                query.expr_type(&query.select.projections[0].expr)
            }
            Expr::Any(value, input) => {
                let element = numeric_array_element(self.expr_type(input)?.0)
                    .ok_or_else(|| ("0A000", "ANY requires a numeric catalog array".into()))?;
                self.expr_type(&Expr::Equal(
                    value.clone(),
                    Box::new(Expr::TypedLiteral(Box::new(Expr::Null), element, 0)),
                ))?;
                Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag))
            }
            Expr::ArrayUnnest { input, projection } => {
                let element = numeric_array_element(self.expr_type(input)?.0)
                    .ok_or_else(|| ("0A000", "unnest requires a numeric catalog array".into()))?;
                let projection = unnest_projection(
                    projection,
                    &Expr::TypedLiteral(Box::new(Expr::Null), element, 0),
                )?;
                let code = self.expr_type(&projection)?.0;
                let array = match code {
                    2 => crate::pg_result::CatalogColumnType::Int2Array,
                    3 => crate::pg_result::CatalogColumnType::Int4Array,
                    8 => crate::pg_result::CatalogColumnType::Int8Array,
                    crate::pg_oid::OID_TYPE => crate::pg_result::CatalogColumnType::OidArray,
                    253 => crate::pg_result::CatalogColumnType::TextArray,
                    _ => return Err(("0A000", "unsupported unnest projection type".into())),
                };
                Ok((array as u8, 0))
            }
            Expr::ArrayAgg(inner, order) => {
                for key in order {
                    self.expr_type(&key.expr)?;
                }
                if self.expr_type(inner)?.0 != 8 {
                    return Err(("0A000", "array_agg currently requires bigint input".into()));
                }
                Ok((crate::pg_result::CatalogColumnType::Int8Array as u8, 0))
            }
            Expr::Integer(_) => Ok((8, 0)),
            Expr::Boolean(_) => Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag)),
            Expr::Cast(inner, target) => {
                let (code, _) = self.expr_type(inner)?;
                if *target == CastType::InternalChar
                    && !matches!(**inner, Expr::Null)
                    && !textual_type(code)
                {
                    return Err(("0A000", "catalog internal char casts require text".into()));
                }
                if *target == CastType::Bigint && code == 1 {
                    return Err((
                        "0A000",
                        "boolean to bigint catalog casts are unsupported".into(),
                    ));
                }
                if *target == CastType::IntArray
                    && !matches!(**inner, Expr::Null)
                    && numeric_array_element(code).is_none()
                {
                    return Err(("0A000", "int[] requires a numeric catalog array".into()));
                }
                if *target == CastType::OperatorName
                    && !matches!(**inner, Expr::Null)
                    && !numeric_type(code)
                {
                    return Err(("0A000", "regoper requires an operator OID".into()));
                }
                Ok((
                    match target {
                        CastType::InternalChar => {
                            crate::pg_result::CatalogColumnType::InternalChar as u8
                        }
                        CastType::Bigint => 8,
                        CastType::Varchar => 253,
                        CastType::Oid => crate::pg_oid::OID_TYPE,
                        CastType::Regclass => crate::pg_oid::REGCLASS_TYPE,
                        // No native exclusion operators have a PG name mapping.
                        // The only supported regoper value is zero, displayed as '-'.
                        CastType::OperatorName => 253,
                        CastType::IntArray => crate::pg_result::CatalogColumnType::Int4Array as u8,
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
                    (name, [oid])
                        if FUNCTION_INTROSPECTION
                            .iter()
                            .any(|function| function.name == name)
                            && (matches!(oid, Expr::Null)
                                || numeric_type(self.expr_type(oid)?.0)) =>
                    {
                        Ok((253, 0))
                    }
                    (
                        "pg_get_partkeydef"
                        | "pg_get_indexdef"
                        | "pg_get_constraintdef"
                        | "pg_get_viewdef",
                        [oid],
                    ) if matches!(oid, Expr::Null) || numeric_type(self.expr_type(oid)?.0) => {
                        Ok((253, 0))
                    }
                    ("pg_get_constraintdef" | "pg_get_viewdef", [oid, pretty])
                        if (matches!(oid, Expr::Null) || numeric_type(self.expr_type(oid)?.0))
                            && (matches!(pretty, Expr::Null) || self.expr_type(pretty)?.0 == 1) =>
                    {
                        Ok((253, 0))
                    }
                    ("pg_get_indexdef", [oid, position, pretty])
                        if (matches!(oid, Expr::Null) || numeric_type(self.expr_type(oid)?.0))
                            && (matches!(position, Expr::Null)
                                || numeric_type(self.expr_type(position)?.0))
                            && (matches!(pretty, Expr::Null) || self.expr_type(pretty)?.0 == 1) =>
                    {
                        Ok((253, 0))
                    }
                    ("format_type", [oid, modifier])
                        if (matches!(oid, Expr::Null) || numeric_type(self.expr_type(oid)?.0))
                            && (matches!(modifier, Expr::Null)
                                || numeric_type(self.expr_type(modifier)?.0)) =>
                    {
                        Ok((253, 0))
                    }
                    ("pg_get_expr", [expression, relation])
                        if (matches!(expression, Expr::Null)
                            || self.expr_type(expression)?.0 == 253)
                            && (matches!(relation, Expr::Null)
                                || numeric_type(self.expr_type(relation)?.0)) =>
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
                    && !(numeric_type(left_type) && numeric_type(right_type))
                    && !(textual_type(left_type) && textual_type(right_type))
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
            Expr::InSubquery(inner, select) => {
                if select.projections.len() != 1 {
                    return Err((
                        "0A000",
                        "catalog IN subqueries require exactly one column".into(),
                    ));
                }
                let query = self.nested((**select).clone());
                query.validate()?;
                let left = self.expr_type(inner)?.0;
                let projection = &select.projections[0].expr;
                let right = query.expr_type(projection)?.0;
                if !matches!(**inner, Expr::Null)
                    && !matches!(projection, Expr::Null)
                    && left != right
                    && !(numeric_type(left) && numeric_type(right))
                    && !(textual_type(left) && textual_type(right))
                {
                    return Err(("0A000", "incompatible catalog IN subquery types".into()));
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
            parameter_count: self.parameter_oids.len(),
            columns,
            native_types,
        }
    }
    fn relations(&self) -> impl Iterator<Item = &pg_catalog_query::Relation> {
        std::iter::once(&self.select.from)
            .chain(self.select.joins.iter().map(|join| &join.relation))
    }
    pub(crate) fn execute(
        &self,
        context: &dyn TiDBContext,
        cancel: &CancellationToken,
    ) -> ConnResult<QueryResult> {
        check_catalog_cancel(cancel)?;
        if self.parameter_values.len() != self.parameter_oids.len() {
            return Err(ConnError::Session("unbound PG catalog parameters".into()));
        }
        let metadata = self.metadata();
        let snapshot = context.schema_snapshot();
        let current = context.execute_query("SELECT DATABASE()", false, cancel)?;
        let database = match current
            .first()
            .and_then(|r| r.rows.first())
            .and_then(|r| r.first())
        {
            Some(Value::Text(name)) => name.clone(),
            _ => String::new(),
        };
        let mut execution = Execution {
            context,
            cancel,
            snapshot,
            database,
            providers: Default::default(),
            ctes: Default::default(),
            materialized: std::cell::Cell::new(0),
            work: std::cell::Cell::new(0),
        };
        let rows = self.execute_select(&mut execution, true)?;
        Ok(QueryResult {
            columns: metadata.columns,
            native_types: metadata.native_types,
            rows,
            state: context.state(),
            response_lifecycle: None,
            result_set: None,
        })
    }
    fn execute_select(
        &self,
        execution: &Execution<'_>,
        display: bool,
    ) -> ConnResult<Vec<Vec<Value>>> {
        check_catalog_cancel(execution.cancel)?;
        for cte in &self.select.ctes {
            if !execution.ctes.borrow().contains_key(&cte.id) {
                let rows = self
                    .nested(cte.query.clone())
                    .execute_select(execution, false)?;
                let mut rows = rows;
                for row in &mut rows {
                    row.resize(CATALOG_ROW_WIDTH, Value::Null);
                }
                let rows = execution.materialize(rows)?;
                execution.ctes.borrow_mut().insert(cte.id, rows);
            }
        }
        let mut query = self.clone();
        visit_select_exprs(&mut query.select, &mut |expr| {
            if let Expr::InSubquery(inner, select) = expr {
                let rows = self
                    .nested((**select).clone())
                    .execute_select(execution, false)?;
                let rows = execution.materialize(rows)?;
                let code = self
                    .nested((**select).clone())
                    .expr_type(&select.projections[0].expr)
                    .expect("validated subquery")
                    .0;
                let values = rows
                    .iter()
                    .map(|row| match &row[0] {
                        Value::Null => Expr::Null,
                        Value::Signed(n) => Expr::Integer(*n),
                        Value::Text(s) if code == 1 => Expr::Boolean(s == "true"),
                        Value::Text(s) => Expr::Text(s.clone()),
                        _ => unreachable!("catalog scalar type"),
                    })
                    .collect();
                *expr = Expr::In(inner.clone(), values);
            }
            Ok(())
        })?;
        query.execute_rows(execution, display)
    }
    fn execute_rows(
        &self,
        execution: &Execution<'_>,
        display: bool,
    ) -> ConnResult<Vec<Vec<Value>>> {
        // CTE rows have the same fixed slots as providers, but distinct IDs.
        let mut providers = Vec::new();
        for relation in self.relations() {
            check_catalog_cancel(execution.cancel)?;
            if let Some(id) = relation.cte_id {
                providers.push(execution.ctes.borrow()[&id].clone());
            } else {
                if !execution.providers.borrow().contains_key(&relation.name) {
                    let mut rows = self.provider_rows(
                        &relation.name,
                        execution.context,
                        execution.snapshot.as_ref(),
                        &execution.database,
                        execution.cancel,
                    )?;
                    for row in &mut rows {
                        row.resize(CATALOG_ROW_WIDTH, Value::Null);
                    }
                    let rows = execution.materialize(rows)?;
                    execution
                        .providers
                        .borrow_mut()
                        .insert(relation.name.clone(), rows);
                }
                providers.push(execution.providers.borrow()[&relation.name].clone());
            }
        }
        let mut rows = (*providers[0]).clone();
        for (index, join) in self.select.joins.iter().enumerate() {
            let mut scope = self.clone();
            scope.select.joins.truncate(index + 1);
            let right = providers[index + 1].as_ref();
            let mut joined = Vec::new();
            for left in rows {
                check_catalog_cancel(execution.cancel)?;
                let mut matched = false;
                for right in right {
                    execution.comparison()?;
                    let mut candidate = left.clone();
                    candidate.extend_from_slice(right);
                    if matches!(scope.evaluate(&join.on, &candidate, execution)?, Value::Text(s) if s == "true")
                    {
                        matched = true;
                        joined.push(candidate);
                        if joined.len() > MAX_CATALOG_ROWS {
                            return Err(catalog_row_limit());
                        }
                    }
                }
                if join.left && !matched {
                    let mut candidate = left;
                    candidate.extend(vec![Value::Null; CATALOG_ROW_WIDTH]);
                    joined.push(candidate);
                    if joined.len() > MAX_CATALOG_ROWS {
                        return Err(catalog_row_limit());
                    }
                }
            }
            rows = joined;
        }
        self.project(rows, execution, display)
    }
    fn provider_rows(
        &self,
        name: &str,
        context: &dyn TiDBContext,
        snapshot: Option<&astersql_infoschema::SchemaRef>,
        database: &str,
        cancel: &CancellationToken,
    ) -> ConnResult<Vec<Vec<Value>>> {
        let rows = match name {
            "pg_depend" => sequence_dependency_rows(
                snapshot
                    .ok_or_else(|| ConnError::Session("schema snapshot is unavailable".into()))?
                    .as_ref(),
                database,
                cancel,
            )?,
            "pg_proc" => function_rows(),
            // Native physical partitions are not independent SQL relations in
            // this adapter. There are no PG inheritance edges to those objects.
            "pg_inherits" | "pg_opclass" => Vec::new(),
            "pg_language" => vec![vec![
                Value::Signed(INTERNAL_LANGUAGE_OID),
                Value::Text("internal".into()),
            ]],
            "pg_index" | "pg_constraint" => index_constraint_rows(
                name,
                snapshot
                    .ok_or_else(|| ConnError::Session("schema snapshot is unavailable".into()))?
                    .as_ref(),
                database,
                cancel,
            )?,
            "pg_attribute" | "pg_attrdef" | "pg_type" => column_catalog_rows(
                name,
                snapshot
                    .ok_or_else(|| ConnError::Session("schema snapshot is unavailable".into()))?
                    .as_ref(),
                database,
                cancel,
            )?,
            "pg_class" | "pg_database" | "pg_namespace" => {
                let snapshot = snapshot
                    .ok_or_else(|| ConnError::Session("schema snapshot is unavailable".into()))?;
                let schemas = snapshot.AllSchemas();
                if name == "pg_class" {
                    class_rows(snapshot.as_ref(), database)?
                } else if name == "pg_namespace" {
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
                    cancel,
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
        Ok(rows)
    }
    fn evaluate(&self, expr: &Expr, row: &[Value], execution: &Execution<'_>) -> ConnResult<Value> {
        let database = execution.database.as_str();
        let snapshot = execution.snapshot.as_deref();
        let evaluate = |expr: &Expr| self.evaluate(expr, row, execution);
        Ok(match expr {
            Expr::Column(path) => row[self.column(path).expect("validated column").0].clone(),
            Expr::Null => Value::Null,
            Expr::TypedLiteral(inner, _, _) => evaluate(inner)?,
            Expr::ArrayAgg(_, _) => return Err(ConnError::UnsupportedCommand(0)),
            Expr::Any(value, input) => {
                let value = evaluate(value)?;
                let input = evaluate(input)?;
                if input == Value::Null {
                    return Ok(Value::Null);
                }
                let values = numeric_array_values(&input)?;
                let mut unknown = false;
                for element in values {
                    execution.comparison()?;
                    if value == Value::Null || element == Value::Null {
                        unknown = true;
                    } else if value == element {
                        return Ok(Value::Text("true".into()));
                    }
                }
                if unknown {
                    Value::Null
                } else {
                    Value::Text("false".into())
                }
            }
            Expr::ArrayUnnest { input, projection } => {
                let element_code = self.expr_type(input).expect("validated array").0;
                let input = evaluate(input)?;
                let element = numeric_array_element(element_code).unwrap();
                let mut values = Vec::new();
                for value in numeric_array_values(&input)? {
                    execution.comparison()?;
                    let literal =
                        Expr::TypedLiteral(Box::new(literal(&value, element)), element, 0);
                    let projection = unnest_projection(projection, &literal)
                        .map_err(|(_, message)| ConnError::Session(message))?;
                    values.push(evaluate(&projection)?);
                }
                Value::Text(array_text(values.into_iter()))
            }
            Expr::ScalarSubquery(select) => {
                let query = self
                    .correlated(select, Some(row))
                    .map_err(|(_, message)| ConnError::Session(message))?;
                let rows = query.execute_select(execution, false)?;
                if rows.len() > 1 {
                    return Err(ConnError::Session(
                        "PG scalar subquery returned more than one row".into(),
                    ));
                }
                rows.first()
                    .and_then(|row| row.first())
                    .cloned()
                    .unwrap_or(Value::Null)
            }
            Expr::Parameter(index) => evaluate(&self.parameter_values[*index])?,
            Expr::Integer(n) => Value::Signed(*n),
            Expr::Boolean(value) => Value::Text(value.to_string()),
            Expr::Text(s) => Value::Text(s.clone()),
            Expr::Cast(inner, target) => match (evaluate(inner)?, target) {
                (Value::Null, _) => Value::Null,
                (Value::Text(s), CastType::IntArray) => {
                    let values = numeric_array_values(&Value::Text(s))?;
                    for value in &values {
                        if let Value::Signed(n) = value {
                            i32::try_from(*n).map_err(|_| {
                                ConnError::Session("PG int[] conversion out of range".into())
                            })?;
                        }
                    }
                    Value::Text(array_text(values.into_iter()))
                }
                (Value::Signed(0), CastType::OperatorName) => Value::Text("-".into()),
                (Value::Signed(_), CastType::OperatorName) => {
                    return Err(ConnError::UnsupportedCommand(0));
                }
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
                (Value::Text(s), CastType::InternalChar) if s.len() == 1 && s.is_ascii() => {
                    Value::Text(s)
                }
                (Value::Text(_), CastType::InternalChar) => {
                    return Err(ConnError::UnsupportedCommand(0));
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
                "pg_get_function_arguments"
                | "pg_get_function_result"
                | "pg_get_function_sqlbody" => {
                    let value = evaluate(&args[0])?;
                    if value == Value::Null {
                        return Ok(Value::Null);
                    }
                    let Value::Signed(oid) = value else {
                        unreachable!("validated function oid");
                    };
                    let function = FUNCTION_INTROSPECTION
                        .iter()
                        .find(|function| function.oid == oid)
                        .ok_or(ConnError::UnsupportedCommand(0))?;
                    match path.last().unwrap().as_str() {
                        "pg_get_function_arguments" => Value::Text(function.arguments.into()),
                        "pg_get_function_result" => Value::Text(function.result.into()),
                        // Native PG adapter primitives have no SQL-language body.
                        _ => Value::Null,
                    }
                }
                "pg_get_viewdef" => {
                    let values = args.iter().map(evaluate).collect::<ConnResult<Vec<_>>>()?;
                    if values.contains(&Value::Null) {
                        return Ok(Value::Null);
                    }
                    let Value::Signed(oid) = values[0] else {
                        unreachable!("validated view oid");
                    };
                    let snapshot = snapshot.ok_or_else(|| {
                        ConnError::Session("schema snapshot is unavailable".into())
                    })?;
                    let mut definition = Value::Null;
                    if let Some(schema) = snapshot
                        .AllSchemas()
                        .into_iter()
                        .find(|s| s.name.lower == database.to_lowercase())
                    {
                        for table in snapshot
                            .SchemaTableInfos(&schema.name)
                            .map_err(|e| ConnError::Session(e.to_string()))?
                        {
                            execution.comparison()?;
                            let model = table
                                .model_meta
                                .as_ref()
                                .ok_or(ConnError::UnsupportedCommand(0))?;
                            if model.State == astersql_meta_model::StatePublic
                                && i64::from(crate::pg_oid::table_oid(model.ID)?) == oid
                            {
                                if let Some(view) = &model.View {
                                    // Return the persisted source, never infer a definition
                                    // from columns or pretty-print through a different parser.
                                    definition = Value::Text(
                                        crate::pg_name::stored_view_select(&view.SelectStmt)
                                            .map_err(|(_, message)| ConnError::Session(message))?,
                                    );
                                }
                                break;
                            }
                        }
                    }
                    definition
                }
                "pg_get_indexdef" | "pg_get_constraintdef" => {
                    let values = args.iter().map(evaluate).collect::<ConnResult<Vec<_>>>()?;
                    if values.contains(&Value::Null) {
                        return Ok(Value::Null);
                    }
                    let Value::Signed(oid) = values[0] else {
                        unreachable!("validated oid");
                    };
                    let provider = if path.last().unwrap() == "pg_get_indexdef" {
                        "pg_index"
                    } else {
                        "pg_constraint"
                    };
                    let fallback;
                    let providers = execution.providers.borrow();
                    let rows = if let Some(rows) = providers.get(provider) {
                        rows.as_slice()
                    } else {
                        fallback = index_constraint_rows(
                            provider,
                            snapshot.ok_or_else(|| {
                                ConnError::Session("schema snapshot is unavailable".into())
                            })?,
                            database,
                            execution.cancel,
                        )?;
                        fallback.as_slice()
                    };
                    let mut result = Value::Null;
                    for row in rows {
                        execution.comparison()?;
                        if row[0] != Value::Signed(oid) {
                            continue;
                        }
                        result = row[21].clone();
                        if let [_, Value::Signed(position), _] = values.as_slice() {
                            if *position != 0 {
                                result = match &row[22] {
                                    Value::Text(columns) => usize::try_from(*position)
                                        .ok()
                                        .and_then(|n| n.checked_sub(1))
                                        .and_then(|n| columns.split('\0').nth(n))
                                        .map_or(Value::Null, |name| Value::Text(name.into())),
                                    _ => Value::Null,
                                };
                            }
                        }
                        break;
                    }
                    result
                }
                "pg_get_partkeydef" => match evaluate(&args[0])? {
                    Value::Null => Value::Null,
                    Value::Signed(oid) => {
                        let snapshot = snapshot.ok_or_else(|| {
                            ConnError::Session("schema snapshot is unavailable".into())
                        })?;
                        let mut key = Value::Null;
                        let tables = snapshot
                            .SchemaTableInfos(&astersql_infoschema::CiString::new(database))
                            .map_err(|e| ConnError::Session(e.to_string()))?;
                        for table in tables {
                            if i64::from(crate::pg_oid::table_oid(table.id)?) != oid {
                                continue;
                            }
                            let model = table
                                .model_meta
                                .as_ref()
                                .ok_or(ConnError::UnsupportedCommand(0))?;
                            if let Some(partition) = model.GetPartitionInfo() {
                                let columns = if partition.Columns.is_empty() {
                                    let column = partition.Expr.trim().trim_matches('`');
                                    if column.is_empty()
                                        || !column.chars().all(|c| c.is_alphanumeric() || c == '_')
                                    {
                                        return Err(ConnError::UnsupportedCommand(0));
                                    }
                                    pg_identifier(column)
                                } else {
                                    partition
                                        .Columns
                                        .iter()
                                        .map(|c| pg_identifier(&c.O))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                };
                                key = Value::Text(format!("{} ({columns})", partition.Type));
                            }
                            break;
                        }
                        key
                    }
                    _ => unreachable!("validated partition key"),
                },
                "format_type" => match (evaluate(&args[0])?, evaluate(&args[1])?) {
                    (Value::Null, _) => Value::Null,
                    (Value::Signed(oid), modifier) => Value::Text(format_column_type(
                        oid,
                        match modifier {
                            Value::Signed(n) => Some(n),
                            Value::Null => None,
                            _ => unreachable!(),
                        },
                    )?),
                    _ => unreachable!("validated format_type"),
                },
                "pg_get_expr" => match (evaluate(&args[0])?, evaluate(&args[1])?) {
                    (Value::Null, _) | (_, Value::Null) => Value::Null,
                    (Value::Text(encoded), Value::Signed(relation)) => {
                        let (owner, expression) = encoded
                            .split_once(':')
                            .ok_or(ConnError::UnsupportedCommand(0))?;
                        if owner.parse::<i64>().ok() != Some(relation) {
                            return Err(ConnError::UnsupportedCommand(0));
                        }
                        Value::Text(expression.into())
                    }
                    _ => return Err(ConnError::UnsupportedCommand(0)),
                },
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
                let mut unknown = !values.is_empty() && matches!(value, Value::Null);
                let mut found = false;
                for item in values {
                    execution.comparison()?;
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
            Expr::InSubquery(_, _) => unreachable!("materialized subquery"),
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
        execution: &Execution<'_>,
        display: bool,
    ) -> ConnResult<Vec<Vec<Value>>> {
        let database = execution.database.as_str();
        let snapshot = execution.snapshot.as_deref();
        let mut selected = Vec::new();
        for row in rows {
            check_catalog_cancel(execution.cancel)?;
            if let Some(filter) = &self.select.filter {
                if !matches!(self.evaluate(filter, &row, execution)?, Value::Text(s) if s == "true")
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
                        execution,
                    )
                })
                .collect::<ConnResult<Vec<_>>>()?;
            selected.push((row, keys));
        }
        if self
            .select
            .projections
            .iter()
            .any(|p| has_aggregate(&p.expr))
        {
            let rows = selected.into_iter().map(|(row, _)| row).collect::<Vec<_>>();
            let mut result = Vec::new();
            for p in &self.select.projections {
                let mut expr = p.expr.clone();
                visit_expr(&mut expr, &mut |expr| {
                    if let Expr::ArrayAgg(value, order) = expr {
                        let mut values = Vec::new();
                        for row in &rows {
                            execution.comparison()?;
                            let keys = order
                                .iter()
                                .map(|o| self.evaluate(&o.expr, row, execution))
                                .collect::<ConnResult<Vec<_>>>()?;
                            values.push((self.evaluate(value, row, execution)?, keys));
                        }
                        values.sort_by(|(_, a), (_, b)| compare_keys(a, b, order));
                        let value = if values.is_empty() {
                            Value::Null
                        } else {
                            Value::Text(array_text(values.into_iter().map(|(v, _)| v)))
                        };
                        *expr = Expr::TypedLiteral(
                            Box::new(literal(&value, 253)),
                            crate::pg_result::CatalogColumnType::Int8Array as u8,
                            0,
                        );
                    }
                    Ok::<_, ConnError>(())
                })?;
                result.push(self.evaluate(&expr, &[], execution)?);
            }
            return Ok(if self.select.limit == Some(0) {
                Vec::new()
            } else {
                vec![result]
            });
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
        check_catalog_cancel(execution.cancel)?;
        selected
            .into_iter()
            .take(self.select.limit.unwrap_or(usize::MAX))
            .map(|(row, _)| {
                check_catalog_cancel(execution.cancel)?;
                self.select
                    .projections
                    .iter()
                    .map(|projection| {
                        let value = self.evaluate(&projection.expr, &row, execution)?;
                        if display
                            && self
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

fn textual_type(code: u8) -> bool {
    code == 253 || code == crate::pg_result::CatalogColumnType::InternalChar as u8
}
fn literal(value: &Value, code: u8) -> Expr {
    match value {
        Value::Null => Expr::Null,
        Value::Signed(n) => Expr::Integer(*n),
        Value::Text(s) if code == 1 => Expr::Boolean(s == "true" || s == "t"),
        Value::Text(s) => Expr::Text(s.clone()),
        _ => unreachable!("catalog scalar value"),
    }
}
fn has_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::ArrayAgg(_, _) => true,
        Expr::Cast(e, _) | Expr::TypedLiteral(e, _, _) | Expr::ArrayUnnest { input: e, .. } => {
            has_aggregate(e)
        }
        Expr::Call(_, args) => args.iter().any(has_aggregate),
        _ => false,
    }
}
fn ungrouped_column(expr: &Expr) -> bool {
    match expr {
        Expr::Column(_) => true,
        Expr::ArrayAgg(_, _) | Expr::ScalarSubquery(_) => false,
        Expr::ArrayUnnest { input, .. } => ungrouped_column(input),
        Expr::Cast(e, _) | Expr::TypedLiteral(e, _, _) => ungrouped_column(e),
        Expr::Call(_, args) => args.iter().any(ungrouped_column),
        _ => false,
    }
}
fn compare_keys(
    a: &[Value],
    b: &[Value],
    order: &[pg_catalog_query::Ordering],
) -> std::cmp::Ordering {
    for ((a, b), key) in a.iter().zip(b).zip(order) {
        let cmp = match (a, b) {
            (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
            (Value::Null, _) => std::cmp::Ordering::Greater,
            (_, Value::Null) => std::cmp::Ordering::Less,
            (Value::Signed(a), Value::Signed(b)) => a.cmp(b),
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            _ => std::cmp::Ordering::Equal,
        };
        let cmp = if key.descending { cmp.reverse() } else { cmp };
        if !cmp.is_eq() {
            return cmp;
        }
    }
    std::cmp::Ordering::Equal
}
fn numeric_array_element(code: u8) -> Option<u8> {
    use crate::pg_result::CatalogColumnType as T;
    if code == T::Int2Array as u8 || code == T::Int2Vector as u8 {
        Some(2)
    } else if code == T::Int4Array as u8 {
        Some(3)
    } else if code == T::Int8Array as u8 {
        Some(8)
    } else if code == T::OidArray as u8 || code == T::OidVector as u8 {
        Some(crate::pg_oid::OID_TYPE)
    } else {
        None
    }
}
// Providers produce one-dimensional numeric arrays/vectors, never arbitrary PG text arrays.
fn numeric_array_values(value: &Value) -> ConnResult<Vec<Value>> {
    let Value::Text(text) = value else {
        return if *value == Value::Null {
            Ok(Vec::new())
        } else {
            Err(ConnError::UnsupportedCommand(0))
        };
    };
    let mut values = Vec::new();
    let array = text.starts_with('{') && text.ends_with('}');
    let text = if array {
        &text[1..text.len() - 1]
    } else {
        text.as_str()
    };
    for item in text
        .split(|c: char| {
            if array {
                c == ','
            } else {
                c.is_ascii_whitespace()
            }
        })
        .filter(|s| !s.is_empty())
    {
        if values.len() >= MAX_CATALOG_ROWS {
            return Err(catalog_row_limit());
        }
        values.push(if item.trim() == "NULL" {
            Value::Null
        } else {
            Value::Signed(
                item.trim()
                    .parse()
                    .map_err(|_| ConnError::Session("invalid numeric catalog array".into()))?,
            )
        });
    }
    Ok(values)
}
fn unnest_projection(projection: &Expr, element: &Expr) -> ParseResult<Expr> {
    // The bounded ARRAY subquery admits scalar casts of its single unnest column.
    // This keeps the local name out of outer catalog binding and correlation.
    match projection {
        Expr::Column(path) if matches!(path.as_slice(), [name] if name == "unnest") => {
            Ok(element.clone())
        }
        Expr::Cast(inner, target) => Ok(Expr::Cast(
            Box::new(unnest_projection(inner, element)?),
            *target,
        )),
        _ => Err(("0A000", "unsupported catalog unnest projection".into())),
    }
}
fn array_text(values: impl Iterator<Item = Value>) -> String {
    let values = values
        .map(|v| match v {
            Value::Null => "NULL".into(),
            Value::Signed(n) => n.to_string(),
            Value::Text(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            _ => unreachable!("catalog array element"),
        })
        .collect::<Vec<_>>();
    format!("{{{}}}", values.join(","))
}
fn numeric_type(code: u8) -> bool {
    matches!(
        code,
        2 | 3 | 8 | crate::pg_oid::OID_TYPE | crate::pg_oid::REGCLASS_TYPE
    )
}
fn catalog_parameter_type(oid: u32) -> ParseResult<(u8, usize)> {
    Ok(match oid {
        0 => return Err(("42P18", "could not determine catalog parameter type".into())),
        26 => (crate::pg_oid::OID_TYPE, 0),
        16 => (1, astersql_parser_mysql::r#type::IsBooleanFlag),
        20 => (8, 0),
        21 => (2, 0),
        23 => (3, 0),
        25 | 1042 | 1043 => (253, 0),
        _ => return Err(("0A000", "unsupported catalog parameter OID".into())),
    })
}

fn visit_all_select_exprs<E>(
    select: &mut Select,
    visitor: &mut impl FnMut(&mut Expr) -> Result<(), E>,
) -> Result<(), E> {
    for cte in &mut select.ctes {
        visit_all_select_exprs(&mut cte.query, visitor)?;
    }
    visit_select_exprs(select, &mut |expr| {
        if let Expr::InSubquery(_, query) | Expr::ScalarSubquery(query) = expr {
            visit_all_select_exprs(query, visitor)?;
        }
        visitor(expr)
    })
}

fn visit_select_exprs<E>(
    select: &mut Select,
    visitor: &mut impl FnMut(&mut Expr) -> Result<(), E>,
) -> Result<(), E> {
    for p in &mut select.projections {
        visit_expr(&mut p.expr, visitor)?;
    }
    for j in &mut select.joins {
        visit_expr(&mut j.on, visitor)?;
    }
    if let Some(f) = &mut select.filter {
        visit_expr(f, visitor)?;
    }
    for o in &mut select.order {
        visit_expr(&mut o.expr, visitor)?;
    }
    Ok(())
}
fn visit_expr<E>(
    expr: &mut Expr,
    visitor: &mut impl FnMut(&mut Expr) -> Result<(), E>,
) -> Result<(), E> {
    match expr {
        Expr::TypedLiteral(e, _, _)
        | Expr::Cast(e, _)
        | Expr::Not(e)
        | Expr::IsNull(e)
        | Expr::NotNull(e)
        | Expr::InSubquery(e, _) => visit_expr(e, visitor)?,
        Expr::ArrayAgg(e, order) => {
            visit_expr(e, visitor)?;
            for key in order {
                visit_expr(&mut key.expr, visitor)?;
            }
        }
        Expr::Call(_, values) => {
            for v in values {
                visit_expr(v, visitor)?;
            }
        }
        Expr::In(e, values) => {
            visit_expr(e, visitor)?;
            for v in values {
                visit_expr(v, visitor)?;
            }
        }
        Expr::ArrayUnnest {
            input,
            projection: _,
        } => visit_expr(input, visitor)?,
        Expr::Any(a, b)
        | Expr::Equal(a, b)
        | Expr::Compare(a, _, b)
        | Expr::And(a, b)
        | Expr::Or(a, b) => {
            visit_expr(a, visitor)?;
            visit_expr(b, visitor)?;
        }
        Expr::Case { condition, yes, no } => {
            visit_expr(condition, visitor)?;
            visit_expr(yes, visitor)?;
            visit_expr(no, visitor)?;
        }
        _ => {}
    }
    visitor(expr)
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
        Expr::InSubquery(inner, _) => contains_age(inner),
        Expr::ArrayUnnest {
            input: left,
            projection: right,
        }
        | Expr::Any(left, right)
        | Expr::Equal(left, right)
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
        Value::Null,
        Value::Signed(0),
        Value::Null,
        Value::Text("p".into()),
        Value::Text("false".into()),
        Value::Null,
        Value::Signed(0),
        Value::Null,
        Value::Null,
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
        for index in &catalog_indexes(model)? {
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

// Project only the representable native sequence dependency: its persistent
// schema membership. SequenceInfo has no owning table/column, and the native
// default builder does not retain resolved sequence IDs. Neither AUTO_INCREMENT
// nor default SQL text justifies a PostgreSQL auto/internal ownership edge.
// This is a bounded sequence provider, not a general PostgreSQL dependency graph.
fn sequence_dependency_rows(
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
    cancel: &CancellationToken,
) -> ConnResult<Vec<Vec<Value>>> {
    check_catalog_cancel(cancel)?;
    let Some(schema) = snapshot
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == database.to_lowercase())
    else {
        return Ok(Vec::new());
    };
    let namespace = i64::from(crate::pg_oid::namespace_oid(schema.id)?);
    // Use the same fixed identities as regclass resolution and pg_class.
    let catalog_oid = |name| {
        crate::pg_oid::SYSTEM_RELATIONS
            .iter()
            .find(|(relation, _)| *relation == name)
            .map(|(_, oid)| i64::from(*oid))
            .expect("sequence dependency catalog must be registered")
    };
    let classid = catalog_oid("pg_class");
    let refclassid = catalog_oid("pg_namespace");
    let mut rows = Vec::new();
    for table in snapshot
        .SchemaTableInfos(&schema.name)
        .map_err(|e| ConnError::Session(e.to_string()))?
    {
        check_catalog_cancel(cancel)?;
        let model = table.model_meta.as_ref().ok_or_else(|| {
            ConnError::Session(format!(
                "complete table metadata is unavailable for {}",
                table.name.original
            ))
        })?;
        if model.State != astersql_meta_model::StatePublic || model.Sequence.is_none() {
            continue;
        }
        rows.push(vec![
            Value::Signed(classid),
            Value::Signed(i64::from(crate::pg_oid::table_oid(model.ID)?)),
            Value::Signed(refclassid),
            Value::Signed(namespace),
            Value::Signed(0),
            Value::Text("n".into()),
        ]);
        if rows.len() > MAX_CATALOG_ROWS {
            return Err(catalog_row_limit());
        }
    }
    Ok(rows)
}

// This bounded registry describes only the PG adapter's introspection
// primitives, not the shared expression engine or PostgreSQL's full pg_proc.
// Native INFORMATION_SCHEMA.ROUTINES/PARAMETERS are empty because stored
// programs are unsupported (session/runtime/system_query.rs). No user routine
// is invented in public. Synthetic OIDs stay below pg_oid's native range.
const INTERNAL_LANGUAGE_OID: i64 = 12;
struct IntrospectionFunction {
    oid: i64,
    name: &'static str,
    arguments: &'static str,
    result: &'static str,
}
const FUNCTION_INTROSPECTION: &[IntrospectionFunction] = &[
    IntrospectionFunction {
        oid: 16000,
        name: "pg_get_function_arguments",
        arguments: "function_oid oid",
        result: "text",
    },
    IntrospectionFunction {
        oid: 16001,
        name: "pg_get_function_result",
        arguments: "function_oid oid",
        result: "text",
    },
    IntrospectionFunction {
        oid: 16002,
        name: "pg_get_function_sqlbody",
        arguments: "function_oid oid",
        result: "text",
    },
];
fn function_rows() -> Vec<Vec<Value>> {
    FUNCTION_INTROSPECTION
        .iter()
        .map(|function| {
            vec![
                Value::Signed(function.oid),
                Value::Text(function.name.into()),
                Value::Signed(11),
                Value::Text("f".into()),
                Value::Signed(INTERNAL_LANGUAGE_OID),
                Value::Text(function.name.into()),
            ]
        })
        .collect()
}

// Optional arrays retain their PG element type even without native FDW options.
fn column_catalog_field(relation: &str, name: &str) -> Option<(usize, u8, usize)> {
    let oid = crate::pg_oid::OID_TYPE;
    let internal_char = crate::pg_result::CatalogColumnType::InternalChar as u8;
    let (slot, code) = match (relation, name) {
        ("pg_opclass", "oid") => (0, oid),
        ("pg_opclass", "opcmethod") => (1, oid),
        ("pg_inherits", "inhrelid") => (0, oid),
        ("pg_inherits", "inhparent") => (1, oid),
        ("pg_inherits", "inhseqno") => (2, 3),
        ("pg_class", "xmin") => (4, 8),
        ("pg_class", "reltablespace") => (5, oid),
        ("pg_class", "reloptions") => (6, crate::pg_result::CatalogColumnType::TextArray as u8),
        ("pg_class", "relpersistence") => (7, internal_char),
        ("pg_class", "relispartition") => (8, 1),
        ("pg_class", "relpartbound") => (9, 253),
        ("pg_class", "relam") => (10, oid),
        ("pg_class", "relowner") => (11, 8),
        ("pg_class", "relacl") => (12, crate::pg_result::CatalogColumnType::TextArray as u8),
        ("pg_depend", "classid") => (0, oid),
        ("pg_depend", "objid") => (1, oid),
        ("pg_depend", "refclassid") => (2, oid),
        ("pg_depend", "refobjid") => (3, oid),
        ("pg_depend", "refobjsubid") => (4, 3),
        ("pg_depend", "deptype") => (5, internal_char),
        ("pg_proc", "oid") => (0, oid),
        ("pg_proc", "proname") => (1, 253),
        ("pg_proc", "pronamespace") => (2, oid),
        ("pg_proc", "prokind") => (3, internal_char),
        ("pg_proc", "prolang") => (4, oid),
        ("pg_proc", "prosrc") => (5, 253),
        ("pg_language", "oid") => (0, oid),
        ("pg_language", "lanname") => (1, 253),
        ("pg_index", "indexrelid") => (0, oid),
        ("pg_index", "indrelid") => (1, oid),
        ("pg_index", "indnatts") => (2, 2),
        ("pg_index", "indnkeyatts") => (3, 2),
        ("pg_index", "indisunique") => (4, 1),
        ("pg_index", "indisprimary") => (5, 1),
        ("pg_index", "indnullsnotdistinct") => (6, 1),
        ("pg_index", "indkey") => (7, crate::pg_result::CatalogColumnType::Int2Vector as u8),
        ("pg_index", "indoption") => (8, crate::pg_result::CatalogColumnType::Int2Vector as u8),
        ("pg_index", "indclass") => (9, crate::pg_result::CatalogColumnType::OidVector as u8),
        ("pg_index", "indcollation") => return None,
        ("pg_index", "indexprs") => (11, 253),
        ("pg_index", "indpred") => (12, 253),
        ("pg_index", "indisvalid") => (13, 1),
        ("pg_index", "indisready") => (14, 1),
        ("pg_constraint", "oid") => (0, oid),
        ("pg_constraint", "conname") => (1, 253),
        ("pg_constraint", "contype") => (2, internal_char),
        ("pg_constraint", "conrelid") => (3, oid),
        ("pg_constraint", "connamespace") => (4, oid),
        ("pg_constraint", "conkey") => (5, crate::pg_result::CatalogColumnType::Int2Array as u8),
        ("pg_constraint", "conindid") => (6, oid),
        ("pg_constraint", "confrelid") => (7, oid),
        ("pg_constraint", "confkey") => (8, crate::pg_result::CatalogColumnType::Int2Array as u8),
        ("pg_constraint", "confupdtype") => (9, internal_char),
        ("pg_constraint", "confdeltype") => (10, internal_char),
        ("pg_constraint", "condeferrable") => (11, 1),
        ("pg_constraint", "condeferred") => (12, 1),
        ("pg_constraint", "connoinherit") => (13, 1),
        ("pg_constraint", "conbin") => (14, 253),
        ("pg_constraint", "conexclop") => (15, crate::pg_result::CatalogColumnType::OidArray as u8),
        ("pg_constraint", "xmin") => (16, 8),
        ("pg_attribute", "attrelid") => (0, oid),
        ("pg_attribute", "attnum") => (1, 2),
        ("pg_attribute", "attname") => (2, 253),
        ("pg_attribute", "atttypid") => (3, oid),
        ("pg_attribute", "atttypmod") => (4, 3),
        ("pg_attribute", "attndims") => (5, 3),
        ("pg_attribute", "attnotnull") => (6, 1),
        ("pg_attribute", "attisdropped") => (7, 1),
        ("pg_attribute", "attislocal") => (8, 1),
        ("pg_attribute", "attidentity") => (9, internal_char),
        ("pg_attribute", "attgenerated") => (10, internal_char),
        ("pg_attribute", "attfdwoptions") => {
            (11, crate::pg_result::CatalogColumnType::TextArray as u8)
        }
        ("pg_attribute", "atthasdef") => (12, 1),
        ("pg_attribute", "xmin") => (13, 8),
        ("pg_attrdef", "adrelid") => (0, oid),
        ("pg_attrdef", "adnum") => (1, 2),
        ("pg_attrdef", "adbin") => (2, 253),
        ("pg_type", "oid") => (0, oid),
        ("pg_type", "typname") => (1, 253),
        ("pg_type", "typnamespace") => (2, oid),
        ("pg_type", "typtype") => (3, internal_char),
        ("pg_type", "typcategory") => (4, internal_char),
        ("pg_type", "typrelid") => (5, oid),
        ("pg_type", "typbasetype") => (6, oid),
        ("pg_type", "typtypmod") => (7, 3),
        ("pg_type", "typndims") => (8, 3),
        ("pg_type", "typdefault") => (9, 253),
        ("pg_type", "typnotnull") => (10, 1),
        ("pg_type", "typowner") => (11, 8),
        ("pg_type", "typelem") => (12, oid),
        ("pg_type", "typisdefined") => (13, 1),
        ("pg_type", "xmin") => (14, 8),
        _ => return None,
    };
    Some((
        slot,
        code,
        if code == 1 {
            astersql_parser_mysql::r#type::IsBooleanFlag
        } else {
            0
        },
    ))
}

// Canonical builtins used by native column mappings and the nullable options
// array. No native enum/set/domain is invented to disguise incompatible types.
const COLUMN_TYPES: &[(i64, &str, &str, &str)] = &[
    (16, "bool", "boolean", "B"),
    (17, "bytea", "bytea", "U"),
    (18, "char", "\"char\"", "Z"),
    (20, "int8", "bigint", "N"),
    (21, "int2", "smallint", "N"),
    (23, "int4", "integer", "N"),
    (25, "text", "text", "S"),
    (26, "oid", "oid", "N"),
    (700, "float4", "real", "N"),
    (701, "float8", "double precision", "N"),
    (1009, "_text", "text[]", "A"),
    (1042, "bpchar", "character", "S"),
    (1043, "varchar", "character varying", "S"),
    (1082, "date", "date", "D"),
    (1114, "timestamp", "timestamp without time zone", "D"),
    (1700, "numeric", "numeric", "N"),
    (114, "json", "json", "U"),
];

fn native_column_type(column: &astersql_meta_model::ColumnInfo) -> ConnResult<(i64, i64)> {
    use astersql_parser_mysql::r#type as mysql;
    if column.FieldType.IsArray() {
        return Err(ConnError::UnsupportedCommand(0));
    }
    if column.GetFlag() & mysql::IsBooleanFlag != 0 {
        return Ok((16, -1));
    }
    let unsigned = mysql::HasUnsignedFlag(column.GetFlag());
    let mut modifier = -1;
    let oid = match column.GetType() {
        1 | 13 => 21,
        2 => {
            if unsigned {
                23
            } else {
                21
            }
        }
        3 => {
            if unsigned {
                20
            } else {
                23
            }
        }
        9 => 23,
        8 => {
            if unsigned {
                1700
            } else {
                20
            }
        }
        4 => 700,
        5 => 701,
        0 | 246 => {
            let precision =
                i64::try_from(column.GetFlen()).map_err(|_| ConnError::UnsupportedCommand(0))?;
            let scale =
                i64::try_from(column.GetDecimal()).map_err(|_| ConnError::UnsupportedCommand(0))?;
            if !(1..=65).contains(&precision) || !(0..=precision).contains(&scale) {
                return Err(ConnError::Session(format!(
                    "incomplete column type metadata for {}",
                    column.Name.O
                )));
            }
            modifier = 4 + (precision << 16) + scale;
            1700
        }
        15 | 253 | 254 => {
            if column.GetCharset() == "binary" {
                17
            } else {
                let length = i64::try_from(column.GetFlen())
                    .map_err(|_| ConnError::UnsupportedCommand(0))?;
                if length <= 0 || length > i64::from(i32::MAX) - 4 {
                    return Err(ConnError::Session(format!(
                        "incomplete column length metadata for {}",
                        column.Name.O
                    )));
                }
                modifier = length + 4;
                if column.GetType() == 254 { 1042 } else { 1043 }
            }
        }
        249..=252 => {
            if column.GetCharset() == "binary" {
                17
            } else {
                25
            }
        }
        10 | 14 => 1082,
        7 | 12 => {
            modifier =
                i64::try_from(column.GetDecimal()).map_err(|_| ConnError::UnsupportedCommand(0))?;
            if !(0..=6).contains(&modifier) {
                return Err(ConnError::Session(format!(
                    "incomplete datetime precision metadata for {}",
                    column.Name.O
                )));
            }
            1114
        }
        245 => 114,
        // Native TIME is a duration, not PG time-of-day; enums and sets need
        // identities/labels not provided by this compatibility phase.
        _ => return Err(ConnError::UnsupportedCommand(0)),
    };
    if column.GetType() == 8 && unsigned {
        modifier = 4 + (20 << 16);
    }
    Ok((oid, modifier))
}

fn format_column_type(oid: i64, modifier: Option<i64>) -> ConnResult<String> {
    u32::try_from(oid).map_err(|_| ConnError::Session("PG oid conversion out of range".into()))?;
    if let Some(modifier) = modifier {
        i32::try_from(modifier)
            .map_err(|_| ConnError::Session("PG oid conversion out of range".into()))?;
    }
    if oid == 0 {
        return Ok("-".into());
    }
    let Some((_, _, name, _)) = COLUMN_TYPES.iter().find(|t| t.0 == oid) else {
        return Ok("???".into());
    };
    let Some(modifier) = modifier else {
        return Ok((*name).into());
    };
    if modifier < 0 {
        return Ok(if oid == 1042 {
            "bpchar".into()
        } else {
            (*name).into()
        });
    }
    Ok(match oid {
        1042 | 1043 if modifier >= 4 => format!("{name}({})", modifier - 4),
        1700 if modifier >= 4 => {
            let n = modifier - 4;
            let precision = (n >> 16) & 0xffff;
            // PG encodes a signed 11-bit scale (including negative scales).
            let scale = ((n & 0x7ff) ^ 1024) - 1024;
            format!("numeric({precision},{scale})")
        }
        1114 if (0..=6).contains(&modifier) => format!("timestamp({modifier}) without time zone"),
        1042 | 1043 | 1700 | 1114 => return Err(ConnError::UnsupportedCommand(0)),
        _ => (*name).into(),
    })
}

fn column_default(
    column: &astersql_meta_model::ColumnInfo,
    oid: i64,
) -> ConnResult<Option<String>> {
    use astersql_meta_model::DefaultValue;
    if !column.GeneratedExprString.is_empty() {
        // A MySQL generated expression cannot be exposed as PG SQL by copying
        // its source text. Reject until its expression semantics are supported.
        return Err(ConnError::UnsupportedCommand(0));
    }
    let Some(value) = column.GetDefaultValue() else {
        return Ok(None);
    };
    let text = match value {
        DefaultValue::Bool(v) => v.to_string(),
        DefaultValue::Int(v) => v.to_string(),
        DefaultValue::Uint(v) => v.to_string(),
        DefaultValue::Float(v) if v.is_finite() => v.to_string(),
        DefaultValue::Float(_) => return Err(ConnError::UnsupportedCommand(0)),
        DefaultValue::String(v) => {
            String::from_utf8(v).map_err(|_| ConnError::UnsupportedCommand(0))?
        }
    };
    if column.DefaultIsExpr || oid == 1114 && text.to_uppercase().starts_with("CURRENT_TIMESTAMP") {
        let mut upper = text.to_uppercase();
        for precision in 0..=6 {
            if upper == format!("CURRENT_TIMESTAMP('{precision}')") {
                upper = format!("CURRENT_TIMESTAMP({precision})");
                break;
            }
        }
        if upper == "CURRENT_TIMESTAMP"
            || (0..=6).any(|n| upper == format!("CURRENT_TIMESTAMP({n})"))
        {
            return Ok(Some(upper));
        }
        return Err(ConnError::UnsupportedCommand(0));
    }
    if oid == 16 {
        return match text.as_str() {
            "0" | "false" => Ok(Some("false".into())),
            "1" | "true" => Ok(Some("true".into())),
            _ => Err(ConnError::UnsupportedCommand(0)),
        };
    }
    if matches!(oid, 20 | 21 | 23 | 700 | 701 | 1700) {
        if !text.parse::<f64>().is_ok_and(f64::is_finite) {
            return Err(ConnError::UnsupportedCommand(0));
        }
        return Ok(Some(text));
    }
    if oid == 17 {
        return Err(ConnError::UnsupportedCommand(0));
    }
    if oid == 1082 && text.starts_with("0000-") || oid == 1114 && text.starts_with("0000-") {
        return Err(ConnError::UnsupportedCommand(0));
    }
    let name = format_column_type(oid, None)?;
    Ok(Some(format!("'{}'::{name}", text.replace('\'', "''"))))
}

fn column_catalog_rows(
    relation: &str,
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
    cancel: &CancellationToken,
) -> ConnResult<Vec<Vec<Value>>> {
    check_catalog_cancel(cancel)?;
    let mut rows = if relation == "pg_type" {
        COLUMN_TYPES
            .iter()
            .map(|(oid, name, _, category)| {
                vec![
                    Value::Signed(*oid),
                    Value::Text((*name).into()),
                    Value::Signed(11),
                    Value::Text("b".into()),
                    Value::Text((*category).into()),
                    Value::Signed(0),
                    Value::Signed(0),
                    Value::Signed(-1),
                    Value::Signed(0),
                    Value::Null,
                    Value::Text("false".into()),
                    Value::Null,
                    Value::Signed(if *oid == 1009 { 25 } else { 0 }),
                    Value::Text("true".into()),
                    Value::Null,
                ]
            })
            .collect()
    } else {
        Vec::new()
    };
    let Some(schema) = snapshot
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == database.to_lowercase())
    else {
        return Ok(rows);
    };
    for table in snapshot
        .SchemaTableInfos(&schema.name)
        .map_err(|e| ConnError::Session(e.to_string()))?
    {
        check_catalog_cancel(cancel)?;
        let model = table.model_meta.as_ref().ok_or_else(|| {
            ConnError::Session(format!(
                "PG column catalog requires complete ModelMeta for {}",
                table.name.original
            ))
        })?;
        if model.State != astersql_meta_model::StatePublic || model.Sequence.is_some() {
            continue;
        }
        let relid = i64::from(crate::pg_oid::table_oid(model.ID)?);
        let mut positions = std::collections::HashSet::new();
        for column in &model.Columns {
            check_catalog_cancel(cancel)?;
            if column.State != astersql_meta_model::StatePublic || column.Hidden {
                continue;
            }
            let position = column
                .Offset
                .checked_add(1)
                .filter(|n| *n > 0 && *n <= i16::MAX as isize)
                .ok_or_else(|| {
                    ConnError::Session(format!(
                        "invalid column offset metadata for {}",
                        column.Name.O
                    ))
                })? as i64;
            if column.ID <= 0 || !positions.insert(position) {
                return Err(ConnError::Session(format!(
                    "invalid column identity metadata for {}",
                    column.Name.O
                )));
            }
            let (oid, modifier) = native_column_type(column)?;
            if relation == "pg_type" {
                // Builtins describe the validated native column set. Do not
                // silently hide an unmapped enum/set or incomplete model as
                // an empty collection of public user-defined types.
                continue;
            }
            let default = column_default(column, oid)?;
            if relation == "pg_attrdef" {
                if let Some(default) = default {
                    // PG-private opaque deparse input, tied to its owning
                    // relation. It is not native SQL passed back to the engine.
                    rows.push(vec![
                        Value::Signed(relid),
                        Value::Signed(position),
                        Value::Text(format!("{relid}:{default}")),
                    ]);
                }
            } else {
                rows.push(vec![
                    Value::Signed(relid),
                    Value::Signed(position),
                    Value::Text(column.Name.O.clone()),
                    Value::Signed(oid),
                    Value::Signed(modifier),
                    Value::Signed(0),
                    Value::Text(
                        astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()).to_string(),
                    ),
                    // Native DROP removes the column rather than retaining PG
                    // tombstone attributes; surviving offsets remain authoritative.
                    Value::Text("false".into()),
                    Value::Text("true".into()),
                    // AUTO_INCREMENT is not a PG identity or sequence.
                    Value::Text(String::new()),
                    Value::Text(String::new()),
                    Value::Null,
                    Value::Text(default.is_some().to_string()),
                    Value::Null,
                ]);
            }
            if rows.len() > MAX_CATALOG_ROWS {
                return Err(catalog_row_limit());
            }
        }
    }
    Ok(rows)
}

// Quote all native names, including PostgreSQL keywords and embedded quotes.
fn pg_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

// Native integer handles have a real primary key but no secondary IndexInfo.
// Reserve the negative local ID -1 for that projection; ordinary IDs are positive.
pub(crate) fn catalog_indexes(
    model: &astersql_meta_model::TableInfo,
) -> ConnResult<Vec<astersql_meta_model::IndexInfo>> {
    if model.Indices.iter().any(|i| i.ID <= 0) {
        return Err(ConnError::UnsupportedCommand(0));
    }
    let mut indexes = model.Indices.clone();
    if model.PKIsHandle && !indexes.iter().any(|i| i.Primary) {
        let column = model
            .Columns
            .iter()
            .find(|c| {
                c.State == astersql_meta_model::StatePublic
                    && !c.Hidden
                    && astersql_parser_mysql::r#type::HasPriKeyFlag(c.GetFlag())
            })
            .ok_or(ConnError::UnsupportedCommand(0))?;
        indexes.push(astersql_meta_model::IndexInfo {
            ID: -1,
            Name: astersql_parser_ast::NewCIStr("PRIMARY"),
            Columns: vec![astersql_meta_model::IndexColumn {
                Name: column.Name.clone(),
                Offset: column.Offset,
                Length: -1,
                ..Default::default()
            }],
            Unique: true,
            Primary: true,
            State: astersql_meta_model::StatePublic,
            ..Default::default()
        });
    }
    Ok(indexes)
}

fn index_constraint_rows(
    relation: &str,
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
    cancel: &CancellationToken,
) -> ConnResult<Vec<Vec<Value>>> {
    use astersql_meta_model::StatePublic;
    check_catalog_cancel(cancel)?;
    let Some(schema) = snapshot
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == database.to_lowercase())
    else {
        return Ok(Vec::new());
    };
    let tables = snapshot
        .SchemaTableInfos(&schema.name)
        .map_err(|e| ConnError::Session(e.to_string()))?;
    let namespace = i64::from(crate::pg_oid::namespace_oid(schema.id)?);
    let mut rows = Vec::new();
    let unsupported = || ConnError::UnsupportedCommand(0);
    let key_columns = |model: &astersql_meta_model::TableInfo,
                       names: &[String]|
     -> ConnResult<Vec<(i64, String)>> {
        names
            .iter()
            .map(|name| {
                let column = model
                    .Columns
                    .iter()
                    .find(|c| c.State == StatePublic && !c.Hidden && c.Name.O == *name)
                    .ok_or_else(unsupported)?;
                let position = column
                    .Offset
                    .checked_add(1)
                    .filter(|n| *n > 0 && *n <= i16::MAX as isize)
                    .ok_or_else(unsupported)?;
                Ok((position as i64, pg_identifier(&column.Name.O)))
            })
            .collect()
    };
    let array = |columns: &[(i64, String)]| {
        Value::Text(format!(
            "{{{}}}",
            columns
                .iter()
                .map(|c| c.0.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ))
    };
    let names = |columns: &[(i64, String)]| {
        columns
            .iter()
            .map(|c| c.1.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let action = |value: i32| -> ConnResult<(&str, &str)> {
        Ok(match value {
            0 | 4 => ("a", "NO ACTION"),
            1 => ("r", "RESTRICT"),
            2 => ("c", "CASCADE"),
            3 => ("n", "SET NULL"),
            5 => ("d", "SET DEFAULT"),
            _ => return Err(unsupported()),
        })
    };
    for table in &tables {
        check_catalog_cancel(cancel)?;
        let model = table.model_meta.as_ref().ok_or_else(unsupported)?;
        if model.State != StatePublic || model.Sequence.is_some() || model.View.is_some() {
            continue;
        }
        let relid = i64::from(crate::pg_oid::table_oid(model.ID)?);
        let indexes = catalog_indexes(model)?;
        for index in indexes.iter().filter(|i| i.State == StatePublic) {
            if !matches!(
                index.Tp,
                astersql_meta_model::ast::IndexType::Invalid
                    | astersql_meta_model::ast::IndexType::Btree
            ) || index.Columns.is_empty()
                || index.Columns.len() > i16::MAX as usize
                || index.MVIndex
                || index.VectorInfo.is_some()
                || index.InvertedInfo.is_some()
                || index.FullTextInfo.is_some()
                || !index.ConditionExprString.is_empty()
                || index
                    .Columns
                    .iter()
                    .any(|c| c.Length != -1 || c.UseChangingType)
            {
                return Err(unsupported());
            }
            let columns = key_columns(
                model,
                &index
                    .Columns
                    .iter()
                    .map(|c| c.Name.O.clone())
                    .collect::<Vec<_>>(),
            )?;
            if index
                .Columns
                .iter()
                .zip(&columns)
                .any(|(c, p)| c.Offset + 1 != p.0 as isize)
            {
                return Err(unsupported());
            }
            let oid = i64::from(crate::pg_oid::index_oid(model.ID, index.ID)?);
            let definition = format!(
                "CREATE {}INDEX {} ON public.{} USING btree ({})",
                if index.Unique { "UNIQUE " } else { "" },
                pg_identifier(&index.Name.O),
                pg_identifier(&model.Name.O),
                names(&columns)
            );
            let mut row = vec![Value::Null; CATALOG_ROW_WIDTH];
            if relation == "pg_index" {
                row[0] = Value::Signed(oid);
                row[1] = Value::Signed(relid);
                row[2] = Value::Signed(columns.len() as i64);
                row[3] = row[2].clone();
                row[4] = Value::Text(index.Unique.to_string());
                row[5] = Value::Text(index.Primary.to_string());
                row[6] = Value::Text("false".into());
                row[7] = Value::Text(
                    columns
                        .iter()
                        .map(|c| c.0.to_string())
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                row[8] = Value::Text(vec!["0"; columns.len()].join(" "));
                // Native indexes do not select PostgreSQL operator classes.
                row[9] = Value::Text(String::new());
                row[13] = Value::Text("true".into());
                row[14] = Value::Text("true".into());
                row[21] = Value::Text(definition);
                row[22] = Value::Text(
                    columns
                        .iter()
                        .map(|c| c.1.clone())
                        .collect::<Vec<_>>()
                        .join("\0"),
                );
            } else if index.Primary || index.Unique {
                row[0] = Value::Signed(oid);
                row[1] = Value::Text(index.Name.O.clone());
                row[2] = Value::Text(if index.Primary { "p" } else { "u" }.into());
                row[3] = Value::Signed(relid);
                row[4] = Value::Signed(namespace);
                row[5] = array(&columns);
                row[6] = Value::Signed(oid);
                row[7] = Value::Signed(0);
                row[9] = Value::Text(" ".into());
                row[10] = Value::Text(" ".into());
                for slot in [11, 12, 13] {
                    row[slot] = Value::Text("false".into());
                }
                row[21] = Value::Text(format!(
                    "{} ({})",
                    if index.Primary {
                        "PRIMARY KEY"
                    } else {
                        "UNIQUE"
                    },
                    names(&columns)
                ));
            } else {
                continue;
            }
            rows.push(row);
            if rows.len() > MAX_CATALOG_ROWS {
                return Err(catalog_row_limit());
            }
        }
        if relation != "pg_constraint" {
            continue;
        }
        // CHECK expression translation is not established by this task. Reject
        // rather than exposing native expressions as PostgreSQL parse trees.
        if model.Constraints.iter().any(|c| c.State == StatePublic) {
            return Err(unsupported());
        }
        for fk in model.ForeignKeys.iter().filter(|f| f.State == StatePublic) {
            if fk.ID <= 0
                || fk.Cols.is_empty()
                || fk.Cols.len() != fk.RefCols.len()
                || fk.RefSchema.L != schema.name.lower
            {
                return Err(unsupported());
            }
            let parent = tables
                .iter()
                .find(|t| t.name.lower == fk.RefTable.L)
                .and_then(|t| t.model_meta.as_ref())
                .ok_or_else(unsupported)?;
            let columns = key_columns(
                model,
                &fk.Cols.iter().map(|c| c.O.clone()).collect::<Vec<_>>(),
            )?;
            let referenced = key_columns(
                parent,
                &fk.RefCols.iter().map(|c| c.O.clone()).collect::<Vec<_>>(),
            )?;
            let parent_indexes = catalog_indexes(parent)?;
            let referenced_index = parent_indexes
                .iter()
                .find(|i| {
                    i.State == StatePublic
                        && i.Unique
                        && i.Columns.len() == referenced.len()
                        && i.Columns
                            .iter()
                            .zip(&fk.RefCols)
                            .all(|(c, r)| c.Name.L == r.L)
                })
                .ok_or_else(unsupported)?;
            let (update, update_sql) = action(fk.OnUpdate)?;
            let (delete, delete_sql) = action(fk.OnDelete)?;
            // Negative even IDs are disjoint from native index constraints and
            // the handle primary key (-1), retaining table-local FK identity.
            let local_id = fk.ID.checked_mul(-2).ok_or_else(unsupported)?;
            let mut row = vec![Value::Null; CATALOG_ROW_WIDTH];
            row[0] = Value::Signed(i64::from(crate::pg_oid::index_oid(model.ID, local_id)?));
            row[1] = Value::Text(fk.Name.O.clone());
            row[2] = Value::Text("f".into());
            row[3] = Value::Signed(relid);
            row[4] = Value::Signed(namespace);
            row[5] = array(&columns);
            row[6] = Value::Signed(i64::from(crate::pg_oid::index_oid(
                parent.ID,
                referenced_index.ID,
            )?));
            row[7] = Value::Signed(i64::from(crate::pg_oid::table_oid(parent.ID)?));
            row[8] = array(&referenced);
            row[9] = Value::Text(update.into());
            row[10] = Value::Text(delete.into());
            for slot in [11, 12, 13] {
                row[slot] = Value::Text("false".into());
            }
            row[21] = Value::Text(format!(
                "FOREIGN KEY ({}) REFERENCES public.{} ({}) ON UPDATE {} ON DELETE {}",
                names(&columns),
                pg_identifier(&parent.Name.O),
                names(&referenced),
                update_sql,
                delete_sql
            ));
            rows.push(row);
            if rows.len() > MAX_CATALOG_ROWS {
                return Err(catalog_row_limit());
            }
        }
    }
    Ok(rows)
}
