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

//! Masking system-table operations in the durable worker transaction.
use crate::job_worker::JobExecutionContext;
use astersql_meta::TransactionMutator;
use astersql_meta_model::{
    SchemaState,
    group_3::{Job, JobState},
};

const SELECT_POLICY: &str = "SELECT policy_id, policy_name, db_name, table_name, table_id, column_name, column_id, expression, status, masking_type, restrict_on, created_at, updated_at, created_by FROM mysql.tidb_masking_policy";

/// Read every field before mutation, preserving Go's malformed-row failure order.
pub fn policies_on_table(
    context: &mut dyn JobExecutionContext,
    table: i64,
) -> Result<Vec<Vec<String>>, String> {
    read_policies(context, &format!("table_id={table}"))
}

/// Read policies for one column; malformed policies on other columns are unrelated.
pub fn policies_on_column(
    context: &mut dyn JobExecutionContext,
    table: i64,
    column: i64,
) -> Result<Vec<Vec<String>>, String> {
    read_policies(context, &format!("table_id={table} AND column_id={column}"))
}

fn read_policies(
    context: &mut dyn JobExecutionContext,
    predicate: &str,
) -> Result<Vec<Vec<String>>, String> {
    let rows = context
        .query(
            &format!("{SELECT_POLICY} WHERE {predicate} ORDER BY policy_id"),
            "query-masking-policy",
        )
        .map_err(|error| {
            if error == "Table 'mysql.tidb_masking_policy' doesn't exist" {
                format!("[schema:1146]{error}")
            } else {
                error
            }
        })?;
    for row in &rows {
        if row.len() != 14 {
            return Err("invalid masking policy row length".into());
        }
        let status = row[8].trim().to_uppercase();
        if !matches!(
            status.as_str(),
            "ENABLE" | "ENABLED" | "DISABLE" | "DISABLED"
        ) {
            return Err(format!("unknown masking policy status: {}", row[8]));
        }
        for token in row[10].trim().to_uppercase().split(',') {
            if !matches!(
                token.trim(),
                "" | "NONE" | "INSERT_INTO_SELECT" | "UPDATE_SELECT" | "DELETE_SELECT" | "CTAS"
            ) {
                return Err(format!("unknown masking policy restrict option: {token}"));
            }
        }
        for index in [11, 12] {
            let parsed =
                astersql_types::time::ParseDatetime(&*astersql_types::StrictContext, &row[index])
                    .map_err(|e| e.to_string())?;
            parsed
                .GoTime(astersql_types::StrictContext.Location())
                .map_err(|e| e.to_string())?;
        }
        for index in [0, 4, 6] {
            row[index].parse::<i64>().map_err(|e| e.to_string())?;
        }
    }
    Ok(rows)
}
pub fn drop_policies_on_table(
    context: &mut dyn JobExecutionContext,
    table: i64,
) -> Result<(), String> {
    for policy in policies_on_table(context, table)? {
        context.query(
            &format!(
                "DELETE FROM mysql.tidb_masking_policy WHERE policy_id={}",
                policy[0].parse::<i64>().map_err(|e| e.to_string())?
            ),
            "drop-masking-policy",
        )?;
    }
    Ok(())
}

