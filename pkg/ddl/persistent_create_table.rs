// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go create_table.go, on the normal worker's durable transaction.
use crate::job_worker::JobExecutionContext;
use astersql_meta::TransactionMutator;
use astersql_meta_model::{
    SchemaState, TableInfo,
    group_3::{Job, JobState, JobVersion},
};
fn cancel(job: &mut Job, error: impl ToString) -> String {
    job.state = JobState::Cancelled;
    error.to_string()
}
fn decode(job: &mut Job) -> Result<(TableInfo, bool), String> {
    let (table, fk_check) = decode_optional(job)?;
    Ok((
        table.ok_or_else(|| cancel(job, "[ddl:1105]missing create-table metadata"))?,
        fk_check,
    ))
}
pub(crate) fn decode_optional(job: &mut Job) -> Result<(Option<TableInfo>, bool), String> {
    let args = astersql_meta_model::group_2::GetCreateTableArgs(job).map_err(|e| cancel(job, e))?;
    let table = args
        .TableInfo
        .map(|t| {
            serde_json::from_value(serde_json::to_value(t).map_err(|e| cancel(job, e))?)
                .map_err(|e| cancel(job, e))
        })
        .transpose()?;
    Ok((table, args.FKCheck))
}
pub(crate) fn save_args(job: &mut Job, table: &TableInfo) -> Result<(), String> {
    let mut value: serde_json::Value =
        serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string())?;
    let encoded = serde_json::to_value(table).map_err(|e| e.to_string())?;
    if job.version == JobVersion::V1 {
        value[0] = encoded;
    } else {
        value["table_info"] = encoded;
    }
    job.raw_args = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    job.args.clear();
    Ok(())
}
pub(crate) struct Policies<'a, 'b>(pub(crate) &'a TransactionMutator<'b>);
impl astersql_ddl_placement::PolicyGetter for Policies<'_, '_> {
    fn GetPolicy(
        &self,
        id: i64,
    ) -> Result<astersql_ddl_placement::model::PolicyInfo, astersql_ddl_placement::Error> {
        self.0
            .get_placement_policy(id)
            .map_err(astersql_ddl_placement::Error::new)?
            .ok_or_else(|| {
                astersql_ddl_placement::Error::new(format!(
                    "[schema:8249]Placement policy {id} doesn't exist"
                ))
            })
    }
}
/// Go createTable: create the physical table and external resources without
/// publishing a version, notification, TTL task or terminal job state.
pub(crate) fn create_table(
    context: &mut dyn JobExecutionContext,
    job: &mut Job,
    table: &mut TableInfo,
    fk_check: bool,
) -> Result<(), String> {
    context
        .check_create_table_columnar(table)
        .map_err(|e| cancel(job, e))?;
    table.State = SchemaState::None;
    let mut bundles = Vec::new();
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        if meta.get_database(job.schema_id)?.is_none() {
            return Err(cancel(job, "[schema:1049]Unknown database ''"));
        }
        let existing = meta.list_tables(job.schema_id)?;
        if existing.iter().any(|t| t.Name.L == table.Name.L) {
            return Err(cancel(
                job,
                format!("[schema:1050]Table '{}' already exists", table.Name.O),
            ));
        }
        for t in &existing {
            for c in &table.Constraints {
                if c.State != SchemaState::WriteOnly
                    && t.Constraints.iter().any(|old| old.Name.L == c.Name.L)
                {
                    return Err(cancel(
                        job,
                        format!(
                            "[schema:3822]Duplicate check constraint name '{}'",
                            c.Name.L
                        ),
                    ));
                }
            }
        }
        check_foreign_keys(&meta, job, table, fk_check).map_err(|e| cancel(job, e))?;
        for fk in &mut table.ForeignKeys {
            table.MaxForeignKeyID += 1;
            fk.ID = table.MaxForeignKeyID;
            fk.State = SchemaState::Public;
        }
        table.State = SchemaState::Public;
        table.UpdateTS = meta.start_ts();
        crate::create_table::check_table_info_valid(table).map_err(|e| cancel(job, e))?;
        meta.create_table(job.schema_id, table)?;
        for p in table
            .PlacementPolicyRef
            .iter()
            .chain(table.Partition.iter().flat_map(|p| {
                p.Definitions
                    .iter()
                    .filter_map(|d| d.PlacementPolicyRef.as_ref())
            }))
        {
            if meta.get_placement_policy(p.ID)?.is_none() {
                return Err(cancel(
                    job,
                    format!("[schema:8249]Placement policy {} doesn't exist", p.ID),
                ));
            }
        }
        Ok(Vec::new())
    })?;
    if table.TiFlashReplica.is_some() {
        context
            .configure_create_table_replica(table)
            .map_err(|e| cancel(job, e))?;
    }
    context.with_transaction(&mut |txn| {
        let meta = TransactionMutator::new(txn);
        let projected =
            serde_json::from_value(serde_json::to_value(&*table).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        bundles = astersql_ddl_placement::NewFullTableBundles(&Policies(&meta), &projected)
            .map_err(|e| cancel(job, e))?;
        Ok(Vec::new())
    })?;
    if !bundles.is_empty() {
        context
            .put_create_table_bundles(&bundles)
            .map_err(|e| cancel(job, format!("failed to notify PD the placement rules: {e}")))?;
    }
    if table.Affinity.is_some() {
        context.create_table_affinity(table).map_err(|e| {
            cancel(
                job,
                format!("failed to create table affinity groups in PD: {e}"),
            )
        })?;
    }
    context.rebase_create_table_ids(job.schema_id, table)?;
    Ok(())
}
pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let (mut table, fk_check) = decode(job)?;
    let has_fk = !table.ForeignKeys.is_empty();
    let mut version = 0;
    if !has_fk || matches!(table.State, SchemaState::None | SchemaState::Public) {
        create_table(context, job, &mut table, fk_check)?;
        context.with_transaction(&mut |txn| {
            let mut meta = TransactionMutator::new(txn);
            if has_fk {
                table.State = SchemaState::DeleteOnly;
                version = crate::persistent_actions::update_version_and_table(
                    &mut meta, job, &mut table,
                )?;
                job.schema_state = SchemaState::DeleteOnly;
            } else {
                version = meta.gen_schema_version()?;
                meta.set_table_schema_diff(job, version)?;
            }
            Ok(Vec::new())
        })?;
    } else if matches!(
        table.State,
        SchemaState::DeleteOnly | SchemaState::WriteOnly
    ) {
        table.State = SchemaState::Public;
        context.with_transaction(&mut |txn| {
            version = crate::persistent_actions::update_version_and_table(
                &mut TransactionMutator::new(txn),
                job,
                &mut table,
            )?;
            Ok(Vec::new())
        })?;
    } else {
        return Err(astersql_util_dbterror::ErrInvalidDDLJob.to_string());
    }
    context.with_transaction(&mut |txn| {
        TransactionMutator::new(txn).set_create_table_schema_diff(
            job,
            version,
            has_fk && table.State == SchemaState::Public,
        )?;
        Ok(Vec::new())
    })?;
    if !has_fk || table.State == SchemaState::Public {
        crate::persistent_actions::async_notify_event(
            context,
            job,
            -1,
            astersql_ddl_notifier::NewCreateTableEvent(Some(Box::new(table.clone()))),
        )?;
        context
            .register_create_table_ttl(&table)
            .map_err(|e| cancel(job, e))?;
        job.finish_table_job(
            JobState::Done,
            SchemaState::Public,
            version,
            std::sync::Arc::new(table.clone()),
        );
    }
    save_args(job, &table)?;
    Ok(version)
}
fn check_foreign_keys(
    meta: &TransactionMutator<'_>,
    job: &Job,
    table: &TableInfo,
    fk_check: bool,
) -> Result<(), String> {
    if !astersql_sessionctx_vardef::EnableForeignKey.Load() {
        return Ok(());
    }
    let dbs = meta.list_databases()?;
    let mut catalog = Vec::new();
    for db in &dbs {
        for t in meta.list_tables(db.ID)? {
            if t.State == SchemaState::Public {
                catalog.push((db.Name.L.clone(), t));
            }
        }
    }
    for fk in &table.ForeignKeys {
        if fk.Version < 1 {
            continue;
        }
        let parent = if fk.RefSchema.L == job.schema_name && fk.RefTable.L == table.Name.L {
            Some(table)
        } else {
            catalog
                .iter()
                .find(|(db, t)| *db == fk.RefSchema.L && t.Name.L == fk.RefTable.L)
                .map(|(_, t)| t)
        };
        match parent {
            Some(p) => check_foreign_key(p, table, fk)?,
            None if !fk_check => {}
            None => {
                if !dbs.iter().any(|d| d.Name.L == fk.RefSchema.L) {
                    return Err(format!(
                        "[schema:1049]Unknown database '{}'",
                        fk.RefSchema.O
                    ));
                }
                return Err(format!(
                    "[schema:1146]Table '{}.{}' doesn't exist",
                    fk.RefSchema.O, fk.RefTable.O
                ));
            }
        }
    }
    for (_, child) in &catalog {
        for fk in &child.ForeignKeys {
            if fk.Version >= 1 && fk.RefSchema.L == job.schema_name && fk.RefTable.L == table.Name.L
            {
                check_foreign_key(table, child, fk)?;
            }
        }
    }
    Ok(())
}
fn check_foreign_key(
    parent: &TableInfo,
    child: &TableInfo,
    fk: &astersql_meta_model::FKInfo,
) -> Result<(), String> {
    use astersql_meta_model::mysql;
    if parent.TempTableType != astersql_meta_model::TempTableNone
        || child.TempTableType != astersql_meta_model::TempTableNone
    {
        return Err("[schema:1215]Cannot add foreign key constraint".into());
    }
    if parent.TTLInfo.is_some() {
        return Err(astersql_util_dbterror::ErrUnsupportedTTLReferencedByFK.to_string());
    }
    if parent.GetPartitionInfo().is_some() || child.GetPartitionInfo().is_some() {
        return Err(
            "[schema:1506]Foreign key clause is not yet supported in conjunction with partitioning"
                .into(),
        );
    }
    for (i, name) in fk.RefCols.iter().enumerate() {
        let reference=parent.Columns.iter().find(|c|c.Name.L==name.L).ok_or_else(||format!("[schema:3734]Failed to add the foreign key constraint. Missing column '{}' for constraint '{}' in the referenced table '{}'",name.O,fk.Name.O,fk.RefTable.O))?;
        if reference.IsVirtualGenerated() {
            return Err(format!(
                "[schema:3733]Foreign key '{}' cannot use virtual column '{}'",
                fk.Name.O, name.O
            ));
        }
        let child_name = fk
            .Cols
            .get(i)
            .ok_or("[ddl:1105]foreign key column count mismatch")?;
        let column = child
            .Columns
            .iter()
            .find(|c| c.Name.L == child_name.L)
            .ok_or_else(|| {
                format!(
                    "[ddl:1072]Key column '{}' doesn't exist in table",
                    child_name.O
                )
            })?;
        if column.GetType() != reference.GetType()
            || mysql::HasUnsignedFlag(column.GetFlag())
                != mysql::HasUnsignedFlag(reference.GetFlag())
            || column.GetCharset() != reference.GetCharset()
            || column.GetCollate() != reference.GetCollate()
        {
            return Err(format!(
                "[ddl:3780]Referencing column '{}' and referenced column '{}' in foreign key constraint '{}' are incompatible",
                column.Name.O, reference.Name.O, fk.Name.O
            ));
        }
        if fk.RefCols.len() == 1 && mysql::HasPriKeyFlag(reference.GetFlag()) && parent.PKIsHandle {
            return Ok(());
        }
    }
    if astersql_meta_model::FindIndexByColumnsForForeignKey(parent, &parent.Indices, &fk.RefCols)
        .is_none()
    {
        return Err(format!(
            "[schema:1822]Failed to add the foreign key constraint. Missing index for constraint '{}' in the referenced table '{}'",
            fk.Name.O, fk.RefTable.O
        ));
    }
    Ok(())
}

