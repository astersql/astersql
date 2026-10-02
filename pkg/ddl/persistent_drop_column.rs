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

//! Durable DROP COLUMN state machine, including masking cleanup in the same transaction.
use crate::job_worker::JobExecutionContext;
use astersql_meta::TransactionMutator;
use astersql_meta_model::{
    ColumnInfo, SchemaState, TableInfo,
    group_3::{Job, JobState, JobVersion},
};
use astersql_util_dbterror as error;

#[allow(non_snake_case)]
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct DropColumnArgs {
    #[serde(rename = "column_info")]
    Col: Option<Box<ColumnInfo>>,
    #[serde(rename = "ignore_existence_err")]
    IgnoreExistenceErr: bool,
    #[serde(rename = "index_ids")]
    IndexIDs: Vec<i64>,
    #[serde(rename = "partition_ids")]
    PartitionIDs: Vec<i64>,
}
fn decode_args(job: &Job) -> Result<DropColumnArgs, String> {
    if job.version != JobVersion::V1 {
        return serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string());
    }
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string())?;
    let mut args = DropColumnArgs::default();
    let mut col = ColumnInfo::default();
    col.Name = serde_json::from_value(
        values
            .first()
            .cloned()
            .ok_or_else(|| "invalid drop column args".to_string())?,
    )
    .map_err(|e| e.to_string())?;
    args.Col = Some(Box::new(col));
    if let Some(v) = values.get(1) {
        args.IgnoreExistenceErr = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    }
    if let Some(v) = values.get(2) {
        args.IndexIDs = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    }
    if let Some(v) = values.get(3) {
        args.PartitionIDs = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    }
    Ok(args)
}

pub(crate) fn finished_range_ids(job: &Job) -> Result<(Vec<i64>, Vec<i64>), String> {
    let args = decode_args(job)?;
    Ok((args.IndexIDs, args.PartitionIDs))
}

fn cancel(job: &mut Job, error: impl ToString) -> String {
    job.state = JobState::Cancelled;
    error.to_string()
}

fn check_column(
    meta: &TransactionMutator<'_>,
    job: &mut Job,
    table: &TableInfo,
    name: &str,
) -> Result<(), String> {
    let col = table.Columns.iter().find(|c| c.Name.L == name).unwrap();
    if col.State == SchemaState::Public {
        if let Some(base) = table
            .MaterializedViewBase
            .as_ref()
            .filter(|b| b.MLogID != 0)
        {
            let log = meta.get_table(job.schema_id, base.MLogID)?;
            let log = log.and_then(|t| t.MaterializedViewLog).ok_or_else(|| {
                cancel(
                    job,
                    error::ErrGeneralUnsupportedDDL.GenWithStackByArgs(&[
                        "ALTER TABLE on base table with invalid materialized view log metadata"
                            .into(),
                    ]),
                )
            })?;
            if log.Columns.iter().any(|c| c.L == name) {
                return Err(cancel(job, error::ErrGeneralUnsupportedDDL.GenWithStackByArgs(&[format!("ALTER TABLE on base table column {name} referenced by materialized view log").into()])));
            }
        }
    }
    for dependent in &table.Columns {
        if dependent.IsGenerated() && dependent.Dependences.contains_key(name) {
            let e = if dependent.Hidden {
                &*error::ErrDependentByFunctionalIndex
            } else {
                &*error::ErrDependentByGeneratedColumn
            };
            return Err(cancel(
                job,
                e.GenWithStackByArgs(&[dependent.Name.O.clone().into()]),
            ));
        }
    }
    if table.Columns.len() == 1 {
        return Err(cancel(
            job,
            error::ErrCantRemoveAllFields.GenWithStackByArgs(&[]),
        ));
    }
    for index in &table.Indices {
        if (index.Primary || index.Columns.len() > 1 || index.IsColumnarIndex())
            && index.Columns.iter().any(|c| c.Name.L == name)
        {
            return Err(cancel(
                job,
                error::ErrCantDropColWithIndex.GenWithStackByArgs(&[]),
            ));
        }
    }
    for constraint in &table.Constraints {
        if constraint.ConstraintCols.len() > 1
            && constraint.ConstraintCols.iter().any(|c| c.L == name)
        {
            return Err(cancel(
                job,
                error::ErrCantDropColWithCheckConstraint.GenWithStackByArgs(&[
                    constraint.Name.O.clone().into(),
                    col.Name.O.clone().into(),
                ]),
            ));
        }
    }
    for index in &table.Indices {
        if index
            .AffectColumn
            .as_ref()
            .is_some_and(|cols| cols.iter().any(|c| c.Name.L == name))
        {
            return Err(cancel(
                job,
                error::ErrModifyColumnReferencedByPartialCondition
                    .GenWithStackByArgs(&[col.Name.O.clone().into(), index.Name.O.clone().into()]),
            ));
        }
    }
    if astersql_sessionctx_vardef::EnableForeignKey.Load() {
        for fk in &table.ForeignKeys {
            if fk.Cols.iter().any(|c| c.L == name) {
                return Err(cancel(
                    job,
                    error::ErrFkColumnCannotDrop
                        .GenWithStackByArgs(&[name.into(), fk.Name.O.clone().into()]),
                ));
            }
        }
        for db in meta.list_databases()? {
            for child in meta.list_tables(db.ID)? {
                for fk in &child.ForeignKeys {
                    if fk.RefSchema.L == job.schema_name.to_lowercase()
                        && fk.RefTable.L == table.Name.L
                        && fk.RefCols.iter().any(|c| c.L == name)
                    {
                        return Err(cancel(
                            job,
                            error::ErrFkColumnCannotDropChild.GenWithStackByArgs(&[
                                name.into(),
                                fk.Name.O.clone().into(),
                                child.Name.O.clone().into(),
                            ]),
                        ));
                    }
                }
            }
        }
    }
    if table
        .TTLInfo
        .as_ref()
        .is_some_and(|ttl| ttl.ColumnName.L == name)
    {
        return Err(cancel(
            job,
            error::ErrTTLColumnCannotDrop.GenWithStackByArgs(&[name.into()]),
        ));
    }
    Ok(())
}