pub fn drop_table(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let args = astersql_meta_model::group_2::GetDropTableArgs(job).map_err(|e| {
        job.state = JobState::Cancelled;
        e.to_string()
    })?;
    let mut table = None;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta=TransactionMutator::new(txn);
        if meta.get_database(job.schema_id)?.is_none() {
            job.state=JobState::Cancelled;
            return Err(format!("[schema:1049]Unknown database '(Schema ID {})'",job.schema_id));
        }
        let mut t=meta.get_table(job.schema_id,job.table_id)?.ok_or_else(|| {
            job.state=JobState::Cancelled;
            format!("[schema:1146]Table '(Schema ID {}).(Table ID {})' doesn't exist",job.schema_id,job.table_id)
        })?;
        match t.State {
            SchemaState::Public => {
                let kind=if t.MaterializedViewLog.is_some() {Some("materialized view log table")}
                    else if t.MaterializedView.is_some() {Some("materialized view table")}
                    else if t.MaterializedViewShadow.is_some() {Some("materialized view shadow table")}
                    else if t.MaterializedViewBase.as_ref().is_some_and(|b| !b.MViewIDs.is_empty()) {Some("base table with materialized view dependencies")}
                    else if t.MaterializedViewBase.as_ref().is_some_and(|b| b.MLogID!=0) {Some("base table with materialized view log")} else {None};
                if let Some(kind)=kind.filter(|_| job.tp == astersql_meta_model::group_3::ACTION_DROP_TABLE) {
                    job.state=JobState::Cancelled;
                    return Err(format!("[ddl:8200]Unsupported DDL operation: DROP TABLE on {kind}"));
                }
                if job.tp == astersql_meta_model::group_3::ACTION_DROP_MATERIALIZED_VIEW
                    && t.MaterializedView.is_none()
                {
                    job.state = JobState::Cancelled;
                    return Err(format!(
                        "[ddl:1347]'{}' is not MATERIALIZED VIEW",
                        t.Name.O
                    ));
                }
                if job.tp
                    == astersql_meta_model::group_3::ACTION_DROP_MATERIALIZED_VIEW_SHADOW
                    && t.MaterializedViewShadow.is_none()
                {
                    job.state = JobState::Cancelled;
                    return Err(format!(
                        "[ddl:1347]'{}' is not MATERIALIZED VIEW SHADOW TABLE",
                        t.Name.O
                    ));
                }
                if job.tp == astersql_meta_model::group_3::ACTION_DROP_MATERIALIZED_VIEW_LOG {
                    let Some(log) = t.MaterializedViewLog.as_ref() else {
                        job.state = JobState::Cancelled;
                        return Err(format!(
                            "[ddl:1347]'{}' is not MATERIALIZED VIEW LOG",
                            t.Name.O
                        ));
                    };
                    if !log.DependentMViewIDs.is_empty() {
                        let base = meta.get_table(job.schema_id, log.BaseTableID)?;
                        let base_name = base.map_or_else(
                            || format!("(Table ID {})", log.BaseTableID),
                            |base| base.Name.O,
                        );
                        job.state = JobState::Cancelled;
                        return Err(format!(
                            "cannot drop materialized view log on {}.{}: dependent materialized views exist",
                            job.schema_name, base_name
                        ));
                    }
                }
                if astersql_sessionctx_vardef::EnableForeignKey.Load() && args.FKCheck {
                    for db in meta.list_databases()? {
                        for child in meta.list_tables(db.ID)? {
                            if args.Identifiers.iter().any(|i| i.Schema.L==db.Name.L && i.Name.L==child.Name.L) {continue;}
                            for fk in &child.ForeignKeys {
                                if fk.Version>=1 && fk.RefSchema.L==job.schema_name && fk.RefTable.L==t.Name.L {
                                    job.state=JobState::Cancelled;
                                    return Err(format!("[ddl:1701]Cannot truncate a table referenced in a foreign key constraint (`{}`.`{}` CONSTRAINT `{}`)",db.Name.O,child.Name.O,fk.Name.O));
                                }
                            }
                        }
                    }
                }
                t.State=SchemaState::WriteOnly;
            }
            SchemaState::WriteOnly => t.State=SchemaState::DeleteOnly,
            SchemaState::DeleteOnly => t.State=SchemaState::None,
            state => return Err(format!("[ddl:8210]Invalid table state: {state:?}")),
        }
        table=Some(t);
        Ok(Vec::new())
    })?;
    let mut t = table.unwrap();
    if t.State == SchemaState::WriteOnly && t.TTLInfo.is_some() {
        context.delete_drop_table_ttl(t.ID).map_err(|error| {
            job.state = JobState::Cancelled;
            error
        })?;
    }
    let mut affected = Vec::new();
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.update_table(job.schema_id, &mut t)?;
        if t.State == SchemaState::None {
            affected = update_materialized_view_dependencies(&mut meta, job, &t)?;
            meta.drop_table_and_auto_ids(job.schema_id, job.table_id)?;
        }
        if affected.is_empty() {
            meta.set_table_schema_diff(job, version)?;
        } else {
            meta.set_drop_mview_schema_diff(job, version, &affected)?;
        }
        Ok(Vec::new())
    })?;
    if t.State == SchemaState::None {
        if t.MaterializedView.is_some() {
            context.query(
                &format!(
                    "DELETE FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",
                    t.ID
                ),
                "mview-refresh-info-delete",
            )?;
            let _ = context.query(
                &format!(
                    "DELETE FROM mysql.tidb_mview_refresh_alert WHERE MVIEW_ID={}",
                    t.ID
                ),
                "mview-refresh-alert-delete",
            );
        }
        if t.MaterializedViewLog.is_some() {
            context.query(
                &format!(
                    "DELETE FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",
                    t.ID
                ),
                "mlog-purge-info-delete",
            )?;
        }
        if t.TiFlashReplica.is_some() {
            let _ = context.cleanup_drop_table_resources(&t);
        }
        crate::persistent_actions::async_notify_event(
            context,
            job,
            -1,
            astersql_ddl_notifier::NewDropTableEvent(Some(Box::new(t.clone()))),
        )?;
        let _ = context.delete_drop_table_affinity(&t);
        drop_policies_on_table(context, t.ID).map_err(|e| {
            if let Some((prefix, message)) = e.strip_prefix('[').and_then(|s| s.split_once(']')) {
                format!(
                    "[{prefix}]failed to drop masking policies on table {}: {message}",
                    t.ID
                )
            } else {
                format!("failed to drop masking policies on table {}: {e}", t.ID)
            }
        })?;
        let partitions = t
            .GetPartitionInfo()
            .map(|p| p.Definitions.iter().map(|d| d.ID).collect())
            .unwrap_or_default();
        let rules = context.drop_table_rule_ids(&job.schema_name, &t)?;
        job.finish_table_job(
            JobState::Done,
            SchemaState::None,
            version,
            std::sync::Arc::new(t.clone()),
        );
        let finished = astersql_meta_model::group_2::DropTableArgs {
            StartKey: astersql_tablecodec::EncodeTablePrefix(job.table_id).0,
            OldPartitionIDs: partitions,
            OldRuleIDs: rules,
            ..Default::default()
        };
        job.raw_args = serde_json::to_vec(&if job.version
            == astersql_meta_model::group_3::JobVersion::V1
        {
            use astersql_meta_model::group_2::FinishedJobArgs;
            serde_json::Value::Array(finished.getFinishedArgsV1(job))
        } else {
            serde_json::to_value(&finished).map_err(|e| e.to_string())?
        })
        .map_err(|e| e.to_string())?;
    }
    job.schema_state = t.State;
    Ok(version)
}