/// Go onCreateTables: reuse the physical-table path, publish one schema version,
/// then notify/register all tables. A controller failure compensates in reverse.
pub fn batch_step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let args =
        astersql_meta_model::group_2::GetBatchCreateTableArgs(job).map_err(|e| cancel(job, e))?;
    let mut tables = Vec::with_capacity(args.Tables.len());
    let mut stub = job.clone_job().map_err(|e| cancel(job, e))?;
    for args in args.Tables {
        let info = args
            .TableInfo
            .ok_or_else(|| cancel(job, "missing batch table metadata"))?;
        let mut table: TableInfo =
            serde_json::from_value(serde_json::to_value(info).map_err(|e| cancel(job, e))?)
                .map_err(|e| cancel(job, e))?;
        stub.table_id = table.ID;
        create_table(context, &mut stub, &mut table, args.FKCheck).map_err(|e| cancel(job, e))?;
        tables.push(table);
    }
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        version = TransactionMutator::new(txn).gen_schema_version()?;
        let options = tables
            .iter()
            .map(|table| {
                serde_json::json!({
                    "schema_id": job.schema_id,
                    "old_schema_id": job.schema_id,
                    "table_id": table.ID,
                    "old_table_id": table.ID,
                })
            })
            .collect::<Vec<_>>();
        let diff = serde_json::json!({
            "version": version,
            "type": job.tp,
            "schema_id": job.schema_id,
            "table_id": job.table_id,
            "old_table_id": 0,
            "old_schema_id": 0,
            "regenerate_schema_map": false,
            "affected_options": options,
        });
        txn.Set(
            astersql_meta::transaction_meta_string_key(format!("Diff:{version}").as_bytes()),
            serde_json::to_vec(&diff).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(Vec::new())
    })?;
    for (i, table) in tables.iter().enumerate() {
        crate::persistent_actions::async_notify_event(
            context,
            job,
            i as i64,
            astersql_ddl_notifier::NewCreateTableEvent(Some(Box::new(table.clone()))),
        )?;
    }
    let mut registered: Vec<i64> = Vec::new();
    for table in &tables {
        if let Err(error) = context.register_create_table_ttl(table) {
            for id in registered.into_iter().rev() {
                if let Err(compensation) = context.delete_drop_table_ttl(id) {
                    astersql_util_logutil::log::background_logger().warn(format!(
                        "failed to roll back TTL table registration tableID={id}: {compensation}"
                    ));
                }
            }
            return Err(cancel(job, error));
        }
        if table.TTLInfo.as_ref().is_some_and(|ttl| ttl.Enable) {
            registered.push(table.ID);
        }
    }
    job.finish_multiple_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        tables.into_iter().map(std::sync::Arc::new).collect(),
    );
    Ok(version)
}
