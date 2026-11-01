// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::mview_refresh::{initialize_session, sql};
use super::*;
use astersql_executor::adapter::{ExecExecutor, ExecutionContext};
use astersql_executor::mview_compare::{
    CompareLayout, differ_type_column_name, output_batch_bytes,
};

fn ident(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
fn qualified(database: &str, table: &str) -> String {
    format!("{}.{}", ident(database), ident(table))
}
fn select(query: &str) -> SessionResult<ast::SelectStmt> {
    astersql_parser::New()
        .ParseOneStmt(query, "", "")
        .map_err(|e| SessionError::new(e.to_string()))?
        .into_any()
        .downcast::<ast::SelectStmt>()
        .map(|s| *s)
        .map_err(|_| SessionError::new("compare materialized view: invalid select sql"))
}
fn rows(session: &ConcreteSession, query: &str) -> SessionResult<Vec<Vec<String>>> {
    let mut result = Vec::new();
    for mut set in session.execute(query)? {
        let read = (|| {
            while let Some(row) = set.next_row()? {
                result.push(row);
            }
            Ok(())
        })();
        let close = set.close();
        read?;
        close?;
    }
    Ok(result)
}
fn database(session: &ConcreteSession, target: &ast::TableName) -> SessionResult<String> {
    let db = if target.Schema.O.is_empty() {
        session.current_database()
    } else {
        target.Schema.O.clone()
    };
    if db.is_empty() {
        return Err(SessionError::new("No database selected"));
    }
    Ok(db)
}
fn target(
    session: &ConcreteSession,
    is: &dyn astersql_infoschema::infoschema::InfoSchema,
    db: &str,
    name: &str,
) -> SessionResult<Arc<astersql_meta_model::TableInfo>> {
    let table = is
        .TableByName(
            &astersql_infoschema::CiString::new(db),
            &astersql_infoschema::CiString::new(name),
        )
        .map_err(|e| SessionError::new(e.to_string()))?;
    let table = table
        .ModelMeta()
        .map_err(|e| SessionError::new(e.to_string()))?;
    let info = table.MaterializedView.as_ref().ok_or_else(|| {
        SessionError::new(format!(
            "[schema:1347]'{db}.{name}' is not MATERIALIZED VIEW"
        ))
    })?;
    if info.SQLContent.is_empty() {
        return Err(SessionError::new(
            "compare materialized view: invalid select sql",
        ));
    }
    if !info.GetInitBuildState().IsReady() {
        return Err(SessionError::new(
            info.GetInitBuildState()
                .AccessErrorMessage(&format!("{db}.{name}")),
        ));
    }
    session.check_compare_base_select(info, is)?;
    Ok(table)
}

/// Output DDL and every committed batch belong to this guard until success.
/// Cleanup drops the table even if failure happens after an earlier batch commit.
struct Output {
    session: ConcreteSession,
    name: String,
    complete: bool,
}
impl Drop for Output {
    fn drop(&mut self) {
        if !self.complete {
            let _ = sql(&self.session, "ROLLBACK");
            if let Err(error) = sql(
                &self.session,
                &format!("DROP TABLE IF EXISTS {}", self.name),
            ) {
                BgLogger().log(
                    LogLevel::Warn,
                    "failed to cleanup compare materialized view output table",
                    [LogField::String("error".into(), error.to_string())],
                );
            }
        }
    }
}
impl ConcreteSession {
    fn compare_output_available(&self, db: &str, name: &str) -> SessionResult<()> {
        let is = self.domain.info_schema();
        if is
            .SchemaByName(&astersql_infoschema::CiString::new(db))
            .is_none()
        {
            return Err(SessionError::new(format!("Unknown database '{db}'")));
        }
        if is
            .TableByName(
                &astersql_infoschema::CiString::new(db),
                &astersql_infoschema::CiString::new(name),
            )
            .is_ok()
        {
            return Err(SessionError::new(format!(
                "[schema:1050]Table '{db}.{name}' already exists"
            )));
        }
        Ok(())
    }
    fn check_compare_base_select(
        &self,
        info: &astersql_meta_model::MaterializedViewInfo,
        is: &dyn astersql_infoschema::infoschema::InfoSchema,
    ) -> SessionResult<()> {
        let Some(user) = self.login_user.as_deref() else {
            return Ok(());
        };
        let host = self
            .authenticated_host
            .as_deref()
            .or(self.login_host.as_deref())
            .unwrap_or("%");
        for id in &info.BaseTableIDs {
            let table = is
                .TableByID(*id)
                .ok_or_else(|| SessionError::new("materialized view base table is missing"))?;
            let schema = astersql_infoschema::SchemaByTable(is, table.Meta())
                .ok_or_else(|| SessionError::new("materialized view base schema is missing"))?;
            let model = table
                .ModelMeta()
                .map_err(|e| SessionError::new(e.to_string()))?;
            if !runtime_privilege_handle(&self.domain)
                .Get()
                .RequestVerification(
                    &self.active_roles.borrow(),
                    user,
                    host,
                    &schema.name.lower,
                    &model.Name.L,
                    "",
                    astersql_privilege_privileges::SelectPriv,
                )
            {
                return Err(SessionError::new(format!(
                    "SELECT command denied to user '{user}'@'{host}' for table '{}'",
                    model.Name.O
                )));
            }
        }
        Ok(())
    }
    pub(super) fn execute_compare_sql(
        &self,
        statement: &ast::CompareMaterializedViewStmt,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        if self.sql_dialect() != ast::SqlDialect::MySQL {
            return Err(SessionError::new(
                "materialized view comparison requires the MySQL dialect",
            ));
        }
        let mut planner = astersql_planner_core::planbuilder::NewPlanBuilder(&[]);
        let _plan = planner
            .buildCompareMaterializedView(statement, &self.current_database())
            .map_err(|e| SessionError::new(e.0))?;
        let view = statement
            .ViewName
            .as_ref()
            .ok_or_else(|| SessionError::new("compare materialized view: missing view name"))?;
        let db = database(self, view)?;
        // Planner privileges precede snapshot evaluation and metadata lookup.
        self.check_mlog_operate_privilege(&db, &view.Name.L)?;
        let output = statement
            .OutputTable
            .as_ref()
            .map(|out| database(self, out).map(|db| (db, out.Name.O.clone())))
            .transpose()?;
        if let (Some((outdb, out)), Some(user)) = (&output, self.login_user.as_deref()) {
            let host = self
                .authenticated_host
                .as_deref()
                .or(self.login_host.as_deref())
                .unwrap_or("%");
            for (privilege, label) in [
                (astersql_privilege_privileges::CreatePriv, "CREATE"),
                (astersql_privilege_privileges::InsertPriv, "INSERT"),
            ] {
                if !runtime_privilege_handle(&self.domain)
                    .Get()
                    .RequestVerification(
                        &self.active_roles.borrow(),
                        user,
                        host,
                        outdb,
                        out,
                        "",
                        privilege,
                    )
                {
                    return Err(SessionError::new(format!(
                        "{label} command denied to user '{user}'@'{host}' for table '{out}'"
                    )));
                }
            }
        }
        let asof = statement.AsOf.as_ref().ok_or_else(|| {
            SessionError::new("compare materialized view: missing AS OF TIMESTAMP")
        })?;
        let snapshot = self.evaluate_stale_read_ts(&asof.TsExpr)?;
        target(self, self.domain.info_schema().as_ref(), &db, &view.Name.O)?;
        if let Some((db, name)) = &output {
            self.compare_output_available(db, name)?;
        }
        let is = self
            .domain
            .snapshot_info_schema(snapshot)
            .map_err(|e| SessionError::new(e.to_string()))?;
        let table = target(self, is.as_ref(), &db, &view.Name.O)?;
        let info = table.MaterializedView.as_ref().unwrap();
        if let Some((db, name)) = &output {
            self.compare_output_available(db, name)?;
        }
        let history = ConcreteSession::new(self.domain.clone());
        history.state.borrow_mut().in_restricted_sql = true;
        sql(&history, &format!("SET tidb_snapshot='{snapshot}'"))?;
        let watermark = rows(
            &history,
            &format!(
                "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",
                table.ID
            ),
        )?
        .first()
        .and_then(|r| r.first())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            SessionError::new("compare materialized view: LAST_SUCCESS_READ_TSO is NULL or missing")
        })?;
        if watermark > snapshot {
            return Err(SessionError::new(format!(
                "compare materialized view: LAST_SUCCESS_READ_TSO {watermark} is newer than compare snapshot {snapshot}"
            )));
        }
        let (definition, source, _) =
            super::mview_source::definition_with_schema(&db, &table, is.as_ref())?;
        let source = *source;
        let diff =
            astersql_planner_core::mview_refresh_builder::BuildCompleteDiffSource(&definition)
                .map_err(SessionError::new)?;
        if table.Columns.iter().any(|c| c.Hidden) {
            return Err(SessionError::new(
                "compare materialized view: hidden MV columns are unsupported",
            ));
        }
        let keys = diff.group_key_mv_offsets;
        let marker = diff.marker_mv_offset;
        let layout = CompareLayout {
            columns: table.Columns.len(),
            marker,
            group_keys: keys.clone(),
        };
        let execution = ConcreteSession::new(self.domain.clone());
        execution.state.borrow_mut().in_restricted_sql = true;
        sql(&execution, &format!("USE {}", ident(&db)))?;
        let _settings = initialize_session(&execution, info, self, false)?;
        let current_sql = format!(
            "SELECT {} FROM {}",
            table
                .Columns
                .iter()
                .map(|c| ident(&c.Name.O))
                .collect::<Vec<_>>()
                .join(","),
            qualified(&db, &table.Name.O)
        );
        let left = execution
            .compile_mview_source(select(&current_sql)?, snapshot)
            .map_err(|e| SessionError::new(e.to_string()))?;
        let right = execution
            .compile_mview_source(source, watermark)
            .map_err(|e| SessionError::new(e.to_string()))?;
        let variables = Arc::new(astersql_sessionctx_variable::session::SessionVars::default());
        let context = super::planning::mview_plan_context(
            variables,
            info.DefinitionSQLMode,
            *execution.time_zone.borrow(),
        );
        let join_keys = keys
            .iter()
            .map(|offset| {
                astersql_expression::Column::new(
                    table.Columns[*offset].FieldType.clone(),
                    *offset as i64,
                    *offset as i64,
                    *offset as isize,
                )
            })
            .collect::<Vec<_>>();
        let null_equal = keys
            .iter()
            .map(|offset| {
                !astersql_parser_mysql::r#type::HasNotNullFlag(table.Columns[*offset].GetFlag())
            })
            .collect();
        let mut join = astersql_executor::typed_hash_join::TypedHashJoin::new(
            left,
            right,
            join_keys.clone(),
            join_keys,
            null_equal,
            vec![],
            vec![],
            vec![],
            context,
            astersql_planner_core_base::JoinType::FullOuterJoin,
        )
        .map_err(SessionError::new)?;
        let fields = join
            .Schema()
            .iter()
            .map(|c| c.field_type.clone())
            .collect::<Vec<_>>();
        let mut writer = if let Some((db, name)) = &output {
            let session = ConcreteSession::new(self.domain.clone());
            session.state.borrow_mut().in_restricted_sql = true;
            let diffname = differ_type_column_name(table.Columns.iter().map(|c| c.Name.O.clone()));
            let mut columns = table.Columns.clone();
            use astersql_parser_mysql::r#type::*;
            let excluded = PriKeyFlag
                | UniqueKeyFlag
                | MultipleKeyFlag
                | AutoIncrementFlag
                | NoDefaultValueFlag
                | OnUpdateNowFlag;
            for (offset, column) in columns.iter_mut().enumerate() {
                column.Offset = offset as isize;
                column.State = astersql_meta_model::StatePublic;
                column.FieldType.SetFlag(column.GetFlag() & !excluded);
            }
            let charset = if table.Charset.is_empty() {
                "utf8mb4"
            } else {
                &table.Charset
            };
            let collate = if table.Collate.is_empty() {
                "utf8mb4_bin"
            } else {
                &table.Collate
            };
            let mut diff_type = astersql_parser_types::NewFieldType(TypeVarchar);
            diff_type.SetFlen(32);
            diff_type.SetFlag(NotNullFlag);
            diff_type.SetCharset(charset.to_owned());
            diff_type.SetCollate(collate.to_owned());
            columns.push(astersql_meta_model::ColumnInfo {
                ID: columns.len() as i64 + 1,
                Name: ast::NewCIStr(&diffname),
                Offset: columns.len() as isize,
                State: astersql_meta_model::StatePublic,
                FieldType: diff_type,
                ..Default::default()
            });
            sql(
                &session,
                &output_create_sql(db, name, &columns, charset, collate),
            )?;
            let name = qualified(db, name);
            Some(Output {
                session,
                name,
                complete: false,
            })
        } else {
            None
        };
        let _writer_settings = writer
            .as_ref()
            .map(|w| initialize_session(&w.session, info, self, false))
            .transpose()?;
        let mut count = 0_i64;
        let run = (|| -> SessionResult<()> {
            join.Open().map_err(|e| SessionError::new(e.to_string()))?;
            let mut chunk = join.NewChunk();
            let mut pending = 0;
            let limit = output_batch_bytes(
                kv::TxnTotalSizeLimit.load(std::sync::atomic::Ordering::Relaxed),
            );
            loop {
                join.NextWithContext(
                    &ExecutionContext {
                        sql_killer: Some(self.sql_killer.clone()),
                        ..Default::default()
                    },
                    &mut chunk,
                )
                .map_err(|e| SessionError::new(e.to_string()))?;
                if chunk.NumRows() == 0 {
                    break;
                }
                for index in 0..chunk.NumRows() {
                    let joined = chunk.GetRow(index).GetDatumRow(&fields);
                    let Some((row, kind)) = layout.classify(&joined).map_err(SessionError::new)?
                    else {
                        continue;
                    };
                    count += 1;
                    if let Some(writer) = &writer {
                        if pending == 0 {
                            sql(&writer.session, "BEGIN")?;
                        }
                        let mut values = row
                            .iter()
                            .map(super::mview_source::datum_literal)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(SessionError::new)?;
                        values.push(format!("'{kind}'"));
                        let insert =
                            format!("INSERT INTO {} VALUES ({})", writer.name, values.join(","));
                        sql(&writer.session, &insert)?;
                        pending += 1;
                        let bytes = writer
                            .session
                            .state
                            .borrow()
                            .transaction
                            .as_ref()
                            .map_or(0, |txn| txn.Size());
                        if pending >= 1024 || bytes >= limit {
                            flush(writer)?;
                            pending = 0;
                        }
                    }
                }
            }
            if pending > 0 {
                flush(writer.as_ref().unwrap())?;
            }
            Ok(())
        })();
        let closed = join.Close().map_err(|e| SessionError::new(e.to_string()));
        run?;
        closed?;
        if let Some(writer) = writer.as_mut() {
            writer.complete = true;
            return Ok(None);
        }
        let instant = chrono::DateTime::from_timestamp_millis((watermark >> 18) as i64)
            .ok_or_else(|| SessionError::new("compare timestamp is out of range"))?;
        let timestamp = match *self.time_zone.borrow() {
            super::session::RuntimeTimeZone::Named(zone) => instant
                .with_timezone(&zone)
                .format("%Y-%m-%d %H:%M:%S%.6f %:z")
                .to_string(),
            super::session::RuntimeTimeZone::Fixed(zone) => instant
                .with_timezone(&zone)
                .format("%Y-%m-%d %H:%M:%S%.6f %:z")
                .to_string(),
        };
        Ok(Some(ConcreteRecordSet::new(
            vec!["compare result".into()],
            vec![vec![format!(
                "There are {}differences result in {count} rows compared to source base tables at timestamp '{timestamp}' (TSO: {watermark})",
                if count == 0 { "no " } else { "" }
            )]],
        )))
    }
}
fn flush(writer: &Output) -> SessionResult<()> {
    sql(&writer.session, "COMMIT")?;
    if let Some(message) = astersql_testkit_testfailpoint::eval_string(
        "github.com/pingcap/tidb/pkg/executor/mockCompareMaterializedViewOutputFlushError",
    ) {
        return Err(SessionError::new(
            serde_json::from_str::<String>(&message).unwrap_or(message),
        ));
    }
    Ok(())
}