fn update_materialized_view_dependencies(
    meta: &mut TransactionMutator<'_>,
    job: &Job,
    dropping: &astersql_meta_model::TableInfo,
) -> Result<Vec<i64>, String> {
    let mut affected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(view) = &dropping.MaterializedView {
        for base_id in &view.BaseTableIDs {
            if !seen.insert(*base_id) {
                continue;
            }
            let Some(mut base) = meta.get_table(job.schema_id, *base_id)? else {
                continue;
            };
            let log_id = base
                .MaterializedViewBase
                .as_ref()
                .map_or(0, |info| info.MLogID);
            if let Some(info) = base.MaterializedViewBase.as_mut() {
                info.MViewIDs.retain(|id| *id != job.table_id);
                if info.MLogID == 0 && info.MViewIDs.is_empty() {
                    base.MaterializedViewBase = None;
                }
            }
            meta.update_table(job.schema_id, &mut base)?;
            affected.push(*base_id);
            if log_id == 0 {
                continue;
            }
            let Some(mut log) = meta.get_table(job.schema_id, log_id)? else {
                continue;
            };
            let Some(info) = log.MaterializedViewLog.as_mut() else {
                continue;
            };
            if info.BaseTableID != *base_id {
                continue;
            }
            let before = info.DependentMViewIDs.len();
            info.DependentMViewIDs.retain(|id| *id != job.table_id);
            if info.DependentMViewIDs.len() != before {
                meta.update_table(job.schema_id, &mut log)?;
                affected.push(log_id);
            }
        }
    } else if let Some(log) = &dropping.MaterializedViewLog {
        if let Some(mut base) = meta.get_table(job.schema_id, log.BaseTableID)? {
            if let Some(info) = base.MaterializedViewBase.as_mut() {
                if info.MLogID == job.table_id {
                    info.MLogID = 0;
                }
                if info.MLogID == 0 && info.MViewIDs.is_empty() {
                    base.MaterializedViewBase = None;
                }
            }
            meta.update_table(job.schema_id, &mut base)?;
            affected.push(log.BaseTableID);
        }
    }
    Ok(affected)
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}
fn update_policy_names(
    context: &mut dyn JobExecutionContext,
    id: i64,
    schema: &str,
    table: &str,
) -> Result<(), String> {
    for policy in policies_on_table(context, id)? {
        if policy[2].to_lowercase() == schema.to_lowercase()
            && policy[3].to_lowercase() == table.to_lowercase()
        {
            continue;
        }
        let now = context.masking_policy_timestamp()?;
        context.query(&format!("UPDATE mysql.tidb_masking_policy SET db_name={},table_name={},updated_at={} WHERE policy_id={}",sql_string(schema),sql_string(table),sql_string(&now),policy[0]),"update-masking-policy-names")?;
    }
    Ok(())
}

