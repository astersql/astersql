// Copyright 2026 AsterSQL.

use super::*;

pub(super) fn mlog_schedule_unix_seconds(
    session: &ConcreteSession,
    expression: &ast::ExprNode,
) -> SessionResult<Option<i64>> {
    let context = plan_context_with_params(Arc::clone(&session.session_vars), &[], false);
    let expression_context = astersql_planner_core_base::PlanContext::GetExprCtx(context.as_ref());
    let built =
        astersql_planner_core::PlannerBuildSimpleExpr(expression_context, expression, Vec::new())
            .map_err(|error| session_error("build MLog purge schedule", error))?;
    let (value, null) = built
        .EvalTime(
            expression_context.GetEvalCtx(),
            astersql_expression::chunk::Row::default(),
        )
        .map_err(|error| session_error("evaluate MLog purge schedule", error))?;
    let value = (!null).then(|| value.String());
    value
        .map(|value| {
            let local = parse_runtime_datetime(&value).ok_or_else(|| {
                SessionError::new(format!("invalid MLog purge schedule time {value:?}"))
            })?;
            Ok(local.and_utc().timestamp())
        })
        .transpose()
}

pub(super) fn mlog_schedule_unix_seconds_with_mode(
    expression: &ast::ExprNode,
    sql_mode: astersql_parser_mysql::r#const::SQLMode,
) -> SessionResult<Option<i64>> {
    let eval = Arc::new(astersql_expression_exprstatic::NewEvalContext(vec![
        astersql_expression_exprstatic::WithSQLMode(sql_mode),
        astersql_expression_exprstatic::WithLocation(chrono_tz::UTC),
    ]));
    let context = astersql_expression_exprstatic::NewExprContext(vec![
        astersql_expression_exprstatic::WithEvalCtx(Arc::clone(&eval)),
    ]);
    let built = astersql_planner_core::PlannerBuildSimpleExpr(&context, expression, Vec::new())
        .map_err(|error| session_error("build MLog purge NEXT", error))?;
    let (value, null) = built
        .EvalTime(eval.as_ref(), astersql_expression::chunk::Row::default())
        .map_err(|error| session_error("evaluate MLog purge NEXT", error))?;
    if null {
        return Ok(None);
    }
    let value = value.String();
    let datetime = parse_runtime_datetime(&value)
        .ok_or_else(|| SessionError::new(format!("invalid MLog purge NEXT time {value:?}")))?;
    Ok(Some(datetime.and_utc().timestamp()))
}