fn output_create_sql(
    db: &str,
    name: &str,
    columns: &[astersql_meta_model::ColumnInfo],
    charset: &str,
    collate: &str,
) -> String {
    fn quoted(value: &str) -> String {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
    }
    let definitions = columns
        .iter()
        .map(|column| {
            let mut definition = format!("{} {}", ident(&column.Name.O), column.GetTypeDesc());
            if !column.GetCharset().is_empty() && column.GetCharset() != "binary" {
                definition.push_str(&format!(
                    " CHARACTER SET {} COLLATE {}",
                    ident(column.GetCharset()),
                    ident(column.GetCollate())
                ));
            }
            if !column.GeneratedExprString.is_empty() {
                definition.push_str(&format!(
                    " GENERATED ALWAYS AS ({}) {}",
                    column.GeneratedExprString,
                    if column.GeneratedStored {
                        "STORED"
                    } else {
                        "VIRTUAL"
                    }
                ));
            }
            if astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()) {
                definition.push_str(" NOT NULL");
            }
            if let Some(value) = column.GetDefaultValue() {
                use astersql_meta_model::DefaultValue;
                let value = match value {
                    DefaultValue::String(value) => {
                        let value = String::from_utf8_lossy(&value);
                        if column.DefaultIsExpr {
                            format!("({value})")
                        } else {
                            quoted(&value)
                        }
                    }
                    DefaultValue::Int(value) => value.to_string(),
                    DefaultValue::Uint(value) => value.to_string(),
                    DefaultValue::Float(value) => value.to_string(),
                    DefaultValue::Bool(value) => u8::from(value).to_string(),
                };
                definition.push_str(&format!(" DEFAULT {value}"));
            }
            if !column.Comment.is_empty() {
                definition.push_str(&format!(" COMMENT {}", quoted(&column.Comment)));
            }
            definition
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "CREATE TABLE {} ({definitions}) DEFAULT CHARSET={} COLLATE={}",
        qualified(db, name),
        ident(charset),
        ident(collate)
    )
}