pub fn rename_tables(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    use astersql_meta_model::group_2::{GetRenameTableArgs, GetRenameTablesArgs};
    let infos = if job.tp == 14 {
        let mut info = GetRenameTableArgs(job).map_err(|e| {
            job.state = JobState::Cancelled;
            e.to_string()
        })?;
        info.TableID = job.table_id;
        vec![info]
    } else {
        GetRenameTablesArgs(job)
            .map_err(|e| {
                job.state = JobState::Cancelled;
                e.to_string()
            })?
            .RenameTableInfos
    };
    let finishing = job.schema_state == SchemaState::Public;
    let mut finished = Vec::new();
    let mut affected = Vec::new();
    for info in &infos {
        if job.tp == 47 {
            job.table_id = info.TableID;
            job.table_name = info.OldTableName.L.clone();
        }
        let mut table = None;
        let mut database = None;
        let mut old_table_name = String::new();
        context.with_transaction(&mut |txn| {
            let mut meta = TransactionMutator::new(txn);
            let new_db = meta.get_database(info.NewSchemaID)?.ok_or_else(|| {
                job.state = JobState::Cancelled;
                format!(
                    "[schema:1049]Unknown database 'schema-ID: {}'",
                    info.NewSchemaID
                )
            })?;
            if !finishing
                && meta
                    .list_tables(info.NewSchemaID)?
                    .iter()
                    .any(|t| t.Name.L == info.NewTableName.L)
            {
                job.state = JobState::Cancelled;
                return Err(format!(
                    "[schema:1050]Table '{}' already exists",
                    info.NewTableName.O
                ));
            }
            let old_db = if finishing {
                info.NewSchemaID
            } else {
                info.OldSchemaID
            };
            let mut t = meta.get_table(old_db, info.TableID)?.ok_or_else(|| {
                job.state = JobState::Cancelled;
                format!(
                    "[schema:1146]Table '(Schema ID {}).(Table ID {})' doesn't exist",
                    old_db, info.TableID
                )
            })?;
            if !finishing && t.State != SchemaState::Public {
                job.state = JobState::Cancelled;
                return Err(format!(
                    "[ddl:8210]table {} is not in public, but {:?}",
                    t.Name.O, t.State
                ));
            }
            old_table_name = t.Name.L.clone();
            if !finishing {
                // Rename preserves all auto-ID fields in the original schema.
                txn_delete_table(&mut meta, old_db, t.ID)?;
                if t.AutoIDSchemaID == 0 && info.NewSchemaID != old_db {
                    t.AutoIDSchemaID = old_db;
                }
                if info.NewSchemaID == t.AutoIDSchemaID {
                    t.AutoIDSchemaID = 0;
                }
                t.Name = astersql_meta_model::ast::CIStr {
                    O: info.NewTableName.O.clone(),
                    L: info.NewTableName.L.clone(),
                };
                meta.create_table(info.NewSchemaID, &t)?;
                if astersql_sessionctx_vardef::EnableForeignKey.Load()
                    && old_table_name != info.NewTableName.L
                {
                    for db in meta.list_databases()? {
                        for mut child in meta.list_tables(db.ID)? {
                            let mut changed = false;
                            for fk in &mut child.ForeignKeys {
                                if fk.Version >= 1
                                    && fk.RefSchema.L == info.OldSchemaName.L
                                    && fk.RefTable.L == old_table_name
                                {
                                    fk.RefSchema = new_db.Name.clone();
                                    fk.RefTable = t.Name.clone();
                                    changed = true;
                                }
                            }
                            if changed {
                                meta.update_table(db.ID, &mut child)?;
                                affected.push((db.ID, child.ID));
                            }
                        }
                    }
                }
            }
            table = Some(t);
            database = Some(new_db);
            Ok(Vec::new())
        })?;
        let table = table.unwrap();
        if !finishing {
            let old_name = old_table_name.as_str();
            context
                .update_table_labels(
                    &info.OldSchemaName.L,
                    old_name,
                    &database.as_ref().unwrap().Name.L,
                    &table,
                    true,
                )
                .map_err(|e| {
                    job.state = JobState::Cancelled;
                    format!("failed to update the label rule to PD: {e}")
                })?;
            update_policy_names(context, table.ID, &database.unwrap().Name.O, &table.Name.O)?;
        }
        finished.push(std::sync::Arc::new(table));
    }
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        version=TransactionMutator::new(txn).gen_schema_version()?;
        let mut options:Vec<serde_json::Value>=infos.iter().skip(1).map(|i|serde_json::json!({"schema_id":i.NewSchemaID,"old_schema_id":if finishing{i.NewSchemaID}else{i.OldSchemaID},"table_id":i.TableID,"old_table_id":i.TableID})).collect();
        options.extend(affected.iter().map(|(db,id)|serde_json::json!({"schema_id":db,"old_schema_id":db,"table_id":id,"old_table_id":id})));
        let diff=serde_json::json!({"version":version,"type":job.tp,"schema_id":infos[0].NewSchemaID,"table_id":infos[0].TableID,"old_table_id":0,"old_schema_id":if finishing{infos[0].NewSchemaID}else{infos[0].OldSchemaID},"regenerate_schema_map":false,"affected_options":if job.tp==47 || !affected.is_empty(){serde_json::json!(options)}else{serde_json::Value::Null}});
        txn.Set(astersql_meta::transaction_meta_string_key(format!("Diff:{version}").as_bytes()),serde_json::to_vec(&diff).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        Ok(Vec::new())
    })?;
    job.schema_state = SchemaState::Public;
    if finishing {
        if job.tp == 14 {
            job.finish_table_job(
                JobState::Done,
                SchemaState::Public,
                version,
                finished.pop().unwrap(),
            );
        } else {
            job.finish_multiple_table_job(JobState::Done, SchemaState::Public, version, finished);
        }
    }
    Ok(version)
}
fn txn_delete_table(meta: &mut TransactionMutator<'_>, db: i64, id: i64) -> Result<(), String> {
    meta.drop_table_only(db, id)
}

