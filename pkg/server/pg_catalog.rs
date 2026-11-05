// Copyright 2026 AsterSQL.
//! PG-only DataGrip catalog probes backed by canonical schema/transaction data.
use crate::conn::{
    CancellationToken, ColumnInfo, ConnError, ConnResult, NativeType, PreparedMetadata,
    QueryResult, TiDBContext, Value,
};

// Fixed provider slots preserve the existing catalog column layout. Joined
// relations get separate slots, including empty providers after LEFT JOIN.
// Includes constraint state and auxiliary identities; every JOIN uses this stride.
const CATALOG_ROW_WIDTH: usize = 34;
// Preserve the existing public CTE projection bound independently of provider slots.
const MAX_CATALOG_CTE_COLUMNS: usize = 24;
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
    database_facts: std::cell::RefCell<Option<Vec<astersql_session::runtime::PgDatabaseFact>>>,
    database_role_names: std::cell::RefCell<std::collections::BTreeMap<i64, String>>,
    providers: std::cell::RefCell<std::collections::HashMap<String, CatalogRows>>,
    ctes: std::cell::RefCell<std::collections::HashMap<usize, CatalogRows>>,
    materialized: std::cell::Cell<usize>,
    work: std::cell::Cell<usize>,
}
impl Execution<'_> {
    fn database_facts(&self) -> ConnResult<Vec<astersql_session::runtime::PgDatabaseFact>> {
        if self.database_facts.borrow().is_none() {
            let facts = self.context.pg_database_facts(None)?;
            *self.database_role_names.borrow_mut() = facts
                .first()
                .map_or_else(Default::default, |f| (*f.roles).clone());
            *self.database_facts.borrow_mut() = Some(facts);
        }
        Ok(self.database_facts.borrow().as_ref().unwrap().clone())
    }
    fn database_role_name(&self, oid: i64) -> ConnResult<Value> {
        if self.database_facts.borrow().is_none() {
            self.database_facts()?;
        }
        Ok(self
            .database_role_names
            .borrow()
            .get(&oid)
            .cloned()
            .map_or(Value::Null, Value::Text))
    }
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
fn database_identity_unavailable(message: &str) -> ConnError {
    ConnError::PgFunction(astersql_session::runtime::PgFunctionError {
        state: "0A000",
        message: message.into(),
        detail: None,
        hint: None,
        position: None,
    })
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
    relation_path: Vec<String>,
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
            relation_path: vec!["pg_catalog".into(), "public".into()],
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
        let names = pg_catalog_query::implicit_relations(sql);
        if names.is_empty() && pg_catalog_query::parse(sql)?.is_none() {
            return Ok(None);
        }
        let mut shadowed = Vec::new();
        let path = session
            .effective_explicit_path(context)
            .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?;
        let public_first = path
            .iter()
            .position(|s| s == "public")
            .zip(path.iter().position(|s| s == "pg_catalog"))
            .is_some_and(|(public, catalog)| public < catalog);
        // Only namespaces explicitly before pg_catalog can shadow it. If it
        // was omitted, PostgreSQL searches the implicit catalog first.
        if let Some(catalog) = path.iter().position(|name| name == "pg_catalog") {
            for schema in &path[..catalog] {
                for name in &names {
                    if let Some(bound) =
                        context.pg_bind_relation(Some(schema.clone()), name.clone(), false)
                    {
                        match bound {
                            Ok(_) => shadowed.push(name.clone()),
                            Err(error) if crate::pg_conn::sqlstate(&error) == "42P01" => {}
                            Err(error) => {
                                return Err((crate::pg_conn::sqlstate(&error), error.to_string()));
                            }
                        }
                    }
                }
            }
        }
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
            current_schema: path.first().cloned(),
            public_first,
            relation_path: path,
            cte_columns: CteColumns::new(),
            parameter_oids: Vec::new(),
            parameter_values: Vec::new(),
        };
        query.parameters(oids)?;
        query.bind()?;
        query.validate_regclass_literals(context)?;
        Ok(Some(query))
    }
    // Literal regclass input is analyzed before Bind/Execute. In particular,
    // a dropped owned sequence must fail Parse, without a portal descriptor.
    // Provider rows and parameter values still use the execution snapshot.
    fn validate_regclass_literals(&mut self, context: &dyn TiDBContext) -> ParseResult<()> {
        let mut literals = Vec::new();
        visit_all_select_exprs(&mut self.select, &mut |expr| {
            if let Expr::Cast(value, CastType::Regclass) = expr
                && let Expr::Text(input) = value.as_ref()
            {
                literals.push(input.clone());
            }
            Ok::<_, (&'static str, String)>(())
        })?;
        if literals.is_empty() {
            return Ok(());
        }
        let Some(snapshot) = context.schema_snapshot() else {
            return Ok(());
        };
        let current = context
            .execute_query("SELECT DATABASE()", false, &CancellationToken::new())
            .map_err(|error| (crate::pg_conn::sqlstate(&error), error.to_string()))?;
        let database = match current
            .first()
            .and_then(|r| r.rows.first())
            .and_then(|r| r.first())
        {
            Some(Value::Text(name)) => name.as_str(),
            _ => "",
        };
        for input in literals {
            crate::pg_oid::resolve_in_path(
                &input,
                database,
                snapshot.as_ref(),
                &self.relation_path,
                &context
                    .pg_namespace_facts()
                    .transpose()
                    .map_err(|e| (crate::pg_conn::sqlstate(&e), e.to_string()))?
                    .unwrap_or_default(),
            )
            .map_err(|error| (crate::pg_conn::sqlstate(&error), error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn classify(sql: &str) -> Option<Self> {
        Self::parse(sql).ok().flatten()
    }
    fn nested(&self, select: Select) -> Self {
        Self {
            select,
            current_schema: self.current_schema.clone(),
            public_first: self.public_first,
            relation_path: self.relation_path.clone(),
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
            if let Expr::Cast(inner, target) = expr {
                if let Expr::Parameter(index) = **inner {
                    if self.parameter_oids[index] == 0 {
                        self.parameter_oids[index] = match target {
                            CastType::Oid => 26,
                            CastType::Boolean => 16,
                            CastType::Integer => 23,
                            CastType::Varchar => 25,
                            _ => 0,
                        };
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
        for (cte_index, cte) in self.select.ctes.clone().into_iter().enumerate() {
            let mut query = self.nested(cte.query);
            query.bind()?;
            if query.select.projections.len() > MAX_CATALOG_CTE_COLUMNS {
                return Err(("0A000", "catalog CTEs exceed the column limit".into()));
            }
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
            self.select.ctes[cte_index].query = query.select;
        }
        // NATURAL joins compare every shared name and expose it once in
        // unqualified references and SELECT *. Qualified columns retain slots.
        for index in 0..self.select.joins.len() {
            if !self.select.joins[index].natural {
                continue;
            }
            let right = self.select.joins[index].relation.clone();
            let right_names = self.relation_names(&right)?;
            let mut scope = self.clone();
            scope.select.joins.truncate(index);
            let mut on = Expr::Boolean(true);
            let mut seen = std::collections::HashSet::new();
            for name in right_names {
                if !seen.insert(name.clone()) {
                    return Err(("42702", "ambiguous NATURAL JOIN column".into()));
                }
                let field = scope.column(&[name.clone()]);
                if field.as_ref().is_err_and(|(code, _)| *code == "42702") {
                    return Err(field.unwrap_err());
                }
                if field.is_ok() {
                    let left = scope
                        .relations()
                        .find(|r| scope.column(&[r.alias.clone(), name.clone()]).is_ok())
                        .unwrap();
                    on = Expr::And(
                        Box::new(on),
                        Box::new(Expr::Equal(
                            Box::new(Expr::Column(vec![left.alias.clone(), name.clone()])),
                            Box::new(Expr::Column(vec![right.alias.clone(), name])),
                        )),
                    );
                }
            }
            self.select.joins[index].on = on;
        }
        let mut expanded = Vec::new();
        for projection in &self.select.projections {
            if projection.expr == Expr::Column(vec!["*".into()]) {
                let mut seen = std::collections::HashSet::new();
                let mut wildcard = Vec::new();
                for (index, relation) in self.relations().enumerate() {
                    let names = self.relation_names(relation)?;
                    let natural = index > 0 && self.select.joins[index - 1].natural;
                    if natural {
                        // SQL orders merged columns first, preserving their left order.
                        let (mut common, remaining): (Vec<pg_catalog_query::Projection>, Vec<_>) =
                            wildcard
                                .into_iter()
                                .partition(|p: &pg_catalog_query::Projection| {
                                    names.contains(&p.name)
                                });
                        common.extend(remaining);
                        wildcard = common;
                    }
                    for name in names {
                        if natural && seen.contains(&name) {
                            continue;
                        }
                        seen.insert(name.clone());
                        wildcard.push(pg_catalog_query::Projection {
                            expr: Expr::Column(vec![relation.alias.clone(), name.clone()]),
                            name,
                        });
                    }
                }
                expanded.extend(wildcard);
            } else {
                expanded.push(projection.clone());
            }
        }
        self.select.projections = expanded;
        for index in 0..self.select.unions.len() {
            let mut query = self.nested(self.select.unions[index].1.clone());
            query.bind()?;
            self.cte_columns.extend(query.cte_columns.clone());
            if query.select.projections.len() != self.select.projections.len() {
                return Err(("42601", "UNION requires matching column counts".into()));
            }
            for column in 0..self.select.projections.len() {
                let left = self.select.projections[column].expr.clone();
                let right = query.select.projections[column].expr.clone();
                let a = self.expr_type(&left)?.0;
                let b = query.expr_type(&right)?.0;
                if a != b {
                    if numeric_type(a) && numeric_type(b) {
                        // All arms share the promoted type announced in RowDescription.
                        self.select.projections[column].expr =
                            Expr::Cast(Box::new(left), CastType::Bigint);
                        for (_, previous) in &mut self.select.unions[..index] {
                            previous.projections[column].expr = Expr::Cast(
                                Box::new(previous.projections[column].expr.clone()),
                                CastType::Bigint,
                            );
                        }
                        query.select.projections[column].expr =
                            Expr::Cast(Box::new(right), CastType::Bigint);
                    } else {
                        return Err(("42804", "UNION catalog types do not match".into()));
                    }
                }
            }
            self.select.unions[index].1 = query.select;
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
        for (_, arm) in &self.select.unions {
            self.nested(arm.clone()).validate()?;
        }
        self.validate()
    }
    fn validate(&self) -> ParseResult<()> {
        let mut aliases = std::collections::HashSet::new();
        for (index, relation) in self.relations().enumerate() {
            if let Some(function) = &relation.function {
                if index == 0 {
                    return Err((
                        "0A000",
                        "catalog table functions require a left source".into(),
                    ));
                }
                let mut scope = self.clone();
                scope.select.joins.truncate(index - 1);
                scope.expr_type(&function.expr)?;
                if function.ordinality && relation.name != "unnest" {
                    return Err(("0A000", "ordinality requires unnest".into()));
                }
            }
            if relation.function.is_none()
                && relation.cte_id.is_none()
                && !matches!(
                    relation.name.as_str(),
                    "__pg_scalar"
                        | "pg_user"
                        | "pg_authid"
                        | "pg_class"
                        | "pg_attribute"
                        | "pg_type"
                        | "pg_attrdef"
                        | "pg_index"
                        | "pg_constraint"
                        | "pg_proc"
                        | "pg_language"
                        | "pg_operator"
                        | "pg_aggregate"
                        | "pg_depend"
                        | "pg_database"
                        | "pg_locks"
                        | "pg_namespace"
                        | "pg_tablespace"
                        | "pg_description"
                        | "pg_shdescription"
                        | "pg_sequence"
                        | "pg_foreign_table"
                        | "pg_foreign_server"
                        | "pg_collation"
                        | "pg_opfamily"
                        | "pg_amop"
                        | "pg_amproc"
                        | "pg_policy"
                        | "pg_trigger"
                        | "pg_rewrite"
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
            reject_projection_unnest(&join.on)?;
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
            if let Expr::Call(path, args) = &projection.expr {
                if path.last().is_some_and(|name| name == "unnest") {
                    for arg in args {
                        reject_projection_unnest(arg)?;
                    }
                } else {
                    reject_projection_unnest(&projection.expr)?;
                }
            } else {
                reject_projection_unnest(&projection.expr)?;
            }
            self.expr_type(&projection.expr)?;
        }
        if let Some(filter) = &self.select.filter {
            reject_projection_unnest(filter)?;
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
            let expr = self.order_expr(&order.expr)?;
            reject_projection_unnest(expr)?;
            self.expr_type(expr)?;
            if (self.select.distinct || !self.select.unions.is_empty())
                && !self.select.projections.iter().any(|p| &p.expr == expr)
            {
                return Err((
                    "0A000",
                    "DISTINCT/UNION ORDER BY requires an output column".into(),
                ));
            }
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
    fn relation_names(&self, relation: &pg_catalog_query::Relation) -> ParseResult<Vec<String>> {
        if let Some(id) = relation.cte_id {
            Ok(self.cte_columns[&id]
                .iter()
                .map(|(name, _, _)| name.clone())
                .collect())
        } else {
            Err((
                "0A000",
                "catalog wildcard expansion requires a CTE source".into(),
            ))
        }
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
            let field = if let Some(function) = &relation.function {
                let mut scope = self.clone();
                scope.select.joins.truncate(index.saturating_sub(1));
                let mut matches = function
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.as_str() == name);
                let field = matches
                    .next()
                    .map(|(slot, _)| {
                        if slot == 1 {
                            Ok((slot, 8, 0))
                        } else {
                            scope
                                .expr_type(&function.expr)
                                .map(|(code, flags)| (slot, code, flags))
                        }
                    })
                    .transpose()?;
                if matches.next().is_some() {
                    return Err(("42702", "ambiguous table function column".into()));
                }
                field
            } else if let Some(id) = relation.cte_id {
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
                    ("pg_class", "relname") => {
                        Some((1, crate::pg_result::CatalogColumnType::Name as u8, 0))
                    }
                    ("pg_class", "relnamespace") => Some((2, crate::pg_oid::OID_TYPE, 0)),
                    ("pg_class", "relkind") => Some((
                        3,
                        crate::pg_result::CatalogColumnType::InternalChar as u8,
                        0,
                    )),
                    ("pg_database", "oid") => Some((0, crate::pg_oid::OID_TYPE, 0)),
                    ("pg_database", "datname") => {
                        Some((1, crate::pg_result::CatalogColumnType::Name as u8, 0))
                    }
                    ("pg_database", "datistemplate") => Some((3, 1, boolean)),
                    ("pg_database", "datallowconn") => Some((4, 1, boolean)),
                    ("pg_database", "datdba") => Some((5, crate::pg_oid::OID_TYPE, 0)),
                    ("pg_namespace", "oid") => Some((0, crate::pg_oid::OID_TYPE, 0)),
                    ("pg_namespace", "nspname") => {
                        Some((1, crate::pg_result::CatalogColumnType::Name as u8, 0))
                    }
                    ("pg_namespace", "nspowner") => Some((5, crate::pg_oid::OID_TYPE, 0)),
                    ("pg_namespace", "nspacl") => {
                        Some((6, crate::pg_result::CatalogColumnType::AclArray as u8, 0))
                    }
                    ("pg_namespace", "xmin") => Some((8, 8, 0)),
                    ("pg_tablespace", "oid") => Some((0, crate::pg_oid::OID_TYPE, 0)),
                    ("pg_tablespace", "spcname") => Some((1, 253, 0)),
                    ("pg_tablespace", "spcowner") => Some((5, 8, 0)),
                    // Optional ACL/options use the bounded catalog's nullable text
                    // representation; no native tablespace rows currently exist.
                    ("pg_tablespace", "spcacl") => Some((8, 253, 0)),
                    ("pg_tablespace", "spcoptions") => Some((10, 253, 0)),
                    ("pg_authid", "oid") => Some((0, crate::pg_oid::OID_TYPE, 0)),
                    ("pg_authid", "rolname") => {
                        Some((1, crate::pg_result::CatalogColumnType::Name as u8, 0))
                    }
                    ("pg_user", "usename") => Some((0, 253, 0)),
                    ("pg_user", "usesuper") => Some((1, 1, boolean)),
                    ("pg_locks", "transactionid") => Some((0, 8, 0)),
                    ("pg_description" | "pg_shdescription", _) => {
                        description_column(&relation.name, name)
                    }
                    _ => column_catalog_field(&relation.name, name),
                }
            };
            if let Some((slot, code, flags)) = field {
                if found.is_some() {
                    if qualifier.is_none() && index > 0 && self.select.joins[index - 1].natural {
                        continue;
                    }
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
            Expr::Arithmetic(left, op, right) => {
                let a = self.expr_type(left)?.0;
                let b = self.expr_type(right)?.0;
                if *op == '|'
                    && (textual_type(a) || matches!(**left, Expr::Null))
                    && (textual_type(b) || matches!(**right, Expr::Null))
                {
                    return Ok((253, 0));
                }
                if matches!(op, '-' | '%')
                    && (numeric_type(a) || matches!(**left, Expr::Null))
                    && (numeric_type(b) || matches!(**right, Expr::Null))
                {
                    return Ok((8, 0));
                }
                Err(("0A000", "unsupported catalog arithmetic types".into()))
            }
            Expr::Subscript(input, index) => {
                let element = numeric_array_element(self.expr_type(input)?.0).ok_or_else(|| {
                    ("0A000", "subscript requires a numeric catalog array".into())
                })?;
                if !numeric_type(self.expr_type(index)?.0) && !matches!(**index, Expr::Null) {
                    return Err(("0A000", "array subscript requires an integer".into()));
                }
                Ok((element, 0))
            }
            Expr::Integer(_) => Ok((8, 0)),
            Expr::Boolean(_) => Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag)),
            Expr::Cast(inner, target) => {
                let (code, _) = self.expr_type(inner)?;
                if *target == CastType::Boolean && !matches!(**inner, Expr::Null) && code != 1 {
                    return Err((
                        "0A000",
                        "catalog boolean casts require boolean input".into(),
                    ));
                }
                if matches!(target, CastType::InternalChar | CastType::Char)
                    && !matches!(**inner, Expr::Null)
                    && !textual_type(code)
                {
                    return Err(("0A000", "catalog internal char casts require text".into()));
                }
                if matches!(target, CastType::Bigint | CastType::Integer) && code == 1 {
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
                if matches!(
                    target,
                    CastType::OperatorName
                        | CastType::TypeName
                        | CastType::ProcedureName
                        | CastType::FunctionName
                ) && !matches!(**inner, Expr::Null)
                    && !numeric_type(code)
                {
                    return Err((
                        "0A000",
                        "catalog object name conversions require an OID".into(),
                    ));
                }
                Ok((
                    match target {
                        CastType::Boolean => 1,
                        CastType::InternalChar => {
                            crate::pg_result::CatalogColumnType::InternalChar as u8
                        }
                        CastType::Char => crate::pg_result::CatalogColumnType::Char as u8,
                        CastType::Bigint => 8,
                        CastType::Integer => 3,
                        CastType::Varchar => 253,
                        CastType::Oid => crate::pg_oid::OID_TYPE,
                        CastType::Regclass => crate::pg_oid::REGCLASS_TYPE,
                        // No native exclusion operators have a PG name mapping.
                        // The only supported regoper value is zero, displayed as '-'.
                        CastType::OperatorName
                        | CastType::TypeName
                        | CastType::ProcedureName
                        | CastType::FunctionName => 253,
                        CastType::IntArray => crate::pg_result::CatalogColumnType::Int4Array as u8,
                    },
                    if *target == CastType::Boolean {
                        astersql_parser_mysql::r#type::IsBooleanFlag
                    } else {
                        0
                    },
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
                    ("to_regclass", [arg])
                        if matches!(arg, Expr::Null) || textual_type(self.expr_type(arg)?.0) =>
                    {
                        Ok((crate::pg_oid::REGCLASS_TYPE, 0))
                    }
                    ("current_user", []) => Ok((253, 0)),
                    ("pg_get_serial_sequence", [a, b])
                        if [a, b].iter().all(|arg| {
                            matches!(arg, Expr::Null)
                                || self
                                    .expr_type(arg)
                                    .is_ok_and(|(code, _)| textual_type(code))
                        }) =>
                    {
                        Ok((253, 0))
                    }
                    ("pg_is_in_recovery", []) => {
                        Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag))
                    }
                    ("txid_current", []) => Ok((8, 0)),
                    ("chr", [arg])
                        if numeric_type(self.expr_type(arg)?.0) || matches!(arg, Expr::Null) =>
                    {
                        Ok((253, 0))
                    }
                    ("pg_indexam_has_property", [oid, property])
                        if (numeric_type(self.expr_type(oid)?.0) || matches!(oid, Expr::Null))
                            && matches!(property,Expr::Text(s) if s=="can_order") =>
                    {
                        Ok((1, astersql_parser_mysql::r#type::IsBooleanFlag))
                    }
                    ("unnest", [input]) => numeric_array_element(self.expr_type(input)?.0)
                        .map(|code| (code, 0))
                        .ok_or_else(|| ("0A000", "unnest requires a numeric catalog array".into())),
                    ("current_database" | "current_catalog", [])
                        if self.select.from.name == "pg_database" =>
                    {
                        Ok((crate::pg_result::CatalogColumnType::Name as u8, 0))
                    }
                    ("current_schema", []) if self.select.from.name == "pg_namespace" => {
                        Ok((crate::pg_result::CatalogColumnType::Name as u8, 0))
                    }
                    ("pg_get_userbyid", [arg])
                        if matches!(arg, Expr::Null) || numeric_type(self.expr_type(arg)?.0) =>
                    {
                        Ok((crate::pg_result::CatalogColumnType::Name as u8, 0))
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
                    ("pg_get_viewdef", [arg])
                        if matches!(arg, Expr::Null)
                            || matches!(self.expr_type(arg)?.0, 253 | 254 | 15) =>
                    {
                        Ok((253, 0))
                    }
                    ("pg_get_viewdef", [arg, option])
                        if (matches!(arg, Expr::Null)
                            || numeric_type(self.expr_type(arg)?.0)
                            || matches!(self.expr_type(arg)?.0, 253 | 254 | 15))
                            && (matches!(option, Expr::Null)
                                || self.expr_type(option)?.0 == 1
                                || (self.expr_type(option)?.0 == 3
                                    || matches!(option, Expr::Integer(n) if i32::try_from(*n).is_ok()))
                                    && (matches!(arg, Expr::Null)
                                        || numeric_type(self.expr_type(arg)?.0))) =>
                    {
                        Ok((253, 0))
                    }
                    ("pg_get_constraintdef", [oid, pretty])
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
                    ("translate", [text, from, to])
                        if [text, from, to].iter().all(|arg| {
                            matches!(**arg, Expr::Null)
                                || self
                                    .expr_type(arg)
                                    .is_ok_and(|(code, _)| textual_type(code))
                        }) =>
                    {
                        Ok((253, 0))
                    }
                    ("pg_get_expr", [expression, relation] | [expression, relation, _])
                        if (matches!(expression, Expr::Null)
                            || self.expr_type(expression)?.0
                                == crate::pg_result::CatalogColumnType::PgNodeTree as u8)
                            && (matches!(relation, Expr::Null)
                                || numeric_type(self.expr_type(relation)?.0))
                            && (args.len() == 2
                                || matches!(args[2], Expr::Null)
                                || self.expr_type(&args[2])?.0 == 1) =>
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
                if self.expr_type(condition)?.0 != 1 {
                    return Err(("0A000", "unsupported CASE condition".into()));
                }
                let yes_type = self.expr_type(yes)?;
                let no_type = self.expr_type(no)?;
                if matches!(**yes, Expr::Null) {
                    return Ok(no_type);
                }
                if matches!(**no, Expr::Null) {
                    return Ok(yes_type);
                }
                let yes = yes_type;
                let no = no_type;
                if yes == no {
                    Ok(yes)
                } else if numeric_type(yes.0) && numeric_type(no.0) {
                    // Source aggregate metadata mixes zero with an OID. Both
                    // branches store signed integers, as in UNION promotion.
                    Ok((8, 0))
                } else {
                    Err(("0A000", "unsupported CASE types".into()))
                }
            }
        }
    }
    /// The lookup runs after result description, unlike regclass literal binding.
    pub(crate) fn describes_identity_lookup_error(&self) -> bool {
        self.select.projections.iter().any(|projection| {
            matches!(&projection.expr, Expr::Call(path, _) if path.last().is_some_and(|name| name == "pg_get_serial_sequence"))
        })
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
                postgresql_text_oid: None,
                postgresql_array_element: None,
                postgresql_record_fields: None,
            })
            .collect();
        // Catalog statement identity belongs to the PG statement name, not an
        // engine prepared statement. No backend handle is allocated or closed.
        PreparedMetadata {
            execution_native_types: Vec::new(),
            catalog_column_origins: self
                .select
                .projections
                .iter()
                .map(|projection| {
                    let Expr::Column(path) = &projection.expr else {
                        return None;
                    };
                    let (slot, _, _) = self.column(path).ok()?;
                    let relation = self.relations().nth(slot / CATALOG_ROW_WIDTH)?;
                    if relation.cte_id.is_some() {
                        return None;
                    }
                    // These are PostgreSQL catalog attribute numbers, not the
                    // compact row slots used internally by the catalog evaluator.
                    let attribute = match (relation.name.as_str(), slot % CATALOG_ROW_WIDTH) {
                        ("pg_type", 0) => 1,
                        ("pg_type", 1) => 2,
                        ("pg_type", 2) => 3,
                        ("pg_type", 3) => 7,
                        ("pg_type", 4) => 8,
                        ("pg_type", 5) => 12,
                        ("pg_type", 6) => 26,
                        ("pg_type", 7) => 27,
                        ("pg_type", 8) => 28,
                        ("pg_type", 9) => 31,
                        ("pg_type", 10) => 25,
                        ("pg_type", 12) => 14,
                        ("pg_type", 13) => 10,
                        ("pg_type", 16) => 15,
                        ("pg_type", 17) => 11,
                        ("pg_type", 18) => 5,
                        ("pg_type", 19) => 6,
                        ("pg_attribute", 1) => 5,
                        ("pg_attribute", 2) => 2,
                        ("pg_attribute", 4) => 6,
                        ("pg_attribute", 7) => 17,
                        ("pg_attribute", 10) => 16,
                        ("pg_shdescription", 6) => 1,
                        ("pg_shdescription", 7) => 2,
                        ("pg_shdescription", 2) => 3,
                        ("pg_authid", 0) => 1,
                        ("pg_authid", 1) => 2,
                        ("pg_database", 0) => 1,
                        ("pg_database", 1) => 2,
                        ("pg_database", 5) => 3,
                        ("pg_database", 3) => 6,
                        ("pg_database", 4) => 7,
                        ("pg_namespace", 0) => 1,
                        ("pg_namespace", 1) => 2,
                        ("pg_namespace", 5) => 3,
                        ("pg_namespace", 6) => 4,
                        ("pg_constraint", 0) => 1,
                        ("pg_constraint", 1) => 2,
                        ("pg_constraint", 2) => 4,
                        ("pg_constraint", 3) => 9,
                        ("pg_constraint", 4) => 3,
                        ("pg_constraint", 6) => 11,
                        ("pg_constraint", 5) => 21,
                        ("pg_constraint", 7) => 13,
                        ("pg_constraint", 8) => 22,
                        ("pg_constraint", 9) => 14,
                        ("pg_constraint", 10) => 15,
                        ("pg_constraint", 22) => 16,
                        ("pg_constraint", 11) => 5,
                        ("pg_constraint", 12) => 6,
                        ("pg_constraint", 23) => 8,
                        ("pg_constraint", 24) => 7,
                        ("pg_constraint", 25) => 17,
                        ("pg_constraint", 26) => 18,
                        ("pg_constraint", 27) => 12,
                        ("pg_constraint", 28) => 10,
                        ("pg_constraint", 29) => 20,
                        ("pg_constraint", 30) => 26,
                        ("pg_constraint", 31) => 23,
                        ("pg_constraint", 32) => 24,
                        ("pg_constraint", 33) => 25,
                        ("pg_constraint", 13) => 19,
                        ("pg_constraint", 15) => 27,
                        ("pg_operator", 0) => 8,
                        ("pg_operator", 1) => 9,
                        ("pg_operator", 2) => 10,
                        ("pg_operator", 3) => 3,
                        ("pg_operator", 4) => 1,
                        ("pg_operator", 6) => 2,
                        ("pg_operator", 7) => 5,
                        ("pg_index", 0) => 1,
                        ("pg_index", 1) => 2,
                        ("pg_index", 2) => 3,
                        ("pg_index", 3) => 4,
                        ("pg_index", 4) => 5,
                        ("pg_index", 5) => 7,
                        ("pg_index", 6) => 6,
                        ("pg_index", 13) => 11,
                        ("pg_index", 14) => 13,
                        ("pg_index", 7) => 16,
                        ("pg_index", 8) => 19,
                        ("pg_index", 10) => 17,
                        ("pg_index", 9) => 18,
                        ("pg_index", 11) => 20,
                        ("pg_index", 12) => 21,
                        ("pg_index", 15) => 8,
                        ("pg_index", 16) => 9,
                        ("pg_index", 17) => 10,
                        ("pg_index", 18) => 12,
                        ("pg_index", 19) => 14,
                        ("pg_index", 20) => 15,
                        ("pg_opclass", 0) => 1,
                        ("pg_opclass", 1) => 2,
                        ("pg_opclass", 3) => 3,
                        ("pg_opclass", 4) => 7,
                        ("pg_opclass", 5) => 9,
                        ("pg_opclass", 6) => 8,
                        ("pg_opclass", 7) => 6,
                        ("pg_opclass", 8) => 5,
                        ("pg_opclass", 9) => 4,
                        ("pg_attribute", 0) => 1,
                        ("pg_sequence", 0) => 1,
                        ("pg_attribute", 3) => 3,
                        ("pg_attribute", 6) => 12,
                        ("pg_attribute", 9) => 15,
                        ("pg_attribute", 12) => 13,
                        ("pg_attrdef", 0) => 2,
                        ("pg_attrdef", 1) => 3,
                        ("pg_attrdef", 2) => 4,
                        ("pg_attrdef", 3) => 1,
                        ("pg_sequence", 2) => 3,
                        ("pg_sequence", 3) => 4,
                        ("pg_sequence", 4) => 6,
                        ("pg_sequence", 5) => 5,
                        ("pg_sequence", 6) => 7,
                        ("pg_sequence", 7) => 8,
                        ("pg_depend", 4) => 6,
                        ("pg_depend", 5) => 7,
                        ("pg_class", 0) => 1,
                        ("pg_class", 1) => 2,
                        ("pg_class", 2) => 3,
                        ("pg_class", 3) => 18,
                        ("pg_class", 11) => 6,
                        _ => return None,
                    };
                    let oid = crate::pg_oid::SYSTEM_RELATIONS
                        .iter()
                        .find(|(name, _)| *name == relation.name)?
                        .1;
                    Some((oid, attribute))
                })
                .collect(),
            column_origins: Vec::new(),
            parameter_cast_oids: Vec::new(),
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
            database_facts: Default::default(),
            database_role_names: Default::default(),
            providers: Default::default(),
            ctes: Default::default(),
            materialized: std::cell::Cell::new(0),
            work: std::cell::Cell::new(0),
        };
        let rows = self.execute_select(&mut execution, true)?;
        Ok(QueryResult {
            catalog_column_origins: metadata.catalog_column_origins,
            column_origins: Vec::new(),
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
        if query.select.unions.is_empty() {
            return query.execute_rows(execution, display);
        }
        let unions = std::mem::take(&mut query.select.unions);
        let order = std::mem::take(&mut query.select.order);
        let limit = query.select.limit.take();
        let mut rows = query.execute_rows(execution, display)?;
        for (all, arm) in unions {
            let arm_rows = self.nested(arm).execute_select(execution, display)?;
            rows.extend(arm_rows);
            if rows.len() > MAX_CATALOG_ROWS {
                return Err(catalog_row_limit());
            }
            if !all {
                rows = distinct_rows(rows, execution)?;
            }
        }
        let mut keys = Vec::new();
        for ordering in &order {
            let expression = self
                .order_expr(&ordering.expr)
                .expect("validated UNION order");
            let index = self
                .select
                .projections
                .iter()
                .position(|p| &p.expr == expression)
                .ok_or_else(|| {
                    ConnError::Session("UNION ORDER BY requires an output column".into())
                })?;
            keys.push(index);
        }
        rows.sort_by(|a, b| {
            compare_keys(
                &keys.iter().map(|i| a[*i].clone()).collect::<Vec<_>>(),
                &keys.iter().map(|i| b[*i].clone()).collect::<Vec<_>>(),
                &order,
            )
        });
        check_catalog_cancel(execution.cancel)?;
        rows.truncate(limit.unwrap_or(usize::MAX));
        Ok(rows)
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
            if relation.function.is_some() {
                providers.push(std::sync::Arc::new(Vec::new()));
            } else if let Some(id) = relation.cte_id {
                providers.push(execution.ctes.borrow()[&id].clone());
            } else {
                if !execution.providers.borrow().contains_key(&relation.name) {
                    let mut rows = self.provider_rows(
                        &relation.name,
                        execution.context,
                        execution.snapshot.as_ref(),
                        &execution.database,
                        execution.cancel,
                        execution,
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
                let lateral;
                let right = if let Some(function) = &join.relation.function {
                    let mut input_scope = self.clone();
                    input_scope.select.joins.truncate(index);
                    let values = match &function.expr {
                        Expr::Call(path, args) if path.last().is_some_and(|n| n == "unnest") => {
                            numeric_array_values(
                                &input_scope.evaluate(&args[0], &left, execution)?,
                            )?
                        }
                        expr => vec![input_scope.evaluate(expr, &left, execution)?],
                    };
                    let mut generated = Vec::new();
                    for (position, value) in values.into_iter().enumerate() {
                        execution.comparison()?;
                        let mut row = vec![Value::Null; CATALOG_ROW_WIDTH];
                        row[0] = value;
                        if function.ordinality {
                            row[1] = Value::Signed((position + 1) as i64);
                        }
                        generated.push(row);
                    }
                    lateral = execution.materialize(generated)?;
                    lateral.as_ref()
                } else {
                    right
                };
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
        execution: &Execution<'_>,
    ) -> ConnResult<Vec<Vec<Value>>> {
        let rows = match name {
            "__pg_scalar" => vec![vec![Value::Null; CATALOG_ROW_WIDTH]],
            "pg_authid" => {
                // This bounded provider exposes only registered PG identity facts,
                // never native account credentials. Keep the catalog administrator-only.
                if context.user_identity()?.split('@').next() != Some("root") {
                    return Err(ConnError::PgFunction(
                        astersql_session::runtime::PgFunctionError {
                            state: "42501",
                            message: "permission denied for table pg_authid".into(),
                            detail: None,
                            hint: None,
                            position: None,
                        },
                    ));
                }
                execution.database_facts()?;
                execution
                    .database_role_names
                    .borrow()
                    .iter()
                    .map(|(oid, name)| vec![Value::Signed(*oid), Value::Text(name.clone())])
                    .collect()
            }
            "pg_user" => {
                let results = context.execute_query(
                    "SELECT User, Super_priv FROM mysql.user",
                    false,
                    cancel,
                )?;
                let mut rows = Vec::new();
                for result in results {
                    for row in result.rows {
                        check_catalog_cancel(cancel)?;
                        if let [Value::Text(user), Value::Text(privilege)] = row.as_slice() {
                            let candidate = vec![
                                Value::Text(user.clone()),
                                Value::Text((privilege == "Y").to_string()),
                            ];
                            if !rows.contains(&candidate) {
                                rows.push(candidate);
                            }
                        } else {
                            return Err(ConnError::Session(
                                "invalid native user privilege metadata".into(),
                            ));
                        }
                    }
                }
                rows
            }
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
            // PostgreSQL extension objects (foreign wrappers, custom operators,
            // policies, triggers and rewrite rules) have no native PG object
            // identity or definition. MySQL collations are not PG collations.
            "pg_inherits" | "pg_aggregate" | "pg_foreign_table" | "pg_foreign_server"
            | "pg_collation" | "pg_opfamily" | "pg_amop" | "pg_amproc" | "pg_policy"
            | "pg_trigger" | "pg_rewrite" => Vec::new(),
            "pg_operator" => [(94, 21), (96, 23), (410, 20)]
                .into_iter()
                .map(|(oid, input)| {
                    // The supported FK keys are signed integers. The shared runtime
                    // uses exact integer equality; these PG builtin identities describe
                    // that same comparison, without inventing an operator or function.
                    let mut row = vec![Value::Null; CATALOG_ROW_WIDTH];
                    row[0] = Value::Signed(input);
                    row[1] = Value::Signed(input);
                    row[2] = Value::Signed(16);
                    row[3] = Value::Signed(11);
                    row[4] = Value::Signed(oid);
                    row[6] = Value::Text("=".into());
                    row[7] = Value::Text("b".into());
                    row
                })
                .collect(),
            "pg_opclass" => integer_opclass_rows(),
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
                &context
                    .pg_namespace_facts()
                    .transpose()?
                    .unwrap_or_default(),
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
                let snapshot = snapshot.ok_or_else(|| {
                    if name == "pg_database" {
                        database_identity_unavailable("schema snapshot is unavailable")
                    } else {
                        ConnError::Session("schema snapshot is unavailable".into())
                    }
                })?;
                let schemas = snapshot.AllSchemas();
                if name == "pg_class" {
                    class_rows(snapshot.as_ref(), database)?
                } else if name == "pg_namespace" {
                    namespace_rows_for_context(context, snapshot.as_ref(), database)?
                } else {
                    let facts = execution
                        .database_facts()?
                        .into_iter()
                        .map(|f| (f.id, f))
                        .collect::<std::collections::BTreeMap<_, _>>();
                    // Native schema metadata has no shared-description rows. A
                    // LEFT JOIN to this empty relation keeps every schema and
                    // gives all right-side fields NULL, regardless of its predicate.
                    schemas
                        .into_iter()
                        .map(|schema| {
                            // Both projections refer to the same native database object.
                            let oid = namespace_oid(schema.id).map_err(|_| {
                                database_identity_unavailable(
                                    "PostgreSQL database identity is unavailable",
                                )
                            })?;
                            Ok(vec![
                                Value::Signed(oid),
                                Value::Text(schema.name.original.clone()),
                                Value::Null,
                                facts
                                    .get(&schema.id)
                                    .and_then(|f| f.is_template)
                                    .map_or(Value::Null, |v| Value::Text(v.to_string())),
                                facts
                                    .get(&schema.id)
                                    .and_then(|f| f.allow_connections)
                                    .map_or(Value::Null, |v| Value::Text(v.to_string())),
                                facts
                                    .get(&schema.id)
                                    .and_then(|f| f.owner.as_ref())
                                    .map_or(Value::Null, |(oid, _)| Value::Signed(*oid)),
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
            // Native databases have no shared comment source. Table, column,
            // index and sequence comments are projected separately below.
            "pg_shdescription" => Vec::new(),
            "pg_description" | "pg_sequence" => native_metadata_rows(
                name,
                snapshot
                    .ok_or_else(|| ConnError::Session("schema snapshot is unavailable".into()))?
                    .as_ref(),
                database,
                cancel,
            )?,
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
            Expr::Column(path) => {
                let slot = self.column(path).expect("validated column").0;
                let local = slot % CATALOG_ROW_WIDTH;
                let base = slot - local;
                if self
                    .relations()
                    .nth(slot / CATALOG_ROW_WIDTH)
                    .is_some_and(|r| r.name == "pg_index")
                    && row[base] != Value::Null
                    && matches!(local, 9 | 10 | 13..=20)
                    && row[slot] == Value::Null
                {
                    return Err(database_identity_unavailable(
                        "PostgreSQL index auxiliary identity or synchronous build evidence is unavailable",
                    ));
                }

                if self
                    .relations()
                    .nth(slot / CATALOG_ROW_WIDTH)
                    .is_some_and(|r| r.name == "pg_operator")
                    && row[base + 4] != Value::Null
                    && row[slot] == Value::Null
                {
                    return Err(database_identity_unavailable(
                        "PostgreSQL operator auxiliary identity is unavailable",
                    ));
                }
                if self
                    .relations()
                    .nth(slot / CATALOG_ROW_WIDTH)
                    .is_some_and(|r| r.name == "pg_constraint")
                    && row[base] != Value::Null
                    && (matches!(local, 11..=13 | 23..=29)
                        || (matches!(local, 31..=33) && row[base + 2] == Value::Text("f".into())))
                    && row[slot] == Value::Null
                {
                    return Err(database_identity_unavailable(
                        "PostgreSQL constraint validation, immediate execution or operator identity is unavailable",
                    ));
                }

                if self
                    .relations()
                    .nth(slot / CATALOG_ROW_WIDTH)
                    .is_some_and(|r| r.name == "pg_database")
                    && matches!(slot % CATALOG_ROW_WIDTH, 3..=5)
                    // NULL-extension of an unmatched outer join is a SQL NULL,
                    // not a legacy database with missing ownership metadata.
                    && row[slot - slot % CATALOG_ROW_WIDTH] != Value::Null
                    && row[slot] == Value::Null
                {
                    return Err(database_identity_unavailable(
                        "PostgreSQL database owner or policy requires explicit administrator migration",
                    ));
                }
                if self
                    .relations()
                    .nth(slot / CATALOG_ROW_WIDTH)
                    .is_some_and(|r| r.name == "pg_namespace")
                    && slot % CATALOG_ROW_WIDTH == 5
                    && row[slot - 5] != Value::Null
                    && row[slot] == Value::Null
                {
                    return Err(database_identity_unavailable(
                        "PostgreSQL namespace owner identity is unavailable",
                    ));
                }
                if self
                    .relations()
                    .nth(slot / CATALOG_ROW_WIDTH)
                    .is_some_and(|r| r.name == "pg_namespace")
                    && slot % CATALOG_ROW_WIDTH == 6
                    && row[slot - 6] != Value::Null
                    && row[slot + 1] != Value::Signed(1)
                {
                    return Err(database_identity_unavailable(
                        "PostgreSQL namespace ACL source is unavailable",
                    ));
                }
                if self
                    .relations()
                    .nth(slot / CATALOG_ROW_WIDTH)
                    .is_some_and(|r| r.name == "pg_class")
                {
                    let local = slot % CATALOG_ROW_WIDTH;
                    if local == 3 && row[slot] == Value::Text("?".into()) {
                        return Err(database_identity_unavailable(
                            "native physical partition is not a PostgreSQL partitioned relation",
                        ));
                    }
                    if local == 11 && row[slot - local] != Value::Null {
                        match row[slot] {
                            Value::Signed(oid)
                                if execution.database_role_name(oid)? != Value::Null => {}
                            _ => {
                                return Err(database_identity_unavailable(
                                    "PostgreSQL relation owner requires explicit administrator migration",
                                ));
                            }
                        }
                    }
                }
                if self
                    .relations()
                    .nth(slot / CATALOG_ROW_WIDTH)
                    .is_some_and(|r| r.name == "pg_constraint")
                    && slot % CATALOG_ROW_WIDTH == 14
                    && row[slot - 12] == Value::Text("c".into())
                {
                    return Err(ConnError::UnsupportedCommand(0));
                }
                row[slot].clone()
            }
            Expr::Null => Value::Null,
            Expr::TypedLiteral(inner, _, _) => evaluate(inner)?,
            Expr::Arithmetic(left, op, right) => match (evaluate(left)?, evaluate(right)?) {
                (Value::Null, _) | (_, Value::Null) => Value::Null,
                (Value::Text(a), Value::Text(b)) if *op == '|' => Value::Text(a + &b),
                (Value::Signed(a), Value::Signed(b)) => Value::Signed(
                    match op {
                        '-' => a.checked_sub(b),
                        '%' => a.checked_rem(b),
                        _ => None,
                    }
                    .ok_or_else(|| {
                        ConnError::Session("catalog arithmetic overflow or division by zero".into())
                    })?,
                ),
                _ => unreachable!("validated catalog arithmetic"),
            },
            Expr::Subscript(input, index) => {
                let code = self.expr_type(input).expect("validated array").0;
                match (evaluate(input)?, evaluate(index)?) {
                    (Value::Null, _) | (_, Value::Null) => Value::Null,
                    (array, Value::Signed(index)) => {
                        let lower = if matches!(code, c if c==crate::pg_result::CatalogColumnType::Int2Vector as u8 || c==crate::pg_result::CatalogColumnType::OidVector as u8)
                        {
                            0
                        } else {
                            1
                        };
                        let values = numeric_array_values(&array)?;
                        index
                            .checked_sub(lower)
                            .and_then(|i| usize::try_from(i).ok())
                            .and_then(|i| values.get(i))
                            .cloned()
                            .unwrap_or(Value::Null)
                    }
                    _ => unreachable!("validated subscript"),
                }
            }
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
                (Value::Text(s), CastType::Boolean) => Value::Text(s),
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
                (Value::Text(s), CastType::Char) => Value::Text(s.chars().take(1).collect()),
                (Value::Signed(oid), CastType::TypeName) => {
                    Value::Text(format_column_type(oid, None)?)
                }
                (Value::Signed(0), CastType::FunctionName) => Value::Text("-".into()),
                (Value::Signed(oid), CastType::FunctionName) => Value::Text(
                    FUNCTION_INTROSPECTION
                        .iter()
                        .find(|f| f.oid == oid)
                        .ok_or(ConnError::UnsupportedCommand(0))?
                        .name
                        .into(),
                ),
                (Value::Signed(0), CastType::ProcedureName) => Value::Text("-".into()),
                (Value::Signed(oid), CastType::ProcedureName) => {
                    let function = FUNCTION_INTROSPECTION
                        .iter()
                        .find(|f| f.oid == oid)
                        .ok_or(ConnError::UnsupportedCommand(0))?;
                    Value::Text(format!("{}(oid)", function.name))
                }
                (Value::Signed(0), CastType::OperatorName) => Value::Text("-".into()),
                (Value::Signed(_), CastType::OperatorName) => {
                    return Err(ConnError::UnsupportedCommand(0));
                }
                (Value::Signed(n), CastType::Integer) => {
                    Value::Signed(i64::from(i32::try_from(n).map_err(|_| {
                        ConnError::Session("PG integer conversion out of range".into())
                    })?))
                }
                (Value::Text(s), CastType::Integer) => Value::Signed(i64::from(
                    s.parse::<i32>()
                        .map_err(|_| ConnError::Session("invalid PG integer value".into()))?,
                )),
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
                    Value::Signed(i64::from(crate::pg_oid::resolve_in_path(
                        &s,
                        database,
                        snapshot,
                        &self.relation_path,
                        &execution
                            .context
                            .pg_namespace_facts()
                            .transpose()?
                            .unwrap_or_default(),
                    )?))
                }
                (Value::Signed(n), CastType::Varchar) => {
                    if self.expr_type(inner).expect("validated expression").0
                        == crate::pg_oid::REGCLASS_TYPE
                    {
                        let snapshot = snapshot.ok_or_else(|| {
                            ConnError::Session("schema snapshot is unavailable".into())
                        })?;
                        Value::Text(crate::pg_oid::display_in_path(
                            u32::try_from(n).map_err(|_| {
                                ConnError::Session("PG oid conversion out of range".into())
                            })?,
                            database,
                            snapshot,
                            &self.relation_path,
                            &execution
                                .context
                                .pg_namespace_facts()
                                .transpose()?
                                .unwrap_or_default(),
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
                "to_regclass" => match evaluate(&args[0])? {
                    Value::Null => Value::Null,
                    Value::Text(input) => {
                        let snapshot = snapshot.ok_or(ConnError::UnsupportedCommand(0))?;
                        match crate::pg_oid::resolve_in_path(
                            &input,
                            database,
                            snapshot,
                            &self.relation_path,
                            &execution
                                .context
                                .pg_namespace_facts()
                                .transpose()?
                                .unwrap_or_default(),
                        ) {
                            Ok(oid) => Value::Signed(i64::from(oid)),
                            Err(error)
                                if matches!(
                                    crate::pg_conn::sqlstate(&error),
                                    "42P01" | "42602" | "22003"
                                ) =>
                            {
                                Value::Null
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    _ => unreachable!("validated to_regclass input"),
                },
                "pg_get_serial_sequence" => match (evaluate(&args[0])?, evaluate(&args[1])?) {
                    (Value::Null, _) | (_, Value::Null) => Value::Null,
                    (Value::Text(relation), Value::Text(column)) => {
                        let snapshot = snapshot.ok_or_else(|| {
                            ConnError::Session("schema snapshot is unavailable".into())
                        })?;
                        let oid = crate::pg_oid::resolve_in_path(
                            &relation,
                            database,
                            snapshot,
                            &self.relation_path,
                            &execution
                                .context
                                .pg_namespace_facts()
                                .transpose()?
                                .unwrap_or_default(),
                        )?;
                        // Follow the durable column reference; never infer ownership from names.
                        let mut sequence_id = None;
                        let mut found_column = false;
                        for schema in snapshot
                            .AllSchemas()
                            .into_iter()
                            .filter(|s| s.name.lower == database.to_lowercase())
                        {
                            for table in snapshot
                                .SchemaTableInfos(&schema.name)
                                .map_err(|e| ConnError::Session(e.to_string()))?
                            {
                                if crate::pg_oid::table_oid(table.id).ok() != Some(oid) {
                                    continue;
                                }
                                if let Some(model) = &table.model_meta {
                                    if let Some(col) =
                                        model.Columns.iter().find(|c| c.Name.O == column)
                                    {
                                        found_column = true;
                                        sequence_id =
                                            col.PostgreSQLIdentity.as_ref().map(|i| i.SequenceID);
                                    }
                                }
                            }
                        }
                        if !found_column {
                            return Err(ConnError::Session(format!("Unknown column {column}")));
                        }
                        let mut result = Value::Null;
                        if let Some(id) = sequence_id {
                            for schema in snapshot
                                .AllSchemas()
                                .into_iter()
                                .filter(|s| s.name.lower == database.to_lowercase())
                            {
                                for table in snapshot
                                    .SchemaTableInfos(&schema.name)
                                    .map_err(|e| ConnError::Session(e.to_string()))?
                                {
                                    if table.id == id {
                                        result = Value::Text(format!(
                                            "public.{}",
                                            crate::pg_oid::quoted(&table.name.original)
                                        ));
                                    }
                                }
                            }
                        }
                        result
                    }
                    _ => unreachable!("validated serial sequence arguments"),
                },
                "translate" => match (
                    evaluate(&args[0])?,
                    evaluate(&args[1])?,
                    evaluate(&args[2])?,
                ) {
                    (Value::Null, _, _) | (_, Value::Null, _) | (_, _, Value::Null) => Value::Null,
                    (Value::Text(text), Value::Text(from), Value::Text(to)) => {
                        let mut replacements = std::collections::HashMap::new();
                        let mut to = to.chars();
                        for c in from.chars() {
                            execution.comparison()?;
                            let replacement = to.next();
                            // PostgreSQL uses the first occurrence in `from`.
                            replacements.entry(c).or_insert(replacement);
                        }
                        let mut translated = String::new();
                        for c in text.chars() {
                            execution.comparison()?;
                            match replacements.get(&c) {
                                Some(Some(replacement)) => translated.push(*replacement),
                                Some(None) => {}
                                None => translated.push(c),
                            }
                        }
                        Value::Text(translated)
                    }
                    _ => unreachable!("validated translate arguments"),
                },
                "current_user" => Value::Text(
                    execution
                        .context
                        .user_identity()?
                        .split('@')
                        .next()
                        .unwrap_or("")
                        .into(),
                ),
                "pg_is_in_recovery" => Value::Text("false".into()),
                "txid_current" => {
                    let results = execution.context.execute_query(
                        "SELECT @@tidb_current_ts",
                        false,
                        execution.cancel,
                    )?;
                    match results
                        .first()
                        .and_then(|r| r.rows.first())
                        .and_then(|r| r.first())
                    {
                        Some(Value::Text(ts)) => Value::Signed(ts.parse().map_err(|_| {
                            ConnError::Session("invalid native transaction timestamp".into())
                        })?),
                        Some(Value::Unsigned(ts)) => {
                            Value::Signed(i64::try_from(*ts).map_err(|_| {
                                ConnError::Session(
                                    "native transaction timestamp exceeds bigint".into(),
                                )
                            })?)
                        }
                        Some(Value::Signed(ts)) => Value::Signed(*ts),
                        _ => {
                            return Err(ConnError::Session(
                                "missing native transaction timestamp".into(),
                            ));
                        }
                    }
                }
                "chr" => match evaluate(&args[0])? {
                    Value::Null => Value::Null,
                    Value::Signed(code) => Value::Text(
                        u32::try_from(code)
                            .ok()
                            .filter(|c| *c != 0)
                            .and_then(char::from_u32)
                            .ok_or_else(|| ConnError::Session("invalid character code".into()))?
                            .to_string(),
                    ),
                    _ => unreachable!("validated chr"),
                },
                // No native index publishes a PostgreSQL access-method OID.
                // PostgreSQL returns NULL for an unknown method/property.
                "pg_indexam_has_property" => {
                    for arg in args {
                        evaluate(arg)?;
                    }
                    Value::Null
                }
                "current_database" | "current_catalog" => {
                    if database.is_empty()
                        || !snapshot.is_some_and(|s| {
                            s.AllSchemas().iter().any(|d| d.name.original == database)
                        })
                    {
                        return Err(database_identity_unavailable(
                            "PostgreSQL current database identity is unavailable",
                        ));
                    }
                    Value::Text(database.into())
                }
                "current_schema" => self
                    .current_schema
                    .as_ref()
                    .map_or(Value::Null, |s| Value::Text(s.clone())),
                "pg_get_userbyid" => match evaluate(&args[0])? {
                    Value::Null => Value::Null,
                    Value::Signed(oid) => execution.database_role_name(oid)?,
                    _ => unreachable!("validated role OID"),
                },
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
                    let oid = match &values[0] {
                        Value::Signed(oid) => *oid,
                        Value::Text(name) => i64::from(crate::pg_oid::resolve_in_path(
                            name,
                            database,
                            snapshot.ok_or_else(|| {
                                ConnError::Session("schema snapshot is unavailable".into())
                            })?,
                            &self.relation_path,
                            &execution
                                .context
                                .pg_namespace_facts()
                                .transpose()?
                                .unwrap_or_default(),
                        )?),
                        _ => unreachable!("validated view identity"),
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
                                    crate::pg_name::stored_view_select(&view.SelectStmt).map_err(
                                        |_| {
                                            database_identity_unavailable(
                                                "invalid stored view source",
                                            )
                                        },
                                    )?;
                                    let bound = model.PostgreSQLView.as_ref().ok_or_else(|| {
                                        database_identity_unavailable(
                                            "view has no bound PostgreSQL definition",
                                        )
                                    })?;
                                    let tables =
                                        snapshot.SchemaTableInfos(&schema.name).map_err(|e| {
                                            database_identity_unavailable(&e.to_string())
                                        })?;
                                    let base = bound.RelationID.and_then(|id| {
                                        tables
                                            .iter()
                                            .filter_map(|t| t.model_meta.as_deref())
                                            .find(|t| t.ID == id)
                                    });
                                    let facts = execution
                                        .context
                                        .pg_namespace_facts()
                                        .transpose()?
                                        .unwrap_or_default();
                                    let namespace =
                                        match base.and_then(|t| t.PostgreSQLNamespace.as_ref()) {
                                            Some((id, _)) => facts
                                                .iter()
                                                .find(|f| f.id == *id)
                                                .map(|f| f.name.as_str())
                                                .ok_or_else(|| {
                                                    database_identity_unavailable(
                                                        "view namespace is unavailable",
                                                    )
                                                })?,
                                            None => "public",
                                        };
                                    definition = Value::Text(
                                        bound.render(model, base, namespace, false).ok_or_else(
                                            || {
                                                database_identity_unavailable(
                                                    "view binding is unavailable or incompatible",
                                                )
                                            },
                                        )?,
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
                            &execution
                                .context
                                .pg_namespace_facts()
                                .transpose()?
                                .unwrap_or_default(),
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
                                        .map_or_else(
                                            || Value::Text(String::new()),
                                            |name| Value::Text(name.into()),
                                        ),
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
                "pg_get_expr" => {
                    let expression = evaluate(&args[0])?;
                    let relation = evaluate(&args[1])?;
                    let pretty = if args.len() == 3 {
                        evaluate(&args[2])?
                    } else {
                        Value::Text("false".into())
                    };
                    match (expression, relation, pretty) {
                        (Value::Null, _, _) | (_, Value::Null, _) | (_, _, Value::Null) => {
                            Value::Null
                        }
                        (Value::Text(tree), Value::Signed(relation), _) => {
                            Value::Text(if tree.starts_with("{OPEXPR ") {
                                deparse_generated_tree(
                                    &tree,
                                    relation,
                                    snapshot.ok_or(ConnError::UnsupportedCommand(0))?,
                                    database,
                                )?
                            } else {
                                deparse_default_tree(&tree)?
                            })
                        }
                        _ => return Err(ConnError::UnsupportedCommand(0)),
                    }
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
        if self.select.limit == Some(0) {
            return Ok(Vec::new());
        }
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
        let mut projected = Vec::new();
        'projection: for (row, _) in selected {
            // PostgreSQL zips multiple projection SRFs, padding shorter arrays
            // with NULL. NULL/empty arrays emit no rows when all SRFs are empty.
            let mut arrays = Vec::new();
            let mut count = 1;
            for projection in &self.select.projections {
                if let Expr::Call(path, args) = &projection.expr {
                    if path.last().is_some_and(|name| name == "unnest") {
                        let values =
                            numeric_array_values(&self.evaluate(&args[0], &row, execution)?)?;
                        arrays.push(Some(values));
                        continue;
                    }
                }
                arrays.push(None);
            }
            if arrays.iter().any(Option::is_some) {
                count = arrays.iter().flatten().map(Vec::len).max().unwrap_or(0);
            }
            for index in 0..count {
                execution.comparison()?;
                let output = self
                    .select
                    .projections
                    .iter()
                    .zip(&arrays)
                    .map(|(projection, array)| {
                        if let Some(values) = array {
                            Ok(values.get(index).unwrap_or(&Value::Null).clone())
                        } else {
                            self.evaluate(&projection.expr, &row, execution)
                        }
                    })
                    .collect::<ConnResult<Vec<_>>>()?;
                projected.push(output);
                if projected.len() > MAX_CATALOG_ROWS {
                    return Err(catalog_row_limit());
                }
                if !self.select.distinct && self.select.limit == Some(projected.len()) {
                    break 'projection;
                }
            }
        }
        if self.select.distinct {
            projected = distinct_rows(projected, execution)?;
        }
        projected.truncate(self.select.limit.unwrap_or(usize::MAX));
        projected
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .zip(&self.select.projections)
                    .map(|(value, projection)| {
                        if display
                            && self
                                .expr_type(&projection.expr)
                                .expect("validated expression")
                                .0
                                == crate::pg_oid::REGCLASS_TYPE
                        {
                            if let Value::Signed(oid) = value {
                                return Ok(Value::Text(crate::pg_oid::display_in_path(
                                    u32::try_from(oid).map_err(|_| {
                                        ConnError::Session("PG oid conversion out of range".into())
                                    })?,
                                    database,
                                    snapshot.ok_or_else(|| {
                                        ConnError::Session("schema snapshot is unavailable".into())
                                    })?,
                                    &self.relation_path,
                                    &execution
                                        .context
                                        .pg_namespace_facts()
                                        .transpose()?
                                        .unwrap_or_default(),
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

fn reject_projection_unnest(expr: &Expr) -> ParseResult<()> {
    visit_expr(&mut expr.clone(), &mut |expr| {
        if matches!(expr, Expr::Call(path, _) if path.last().is_some_and(|name| name == "unnest")) {
            return Err((
                "0A000",
                "unnest is only supported as a direct catalog projection".into(),
            ));
        }
        Ok(())
    })
}

fn distinct_rows(rows: Vec<Vec<Value>>, execution: &Execution<'_>) -> ConnResult<Vec<Vec<Value>>> {
    let mut result = Vec::new();
    for row in rows {
        let mut duplicate = false;
        for previous in &result {
            execution.comparison()?;
            if previous == &row {
                duplicate = true;
                break;
            }
        }
        if !duplicate {
            result.push(row);
        }
    }
    Ok(result)
}

fn textual_type(code: u8) -> bool {
    code == crate::pg_result::CatalogColumnType::Name as u8
        || code == 253
        || code == crate::pg_result::CatalogColumnType::InternalChar as u8
        || code == crate::pg_result::CatalogColumnType::Char as u8
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
        Expr::Subscript(a, b) | Expr::Arithmetic(a, _, b) => has_aggregate(a) || has_aggregate(b),
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
        Expr::Subscript(a, b) | Expr::Arithmetic(a, _, b) => {
            ungrouped_column(a) || ungrouped_column(b)
        }
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
        194 => (crate::pg_result::CatalogColumnType::PgNodeTree as u8, 0),
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
    for (_, arm) in &mut select.unions {
        visit_all_select_exprs(arm, visitor)?;
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
    if let Some(function) = &mut select.from.function {
        visit_expr(&mut function.expr, visitor)?;
    }
    for j in &mut select.joins {
        if let Some(function) = &mut j.relation.function {
            visit_expr(&mut function.expr, visitor)?;
        }
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
        Expr::Subscript(a, b)
        | Expr::Arithmetic(a, _, b)
        | Expr::Any(a, b)
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

// Description fields are NULL-extended for an unmatched LEFT JOIN.
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
        | Expr::Subscript(left, right)
        | Expr::Arithmetic(left, _, right)
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
pub(crate) fn class_rows(
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
    // Invalid native identity is a metadata capability gate; literal OID casts
    // retain their separate 22003 numeric-overflow contract.
    let namespace =
        crate::pg_oid::namespace_oid(schema.id).map_err(|_| ConnError::UnsupportedCommand(0))?;
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
        let namespace = model
            .PostgreSQLNamespace
            .as_ref()
            .map_or(Ok(namespace), |(id, _)| crate::pg_oid::namespace_oid(*id))
            .map_err(|_| ConnError::UnsupportedCommand(0))?;
        let partitioned = model.GetPartitionInfo().is_some();
        let kind = if model.View.is_some() {
            "v"
        } else if model.Sequence.is_some() {
            "S"
        } else if partitioned {
            "?"
        } else {
            "r"
        };
        let mut relation = class_row(
            crate::pg_oid::table_oid(model.ID).map_err(|_| ConnError::UnsupportedCommand(0))?,
            model
                .PostgreSQLNamespace
                .as_ref()
                .map_or(model.Name.O.as_str(), |(_, name)| name.as_str()),
            namespace,
            kind,
        );
        relation[11] = model.PostgreSQLOwner.map_or(Value::Null, Value::Signed);
        rows.push(relation);
        for index in &catalog_indexes(model)? {
            if index.State == astersql_meta_model::StatePublic {
                let mut relation = class_row(
                    crate::pg_oid::index_oid(model.ID, index.ID)
                        .map_err(|_| ConnError::UnsupportedCommand(0))?,
                    &index.Name.O,
                    namespace,
                    if partitioned { "?" } else { "i" },
                );
                relation[11] = model.PostgreSQLOwner.map_or(Value::Null, Value::Signed);
                rows.push(relation);
            }
        }
    }
    Ok(rows)
}

// Both providers read the execution snapshot and share pg_class identities.
fn native_metadata_rows(
    name: &str,
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
    cancel: &CancellationToken,
) -> ConnResult<Vec<Vec<Value>>> {
    let mut rows = Vec::new();
    for table in snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new(database))
        .map_err(|e| ConnError::Session(e.to_string()))?
    {
        check_catalog_cancel(cancel)?;
        let model = table
            .model_meta
            .as_ref()
            .ok_or(ConnError::UnsupportedCommand(0))?;
        if model.State != astersql_meta_model::StatePublic {
            continue;
        }
        let oid = i64::from(crate::pg_oid::table_oid(model.ID)?);
        if name == "pg_sequence" {
            if let Some(sequence) = &model.Sequence {
                rows.push(vec![
                    Value::Signed(oid),
                    Value::Signed(20),
                    Value::Signed(sequence.Start),
                    Value::Signed(sequence.Increment),
                    Value::Signed(sequence.MinValue),
                    Value::Signed(sequence.MaxValue),
                    Value::Signed(if sequence.Cache {
                        sequence.CacheValue
                    } else {
                        1
                    }),
                    Value::Text(sequence.Cycle.to_string()),
                ]);
            }
        } else {
            let mut comment = |object: i64, subid: i64, text: &str| {
                if !text.is_empty() {
                    let mut row = vec![Value::Null; 10];
                    row[2] = Value::Text(text.into());
                    row[6] = Value::Signed(object);
                    row[7] = Value::Signed(1259);
                    row[9] = Value::Signed(subid);
                    rows.push(row);
                }
            };
            comment(
                oid,
                0,
                model
                    .Sequence
                    .as_ref()
                    .map_or(model.Comment.as_str(), |s| s.Comment.as_str()),
            );
            for column in &model.Columns {
                if column.State == astersql_meta_model::StatePublic && !column.Hidden {
                    comment(
                        oid,
                        i64::from(
                            model
                                .postgresql_attribute_number(column.ID)
                                .map_err(|_| ConnError::UnsupportedCommand(0))?,
                        ),
                        &column.Comment,
                    );
                }
            }
            for index in &model.Indices {
                if index.State == astersql_meta_model::StatePublic {
                    comment(
                        i64::from(crate::pg_oid::index_oid(model.ID, index.ID)?),
                        0,
                        &index.Comment,
                    );
                }
            }
        }
        if rows.len() > MAX_CATALOG_ROWS {
            return Err(catalog_row_limit());
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
        if model.State == astersql_meta_model::StatePublic {
            for column in &model.Columns {
                if let Some(identity) = &column.PostgreSQLIdentity {
                    rows.push(vec![
                        Value::Signed(classid),
                        Value::Signed(i64::from(crate::pg_oid::table_oid(identity.SequenceID)?)),
                        Value::Signed(classid),
                        Value::Signed(i64::from(crate::pg_oid::table_oid(model.ID)?)),
                        Value::Signed(i64::from(
                            model
                                .postgresql_attribute_number(column.ID)
                                .map_err(|_| ConnError::UnsupportedCommand(0))?,
                        )),
                        Value::Text("i".into()),
                        Value::Signed(0),
                    ]);
                    if rows.len() > MAX_CATALOG_ROWS {
                        return Err(catalog_row_limit());
                    }
                }
            }
        }
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
            Value::Signed(0),
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
                Value::Text("{function_oid}".into()), // argument names
                Value::Null,                          // all arguments are IN
                Value::Text("26".into()),             // one oid input
                Value::Null,                          // no OUT arguments
                Value::Null,                          // no default arguments
                Value::Signed(0),                     // not variadic
                Value::Signed(25),                    // text return type
                Value::Text("false".into()),          // scalar
                Value::Text("s".into()),              // snapshot-stable introspection
                Value::Text("false".into()),          // handles unknown/NULL OIDs
                Value::Text("false".into()),          // invoker security
                Value::Null,                          // no per-function configuration
                Value::Null,                          // no native PostgreSQL cost model
                Value::Null,                          // no PostgreSQL owner
                Value::Signed(0),                     // not set-returning
                Value::Text("false".into()),          // not leakproof
                Value::Text("u".into()),              // no parallel execution contract
                Value::Null,                          // no PostgreSQL transaction xmin
            ]
        })
        .collect()
}

// Optional arrays retain their PG element type even without native FDW options.
fn column_catalog_field(relation: &str, name: &str) -> Option<(usize, u8, usize)> {
    let oid = crate::pg_oid::OID_TYPE;
    let internal_char = crate::pg_result::CatalogColumnType::InternalChar as u8;
    let (slot, code) = match (relation, name) {
        ("pg_foreign_table", "ftrelid") => (0, oid),
        ("pg_foreign_table", "ftserver") => (1, oid),
        ("pg_foreign_table", "ftoptions") => {
            (2, crate::pg_result::CatalogColumnType::TextArray as u8)
        }
        ("pg_foreign_server", "oid") => (0, oid),
        ("pg_foreign_server", "srvname") => (1, 253),
        ("pg_collation", "oid") => (0, oid),
        ("pg_collation", "xmin") => (1, 8),
        ("pg_collation", "collname") => (2, 253),
        ("pg_collation", "collcollate") => (3, 253),
        ("pg_collation", "collctype") => (4, 253),
        ("pg_collation", "collowner") => (5, 8),
        ("pg_collation", "collnamespace") => (6, oid),
        ("pg_opfamily", "oid") => (0, oid),
        ("pg_opfamily", "xmin") => (1, 8),
        ("pg_opfamily", "opfname") => (2, 253),
        ("pg_opfamily", "opfmethod") => (3, oid),
        ("pg_opfamily", "opfowner") => (4, 8),
        ("pg_opfamily", "opfnamespace") => (5, oid),
        ("pg_amop", "oid") => (0, oid),
        ("pg_amop", "amopstrategy") => (1, 2),
        ("pg_amop", "amopopr") => (2, oid),
        ("pg_amop", "amopsortfamily") => (3, oid),
        ("pg_amop", "amopfamily") => (4, oid),
        ("pg_amproc", "oid") => (0, oid),
        ("pg_amproc", "amprocnum") => (1, 2),
        ("pg_amproc", "amproc") => (2, oid),
        ("pg_amproc", "amproclefttype") => (3, oid),
        ("pg_amproc", "amprocrighttype") => (4, oid),
        ("pg_amproc", "amprocfamily") => (5, oid),
        ("pg_policy", "oid") => (0, oid),
        ("pg_policy", "xmin") => (1, 8),
        ("pg_policy", "polname") => (2, 253),
        ("pg_policy", "polrelid") => (3, oid),
        ("pg_policy", "polpermissive") => (4, 1),
        ("pg_policy", "polroles") => (5, crate::pg_result::CatalogColumnType::OidArray as u8),
        ("pg_policy", "polcmd") => (6, internal_char),
        ("pg_policy", "polqual") => (7, 253),
        ("pg_policy", "polwithcheck") => (8, 253),
        ("pg_trigger", "oid") => (0, oid),
        ("pg_trigger", "tgrelid") => (1, oid),
        ("pg_rewrite", "oid") => (0, oid),
        ("pg_rewrite", "ev_class") => (1, oid),
        ("pg_opclass", "xmin") => (2, 8),
        ("pg_opclass", "opcname") => (3, crate::pg_result::CatalogColumnType::Name as u8),
        ("pg_opclass", "opcintype") => (4, oid),
        ("pg_opclass", "opckeytype") => (5, oid),
        ("pg_opclass", "opcdefault") => (6, 1),
        ("pg_opclass", "opcfamily") => (7, oid),
        ("pg_opclass", "opcowner") => (8, 8),
        ("pg_opclass", "opcnamespace") => (9, oid),
        ("pg_operator", "oid") => (4, oid),
        ("pg_proc", "proacl") => (24, crate::pg_result::CatalogColumnType::AclArray as u8),
        ("pg_depend", "objsubid") => (6, 3),
        ("pg_operator", "xmin") => (5, 8),
        ("pg_operator", "oprname") => (6, crate::pg_result::CatalogColumnType::Name as u8),
        ("pg_operator", "oprkind") => (7, internal_char),
        ("pg_operator", "oprcode") => (8, oid),
        ("pg_operator", "oprrest") => (9, oid),
        ("pg_operator", "oprjoin") => (10, oid),
        ("pg_operator", "oprcom") => (11, oid),
        ("pg_operator", "oprnegate") => (12, oid),
        ("pg_operator", "oprcanmerge") => (13, 1),
        ("pg_operator", "oprcanhash") => (14, 1),
        ("pg_operator", "oprowner") => (15, 8),
        ("pg_aggregate", "aggtransfn") => (3, oid),
        ("pg_aggregate", "aggfinalfn") => (4, oid),
        ("pg_aggregate", "agginitval") => (5, 253),
        ("pg_aggregate", "aggsortop") => (6, oid),
        ("pg_aggregate", "aggfinalextra") => (7, 1),
        ("pg_aggregate", "aggtransspace") => (8, 3),
        ("pg_aggregate", "aggmtransfn") => (9, oid),
        ("pg_aggregate", "aggminvtransfn") => (10, oid),
        ("pg_aggregate", "aggmtransspace") => (11, 3),
        ("pg_aggregate", "aggmfinalfn") => (12, oid),
        ("pg_aggregate", "aggmfinalextra") => (13, 1),
        ("pg_aggregate", "aggminitval") => (14, 253),
        ("pg_aggregate", "aggkind") => (15, internal_char),
        ("pg_aggregate", "aggnumdirectargs") => (16, 2),
        ("pg_aggregate", "aggcombinefn") => (17, oid),
        ("pg_aggregate", "aggserialfn") => (18, oid),
        ("pg_aggregate", "aggdeserialfn") => (19, oid),
        ("pg_sequence", "seqrelid") => (0, oid),
        ("pg_sequence", "seqtypid") => (1, oid),
        ("pg_sequence", "seqstart") => (2, 8),
        ("pg_sequence", "seqincrement") => (3, 8),
        ("pg_sequence", "seqmin") => (4, 8),
        ("pg_sequence", "seqmax") => (5, 8),
        ("pg_sequence", "seqcache") => (6, 8),
        ("pg_sequence", "seqcycle") => (7, 1),
        ("pg_opclass", "oid") => (0, oid),
        ("pg_opclass", "opcmethod") => (1, oid),
        ("pg_inherits", "inhrelid") => (0, oid),
        ("pg_inherits", "inhparent") => (1, oid),
        ("pg_inherits", "inhseqno") => (2, 3),
        // No PG transaction/storage/partition/ACL facts back these fields yet.
        // Refuse the requested field at bind time, including LIMIT 0 and predicates.
        (
            "pg_class",
            "xmin" | "reltablespace" | "reloptions" | "relpersistence" | "relispartition"
            | "relpartbound" | "relam" | "relacl",
        ) => return None,
        ("pg_class", "relowner") => (11, oid),
        ("pg_depend", "classid") => (0, oid),
        ("pg_depend", "objid") => (1, oid),
        ("pg_depend", "refclassid") => (2, oid),
        ("pg_depend", "refobjid") => (3, oid),
        ("pg_depend", "refobjsubid") => (4, 3),
        ("pg_depend", "deptype") => (5, internal_char),
        ("pg_operator", "oprleft") => (0, oid),
        ("pg_operator", "oprright") => (1, oid),
        ("pg_operator", "oprresult") => (2, oid),
        ("pg_operator", "oprnamespace") => (3, oid),
        ("pg_aggregate", "aggfnoid") => (0, oid),
        ("pg_aggregate", "aggtranstype") => (1, oid),
        ("pg_aggregate", "aggmtranstype") => (2, oid),
        ("pg_proc", "proargnames") => (6, crate::pg_result::CatalogColumnType::TextArray as u8),
        ("pg_proc", "proargmodes") => (7, crate::pg_result::CatalogColumnType::TextArray as u8),
        ("pg_proc", "proargtypes") => (8, crate::pg_result::CatalogColumnType::OidVector as u8),
        ("pg_proc", "proallargtypes") => (9, crate::pg_result::CatalogColumnType::OidArray as u8),
        ("pg_proc", "proargdefaults") => (10, 253),
        ("pg_proc", "provariadic") => (11, oid),
        ("pg_proc", "prorettype") => (12, oid),
        ("pg_proc", "proretset") => (13, 1),
        ("pg_proc", "provolatile") => (14, internal_char),
        ("pg_proc", "proisstrict") => (15, 1),
        ("pg_proc", "prosecdef") => (16, 1),
        ("pg_proc", "proconfig") => (17, crate::pg_result::CatalogColumnType::TextArray as u8),
        ("pg_proc", "procost") => (18, 8),
        ("pg_proc", "proowner") => (19, 8),
        ("pg_proc", "prorows") => (20, 8),
        ("pg_proc", "proleakproof") => (21, 1),
        ("pg_proc", "proparallel") => (22, internal_char),
        ("pg_proc", "xmin") => (23, 8),
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
        ("pg_index", "indcollation") => (10, crate::pg_result::CatalogColumnType::OidVector as u8),
        ("pg_index", "indexprs") => (11, crate::pg_result::CatalogColumnType::PgNodeTree as u8),
        ("pg_index", "indpred") => (12, crate::pg_result::CatalogColumnType::PgNodeTree as u8),
        ("pg_index", "indisvalid") => (13, 1),
        ("pg_index", "indisready") => (14, 1),
        ("pg_index", "indisexclusion") => (15, 1),
        ("pg_index", "indimmediate") => (16, 1),
        ("pg_index", "indisclustered") => (17, 1),
        ("pg_index", "indcheckxmin") => (18, 1),
        ("pg_index", "indislive") => (19, 1),
        ("pg_index", "indisreplident") => (20, 1),
        ("pg_constraint", "oid") => (0, oid),
        ("pg_constraint", "conname") => (1, crate::pg_result::CatalogColumnType::Name as u8),
        ("pg_constraint", "contype") => (2, internal_char),
        ("pg_constraint", "conrelid") => (3, oid),
        ("pg_constraint", "connamespace") => (4, oid),
        ("pg_constraint", "conkey") => (5, crate::pg_result::CatalogColumnType::Int2Array as u8),
        ("pg_constraint", "conindid") => (6, oid),
        ("pg_constraint", "confrelid") => (7, oid),
        ("pg_constraint", "confkey") => (8, crate::pg_result::CatalogColumnType::Int2Array as u8),
        ("pg_constraint", "confupdtype") => (9, internal_char),
        ("pg_constraint", "confdeltype") => (10, internal_char),
        ("pg_constraint", "confmatchtype") => (22, internal_char),
        ("pg_constraint", "convalidated") => (23, 1),
        ("pg_constraint", "conenforced") => (24, 1),
        ("pg_constraint", "conislocal") => (25, 1),
        ("pg_constraint", "coninhcount") => (26, 2),
        ("pg_constraint", "conparentid") => (27, oid),
        ("pg_constraint", "contypid") => (28, oid),
        ("pg_constraint", "conperiod") => (29, 1),
        ("pg_constraint", "confdelsetcols") => {
            (30, crate::pg_result::CatalogColumnType::Int2Array as u8)
        }
        ("pg_constraint", "conpfeqop") => (31, crate::pg_result::CatalogColumnType::OidArray as u8),
        ("pg_constraint", "conppeqop") => (32, crate::pg_result::CatalogColumnType::OidArray as u8),
        ("pg_constraint", "conffeqop") => (33, crate::pg_result::CatalogColumnType::OidArray as u8),
        ("pg_constraint", "condeferrable") => (11, 1),
        ("pg_constraint", "condeferred") => (12, 1),
        ("pg_constraint", "connoinherit") => (13, 1),
        ("pg_constraint", "conbin") => (14, crate::pg_result::CatalogColumnType::PgNodeTree as u8),
        ("pg_constraint", "conexclop") => (15, crate::pg_result::CatalogColumnType::OidArray as u8),
        ("pg_constraint", "xmin") => (16, 8),
        ("pg_attribute", "attrelid") => (0, oid),
        ("pg_attribute", "attnum") => (1, 2),
        ("pg_attribute", "attname") => (2, crate::pg_result::CatalogColumnType::Name as u8),
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
        ("pg_attrdef", "oid") => (3, oid),
        ("pg_attrdef", "adrelid") => (0, oid),
        ("pg_attrdef", "adnum") => (1, 2),
        ("pg_attrdef", "adbin") => (2, crate::pg_result::CatalogColumnType::PgNodeTree as u8),
        ("pg_type", "oid") => (0, oid),
        ("pg_type", "typname") => (1, crate::pg_result::CatalogColumnType::Name as u8),
        ("pg_type", "typnamespace") => (2, oid),
        ("pg_type", "typtype") => (3, internal_char),
        ("pg_type", "typcategory") => (4, internal_char),
        ("pg_type", "typrelid") => (5, oid),
        ("pg_type", "typbasetype") => (6, oid),
        ("pg_type", "typtypmod") => (7, 3),
        ("pg_type", "typndims") => (8, 3),
        ("pg_type", "typdefault") => (9, 253),
        ("pg_type", "typnotnull") => (10, 1),
        // No durable builtin owner/default-ACL facts exist in the role model.
        ("pg_type", "typowner" | "typacl") => return None,
        ("pg_type", "typarray") => (16, oid),
        ("pg_type", "typdelim") => (17, internal_char),
        ("pg_type", "typlen") => (18, 2),
        ("pg_type", "typbyval") => (19, 1),
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

// PostgreSQL 18.6 standard 64-bit build registry, not the engine's storage layout.
// Include every declaration/result identity and its explicit array partner.
// Vectors and name retain their actual category/typelem; record is a pseudo type.
struct BuiltinType {
    oid: i64,
    name: &'static str,
    display: &'static str,
    category: &'static str,
    kind: &'static str,
    element: i64,
    array: i64,
    length: i64,
    by_value: bool,
}
const COLUMN_TYPES: &[BuiltinType] = &[
    BuiltinType {
        oid: 194,
        name: "pg_node_tree",
        display: "pg_node_tree",
        category: "S",
        kind: "b",
        element: 0,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 16,
        name: "bool",
        display: "boolean",
        category: "B",
        kind: "b",
        element: 0,
        array: 1000,
        length: 1,
        by_value: true,
    },
    BuiltinType {
        oid: 17,
        name: "bytea",
        display: "bytea",
        category: "U",
        kind: "b",
        element: 0,
        array: 1001,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 18,
        name: "char",
        display: "\"char\"",
        category: "Z",
        kind: "b",
        element: 0,
        array: 1002,
        length: 1,
        by_value: true,
    },
    BuiltinType {
        oid: 19,
        name: "name",
        display: "name",
        category: "S",
        kind: "b",
        element: 18,
        array: 1003,
        length: 64,
        by_value: false,
    },
    BuiltinType {
        oid: 20,
        name: "int8",
        display: "bigint",
        category: "N",
        kind: "b",
        element: 0,
        array: 1016,
        length: 8,
        by_value: true,
    },
    BuiltinType {
        oid: 21,
        name: "int2",
        display: "smallint",
        category: "N",
        kind: "b",
        element: 0,
        array: 1005,
        length: 2,
        by_value: true,
    },
    BuiltinType {
        oid: 22,
        name: "int2vector",
        display: "int2vector",
        category: "A",
        kind: "b",
        element: 21,
        array: 1006,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 23,
        name: "int4",
        display: "integer",
        category: "N",
        kind: "b",
        element: 0,
        array: 1007,
        length: 4,
        by_value: true,
    },
    BuiltinType {
        oid: 25,
        name: "text",
        display: "text",
        category: "S",
        kind: "b",
        element: 0,
        array: 1009,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 26,
        name: "oid",
        display: "oid",
        category: "N",
        kind: "b",
        element: 0,
        array: 1028,
        length: 4,
        by_value: true,
    },
    BuiltinType {
        oid: 30,
        name: "oidvector",
        display: "oidvector",
        category: "A",
        kind: "b",
        element: 26,
        array: 1013,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 114,
        name: "json",
        display: "json",
        category: "U",
        kind: "b",
        element: 0,
        array: 199,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 199,
        name: "_json",
        display: "json[]",
        category: "A",
        kind: "b",
        element: 114,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 700,
        name: "float4",
        display: "real",
        category: "N",
        kind: "b",
        element: 0,
        array: 1021,
        length: 4,
        by_value: true,
    },
    BuiltinType {
        oid: 701,
        name: "float8",
        display: "double precision",
        category: "N",
        kind: "b",
        element: 0,
        array: 1022,
        length: 8,
        by_value: true,
    },
    BuiltinType {
        oid: 1000,
        name: "_bool",
        display: "boolean[]",
        category: "A",
        kind: "b",
        element: 16,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1001,
        name: "_bytea",
        display: "bytea[]",
        category: "A",
        kind: "b",
        element: 17,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1002,
        name: "_char",
        display: "\"char\"[]",
        category: "A",
        kind: "b",
        element: 18,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1003,
        name: "_name",
        display: "name[]",
        category: "A",
        kind: "b",
        element: 19,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1005,
        name: "_int2",
        display: "smallint[]",
        category: "A",
        kind: "b",
        element: 21,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1006,
        name: "_int2vector",
        display: "int2vector[]",
        category: "A",
        kind: "b",
        element: 22,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1007,
        name: "_int4",
        display: "integer[]",
        category: "A",
        kind: "b",
        element: 23,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1009,
        name: "_text",
        display: "text[]",
        category: "A",
        kind: "b",
        element: 25,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1013,
        name: "_oidvector",
        display: "oidvector[]",
        category: "A",
        kind: "b",
        element: 30,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1014,
        name: "_bpchar",
        display: "character[]",
        category: "A",
        kind: "b",
        element: 1042,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1015,
        name: "_varchar",
        display: "character varying[]",
        category: "A",
        kind: "b",
        element: 1043,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1016,
        name: "_int8",
        display: "bigint[]",
        category: "A",
        kind: "b",
        element: 20,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1021,
        name: "_float4",
        display: "real[]",
        category: "A",
        kind: "b",
        element: 700,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1022,
        name: "_float8",
        display: "double precision[]",
        category: "A",
        kind: "b",
        element: 701,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1028,
        name: "_oid",
        display: "oid[]",
        category: "A",
        kind: "b",
        element: 26,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1033,
        name: "aclitem",
        display: "aclitem",
        category: "U",
        kind: "b",
        element: 0,
        array: 1034,
        length: 16,
        by_value: false,
    },
    BuiltinType {
        oid: 1034,
        name: "_aclitem",
        display: "aclitem[]",
        category: "A",
        kind: "b",
        element: 1033,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1042,
        name: "bpchar",
        display: "character",
        category: "S",
        kind: "b",
        element: 0,
        array: 1014,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1043,
        name: "varchar",
        display: "character varying",
        category: "S",
        kind: "b",
        element: 0,
        array: 1015,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1082,
        name: "date",
        display: "date",
        category: "D",
        kind: "b",
        element: 0,
        array: 1182,
        length: 4,
        by_value: true,
    },
    BuiltinType {
        oid: 1083,
        name: "time",
        display: "time without time zone",
        category: "D",
        kind: "b",
        element: 0,
        array: 1183,
        length: 8,
        by_value: true,
    },
    BuiltinType {
        oid: 1114,
        name: "timestamp",
        display: "timestamp without time zone",
        category: "D",
        kind: "b",
        element: 0,
        array: 1115,
        length: 8,
        by_value: true,
    },
    BuiltinType {
        oid: 1115,
        name: "_timestamp",
        display: "timestamp without time zone[]",
        category: "A",
        kind: "b",
        element: 1114,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1182,
        name: "_date",
        display: "date[]",
        category: "A",
        kind: "b",
        element: 1082,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1183,
        name: "_time",
        display: "time without time zone[]",
        category: "A",
        kind: "b",
        element: 1083,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1184,
        name: "timestamptz",
        display: "timestamp with time zone",
        category: "D",
        kind: "b",
        element: 0,
        array: 1185,
        length: 8,
        by_value: true,
    },
    BuiltinType {
        oid: 1185,
        name: "_timestamptz",
        display: "timestamp with time zone[]",
        category: "A",
        kind: "b",
        element: 1184,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1186,
        name: "interval",
        display: "interval",
        category: "T",
        kind: "b",
        element: 0,
        array: 1187,
        length: 16,
        by_value: false,
    },
    BuiltinType {
        oid: 1187,
        name: "_interval",
        display: "interval[]",
        category: "A",
        kind: "b",
        element: 1186,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1231,
        name: "_numeric",
        display: "numeric[]",
        category: "A",
        kind: "b",
        element: 1700,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 1700,
        name: "numeric",
        display: "numeric",
        category: "N",
        kind: "b",
        element: 0,
        array: 1231,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 2205,
        name: "regclass",
        display: "regclass",
        category: "N",
        kind: "b",
        element: 0,
        array: 2210,
        length: 4,
        by_value: true,
    },
    BuiltinType {
        oid: 2206,
        name: "regtype",
        display: "regtype",
        category: "N",
        kind: "b",
        element: 0,
        array: 2211,
        length: 4,
        by_value: true,
    },
    BuiltinType {
        oid: 2210,
        name: "_regclass",
        display: "regclass[]",
        category: "A",
        kind: "b",
        element: 2205,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 2211,
        name: "_regtype",
        display: "regtype[]",
        category: "A",
        kind: "b",
        element: 2206,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 2249,
        name: "record",
        display: "record",
        category: "P",
        kind: "p",
        element: 0,
        array: 2287,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 2287,
        name: "_record",
        display: "record[]",
        category: "P",
        kind: "p",
        element: 2249,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 2950,
        name: "uuid",
        display: "uuid",
        category: "U",
        kind: "b",
        element: 0,
        array: 2951,
        length: 16,
        by_value: false,
    },
    BuiltinType {
        oid: 2951,
        name: "_uuid",
        display: "uuid[]",
        category: "A",
        kind: "b",
        element: 2950,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 3802,
        name: "jsonb",
        display: "jsonb",
        category: "U",
        kind: "b",
        element: 0,
        array: 3807,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 3807,
        name: "_jsonb",
        display: "jsonb[]",
        category: "A",
        kind: "b",
        element: 3802,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 3904,
        name: "int4range",
        display: "int4range",
        category: "R",
        kind: "r",
        element: 0,
        array: 3905,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 3905,
        name: "_int4range",
        display: "int4range[]",
        category: "A",
        kind: "b",
        element: 3904,
        array: 0,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 3906,
        name: "numrange",
        display: "numrange",
        category: "R",
        kind: "r",
        element: 0,
        array: 3907,
        length: -1,
        by_value: false,
    },
    BuiltinType {
        oid: 3907,
        name: "_numrange",
        display: "numrange[]",
        category: "A",
        kind: "b",
        element: 3906,
        array: 0,
        length: -1,
        by_value: false,
    },
];

fn native_column_type(column: &astersql_meta_model::ColumnInfo) -> ConnResult<(i64, i64)> {
    use astersql_parser_mysql::r#type as mysql;
    let tp = &column.FieldType;
    if tp.IsArray() && tp.PostgreSQLArrayElement().is_none() {
        return Err(ConnError::UnsupportedCommand(0));
    }
    if let Some(element) = tp.PostgreSQLArrayElement() {
        let mut scalar = column.clone();
        // Array storage is textual; the declaration carries its actual element identity.
        let mut field = astersql_types::field::ast::NewFieldType(match element.oid() {
            21 => mysql::TypeShort,
            23 => mysql::TypeLong,
            20 => mysql::TypeLonglong,
            1700 => mysql::TypeNewDecimal,
            1083 => mysql::TypeDuration,
            1114 => mysql::TypeDatetime,
            1184 => mysql::TypePgTimestampTz,
            1186 => mysql::TypePgInterval,
            _ => mysql::TypeVarString,
        });
        field.SetFlen(tp.GetFlen());
        field.SetDecimal(tp.GetDecimal());
        field.SetPostgreSQLDeclaration();
        if matches!(element.oid(), 1042 | 1043) {
            field.SetPostgreSQLTextOid(element.oid());
        }
        scalar.FieldType = field;
        let modifier = if matches!(
            element.oid(),
            1042 | 1043 | 1700 | 1083 | 1114 | 1184 | 1186
        ) {
            native_column_type(&scalar)?.1
        } else {
            -1
        };
        return Ok((i64::from(element.array_oid()), modifier));
    }
    if let Some(oid) = tp.PostgreSQLTextOid() {
        let modifier = if matches!(oid, 1042 | 1043) && tp.GetFlen() >= 0 {
            i64::try_from(tp.GetFlen())
                .ok()
                .and_then(|n| n.checked_add(4))
                .filter(|n| *n <= i64::from(i32::MAX))
                .ok_or(ConnError::UnsupportedCommand(0))?
        } else {
            -1
        };
        return Ok((i64::from(oid), modifier));
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
            if tp.IsPostgreSQLDeclaration() && precision == -1 && scale == -1 {
                return Ok((1700, -1));
            }
            if if tp.IsPostgreSQLDeclaration() {
                !(1..=1000).contains(&precision) || !(-1000..=1000).contains(&scale)
            } else {
                !(1..=65).contains(&precision) || !(0..=precision).contains(&scale)
            } {
                return Err(ConnError::UnsupportedCommand(0));
            }
            modifier = 4 + (precision << 16) + (scale & 0x7ff);
            1700
        }
        15 | 253 | 254 => {
            if column.GetCharset() == "binary" {
                17
            } else {
                let length = i64::try_from(column.GetFlen())
                    .map_err(|_| ConnError::UnsupportedCommand(0))?;
                if length <= 0 || length > i64::from(i32::MAX) - 4 {
                    return Err(ConnError::UnsupportedCommand(0));
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
        11 => {
            modifier =
                i64::try_from(column.GetDecimal()).map_err(|_| ConnError::UnsupportedCommand(0))?;
            if !(-1..=6).contains(&modifier) || modifier == -1 && !tp.IsPostgreSQLDeclaration() {
                return Err(ConnError::UnsupportedCommand(0));
            }
            1083
        }
        7 | 12 => {
            modifier =
                i64::try_from(column.GetDecimal()).map_err(|_| ConnError::UnsupportedCommand(0))?;
            if !(-1..=6).contains(&modifier) || modifier == -1 && !tp.IsPostgreSQLDeclaration() {
                return Err(ConnError::UnsupportedCommand(0));
            }
            1114
        }
        mysql::TypePgTimestampTz => {
            modifier = column.GetDecimal() as i64;
            1184
        }
        mysql::TypePgInterval => {
            modifier = if column.GetFlen() < 0 && column.GetDecimal() < 0 {
                -1
            } else {
                ((if column.GetFlen() < 0 {
                    0x7fff
                } else {
                    column.GetFlen() as i64
                }) << 16)
                    | if column.GetDecimal() < 0 {
                        0xffff
                    } else {
                        column.GetDecimal() as i64
                    }
            };
            1186
        }
        mysql::TypePgUuid => 2950,
        mysql::TypePgJson => 114,
        mysql::TypePgJsonb => 3802,
        mysql::TypePgName => 19,
        mysql::TypePgRegtype => 2206,
        mysql::TypePgInt4Range => 3904,
        mysql::TypePgNumRange => 3906,
        245 => 114,
        // Enums and sets need identities/labels not provided by this compatibility phase.
        _ => return Err(ConnError::UnsupportedCommand(0)),
    };
    if column.GetType() == 8 && unsigned {
        modifier = 4 + (20 << 16);
    }
    Ok((oid, modifier))
}

// Known PostgreSQL 18.6 builtin identities outside COLUMN_TYPES. Source:
// src/include/catalog/pg_type.dat (including array_type_oid); no type rows are
// synthesized from this list. It only distinguishes unsupported from unknown.
const PG18_UNMAPPED_TYPE_OIDS: &[i64] = &[
    24, 27, 28, 29, 32, 71, 75, 81, 83, 142, 143, 210, 269, 270, 271, 272, 273, 325, 600, 601, 602,
    603, 604, 628, 629, 650, 651, 705, 718, 719, 774, 775, 790, 791, 829, 869, 1008, 1010, 1011,
    1012, 1017, 1018, 1019, 1020, 1027, 1040, 1041, 1263, 1266, 1270, 1560, 1561, 1562, 1563, 1790,
    2201, 2202, 2203, 2204, 2207, 2208, 2209, 2275, 2276, 2277, 2278, 2279, 2280, 2281, 2283, 2776,
    2949, 2970, 3115, 3220, 3221, 3310, 3361, 3402, 3500, 3614, 3615, 3642, 3643, 3644, 3645, 3734,
    3735, 3769, 3770, 3831, 3838, 3908, 3909, 3910, 3911, 3912, 3913, 3926, 3927, 4072, 4073, 4089,
    4090, 4096, 4097, 4191, 4192, 4451, 4532, 4533, 4534, 4535, 4536, 4537, 4538, 4600, 4601, 5017,
    5038, 5039, 5069, 5077, 5078, 5079, 5080, 6150, 6151, 6152, 6153, 6155, 6157,
];

fn format_column_type(oid: i64, modifier: Option<i64>) -> ConnResult<String> {
    u32::try_from(oid).map_err(|_| ConnError::Session("PG oid conversion out of range".into()))?;
    if let Some(modifier) = modifier {
        i32::try_from(modifier)
            .map_err(|_| ConnError::Session("PG oid conversion out of range".into()))?;
    }
    if oid == 0 {
        return Ok("-".into());
    }
    let Some(builtin) = COLUMN_TYPES.iter().find(|t| t.oid == oid) else {
        // PostgreSQL 18.6 pg_type.dat identities include generated array OIDs.
        // A known but unregistered builtin must not masquerade as nonexistent.
        if PG18_UNMAPPED_TYPE_OIDS.contains(&oid) {
            return Err(ConnError::UnsupportedCommand(0));
        }
        return Ok("???".into());
    };
    if builtin.category == "A" && builtin.name.starts_with('_') {
        return Ok(format!(
            "{}[]",
            format_column_type(builtin.element, modifier)?
        ));
    }
    let name = builtin.display;
    let Some(modifier) = modifier else {
        return Ok(name.into());
    };
    if modifier < 0 {
        return Ok(if oid == 1042 {
            "bpchar".into()
        } else {
            name.into()
        });
    }
    Ok(match oid {
        1186 => {
            // intervaltypmodout in PostgreSQL 18.6 timestamp.c: the upper
            // bits select fields, the lower bits carry optional precision.
            let fields = match (modifier >> 16) & 0x7fff {
                4 => " year",
                2 => " month",
                8 => " day",
                1024 => " hour",
                2048 => " minute",
                4096 => " second",
                6 => " year to month",
                1032 => " day to hour",
                3080 => " day to minute",
                7176 => " day to second",
                3072 => " hour to minute",
                7168 => " hour to second",
                6144 => " minute to second",
                32767 => "",
                _ => return Err(ConnError::UnsupportedCommand(0)),
            };
            let precision = modifier & 0xffff;
            if precision == 0xffff {
                format!("interval{fields}")
            } else {
                format!("interval{fields}({precision})")
            }
        }
        1042 | 1043 if modifier >= 4 => format!("{name}({})", modifier - 4),
        1700 if modifier >= 4 => {
            let n = modifier - 4;
            let precision = (n >> 16) & 0xffff;
            // PG encodes a signed 11-bit scale (including negative scales).
            let scale = ((n & 0x7ff) ^ 1024) - 1024;
            format!("numeric({precision},{scale})")
        }
        1083 if (0..=6).contains(&modifier) => format!("time({modifier}) without time zone"),
        1114 if (0..=6).contains(&modifier) => format!("timestamp({modifier}) without time zone"),
        1184 if (0..=6).contains(&modifier) => format!("timestamp({modifier}) with time zone"),
        1042 | 1043 | 1083 | 1700 | 1114 | 1184 => return Err(ConnError::UnsupportedCommand(0)),
        _ => name.into(),
    })
}

fn column_default(
    column: &astersql_meta_model::ColumnInfo,
    oid: i64,
) -> ConnResult<Option<String>> {
    use astersql_meta_model::DefaultValue;
    if !column.GeneratedExprString.is_empty() {
        // Native expression text is never exposed or parsed as PostgreSQL SQL.
        return Err(ConnError::UnsupportedCommand(0));
    }
    let Some(value) = column.GetDefaultValue() else {
        return Ok(None);
    };
    let text = match value {
        // Native TRUE/FALSE defaults are numeric constants on integer columns;
        // their value does not establish the column's PostgreSQL identity.
        DefaultValue::Bool(v) if matches!(oid, 20 | 21 | 23) => i32::from(v).to_string(),
        DefaultValue::Bool(v) => v.to_string(),
        DefaultValue::Int(v) => v.to_string(),
        DefaultValue::Uint(v) => v.to_string(),
        DefaultValue::Float(v) if v.is_finite() => v.to_string(),
        DefaultValue::Float(_) => return Err(ConnError::UnsupportedCommand(0)),
        DefaultValue::String(v) => {
            String::from_utf8(v).map_err(|_| ConnError::UnsupportedCommand(0))?
        }
    };
    if column.DefaultIsExpr
        || matches!(oid, 1114 | 1184) && text.to_uppercase().starts_with("CURRENT_TIMESTAMP")
    {
        let mut upper = text.to_uppercase();
        if upper == "CURRENT_TIMESTAMP()" {
            upper = "CURRENT_TIMESTAMP".into();
        }
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

// PostgreSQL nodeToString is an architecture-dependent internal format. This
// bounded codec uses 64-bit little-endian Datum/varlena layout (the supported
// server targets), and only nodes whose semantics we can reconstruct. No SQL
// suffix or function body from a client is ever returned as a definition.
fn default_const_tree(oid: i64, bytes: &[u8], len: i16, byval: bool) -> String {
    let value = bytes
        .iter()
        .map(|b| (*b as i8).to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let size = if byval { len as usize } else { bytes.len() };
    let collid = if matches!(oid, 25 | 1042 | 1043) {
        100
    } else {
        0
    };
    format!(
        "{{CONST :consttype {oid} :consttypmod -1 :constcollid {collid} :constlen {len} :constbyval {byval} :constisnull false :location -1 :constvalue {size} [ {value} ]}}"
    )
}

fn default_varlena(data: &[u8]) -> ConnResult<Vec<u8>> {
    let size = u32::try_from(data.len() + 4).map_err(|_| ConnError::UnsupportedCommand(0))?;
    if size > 0x3fff_ffff {
        return Err(ConnError::UnsupportedCommand(0));
    }
    let mut bytes = (size << 2).to_le_bytes().to_vec();
    bytes.extend_from_slice(data);
    Ok(bytes)
}

fn default_node_tree(
    column: &astersql_meta_model::ColumnInfo,
    oid: i64,
    sql: &str,
) -> ConnResult<String> {
    let unsupported = || ConnError::UnsupportedCommand(0);
    if !cfg!(target_endian = "little") || std::mem::size_of::<usize>() != 8 {
        return Err(unsupported());
    }
    if sql.starts_with("CURRENT_TIMESTAMP") {
        let precision = if sql == "CURRENT_TIMESTAMP" {
            -1
        } else {
            sql.strip_prefix("CURRENT_TIMESTAMP(")
                .and_then(|s| s.strip_suffix(')'))
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|n| (0..=6).contains(n))
                .ok_or_else(unsupported)?
        };
        if !matches!(oid, 1114 | 1184) {
            return Err(unsupported());
        }
        let tree = format!(
            "{{SQLVALUEFUNCTION :op {} :type 1184 :typmod {precision} :location -1}}",
            if precision == -1 { 3 } else { 4 }
        );
        return Ok(if oid == 1114 {
            format!(
                "{{FUNCEXPR :funcid 2027 :funcresulttype 1114 :funcretset false :funcvariadic false :funcformat 2 :funccollid 0 :inputcollid 0 :args ({tree}) :location -1}}"
            )
        } else {
            tree
        });
    }
    let (mut bytes, len, byval) = match oid {
        16 => (vec![u8::from(sql == "true")], 1, true),
        21 => (
            sql.parse::<i16>()
                .map_err(|_| unsupported())?
                .to_le_bytes()
                .to_vec(),
            2,
            true,
        ),
        23 => (
            sql.parse::<i32>()
                .map_err(|_| unsupported())?
                .to_le_bytes()
                .to_vec(),
            4,
            true,
        ),
        20 => (
            sql.parse::<i64>()
                .map_err(|_| unsupported())?
                .to_le_bytes()
                .to_vec(),
            8,
            true,
        ),
        700 => (
            sql.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(unsupported)?
                .to_le_bytes()
                .to_vec(),
            4,
            true,
        ),
        701 => (
            sql.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(unsupported)?
                .to_le_bytes()
                .to_vec(),
            8,
            true,
        ),
        1700 => {
            let wire = sql
                .parse::<astersql_types::pg_numeric::PgNumeric>()
                .map_err(|_| unsupported())?
                .to_binary();
            let word = |at| u16::from_be_bytes([wire[at], wire[at + 1]]);
            let mut data = (word(4) | word(6)).to_le_bytes().to_vec();
            data.extend_from_slice(&word(2).to_le_bytes());
            for pair in wire[8..].chunks_exact(2) {
                data.extend_from_slice(&u16::from_be_bytes([pair[0], pair[1]]).to_le_bytes());
            }
            (default_varlena(&data)?, -1, false)
        }
        25 | 1042 | 1043 => {
            let Some(astersql_meta_model::DefaultValue::String(value)) = column.GetDefaultValue()
            else {
                return Err(unsupported());
            };
            let text = std::str::from_utf8(&value).map_err(|_| unsupported())?;
            if text.contains('\0') {
                return Err(unsupported());
            }
            (default_varlena(text.as_bytes())?, -1, false)
        }
        1082 | 1083 | 1114 | 1184 => {
            let Some(astersql_meta_model::DefaultValue::String(value)) = column.GetDefaultValue()
            else {
                return Err(unsupported());
            };
            let text = std::str::from_utf8(&value).map_err(|_| unsupported())?;
            use astersql_types::pg_temporal::{PgDate, PgTime, PgTimestamp, PgTimestampTz};
            let bytes = match oid {
                1082 => i32::from_be_bytes(
                    text.parse::<PgDate>()
                        .map_err(|_| unsupported())?
                        .to_binary(),
                )
                .to_le_bytes()
                .to_vec(),
                1083 => i64::from_be_bytes(
                    text.parse::<PgTime>()
                        .map_err(|_| unsupported())?
                        .to_binary(),
                )
                .to_le_bytes()
                .to_vec(),
                1114 => i64::from_be_bytes(
                    text.parse::<PgTimestamp>()
                        .map_err(|_| unsupported())?
                        .to_binary(),
                )
                .to_le_bytes()
                .to_vec(),
                _ => i64::from_be_bytes(
                    text.parse::<PgTimestampTz>()
                        .map_err(|_| unsupported())?
                        .to_binary(),
                )
                .to_le_bytes()
                .to_vec(),
            };
            (bytes, if oid == 1082 { 4 } else { 8 }, true)
        }
        // These types need their own Datum conversion; never label a SQL
        // fragment or a native datetime's bytes as a PostgreSQL tree.
        _ => return Err(unsupported()),
    };
    if byval {
        bytes.resize(8, 0);
    }
    Ok(default_const_tree(oid, &bytes, len, byval))
}

// The admitted generated-column subset is int4-column + int4-constant.
// Encode actual PG18 nodes; attribute slots, not mutable offsets, identify Vars.
fn generated_node_tree(
    expression: &astersql_meta_model::PostgreSQLCheckExpression,
    table: &astersql_meta_model::TableInfo,
) -> ConnResult<String> {
    use astersql_meta_model::PostgreSQLCheckExpression as B;
    let unsupported = || ConnError::UnsupportedCommand(0);
    let B::Binary(op, left, right) = expression else {
        return Err(unsupported());
    };
    let (B::Column(id), B::Literal(value)) = (left.as_ref(), right.as_ref()) else {
        return Err(unsupported());
    };
    if op != "+" {
        return Err(unsupported());
    }
    let column = table
        .Columns
        .iter()
        .find(|c| {
            c.ID == *id
                && !c.Hidden
                && !c.IsGenerated()
                && c.State == astersql_meta_model::StatePublic
        })
        .ok_or_else(unsupported)?;
    if native_column_type(column)?.0 != 23 {
        return Err(unsupported());
    }
    let slots = table
        .postgresql_attribute_slots()
        .map_err(|_| unsupported())?;
    let attnum = slots
        .iter()
        .position(|(slot, dropped)| slot == id && !dropped)
        .ok_or_else(unsupported)?
        + 1;
    let number = value.parse::<i32>().map_err(|_| unsupported())?;
    let mut bytes = number.to_le_bytes().to_vec();
    bytes.resize(8, 0);
    let constant = default_const_tree(23, &bytes, 4, true);
    Ok(format!(
        "{{OPEXPR :opno 551 :opfuncid 177 :opresulttype 23 :opretset false :opcollid 0 :inputcollid 0 :args ({{VAR :varno 1 :varattno {attnum} :vartype 23 :vartypmod -1 :varcollid 0 :varnullingrels (b) :varlevelsup 0 :varreturningtype 0 :varnosyn 1 :varattnosyn {attnum} :location -1}} {constant}) :location -1}}"
    ))
}

fn deparse_generated_tree(
    tree: &str,
    relation: i64,
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
) -> ConnResult<String> {
    let unsupported = || ConnError::UnsupportedCommand(0);
    if relation <= 0 || tree.len() > 1_048_576 {
        return Err(unsupported());
    }
    let tables = snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new(database))
        .map_err(|_| unsupported())?;
    for table in tables {
        let Some(model) = table.model_meta.as_ref() else {
            continue;
        };
        if i64::from(crate::pg_oid::table_oid(model.ID)?) != relation {
            continue;
        }
        for column in &model.Columns {
            if let Some(bound) = &column.PostgreSQLGenerated {
                if generated_node_tree(bound, model)? == tree {
                    return bound
                        .render(model, false, &mut Vec::new())
                        .ok_or_else(unsupported);
                }
            }
        }
    }
    Err(unsupported())
}

fn deparse_default_tree(tree: &str) -> ConnResult<String> {
    let unsupported = || ConnError::UnsupportedCommand(0);
    if tree.len() > 1_048_576
        || !cfg!(target_endian = "little")
        || std::mem::size_of::<usize>() != 8
    {
        return Err(unsupported());
    }
    if let Some(inner) = tree.strip_prefix("{FUNCEXPR :funcid 2027 :funcresulttype 1114 :funcretset false :funcvariadic false :funcformat 2 :funccollid 0 :inputcollid 0 :args (").and_then(|s| s.strip_suffix(") :location -1}")) {
        if !inner.starts_with("{SQLVALUEFUNCTION ") { return Err(unsupported()); }
        return deparse_default_tree(inner);
    }
    if let Some(body) = tree
        .strip_prefix("{SQLVALUEFUNCTION :op ")
        .and_then(|s| s.strip_suffix(" :location -1}"))
    {
        let words: Vec<_> = body.split_whitespace().collect();
        if words.len() != 5 || words[1..4] != [":type", "1184", ":typmod"] {
            return Err(unsupported());
        }
        let precision = words[4].parse::<i32>().map_err(|_| unsupported())?;
        return match (words[0], precision) {
            ("3", -1) => Ok("CURRENT_TIMESTAMP".into()),
            ("4", 0..=6) => Ok(format!("CURRENT_TIMESTAMP({precision})")),
            _ => Err(unsupported()),
        };
    }
    let words: Vec<_> = tree.split_whitespace().collect();
    if words.len() < 22
        || words[0] != "{CONST"
        || words[1] != ":consttype"
        || words[3..6] != [":consttypmod", "-1", ":constcollid"]
        || words[7] != ":constlen"
        || words[9] != ":constbyval"
        || words[11..16] != [":constisnull", "false", ":location", "-1", ":constvalue"]
        || words[17] != "["
        || words[words.len() - 1] != "]}"
    {
        return Err(unsupported());
    }
    let oid = words[2].parse::<i64>().map_err(|_| unsupported())?;
    let len = words[8].parse::<i16>().map_err(|_| unsupported())?;
    let byval = match words[10] {
        "true" => true,
        "false" => false,
        _ => return Err(unsupported()),
    };
    let bytes = words[18..words.len() - 1]
        .iter()
        .map(|s| s.parse::<i8>().map(|b| b as u8).map_err(|_| unsupported()))
        .collect::<ConnResult<Vec<_>>>()?;
    // Canonical reconstruction also rejects unrecognized/trailing fields,
    // incorrect lengths, collation identities and malformed Datum padding.
    if default_const_tree(oid, &bytes, len, byval) != tree {
        return Err(unsupported());
    }
    let expected_len = match oid {
        16 => 1,
        21 => 2,
        23 | 700 | 1082 => 4,
        20 | 701 | 1083 | 1114 | 1184 => 8,
        25 | 1042 | 1043 | 1700 => -1,
        _ => return Err(unsupported()),
    };
    if len != expected_len
        || byval != (len > 0)
        || (byval && (bytes.len() != 8 || bytes[len as usize..].iter().any(|b| *b != 0)))
    {
        return Err(unsupported());
    }
    let text = if byval {
        match oid {
            16 => match bytes[0] {
                0 => "false".into(),
                1 => "true".into(),
                _ => return Err(unsupported()),
            },
            21 => i16::from_le_bytes(bytes[..2].try_into().unwrap()).to_string(),
            23 => i32::from_le_bytes(bytes[..4].try_into().unwrap()).to_string(),
            20 => i64::from_le_bytes(bytes[..8].try_into().unwrap()).to_string(),
            700 => {
                let n = f32::from_le_bytes(bytes[..4].try_into().unwrap());
                if !n.is_finite() {
                    return Err(unsupported());
                }
                n.to_string()
            }
            701 => {
                let n = f64::from_le_bytes(bytes[..8].try_into().unwrap());
                if !n.is_finite() {
                    return Err(unsupported());
                }
                n.to_string()
            }
            1082 | 1083 | 1114 | 1184 => {
                use astersql_types::pg_temporal::{PgDate, PgTime, PgTimestamp, PgTimestampTz};
                let data = if oid == 1082 {
                    i32::from_le_bytes(bytes[..4].try_into().unwrap())
                        .to_be_bytes()
                        .to_vec()
                } else {
                    i64::from_le_bytes(bytes[..8].try_into().unwrap())
                        .to_be_bytes()
                        .to_vec()
                };
                let value = match oid {
                    1082 => PgDate::from_binary(&data)
                        .map_err(|_| unsupported())?
                        .to_string(),
                    1083 => PgTime::from_binary(&data)
                        .map_err(|_| unsupported())?
                        .to_string(),
                    1114 => PgTimestamp::from_binary(&data)
                        .map_err(|_| unsupported())?
                        .to_string(),
                    _ => PgTimestampTz::from_binary(&data)
                        .map_err(|_| unsupported())?
                        .to_string(),
                };
                format!(
                    "'{}'::{}",
                    value.replace('\'', "''"),
                    format_column_type(oid, None)?
                )
            }
            _ => return Err(unsupported()),
        }
    } else {
        if bytes.len() < 4
            || u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize != bytes.len() << 2
        {
            return Err(unsupported());
        }
        let data = &bytes[4..];
        if oid == 1700 {
            if data.len() < 2 || data.len() % 2 != 0 {
                return Err(unsupported());
            }
            let header = u16::from_le_bytes(data[..2].try_into().unwrap());
            let (sign, scale, weight, offset) = if header & 0xc000 == 0x8000 {
                (
                    if header & 0x2000 != 0 { 0x4000u16 } else { 0 },
                    (header >> 7) & 0x3f,
                    ((header & 0x3f) as i16) - if header & 0x40 != 0 { 64 } else { 0 },
                    2,
                )
            } else {
                if data.len() < 4 || header & 0xc000 >= 0x8000 {
                    return Err(unsupported());
                }
                (
                    header & 0x4000,
                    header & 0x3fff,
                    i16::from_le_bytes(data[2..4].try_into().unwrap()),
                    4,
                )
            };
            let mut wire = (((data.len() - offset) / 2) as u16).to_be_bytes().to_vec();
            wire.extend_from_slice(&weight.to_be_bytes());
            wire.extend_from_slice(&sign.to_be_bytes());
            wire.extend_from_slice(&scale.to_be_bytes());
            for pair in data[offset..].chunks_exact(2) {
                wire.extend_from_slice(&u16::from_le_bytes([pair[0], pair[1]]).to_be_bytes());
            }
            astersql_types::pg_numeric::PgNumeric::from_binary(&wire)
                .map_err(|_| unsupported())?
                .to_string()
        } else {
            let text = std::str::from_utf8(data).map_err(|_| unsupported())?;
            if text.contains('\0') {
                return Err(unsupported());
            }
            let literal = if text.contains('\\') {
                format!("E'{}'", text.replace('\\', "\\\\").replace('\'', "''"))
            } else {
                format!("'{}'", text.replace('\'', "''"))
            };
            // Bare CHARACTER means character(1) in SQL. bpchar retains the
            // unconstrained internal type, so cloning never truncates to one.
            let name = if oid == 1042 {
                "bpchar".into()
            } else {
                format_column_type(oid, None)?
            };
            format!("{literal}::{name}")
        }
    };
    Ok(text)
}

pub(crate) fn column_catalog_rows(
    relation: &str,
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
    cancel: &CancellationToken,
) -> ConnResult<Vec<Vec<Value>>> {
    check_catalog_cancel(cancel)?;
    if relation == "pg_type" {
        // Builtins are independent of user relations, including malformed native
        // models. User types remain gated at DDL until they have durable identities.
        return Ok(COLUMN_TYPES
            .iter()
            .map(|t| {
                vec![
                    Value::Signed(t.oid),
                    Value::Text(t.name.into()),
                    Value::Signed(11),
                    Value::Text(t.kind.into()),
                    Value::Text(t.category.into()),
                    Value::Signed(0),
                    Value::Signed(0),
                    Value::Signed(-1),
                    Value::Signed(0),
                    Value::Null,
                    Value::Text("false".into()),
                    Value::Null,
                    Value::Signed(t.element),
                    Value::Text("true".into()),
                    Value::Null,
                    Value::Null,
                    Value::Signed(t.array),
                    Value::Text(",".into()),
                    Value::Signed(t.length),
                    Value::Text(t.by_value.to_string()),
                ]
            })
            .collect());
    }
    let mut rows = Vec::new();
    let mut default_ids = std::collections::HashSet::new();
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
        let slots = model
            .postgresql_attribute_slots()
            .map_err(|_| ConnError::UnsupportedCommand(0))?;
        for (index, (id, dropped)) in slots.iter().enumerate() {
            check_catalog_cancel(cancel)?;
            let position = (index + 1) as i64;
            if *dropped {
                if relation == "pg_attribute" {
                    rows.push(vec![
                        Value::Signed(relid),
                        Value::Signed(position),
                        Value::Text(format!("........pg.dropped.{position}........")),
                        Value::Signed(0),
                        Value::Signed(-1),
                        Value::Signed(0),
                        Value::Text("false".into()),
                        Value::Text("true".into()),
                        Value::Text("true".into()),
                        Value::Text(String::new()),
                        Value::Text(String::new()),
                        Value::Null,
                        Value::Text("false".into()),
                        Value::Null,
                    ]);
                    if rows.len() > MAX_CATALOG_ROWS {
                        return Err(catalog_row_limit());
                    }
                }
                continue;
            }
            let column = model
                .Columns
                .iter()
                .find(|c| c.ID == *id)
                .ok_or(ConnError::UnsupportedCommand(0))?;
            if column.State != astersql_meta_model::StatePublic {
                continue;
            }
            let (oid, modifier) = native_column_type(column)?;
            let generated = column.PostgreSQLGenerated.as_ref();
            if generated.is_some() && (!column.IsGenerated() || oid != 23) {
                return Err(ConnError::UnsupportedCommand(0));
            }
            let generated_tree = if let Some(bound) = generated {
                if bound.render(&model, true, &mut Vec::new()).as_deref()
                    != Some(column.GeneratedExprString.as_str())
                {
                    return Err(ConnError::UnsupportedCommand(0));
                }
                Some(generated_node_tree(bound, &model)?)
            } else {
                None
            };
            let default = if generated.is_some() {
                None
            } else {
                column_default(column, oid)?
            };
            let has_definition = generated.is_some() || default.is_some();
            if column.PostgreSQLHasDefault.is_some_and(|exists| {
                exists != has_definition || exists != column.PostgreSQLDefaultID.is_some()
            }) || column.PostgreSQLDefaultID.is_some_and(|id| id <= 0)
            {
                return Err(ConnError::UnsupportedCommand(0));
            }
            if let Some(id) = column.PostgreSQLDefaultID {
                if !default_ids.insert(id) {
                    return Err(ConnError::UnsupportedCommand(0));
                }
            }
            if relation == "pg_attrdef" {
                if has_definition {
                    // Bind the typed value to an actual PostgreSQL node, without
                    // evaluating runtime functions during catalog reads.
                    rows.push(vec![
                        Value::Signed(relid),
                        Value::Signed(position),
                        Value::Text(if let Some(tree) = &generated_tree {
                            tree.clone()
                        } else {
                            default_node_tree(
                                column,
                                oid,
                                default.as_ref().ok_or(ConnError::UnsupportedCommand(0))?,
                            )?
                        }),
                        Value::Signed(i64::from(crate::pg_oid::namespace_oid(
                            column
                                .PostgreSQLDefaultID
                                .ok_or(ConnError::UnsupportedCommand(0))?,
                        )?)),
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
                    Value::Text("false".into()),
                    Value::Text("true".into()),
                    // Only explicit durable identity metadata supplies this attribute.
                    Value::Text(
                        column
                            .PostgreSQLIdentity
                            .as_ref()
                            .map(|i| if i.Always { "a" } else { "d" })
                            .unwrap_or("")
                            .into(),
                    ),
                    Value::Text(
                        if column.PostgreSQLGenerated.is_some() {
                            if column.GeneratedStored { "s" } else { "v" }
                        } else {
                            ""
                        }
                        .into(),
                    ),
                    Value::Null,
                    Value::Text(
                        column
                            .PostgreSQLHasDefault
                            .unwrap_or(has_definition)
                            .to_string(),
                    ),
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
    let mut ids = std::collections::HashSet::new();
    if model.Indices.iter().any(|i| !ids.insert(i.ID))
        || model.Indices.iter().filter(|i| i.Primary).count() > 1
    {
        return Err(ConnError::UnsupportedCommand(0));
    }
    let mut indexes = model.Indices.clone();
    // A named PG declaration is a schema relation name, unlike native PRIMARY.
    for index in indexes.iter_mut().filter(|i| i.Primary) {
        if let Some(name) = &index.PostgreSQLConstraintName {
            index.Name = astersql_parser_ast::NewCIStr(name);
        }
    }
    if model.PKIsHandle && !indexes.iter().any(|i| i.Primary) {
        let columns: Vec<_> = model
            .Columns
            .iter()
            .filter(|c| {
                c.State == astersql_meta_model::StatePublic
                    && !c.Hidden
                    && astersql_parser_mysql::r#type::HasPriKeyFlag(c.GetFlag())
            })
            .collect();
        let [column] = columns.as_slice() else {
            return Err(ConnError::UnsupportedCommand(0));
        };
        if !matches!(
            column.GetType(),
            astersql_parser_mysql::r#type::TypeTiny
                | astersql_parser_mysql::r#type::TypeShort
                | astersql_parser_mysql::r#type::TypeInt24
                | astersql_parser_mysql::r#type::TypeLong
                | astersql_parser_mysql::r#type::TypeLonglong
        ) {
            return Err(ConnError::UnsupportedCommand(0));
        }
        indexes.push(astersql_meta_model::IndexInfo {
            ID: -1,
            // The primary handle is the row storage itself, not a secondary
            // index awaiting a backfill job.
            PostgreSQLBuildComplete: true,
            Name: astersql_parser_ast::NewCIStr(
                model
                    .PostgreSQLPrimaryConstraint
                    .as_deref()
                    .unwrap_or("PRIMARY"),
            ),
            PostgreSQLConstraintName: model.PostgreSQLPrimaryConstraint.clone(),
            Columns: vec![astersql_meta_model::IndexColumn {
                Name: column.Name.clone(),
                Offset: column.Offset,
                Length: -1,
                ..Default::default()
            }],
            Tp: astersql_meta_model::ast::IndexType::Btree,
            Unique: true,
            Primary: true,
            State: astersql_meta_model::StatePublic,
            ..Default::default()
        });
    }
    crate::pg_oid::table_oid(model.ID).map_err(|_| ConnError::UnsupportedCommand(0))?;
    for index in &indexes {
        crate::pg_oid::index_oid(model.ID, index.ID)
            .map_err(|_| ConnError::UnsupportedCommand(0))?;
    }
    Ok(indexes)
}

// PostgreSQL 18.6 pg_opclass.dat: integer_ops family, btree method 403.
// The relational runtime encodes signed integer datums in ordered keys and
// checks unique integer keys immediately; arbitrary native collations and
// custom methods do not establish equivalence to these builtins.
const INTEGER_OPCLASSES: &[(i64, &str, i64)] = &[
    (1979, "int2_ops", 21),
    (1978, "int4_ops", 23),
    (3124, "int8_ops", 20),
];
fn integer_opclass_rows() -> Vec<Vec<Value>> {
    INTEGER_OPCLASSES
        .iter()
        .map(|&(oid, name, input)| {
            let mut row = vec![Value::Null; CATALOG_ROW_WIDTH];
            row[0] = Value::Signed(oid);
            row[1] = Value::Signed(403);
            row[3] = Value::Text(name.into());
            row[4] = Value::Signed(input);
            row[5] = Value::Signed(0);
            row[6] = Value::Text("true".into());
            row[7] = Value::Signed(1976);
            row[9] = Value::Signed(11);
            row
        })
        .collect()
}

// A certificate is written only by synchronous PG CREATE TABLE after rejecting
// deferred/NOT VALID/partition forms, before publishing its empty storage. Native
// legacy models and subsequently introduced identities have no certificate.
fn constraint_state_row(
    row: &mut [Value],
    model: &astersql_meta_model::TableInfo,
    key: &str,
    enforced: bool,
) {
    if model.Partition.is_some() {
        return;
    }
    let Some(validated) = model.PostgreSQLImmediateConstraintValidation.get(key) else {
        return;
    };
    if *validated != enforced {
        return;
    }
    for (slot, value) in [
        (11, false),
        (12, false),
        (13, key.starts_with("i:") || key.starts_with("f:")),
        (23, *validated),
        (24, enforced),
        (25, true),
        (29, false),
    ] {
        row[slot] = Value::Text(value.to_string());
    }
    for slot in [26, 27, 28] {
        row[slot] = Value::Signed(0);
    }
}

pub(crate) fn index_constraint_rows(
    relation: &str,
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
    cancel: &CancellationToken,
    namespaces: &[astersql_session::runtime::PgNamespaceFact],
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
                let position = model
                    .postgresql_attribute_number(column.ID)
                    .map_err(|_| unsupported())?;
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
        let namespace = model
            .PostgreSQLNamespace
            .as_ref()
            .map_or(Ok(namespace), |(id, _)| namespace_oid(*id))?;
        if model.State != StatePublic || model.Sequence.is_some() || model.View.is_some() {
            continue;
        }
        let relid = i64::from(crate::pg_oid::table_oid(model.ID)?);
        let indexes = catalog_indexes(model)?;
        // Legacy native UNIQUE metadata does not distinguish a declaration from
        // CREATE UNIQUE INDEX. Refuse that ambiguous projection; PG-origin
        // standalone indexes have explicit dialect provenance and no owner.
        if relation == "pg_constraint"
            && indexes.iter().any(|i| {
                i.State == StatePublic
                    && i.Unique
                    && !i.Primary
                    && !model.ExactName
                    && i.PostgreSQLConstraintName.is_none()
            })
        {
            return Err(unsupported());
        }
        if relation == "pg_index" && indexes.iter().any(|i| i.State != StatePublic) {
            // The PG adapter has no concurrent-index state machine. A native
            // transitional index is unavailable, not an absent successful row.
            return Err(unsupported());
        }
        for index in indexes.iter().filter(|i| i.State == StatePublic) {
            if index.Tp != astersql_meta_model::ast::IndexType::Btree
                || index.Invisible
                || index.Global
                || index.Columns.is_empty()
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
            // Native index offsets still address the compact native row layout;
            // validate them against that layout, never against persistent attnum.
            if index.Columns.iter().any(|key| {
                !model
                    .Columns
                    .iter()
                    .any(|column| column.Name.O == key.Name.O && column.Offset == key.Offset)
            }) {
                return Err(unsupported());
            }
            let oid = i64::from(crate::pg_oid::index_oid(model.ID, index.ID)?);
            let (schema_name, table_name) = match &model.PostgreSQLNamespace {
                Some((id, name)) => (
                    namespaces
                        .iter()
                        .find(|fact| fact.id == *id)
                        .map(|fact| fact.name.as_str())
                        .ok_or_else(unsupported)?,
                    name.as_str(),
                ),
                None => ("public", model.Name.O.as_str()),
            };
            let definition = format!(
                "CREATE {}INDEX {} ON {}.{} USING btree ({})",
                if index.Unique { "UNIQUE " } else { "" },
                pg_identifier(&index.Name.O),
                if schema_name == "public" {
                    "public".into()
                } else {
                    pg_identifier(schema_name)
                },
                pg_identifier(table_name),
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
                // Only ordinary native keys reach this provider. PG declarations
                // with direction/NULL ordering or INCLUDE are rejected before DDL
                // because the native model cannot preserve those facts.
                row[8] = Value::Text(vec!["0"; columns.len()].join(" "));
                // Signed integer keys use the same ordered integer encoding and
                // equality checks as the runtime's relational indexes. No string
                // collation or noninteger operator class is inferred here.
                let classes: Option<Vec<_>> = index
                    .Columns
                    .iter()
                    .map(|key| {
                        let column = model.Columns.iter().find(|c| c.Name.O == key.Name.O)?;
                        if astersql_parser_mysql::r#type::HasUnsignedFlag(column.GetFlag()) {
                            return None;
                        }
                        let oid = native_column_type(column).ok()?.0;
                        INTEGER_OPCLASSES
                            .iter()
                            .find(|(_, _, input)| *input == oid)
                            .map(|(oid, _, _)| oid.to_string())
                    })
                    .collect();
                if let Some(classes) = classes {
                    row[9] = Value::Text(classes.join(" "));
                    row[10] = Value::Text(vec!["0"; columns.len()].join(" "));
                }
                if index.BackfillState != astersql_meta_model::BackfillStateInapplicable {
                    return Err(unsupported());
                }
                // Only synchronous ordinary indexes are exposed. EXCLUDE,
                // deferred constraints, CLUSTER, concurrent rebuilds and replica
                // identity DDL are rejected by the PG entry before mutation.
                for (slot, value) in [
                    (13, true),
                    (14, true),
                    (15, false),
                    (16, true),
                    (17, false),
                    (18, false),
                    (19, true),
                    (20, false),
                ] {
                    if index.PostgreSQLBuildComplete {
                        row[slot] = Value::Text(value.to_string());
                    }
                }
                row[21] = Value::Text(definition);
                row[22] = Value::Text(
                    columns
                        .iter()
                        .map(|c| c.1.clone())
                        .collect::<Vec<_>>()
                        .join("\0"),
                );
            } else if index.Primary || (index.Unique && index.PostgreSQLConstraintName.is_some()) {
                row[0] = Value::Signed(oid);
                row[1] = Value::Text(
                    index
                        .PostgreSQLConstraintName
                        .as_ref()
                        .unwrap_or(&index.Name.O)
                        .clone(),
                );
                row[2] = Value::Text(if index.Primary { "p" } else { "u" }.into());
                row[3] = Value::Signed(relid);
                row[4] = Value::Signed(namespace);
                row[5] = array(&columns);
                row[6] = Value::Signed(oid);
                row[7] = Value::Signed(0);
                row[9] = Value::Text(" ".into());
                row[10] = Value::Text(" ".into());
                row[22] = Value::Text(" ".into());
                constraint_state_row(&mut row, model, &format!("i:{}", index.ID), true);
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
        for constraint in model.Constraints.iter().filter(|c| c.State == StatePublic) {
            let bound = model
                .PostgreSQLChecks
                .get(&constraint.ID)
                .ok_or_else(unsupported)?;
            let mut dependencies = Vec::new();
            let expression = bound
                .render(model, false, &mut dependencies)
                .ok_or_else(unsupported)?;
            let columns = key_columns(
                model,
                &dependencies
                    .iter()
                    .map(|id| {
                        model
                            .Columns
                            .iter()
                            .find(|c| c.ID == *id)
                            .unwrap()
                            .Name
                            .O
                            .clone()
                    })
                    .collect::<Vec<_>>(),
            )?;
            if constraint.ID <= 0 {
                return Err(unsupported());
            }
            // Negative odd IDs below -1 are separate from FK's even IDs and
            // the handle primary key. CHECK and NOT NULL occupy distinct residues.
            let local_id = constraint
                .ID
                .checked_mul(-4)
                .and_then(|id| id.checked_sub(1))
                .ok_or_else(unsupported)?;
            let mut row = vec![Value::Null; CATALOG_ROW_WIDTH];
            row[0] = Value::Signed(i64::from(crate::pg_oid::index_oid(model.ID, local_id)?));
            row[1] = Value::Text(constraint.Name.O.clone());
            row[2] = Value::Text("c".into());
            row[3] = Value::Signed(relid);
            row[4] = Value::Signed(namespace);
            if !columns.is_empty() {
                row[5] = array(&columns);
            }
            for slot in [6, 7] {
                row[slot] = Value::Signed(0);
            }
            for slot in [9, 10, 22] {
                row[slot] = Value::Text(" ".into());
            }
            constraint_state_row(
                &mut row,
                model,
                &format!("c:{}", constraint.ID),
                constraint.Enforced,
            );
            row[21] = Value::Text(format!(
                "CHECK {}{}",
                expression,
                if constraint.Enforced {
                    ""
                } else {
                    " NOT ENFORCED"
                }
            ));
            rows.push(row);
            if rows.len() > MAX_CATALOG_ROWS {
                return Err(catalog_row_limit());
            }
        }
        for column in model.Columns.iter().filter(|c| {
            c.State == StatePublic
                && !c.Hidden
                && astersql_parser_mysql::r#type::HasNotNullFlag(c.GetFlag())
        }) {
            let Some((generation, name)) = &column.PostgreSQLNotNull else {
                // Legacy native nullability has no PG18 constraint identity.
                if model.ExactName {
                    return Err(unsupported());
                }
                continue;
            };
            if column.ID <= 0 || *generation <= 0 {
                return Err(unsupported());
            }
            let a = i128::from(column.ID);
            let b = i128::from(*generation);
            let sum = a.checked_add(b).ok_or_else(unsupported)?;
            let paired = sum
                .checked_mul(sum.checked_add(1).ok_or_else(unsupported)?)
                .and_then(|v| (v / 2).checked_add(b))
                .ok_or_else(unsupported)?;
            let local_id = i64::try_from(paired)
                .ok()
                .and_then(|v| v.checked_mul(-4))
                .and_then(|v| v.checked_sub(3))
                .ok_or_else(unsupported)?;
            let columns = key_columns(model, &[column.Name.O.clone()])?;
            let mut row = vec![Value::Null; CATALOG_ROW_WIDTH];
            row[0] = Value::Signed(i64::from(crate::pg_oid::index_oid(model.ID, local_id)?));
            row[1] = Value::Text(name.clone());
            row[2] = Value::Text("n".into());
            row[3] = Value::Signed(relid);
            row[4] = Value::Signed(namespace);
            row[5] = array(&columns);
            for slot in [6, 7] {
                row[slot] = Value::Signed(0);
            }
            for slot in [9, 10, 22] {
                row[slot] = Value::Text(" ".into());
            }
            constraint_state_row(
                &mut row,
                model,
                &format!("n:{}:{generation}", column.ID),
                true,
            );
            row[21] = Value::Text(format!("NOT NULL {}", names(&columns)));
            rows.push(row);
            if rows.len() > MAX_CATALOG_ROWS {
                return Err(catalog_row_limit());
            }
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
            if parent.State != StatePublic
                || matches!(fk.OnUpdate, 3 | 5)
                || fk.OnDelete == 5
                || fk
                    .RefCols
                    .iter()
                    .enumerate()
                    .any(|(n, c)| fk.RefCols[..n].iter().any(|p| p.L == c.L))
                || fk
                    .Cols
                    .iter()
                    .enumerate()
                    .any(|(n, c)| fk.Cols[..n].iter().any(|p| p.L == c.L))
            {
                return Err(unsupported());
            }
            let parent_indexes = catalog_indexes(parent)?;
            let referenced_index = parent_indexes
                .iter()
                .find(|i| {
                    i.State == StatePublic
                        && i.Unique
                        && i.Columns.len() == referenced.len()
                        && i.ConditionExprString.is_empty()
                        && fk
                            .RefCols
                            .iter()
                            .all(|r| i.Columns.iter().filter(|c| c.Name.L == r.L).count() == 1)
                        && i.Columns.iter().all(|c| {
                            c.Length == -1
                                && !c.UseChangingType
                                && fk.RefCols.iter().any(|r| c.Name.L == r.L)
                                && parent
                                    .Columns
                                    .iter()
                                    .any(|p| p.Name.L == c.Name.L && p.Offset == c.Offset)
                        })
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
            // Shared FK checks exempt any tuple containing NULL; PostgreSQL
            // FULL/PARTIAL are rejected before DDL, so this is SIMPLE semantics.
            row[22] = Value::Text("s".into());
            constraint_state_row(&mut row, model, &format!("f:{}", fk.ID), true);
            let operators: Option<Vec<_>> = fk
                .RefCols
                .iter()
                .zip(&fk.Cols)
                .map(|(p, c)| {
                    let p = parent.Columns.iter().find(|v| v.Name.L == p.L)?;
                    let c = model.Columns.iter().find(|v| v.Name.L == c.L)?;
                    let pt = native_column_type(p).ok()?.0;
                    let ct = native_column_type(c).ok()?.0;
                    if pt != ct
                        || astersql_parser_mysql::r#type::HasUnsignedFlag(p.GetFlag())
                        || astersql_parser_mysql::r#type::HasUnsignedFlag(c.GetFlag())
                    {
                        return None;
                    }
                    match pt {
                        21 => Some("94"),
                        23 => Some("96"),
                        20 => Some("410"),
                        _ => None,
                    }
                })
                .collect();
            if let Some(operators) = operators {
                for slot in [31, 32, 33] {
                    row[slot] = Value::Text(format!("{{{}}}", operators.join(",")));
                }
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

/// Current-database namespace identities, shared by pg_namespace and path binding.
/// Native databases remain separate PG databases, never search-path schemas.
fn namespace_rows(
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
) -> ConnResult<Vec<Vec<Value>>> {
    let mut rows = vec![namespace_row(11, "pg_catalog")];
    if let Some(schema) = snapshot.SchemaByName(&astersql_infoschema::CiString::new(database)) {
        rows.push(namespace_row(namespace_oid(schema.id)?, "public"));
    }
    Ok(rows)
}
pub(crate) fn namespace_names(context: &dyn TiDBContext) -> ConnResult<Vec<String>> {
    let current = context.execute_query("SELECT DATABASE()", false, &CancellationToken::new())?;
    let database = current
        .first()
        .and_then(|r| r.rows.first())
        .and_then(|r| r.first());
    let database = match database {
        Some(Value::Text(name)) => name.as_str(),
        _ => "",
    };
    let snapshot = context
        .schema_snapshot()
        .ok_or_else(|| ConnError::Session("schema snapshot is unavailable".into()))?;
    Ok(
        namespace_rows_for_context(context, snapshot.as_ref(), database)?
            .into_iter()
            .filter_map(|row| match row.into_iter().nth(1) {
                Some(Value::Text(name)) => Some(name),
                _ => None,
            })
            .collect(),
    )
}

fn namespace_rows_for_context(
    context: &dyn TiDBContext,
    snapshot: &dyn astersql_infoschema::InfoSchema,
    database: &str,
) -> ConnResult<Vec<Vec<Value>>> {
    if let Some(facts) = context.pg_namespace_facts() {
        return facts?
            .into_iter()
            .map(|fact| {
                let mut row = namespace_row(
                    if fact.name == "pg_catalog" {
                        11
                    } else {
                        namespace_oid(fact.id)?
                    },
                    &fact.name,
                );
                row[5] = fact.owner.map_or(Value::Null, Value::Signed);
                row[6] = fact.acl.map_or(Value::Null, Value::Text);
                // Internal source marker, never exposed as a catalog attribute.
                row[7] = Value::Signed(i64::from(fact.acl_available));
                Ok(row)
            })
            .collect();
    }
    namespace_rows(snapshot, database)
}