fn origin_default(col: &mut ColumnInfo) -> Result<(), String> {
    use astersql_meta_model::DefaultValue;
    use astersql_parser_mysql::r#type as mysql;
    if col.GetOriginDefaultValue().is_some() || !mysql::HasNotNullFlag(col.GetFlag()) {
        return Ok(());
    }
    let mut value = col.GetDefaultValue();
    let current = matches!(&value, Some(DefaultValue::String(v)) if v == b"CURRENT_TIMESTAMP");
    if value.is_none() || (col.DefaultIsExpr && !current) {
        let zero = if col.GetType() == mysql::TypeEnum {
            col.GetElems()
                .first()
                .cloned()
                .ok_or_else(|| "invalid enum value: 1".to_string())?
        } else {
            astersql_table::column::GetZeroValue(&astersql_table::column::ToColumn(Box::new(
                col.clone(),
            )))
            .ToString()
            .map_err(|e| e.to_string())?
        };
        value = Some(DefaultValue::String(zero.into_bytes()));
    }
    if current && matches!(col.GetType(), mysql::TypeTimestamp | mysql::TypeDatetime) {
        let timestamp = if col.GetType() == mysql::TypeTimestamp {
            chrono::Utc::now()
                .format("%Y-%m-%d %H:%M:%S%.6f")
                .to_string()
        } else {
            chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.6f")
                .to_string()
        };
        let precision = col.GetDecimal().clamp(0, 6) as usize;
        let len = if precision == 0 { 19 } else { 20 + precision };
        value = Some(DefaultValue::String(timestamp[..len].as_bytes().to_vec()));
    }
    col.SetOriginDefaultValue(value).map_err(|e| e.to_string())
}

fn save_args(job: &mut Job, args: &DropColumnArgs) -> Result<(), String> {
    let value = if job.version == JobVersion::V1 {
        let col = args
            .Col
            .as_ref()
            .ok_or_else(|| "invalid drop column args".to_string())?;
        let mut values = vec![
            serde_json::to_value(&col.Name).map_err(|e| e.to_string())?,
            serde_json::json!(args.IgnoreExistenceErr),
        ];
        if !args.IndexIDs.is_empty() {
            values.extend([
                serde_json::json!(args.IndexIDs),
                serde_json::json!(args.PartitionIDs),
            ]);
        }
        serde_json::Value::Array(values)
    } else {
        serde_json::to_value(args).map_err(|e| e.to_string())?
    };
    job.raw_args = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    job.args.clear();
    Ok(())
}

pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let mut args = decode_args(job).map_err(|e| cancel(job, e))?;
    let name = args
        .Col
        .as_ref()
        .ok_or_else(|| cancel(job, "invalid drop column args"))?
        .Name
        .L
        .clone();
    let mut table = None;
    let mut version = 0;
    let mut finished = false;
    let mut dropped_id = 0;
    let mut next_state = None;
    let rollingback = job.is_rollingback();
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        let mut t = crate::persistent_actions::public_table(&meta, job)?;
        let Some(offset) = t.Columns.iter().position(|c| c.Name.L == name && !c.Hidden) else {
            let e =
                error::ErrCantDropFieldOrKey.GenWithStackByArgs(&[format!("column {name}").into()]);
            if args.IgnoreExistenceErr {
                job.warning = Some(e.to_string());
                job.state = JobState::Done;
                return Ok(Vec::new());
            }
            return Err(cancel(job, e));
        };
        check_column(&meta, job, &t, &name)?;
        dropped_id = t.Columns[offset].ID;
        if !rollingback
            && job
                .multi_schema_info
                .as_ref()
                .is_some_and(|info| info.revertible)
        {
            job.mark_non_revertible();
            job.schema_state = t.Columns[offset].State;
            version = meta.gen_schema_version()?;
            meta.set_table_schema_diff(job, version)?;
            meta.update_table(job.schema_id, &mut t)?;
            return Ok(Vec::new());
        }
        let index_ids: Vec<_> = t
            .Indices
            .iter()
            .filter(|i| i.Columns.len() == 1 && i.Columns[0].Name.L == name)
            .map(|i| i.ID)
            .collect();
        let state = t.Columns[offset].State;
        let next = match state {
            SchemaState::Public => SchemaState::WriteOnly,
            SchemaState::WriteOnly => SchemaState::DeleteOnly,
            SchemaState::DeleteOnly => SchemaState::DeleteReorganization,
            SchemaState::DeleteReorganization => SchemaState::None,
            _ => {
                return Err(error::ErrInvalidDDLJob
                    .GenWithStackByArgs(&["table".into(), t.State.to_string().into()])
                    .to_string());
            }
        };
        t.Columns[offset].State = next;
        if state == SchemaState::Public {
            for i in t.Indices.iter_mut().filter(|i| index_ids.contains(&i.ID)) {
                i.State = next;
            }
        }
        let last = t.Columns.len() - 1;
        t.MoveColumnInfo(offset, last);
        if state == SchemaState::Public {
            origin_default(&mut t.Columns[last])?;
        }
        if state == SchemaState::WriteOnly {
            t.Indices.retain(|i| !index_ids.contains(&i.ID));
            args.IndexIDs = index_ids;
            save_args(job, &args)?;
        }
        if next == SchemaState::None {
            t.Columns.pop();
            finished = true;
        }
        version = meta.gen_schema_version()?;
        meta.set_table_schema_diff(job, version)?;
        meta.update_table(job.schema_id, &mut t)?;
        next_state = Some(next);
        table = Some(t);
        Ok(Vec::new())
    })?;
    if finished {
        let t = table.unwrap();
        if !rollingback {
            let col = dropped_id;
            for policy in crate::persistent_masking_actions::policies_on_column(context, t.ID, col)?
            {
                context.query(
                    &format!(
                        "DELETE FROM mysql.tidb_masking_policy WHERE policy_id={}",
                        policy[0]
                    ),
                    "drop-masking-policy",
                )?;
            }
            args.PartitionIDs = t
                .GetPartitionInfo()
                .map(|p| p.Definitions.iter().map(|d| d.ID).collect())
                .unwrap_or_default();
            save_args(job, &args)?;
        }
        job.finish_table_job(
            if rollingback {
                JobState::RollbackDone
            } else {
                JobState::Done
            },
            SchemaState::None,
            version,
            std::sync::Arc::new(t),
        );
    }
    if let Some(next) = next_state {
        job.schema_state = next;
    }
    Ok(version)
}