pub fn truncate_table(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let mut args = astersql_meta_model::group_2::GetTruncateTableArgs(job).map_err(|e| {
        job.state = JobState::Cancelled;
        e.to_string()
    })?;
    let mut table = None;
    context.with_transaction(&mut |txn| {
        let meta=TransactionMutator::new(txn);
        let t=crate::persistent_actions::public_table(&meta,job)?;
        if t.IsView() || t.IsSequence() {job.state=JobState::Cancelled;return Err(format!("[schema:1146]Table '{}.{}' doesn't exist",job.schema_name,t.Name.O));}
        check_materialized_view(&t,"TRUNCATE TABLE").map_err(|e|{job.state=JobState::Cancelled;e})?;
        if astersql_sessionctx_vardef::EnableForeignKey.Load() && args.FKCheck {
            for db in meta.list_databases()? {
                for child in meta.list_tables(db.ID)? {
                    if child.ID==t.ID && db.ID==job.schema_id {continue;}
                    for fk in child.ForeignKeys {
                        if fk.Version>=1 && fk.RefSchema.L==job.schema_name && fk.RefTable.L==t.Name.L {
                            job.state=JobState::Cancelled;
                            return Err(format!("[ddl:1701]Cannot truncate a table referenced in a foreign key constraint (`{}`.`{}` CONSTRAINT `{}`)",db.Name.O,child.Name.O,fk.Name.O));
                        }
                    }
                }
            }
        }
        table=Some(t);
        Ok(Vec::new())
    })?;
    let old = table.unwrap();
    let mut t = old.clone();
    if t.TTLInfo.is_some() {
        context.delete_drop_table_ttl(t.ID).map_err(|e| {
            job.state = JobState::Cancelled;
            e
        })?;
        if t.TTLInfo.as_ref().is_some_and(|ttl| ttl.Enable) {
            let mut new = t.clone();
            new.ID = args.NewTableID;
            if let Err(error) = context.register_create_table_ttl(&new) {
                if let Err(compensation) = context.register_create_table_ttl(&old) {
                    astersql_util_logutil::log::background_logger().warn(format!("truncate_ttl_restore_old_registration_failed oldTableID={} newTableID={} compensation={compensation} registerNewTableErr={error}",old.ID,new.ID));
                }
                job.state = JobState::Cancelled;
                return Err(error);
            }
        }
    }
    context.with_transaction(&mut |txn| {
        TransactionMutator::new(txn)
            .drop_table_and_auto_ids(job.schema_id, job.table_id)
            .map_err(|e| {
                job.state = JobState::Cancelled;
                e
            })?;
        Ok(Vec::new())
    })?;
    if t.TiFlashReplica.is_some() {
        let _ = context.cleanup_drop_table_resources(&t);
    }
    context.with_transaction(&mut |txn| {
        if let Some(partition) = t.Partition.as_mut().filter(|p| p.Enable) {
            args.OldPartitionIDs = partition.Definitions.iter().map(|d| d.ID).collect();
            while args.NewPartitionIDs.len() < partition.Definitions.len() {
                args.NewPartitionIDs.push(
                    astersql_kv::IncInt64(
                        txn,
                        &astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
                        1,
                    )
                    .map_err(|e| e.to_string())?,
                );
            }
            for (index, def) in partition.Definitions.iter_mut().enumerate() {
                if def.PlacementPolicyRef.is_some() {
                    args.OldPartIDsWithPolicy.push(def.ID);
                    args.NewPartIDsWithPolicy.push(args.NewPartitionIDs[index]);
                }
                def.ID = args.NewPartitionIDs[index];
            }
        }
        Ok(Vec::new())
    })?;
    let mut label_table = t.clone();
    label_table.ID = args.NewTableID;
    context
        .update_table_labels(
            &job.schema_name,
            &t.Name.L,
            &job.schema_name,
            &label_table,
            false,
        )
        .map_err(|e| {
            job.state = JobState::Cancelled;
            format!("failed to update the label rule to PD: {e}")
        })?;
    if let Some(replica) = t.TiFlashReplica.as_mut() {
        let mut replica_table = label_table.clone();
        replica_table.TiFlashReplica = Some(replica.clone());
        context
            .configure_create_table_replica(&replica_table)
            .map_err(|e| {
                job.state = JobState::Cancelled;
                e
            })?;
        replica.Available = false;
        replica.AvailablePartitionIDs.clear();
    }
    t.ID = args.NewTableID;
    let policies = policies_on_table(context, old.ID).map_err(|e| {
        job.state = JobState::Cancelled;
        e
    })?;
    let now = context.masking_policy_timestamp()?;
    for policy in policies {
        context.query(&format!("UPDATE mysql.tidb_masking_policy SET table_id={},updated_at={} WHERE policy_id={}",t.ID,sql_string(&now),policy[0]),"update-masking-policy-table-id").map_err(|e|{job.state=JobState::Cancelled;e})?;
    }
    let mut bundles = Vec::new();
    context.with_transaction(&mut |txn| {
        let meta = TransactionMutator::new(txn);
        let projected =
            serde_json::from_value(serde_json::to_value(&t).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        bundles = astersql_ddl_placement::NewFullTableBundles(
            &crate::persistent_create_table::Policies(&meta),
            &projected,
        )
        .map_err(|e| {
            job.state = JobState::Cancelled;
            e.to_string()
        })?;
        Ok(Vec::new())
    })?;
    if !bundles.is_empty() {
        context.put_create_table_bundles(&bundles).map_err(|e| {
            job.state = JobState::Cancelled;
            format!("failed to notify PD the placement rules: {e}")
        })?;
    }
    context.with_transaction(&mut |txn| {
        TransactionMutator::new(txn)
            .create_table(job.schema_id, &t)
            .map_err(|e| {
                job.state = JobState::Cancelled;
                e
            })?;
        Ok(Vec::new())
    })?;
    if t.Affinity.is_some() {
        context.create_table_affinity(&t).map_err(|e| {
            job.state = JobState::Cancelled;
            e
        })?;
    }
    if old.Affinity.is_some() {
        let _ = context.delete_drop_table_affinity(&old);
    }
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        version=TransactionMutator::new(txn).gen_schema_version()?;
        let options:Vec<_>=args.OldPartIDsWithPolicy.iter().zip(&args.NewPartIDsWithPolicy).map(|(old,new)|serde_json::json!({"schema_id":0,"old_schema_id":0,"table_id":new,"old_table_id":old})).collect();
        let diff=serde_json::json!({"version":version,"type":job.tp,"schema_id":job.schema_id,"table_id":t.ID,"old_table_id":job.table_id,"old_schema_id":0,"regenerate_schema_map":false,"affected_options":if options.is_empty(){serde_json::Value::Null}else{serde_json::json!(options)}});
        txn.Set(astersql_meta::transaction_meta_string_key(format!("Diff:{version}").as_bytes()),serde_json::to_vec(&diff).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        Ok(Vec::new())
    })?;
    crate::persistent_actions::async_notify_event(
        context,
        job,
        -1,
        astersql_ddl_notifier::NewTruncateTableEvent(
            Some(Box::new(t.clone())),
            Some(Box::new(old)),
        ),
    )?;
    job.finish_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(t),
    );
    use astersql_meta_model::group_2::FinishedJobArgs;
    job.raw_args = serde_json::to_vec(&if job.version
        == astersql_meta_model::group_3::JobVersion::V1
    {
        serde_json::Value::Array(args.getFinishedArgsV1(job))
    } else {
        serde_json::to_value(args).map_err(|e| e.to_string())?
    })
    .map_err(|e| e.to_string())?;
    job.args.clear();
    Ok(version)
}
fn check_materialized_view(t: &astersql_meta_model::TableInfo, op: &str) -> Result<(), String> {
    let kind = if t.MaterializedViewLog.is_some() {
        Some("materialized view log table")
    } else if t.MaterializedView.is_some() {
        Some("materialized view table")
    } else if t.MaterializedViewShadow.is_some() {
        Some("materialized view shadow table")
    } else if t
        .MaterializedViewBase
        .as_ref()
        .is_some_and(|b| !b.MViewIDs.is_empty())
    {
        Some("base table with materialized view dependencies")
    } else if t
        .MaterializedViewBase
        .as_ref()
        .is_some_and(|b| b.MLogID != 0)
    {
        Some("base table with materialized view log")
    } else {
        None
    };
    if let Some(kind) = kind {
        Err(format!(
            "[ddl:8200]Unsupported DDL operation: {op} on {kind}"
        ))
    } else {
        Ok(())
    }
}