impl ConcreteSession {
    pub(super) fn execute_create_materialized_view_log(
        &self,
        statement: &ast::CreateMaterializedViewLogStmt,
    ) -> SessionResult<()> {
        let target = statement
            .Table
            .as_ref()
            .ok_or_else(|| SessionError::new("CREATE MATERIALIZED VIEW LOG has no base table"))?;
        let database = if target.Schema.L.is_empty() {
            self.current_database()
        } else {
            target.Schema.L.clone()
        };
        let base = self
            .resolve_runtime_table(&database, &target.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!(
                    "base table {}.{} does not exist",
                    database, target.Name.O
                ))
            })?;
        let log_name = astersql_meta_model::MaterializedViewLogTableName(&base.Name);
        if base.Partition.is_some() {
            return Err(SessionError::new(
                "CREATE MATERIALIZED VIEW LOG on partition table is unsupported",
            ));
        }
        if base.View.is_some()
            || base.Sequence.is_some()
            || base.MaterializedViewLog.is_some()
            || base.MaterializedView.is_some()
            || base.MaterializedViewShadow.is_some()
            || base.TempTableType != astersql_meta_model::TempTableNone
        {
            return Err(SessionError::new(
                "CREATE MATERIALIZED VIEW LOG requires a base table",
            ));
        }
        let (purge_method, purge_start_with, purge_next) = if let Some(purge) = &statement.Purge {
            if purge.Immediate {
                return Err(SessionError::new(
                    "PURGE IMMEDIATE is not supported for CREATE MATERIALIZED VIEW LOG",
                ));
            }
            let next = purge.Next.as_ref().ok_or_else(|| {
                SessionError::new("PURGE NEXT is required for CREATE MATERIALIZED VIEW LOG")
            })?;
            let context = plan_context_with_params(Arc::clone(&self.session_vars), &[], false);
            let expression_context =
                astersql_planner_core_base::PlanContext::GetExprCtx(context.as_ref());
            let validate = |expression: &ast::ExprNode, clause: &str| -> SessionResult<String> {
                let built = astersql_planner_core::PlannerBuildSimpleExpr(
                    expression_context,
                    expression,
                    Vec::new(),
                )
                .map_err(|error| session_error("build materialized view log schedule", error))?;
                let tp = built.GetType(expression_context.GetEvalCtx()).GetType();
                if !matches!(
                    tp,
                    astersql_parser_mysql::r#type::TypeDate
                        | astersql_parser_mysql::r#type::TypeDatetime
                        | astersql_parser_mysql::r#type::TypeTimestamp
                ) {
                    return Err(SessionError::new(format!(
                        "{clause} expression must return DATE/DATETIME/TIMESTAMP"
                    )));
                }
                ast::sql_restore::restore_expr(expression)
                    .map_err(|error| SessionError::new(format!("restore {clause}: {error}")))
            };
            (
                "DEFERRED".to_owned(),
                purge
                    .StartWith
                    .as_ref()
                    .map(|expr| validate(expr, "PURGE START WITH"))
                    .transpose()?
                    .unwrap_or_default(),
                validate(next, "PURGE NEXT")?,
            )
        } else {
            (String::new(), String::new(), String::new())
        };
        let purge_sql_mode =
            astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
                .map_err(|error| session_error("parse MLog purge SQL mode", error))?;
        let alert_rows = statement
            .AccumulationAlert
            .as_ref()
            .map(|alert| {
                u64::try_from(alert.Rows).map_err(|_| SessionError::new("invalid ALERT ROWS value"))
            })
            .transpose()?;
        let mut columns = Vec::with_capacity(statement.Cols.len() + 2);
        let mut seen = HashSet::new();
        for name in &statement.Cols {
            if name.L
                == astersql_meta_model::MaterializedViewLogDMLTypeColumnName.to_ascii_lowercase()
                || name.L
                    == astersql_meta_model::MaterializedViewLogOldNewColumnName.to_ascii_lowercase()
            {
                return Err(SessionError::new(format!(
                    "reserved MLog column {}",
                    name.O
                )));
            }
            if !seen.insert(name.L.clone()) {
                return Err(SessionError::new(format!(
                    "duplicate MLog column {}",
                    name.O
                )));
            }
            let base_column = base
                .Columns
                .iter()
                .find(|column| column.Name.L == name.L)
                .ok_or_else(|| SessionError::new(format!("unknown MLog column {}", name.O)))?;
            if base_column.GetType() == astersql_parser_mysql::r#type::TypeJSON {
                return Err(SessionError::new(format!(
                    "MLog does not support JSON column {}",
                    name.O
                )));
            }
            if astersql_parser_types::IsTypeBlob(base_column.GetType())
                && base_column.FieldType.GetCharset() == astersql_parser_charset::CharsetBin
            {
                return Err(SessionError::new(format!(
                    "MLog does not support BLOB column {}",
                    name.O
                )));
            }
            let mut field_type = base_column.FieldType.clone();
            field_type.DelFlag(
                astersql_parser_mysql::r#type::PriKeyFlag
                    | astersql_parser_mysql::r#type::UniqueKeyFlag
                    | astersql_parser_mysql::r#type::MultipleKeyFlag
                    | astersql_parser_mysql::r#type::AutoIncrementFlag
                    | astersql_parser_mysql::r#type::OnUpdateNowFlag,
            );
            columns.push(ast::ColumnDef {
                Name: ast::ColumnName {
                    Name: name.clone(),
                    ..Default::default()
                },
                Tp: field_type,
                Options: Vec::new(),
            });
        }
        for (name, tp, flen) in [
            (
                astersql_meta_model::MaterializedViewLogDMLTypeColumnName,
                astersql_parser_mysql::r#type::TypeVarchar,
                1,
            ),
            (
                astersql_meta_model::MaterializedViewLogOldNewColumnName,
                astersql_parser_mysql::r#type::TypeTiny,
                4,
            ),
        ] {
            let mut field_type = astersql_parser_types::NewFieldType(tp);
            field_type.SetFlen(flen);
            field_type.SetFlag(astersql_parser_mysql::r#type::NotNullFlag);
            columns.push(ast::ColumnDef {
                Name: ast::ColumnName {
                    Name: ast::NewCIStr(name),
                    ..Default::default()
                },
                Tp: field_type,
                Options: Vec::new(),
            });
        }
        let create = ast::CreateTableStmt {
            Table: ast::TableName {
                Schema: ast::NewCIStr(&database),
                Name: log_name.clone(),
                ..Default::default()
            },
            Cols: columns,
            Options: statement.Options.clone(),
            ..Default::default()
        };
        let context =
            astersql_meta_metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
        let mut log = astersql_ddl::BuildTableInfoFromAST(&context, &create)
            .map_err(|error| session_error("build materialized view log", error))?;
        log.MaterializedViewLog = Some(astersql_meta_model::MaterializedViewLogInfo {
            BaseTableID: base.ID,
            Columns: statement.Cols.clone(),
            PurgeMethod: purge_method,
            PurgeStartWith: purge_start_with.clone(),
            PurgeNext: purge_next.clone(),
            PurgeScheduleSQLMode: purge_sql_mode,
            LogAccumulationAlertRows: alert_rows,
            ..Default::default()
        });
        let next_purge_unix_seconds = if let Some(purge) = &statement.Purge {
            let next = purge
                .Next
                .as_ref()
                .map(|expr| mlog_schedule_unix_seconds(self, expr))
                .transpose()?
                .flatten();
            let start = purge
                .StartWith
                .as_ref()
                .map(|expr| mlog_schedule_unix_seconds(self, expr))
                .transpose()?
                .flatten();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| session_error("read MLog purge clock", error))?
                .as_secs() as i64;
            start
                .filter(|start| *start >= now.saturating_add(10))
                .or(next)
        } else {
            None
        };
        self.domain
            .ddl_create_materialized_view_log(&database, &base.Name.L, log, next_purge_unix_seconds)
            .map_err(|error| session_error("create materialized view log", error))
    }
}
