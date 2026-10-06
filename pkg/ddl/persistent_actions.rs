// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! Go-wire action dispatch inside the normal worker's persistent transaction.
use astersql_meta_model::SchemaState;
use astersql_meta_model::group_3::{Job, JobState};

pub fn handler_available(action: u8) -> bool {
    action == astersql_meta_model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES
        || matches!(
            action,
            1 | 3
                | 60
                | 65
                | 67
                | 4
                | 6
                | 7
                | 10
                | 11
                | 12
                | 14
                | 17
                | 47
                | 26
                | 32
                | 39
                | 55
                | 74
                | 75
                | 76
                | 85
                | 86
                | 87
                | 88
                | 93
        )
}

pub fn step(
    context: &mut dyn crate::job_worker::JobExecutionContext,
    job: &mut Job,
) -> Result<i64, String> {
    if job.tp == 60 {
        return crate::persistent_create_table::batch_step(context, job);
    }
    if matches!(job.tp, 65 | 67) {
        return alter_ttl(context, job);
    }
    if job.tp == 12 {
        return crate::persistent_modify_column::step(context, job);
    }
    if job.tp == 6 {
        return crate::persistent_drop_column::step(context, job);
    }
    if job.tp == 11 {
        return crate::persistent_masking_actions::truncate_table(context, job);
    }
    if matches!(job.tp, 14 | 47) {
        return crate::persistent_masking_actions::rename_tables(context, job);
    }
    if matches!(job.tp, 4 | 87 | 88) {
        return crate::persistent_masking_actions::drop_table(context, job);
    }
    if matches!(job.tp, 7 | 32) {
        return initialize_prepared_index_action(context, job);
    }
    if job.tp == 86 {
        return crate::persistent_create_materialized_view::step(context, job);
    }
    if job.tp == 85 {
        return crate::persistent_create_materialized_view_log::step(context, job);
    }
    if job.tp == 93 {
        return crate::persistent_create_materialized_view_shadow::step(context, job);
    }
    if job.tp == 3 {
        return crate::persistent_create_table::step(context, job);
    }
    if job.tp == astersql_meta_model::group_3::ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES {
        return crate::persistent_alter_materialized_view_attributes::step(context, job);
    }
    if job.tp == 74 {
        return modify_engine_attribute(context, job);
    }
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        version = step_metadata(txn, job)?;
        Ok(Vec::new())
    })?;
    Ok(version)
}

fn step_metadata(txn: &mut dyn astersql_kv::Transaction, job: &mut Job) -> Result<i64, String> {
    match job.tp {
        1 => create_schema(txn, job),
        10 => drop_foreign_key(txn, job),
        17 | 39 => modify_table_metadata(txn, job),
        26 => modify_schema_charset(txn, job),
        55 => modify_schema_placement(txn, job),
        75 => crate::table_mode::on_persistent_alter_table_mode(txn, job),
        76 => refresh_meta(txn, job),
        action => Err(format!(
            "normal DDL persistent handler unavailable for action {action}"
        )),
    }
}

/// Corresponds to schema.go onCreateSchema: assigned ID, none -> public,
/// schema diff and binlog metadata in the same worker transaction.
fn create_schema(txn: &mut dyn astersql_kv::Transaction, job: &mut Job) -> Result<i64, String> {
    let database = (|| -> Result<astersql_meta_model::DBInfo, String> {
        let args: serde_json::Value =
            serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string())?;
        let value = if job.version == astersql_meta_model::group_3::JobVersion::V1 {
            args.as_array().and_then(|args| args.first()).cloned()
        } else {
            args.get("db_info").cloned()
        }
        .ok_or("missing create database metadata")?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    })()
    .map_err(|error| {
        job.state = JobState::Cancelled;
        format!("[ddl:1105]{error}")
    })?;
    let mut database = database;
    database.ID = job.schema_id;
    database.State = SchemaState::None;
    let mut meta = astersql_meta::TransactionMutator::new(txn);
    if meta
        .list_databases()?
        .iter()
        .any(|existing| existing.ID == database.ID || existing.Name.L == database.Name.L)
    {
        job.state = JobState::Cancelled;
        return Err(format!(
            "[schema:1007]Can't create database '{}'; database exists",
            database.Name.O
        ));
    }
    let version = meta.gen_schema_version()?;
    meta.set_table_schema_diff(job, version)?;
    database.State = SchemaState::Public;
    meta.create_database(&database)?;
    job.finish_db_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(database),
    );
    Ok(version)
}

/// schema.go onModifySchemaCharsetAndCollate: equal values finish without a diff.
fn modify_schema_charset(
    txn: &mut dyn astersql_kv::Transaction,
    job: &mut Job,
) -> Result<i64, String> {
    let args = astersql_meta_model::group_2::GetModifySchemaArgs(job).map_err(|error| {
        job.state = JobState::Cancelled;
        format!("[ddl:1105]{error}")
    })?;
    let mut meta = astersql_meta::TransactionMutator::new(txn);
    let mut database = meta.get_database(job.schema_id)?.ok_or_else(|| {
        job.state = JobState::Cancelled;
        "[schema:1008]Can't drop database ''; database doesn't exist".to_owned()
    })?;
    let mut version = 0;
    if database.Charset != args.ToCharset || database.Collate != args.ToCollate {
        database.Charset = args.ToCharset;
        database.Collate = args.ToCollate;
        meta.update_database(&database)?;
        version = meta.gen_schema_version()?;
        meta.set_table_schema_diff(job, version)?;
    }
    job.finish_db_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(database),
    );
    Ok(version)
}

/// table.go GetTableInfoAndCancelFaultJob, shared by persistent table actions.
pub(crate) fn public_table(
    meta: &astersql_meta::TransactionMutator<'_>,
    job: &mut Job,
) -> Result<astersql_meta_model::TableInfo, String> {
    if meta.get_database(job.schema_id)?.is_none() {
        job.state = JobState::Cancelled;
        return Err(format!(
            "[schema:1049]Unknown database '(Schema ID {})'",
            job.schema_id
        ));
    }
    let table = meta
        .get_table(job.schema_id, job.table_id)?
        .ok_or_else(|| {
            job.state = JobState::Cancelled;
            format!(
                "[schema:1146]Table '(Schema ID {}).(Table ID {})' doesn't exist",
                job.schema_id, job.table_id
            )
        })?;
    if !job.table_name.is_empty()
        && table.Name.L != job.table_name
        && job.tp != astersql_meta_model::group_3::ACTION_REPAIR_TABLE
    {
        job.state = JobState::Cancelled;
        return Err(format!(
            "[schema:1146]Table '{}.{}' doesn't exist",
            job.schema_name, job.table_name
        ));
    }
    if table.State != SchemaState::Public {
        job.state = JobState::Cancelled;
        return Err(format!(
            "[ddl:8210]table {} is not in public, but {:?}",
            table.Name.O, table.State
        ));
    }
    Ok(table)
}

/// table.go onModifyTableComment/onModifyTableAutoIDCache.
fn modify_table_metadata(
    txn: &mut dyn astersql_kv::Transaction,
    job: &mut Job,
) -> Result<i64, String> {
    enum Change {
        Comment(String),
        AutoIDCache(i64),
    }
    let change = match job.tp {
        17 => astersql_meta_model::group_2::GetModifyTableCommentArgs(job)
            .map(|args| Change::Comment(args.Comment)),
        39 => astersql_meta_model::group_2::GetModifyTableAutoIDCacheArgs(job)
            .map(|args| Change::AutoIDCache(args.NewCache)),
        _ => unreachable!(),
    }
    .map_err(|error| {
        job.state = JobState::Cancelled;
        format!("[ddl:1105]{error}")
    })?;
    let mut meta = astersql_meta::TransactionMutator::new(txn);
    let mut table = public_table(&meta, job)?;
    if job.tp == 17
        && job
            .multi_schema_info
            .as_ref()
            .is_some_and(|info| info.revertible)
    {
        job.mark_non_revertible();
        return Ok(0);
    }
    match change {
        Change::Comment(comment) => table.Comment = comment,
        Change::AutoIDCache(cache) => table.AutoIDCache = cache,
    }
    let version = update_version_and_table(&mut meta, job, &mut table)?;
    job.finish_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(table),
    );
    Ok(version)
}

/// Go updateVersionAndTableInfoWithCheck. Call only from handlers whose Go
/// counterparts use the checked helper; unchecked metadata-only handlers must
/// retain their original behavior.
pub fn update_version_and_table_with_check(
    meta: &mut astersql_meta::TransactionMutator<'_>,
    job: &mut Job,
    table: &mut astersql_meta_model::TableInfo,
) -> Result<i64, String> {
    crate::create_table::check_table_info_valid(table).map_err(|error| {
        job.state = JobState::Cancelled;
        error
    })?;
    update_version_and_table(meta, job, table)
}

/// Go updateVersionAndTableInfo for a single metadata-only table action.
pub(crate) fn update_version_and_table(
    meta: &mut astersql_meta::TransactionMutator<'_>,
    job: &Job,
    table: &mut astersql_meta_model::TableInfo,
) -> Result<i64, String> {
    let version = if job
        .multi_schema_info
        .as_ref()
        .is_some_and(|info| info.skip_version)
    {
        0
    } else {
        let version = meta.gen_schema_version()?;
        meta.set_table_schema_diff(job, version)?;
        version
    };
    meta.update_table(job.schema_id, table)?;
    Ok(version)
}

/// schema.go onModifySchemaDefaultPlacement, including nil/nil version updates.
fn modify_schema_placement(
    txn: &mut dyn astersql_kv::Transaction,
    job: &mut Job,
) -> Result<i64, String> {
    let policy = (|| -> Result<Option<astersql_meta_model::PolicyRefInfo>, String> {
        let args: serde_json::Value =
            serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string())?;
        let value = if job.version == astersql_meta_model::group_3::JobVersion::V1 {
            args.as_array()
                .and_then(|args| args.first())
                .cloned()
                .ok_or("invalid V1 schema placement arguments")?
        } else {
            args.as_object()
                .ok_or("invalid V2 schema placement arguments")?
                .get("policy_ref")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        };
        if value.is_null() {
            return Ok(None);
        }
        #[derive(Default, serde::Deserialize)]
        #[serde(default)]
        struct PolicyReference {
            id: i64,
            name: astersql_meta_model::ast::CIStr,
        }
        let reference: PolicyReference =
            serde_json::from_value(value).map_err(|e| e.to_string())?;
        Ok(Some(astersql_meta_model::PolicyRefInfo {
            ID: reference.id,
            Name: reference.name,
        }))
    })()
    .map_err(|error| {
        job.state = JobState::Cancelled;
        format!("[ddl:1105]{error}")
    })?;
    let mut meta = astersql_meta::TransactionMutator::new(txn);
    let mut database = meta.get_database(job.schema_id)?.ok_or_else(|| {
        job.state = JobState::Cancelled;
        "[schema:1008]Can't drop database ''; database doesn't exist".to_owned()
    })?;
    if let Some(reference) = &policy {
        if meta.get_placement_policy(reference.ID)?.is_none() {
            job.state = JobState::Cancelled;
            return Err(format!(
                "[schema:8239]Unknown placement policy '(Policy ID {})'",
                reference.ID
            ));
        }
        if database.PlacementPolicyRef.as_ref() == Some(reference) {
            job.finish_db_job(
                JobState::Done,
                SchemaState::Public,
                0,
                std::sync::Arc::new(database),
            );
            return Ok(0);
        }
    }
    database.PlacementPolicyRef = policy;
    meta.update_database(&database)?;
    let version = meta.gen_schema_version()?;
    meta.set_table_schema_diff(job, version)?;
    job.finish_db_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(database),
    );
    Ok(version)
}

/// foreign_key.go onDropForeignKey/dropForeignKey, including rollback completion.
fn drop_foreign_key(txn: &mut dyn astersql_kv::Transaction, job: &mut Job) -> Result<i64, String> {
    let mut meta = astersql_meta::TransactionMutator::new(txn);
    let mut table = public_table(&meta, job)?;
    let args = astersql_meta_model::group_2::GetDropForeignKeyArgs(job).map_err(|error| {
        job.state = JobState::Cancelled;
        format!("[ddl:1105]{error}")
    })?;
    if !table
        .ForeignKeys
        .iter()
        .any(|fk| fk.Name.L == args.FkName.L)
    {
        job.state = JobState::Cancelled;
        return Err(format!(
            "[schema:1091]Can't DROP '{}'; check that column/key exists",
            args.FkName.O
        ));
    }
    table.ForeignKeys.retain(|fk| fk.Name.L != args.FkName.L);
    let version = update_version_and_table(&mut meta, job, &mut table)?;
    let state = if job.state == JobState::Rollingback {
        JobState::RollbackDone
    } else {
        JobState::Done
    };
    job.finish_table_job(
        state,
        SchemaState::None,
        version,
        std::sync::Arc::new(table),
    );
    job.schema_state = SchemaState::None;
    Ok(version)
}

/// table.go onRefreshMeta intentionally does not rewrite or validate table metadata.
fn refresh_meta(txn: &mut dyn astersql_kv::Transaction, job: &mut Job) -> Result<i64, String> {
    astersql_meta_model::group_2::GetRefreshMetaArgs(job).map_err(|error| {
        job.state = JobState::Cancelled;
        format!("[ddl:1105]{error}")
    })?;
    let mut meta = astersql_meta::TransactionMutator::new(txn);
    let version = meta.gen_schema_version()?;
    meta.set_table_schema_diff(job, version)?;
    job.state = JobState::Done;
    job.schema_state = SchemaState::Public;
    Ok(version)
}

/// Go ddl.go asyncNotifyEvent: system schemas are skipped and implicit sub-job
/// IDs resolve to MultiSchemaInfo.Seq. Duplicate keys remain SQL errors.
pub fn async_notify_event(
    context: &mut dyn crate::job_worker::JobExecutionContext,
    job: &Job,
    sub_job_id: i64,
    event: astersql_ddl_notifier::SchemaChangeEvent,
) -> Result<(), String> {
    if astersql_meta_metadef::IsMemOrSysDB(&job.schema_name) {
        return Ok(());
    }
    let sub_job_id = if sub_job_id == -1 {
        job.multi_schema_info
            .as_ref()
            .map_or(-1, |m| i64::from(m.seq))
    } else {
        sub_job_id
    };
    astersql_ddl_notifier::PubSchemaChangeInTransaction(job.id, sub_job_id, event, |sql, args| {
        // Only the notifier's integer keys and JSON bytes are bound here.
        // Hex literals preserve JSON bytes without SQL string escaping.
        let mut query = sql.to_owned();
        for arg in args {
            let value = match arg {
                astersql_ddl_notifier::SqlValue::Integer(v) => v.to_string(),
                astersql_ddl_notifier::SqlValue::Bytes(v) => format!(
                    "X'{}'",
                    v.iter().map(|b| format!("{b:02x}")).collect::<String>()
                ),
                _ => {
                    return Err(astersql_ddl_notifier::Error::Message(
                        "unsupported notifier insert argument".into(),
                    ));
                }
            };
            query = query.replacen("%?", &value, 1);
        }
        context
            .query(&query, "publish-schema-change")
            .map(|_| ())
            .map_err(astersql_ddl_notifier::Error::Message)
    })
    .map_err(|e| e.to_string())
}

/// Shared initialization stage of ADD INDEX and changing-index reorganization.
/// The caller owns building the index metadata and its subsequent schema step;
/// this dispatcher must not publish indexes or substitute a transaction backend.
pub fn initialize_reorg_indexes(
    context: &mut dyn crate::job_worker::JobExecutionContext,
    job: &mut Job,
    indexes: &mut [astersql_meta_model::IndexInfo],
) -> Result<(), String> {
    use astersql_meta_model::group_3::{
        ACTION_ADD_INDEX, ACTION_ADD_PRIMARY_KEY, ACTION_MODIFY_COLUMN,
    };
    if !matches!(
        job.tp,
        ACTION_ADD_INDEX | ACTION_ADD_PRIMARY_KEY | ACTION_MODIFY_COLUMN
    ) {
        return Err(format!(
            "index reorg initialization unavailable for action {}",
            job.tp
        ));
    }
    if indexes.is_empty() {
        return Ok(());
    }
    crate::index::init_for_reorg_indexes(context.reorg_index_environment()?, job, indexes)
}

/// Resume the initialization phase using the action's prepared canonical indexes.
/// Building missing indexes, schema transitions and backend execution remain
/// separate stages: never claim publication or fall back to a different backend.
fn initialize_prepared_index_action(
    context: &mut dyn crate::job_worker::JobExecutionContext,
    job: &mut Job,
) -> Result<i64, String> {
    use astersql_meta_model::group_3::ReorgType;
    if job.state == JobState::Rollingback {
        return Err("ADD INDEX rollback stage is not implemented".into());
    }
    if job.schema_state != SchemaState::None
        || job
            .reorg_meta
            .as_ref()
            .is_some_and(|meta| meta.ReorgTp != ReorgType::ReorgTypeNone)
    {
        return Err("ADD INDEX stage after reorg initialization is not implemented".into());
    }
    let args = astersql_meta_model::group_2::GetModifyIndexArgs(job).map_err(|error| {
        job.state = JobState::Cancelled;
        error.to_string()
    })?;
    if args.IndexArgs.is_empty() {
        return Err(
            "ADD INDEX metadata preparation stage is not implemented: no index arguments".into(),
        );
    }
    let mut table = None;
    context.with_transaction(&mut |txn| {
        table = Some(public_table(
            &astersql_meta::TransactionMutator::new(txn),
            job,
        )?);
        Ok(Vec::new())
    })?;
    let mut table = table.ok_or("ADD INDEX table metadata missing")?;
    let mut positions = Vec::with_capacity(args.IndexArgs.len());
    for arg in &args.IndexArgs {
        let position = table
            .Indices
            .iter()
            .position(|index| index.Name.L == arg.IndexName.L)
            .ok_or("ADD INDEX metadata preparation stage is not implemented: index missing")?;
        if table.Indices[position].State != SchemaState::None {
            return Err("ADD INDEX stage after reorg initialization is not implemented".into());
        }
        positions.push(position);
    }
    let mut indexes = positions
        .iter()
        .map(|position| table.Indices[*position].clone())
        .collect::<Vec<_>>();
    // Go onCreateIndex cancels initialization failures, unlike later retryable
    // backfill errors. The normal executor retains the error in durable history.
    initialize_reorg_indexes(context, job, &mut indexes).map_err(|error| {
        job.state = JobState::Cancelled;
        error
    })?;
    for (position, index) in positions.into_iter().zip(indexes) {
        table.Indices[position] = index;
    }
    context.with_transaction(&mut |txn| {
        astersql_meta::TransactionMutator::new(txn).update_table(job.schema_id, &mut table)?;
        Ok(Vec::new())
    })?;
    // Initialization does not itself publish a schema state; the next invocation
    // must report the unimplemented following stage without repeating telemetry.
    Ok(0)
}

/// engine_attribute.go onModifyTableEngineAttribute: retain original JSON and
/// atomically rebuild canonical metadata with the schema version and job result.
fn modify_engine_attribute(
    context: &mut dyn crate::job_worker::JobExecutionContext,
    job: &mut Job,
) -> Result<i64, String> {
    let args =
        astersql_meta_model::group_2::GetModifyTableEngineAttributeArgs(job).map_err(|error| {
            job.state = JobState::Cancelled;
            error.to_string()
        })?;
    astersql_meta_model::ParseEngineAttributeFromString(&args.EngineAttribute).map_err(
        |error| {
            job.state = JobState::Cancelled;
            error.to_string()
        },
    )?;
    let mut old = None;
    let mut current = None;
    let mut schema_name = None;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = astersql_meta::TransactionMutator::new(txn);
        let mut table = public_table(&meta, job)?;
        if job
            .multi_schema_info
            .as_ref()
            .is_some_and(|info| info.revertible)
        {
            job.mark_non_revertible();
            return Ok(Vec::new());
        }
        old = Some(crate::storage_class_transition::snapshot_physical_storage_classes(&table));
        schema_name = Some(
            meta.get_database(job.schema_id)?
                .ok_or_else(|| format!("database {} does not exist", job.schema_id))?
                .Name
                .O,
        );
        table.EngineAttribute = args.EngineAttribute.clone();
        let settings = crate::storage_class::get_settings(&table).map_err(|error| {
            job.state = JobState::Cancelled;
            error
        })?;
        crate::storage_class::BuildStorageClassForTable(&mut table, settings.as_ref()).map_err(
            |error| {
                job.state = JobState::Cancelled;
                error
            },
        )?;
        crate::storage_class::rebuild_partitions(&mut table).map_err(|error| {
            job.state = JobState::Cancelled;
            error
        })?;
        version = update_version_and_table(&mut meta, job, &mut table)?;
        current = Some(table);
        Ok(Vec::new())
    })?;
    if current.is_none() {
        return Ok(0);
    }
    let table = current.unwrap();
    if astersql_config_kerneltype::IsNextGen() {
        stage_storage_class_transitions(
            context,
            job,
            old.as_ref().unwrap(),
            &table,
            version,
            schema_name.as_deref().unwrap(),
        )?;
    }
    job.finish_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(table),
    );
    Ok(version)
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

fn stage_storage_class_transitions(
    context: &mut dyn crate::job_worker::JobExecutionContext,
    job: &Job,
    old: &std::collections::BTreeMap<i64, crate::storage_class_transition::PhysicalStorageClass>,
    table: &astersql_meta_model::TableInfo,
    schema_version: i64,
    schema_name: &str,
) -> Result<(), String> {
    use crate::storage_class_transition as transition;
    let current = transition::snapshot_physical_storage_classes(table);
    let mut changed = transition::changed_physical_ids(old, &current);
    if changed.is_empty() {
        return Ok(());
    }
    let rows = context.query(
        &format!(
            "SELECT direction,start_ts,physical_targets FROM mysql.tidb_storage_class_transition_history WHERE state='RUNNING' AND table_id={}",
            table.ID
        ),
        "load-table-storage-class-transitions",
    )?;
    let finish = chrono::Utc::now();
    for row in rows {
        if row.len() < 3 {
            return Err("invalid storage class transition history row".into());
        }
        let targets: Vec<transition::StorageClassTransitionTarget> =
            serde_json::from_str(&row[2]).map_err(|error| error.to_string())?;
        transition::validate_targets(&targets)?;
        if !targets
            .iter()
            .any(|target| changed.contains(&target.physical_id))
        {
            continue;
        }
        let start_ts = row[1].parse::<u64>().map_err(|error| error.to_string())?;
        context.query(
            &format!(
                "UPDATE mysql.tidb_storage_class_transition_history SET state='SUPERSEDED',finish_time={},duration=GREATEST(TIMESTAMPDIFF(SECOND,start_time,{}),0) WHERE table_id={} AND start_ts={} AND direction={} AND state='RUNNING'",
                sql_string(&finish.format("%Y-%m-%d %H:%M:%S%.6f").to_string()),
                sql_string(&finish.format("%Y-%m-%d %H:%M:%S%.6f").to_string()),
                table.ID,
                start_ts,
                sql_string(&row[0]),
            ),
            "supersede-storage-class-transition",
        )?;
        transition::add_current_targets(&mut changed, &current, &targets);
    }
    let start_ts = if job.real_start_ts != 0 {
        job.real_start_ts
    } else {
        job.start_ts
    };
    for operation in transition::build_operations(
        table,
        &changed,
        schema_version,
        start_ts,
        schema_name,
        &table.Name.O,
    )? {
        let targets =
            serde_json::to_string(&operation.targets).map_err(|error| error.to_string())?;
        let partition_name = if operation.status.partition_id == 0 {
            "NULL".to_owned()
        } else {
            sql_string(&operation.status.partition_name)
        };
        let partition_id = if operation.status.partition_id == 0 {
            "NULL".to_owned()
        } else {
            operation.status.partition_id.to_string()
        };
        context.query(
            &format!(
                "INSERT INTO mysql.tidb_storage_class_transition_history (table_schema,table_name,table_id,partition_name,partition_id,direction,state,schema_version,start_ts,start_time,physical_targets) VALUES ({},{},{},{},{},{},'RUNNING',{},{},{},{})",
                sql_string(&operation.status.table_schema),
                sql_string(&operation.status.table_name),
                operation.status.table_id,
                partition_name,
                partition_id,
                sql_string(&operation.status.direction),
                operation.status.schema_version,
                operation.status.start_ts,
                sql_string(&operation.status.start_time.format("%Y-%m-%d %H:%M:%S%.6f").to_string()),
                sql_string(&targets),
            ),
            "insert-storage-class-transition",
        )?;
    }
    Ok(())
}

/// Go onTTLInfoChange/onTTLInfoRemove, within the owner's job transaction.
fn alter_ttl(
    context: &mut dyn crate::job_worker::JobExecutionContext,
    job: &mut Job,
) -> Result<i64, String> {
    let args = if job.tp == 65 {
        Some(
            astersql_meta_model::group_2::GetAlterTTLInfoArgs(job).map_err(|error| {
                job.state = JobState::Cancelled;
                error.to_string()
            })?,
        )
    } else {
        None
    };
    let mut table = None;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = astersql_meta::TransactionMutator::new(txn);
        let mut t = public_table(&meta, job)?;
        if let Some(args) = &args {
            let info = args
                .TTLInfo
                .as_ref()
                .map(|info| {
                    serde_json::from_value(serde_json::to_value(info).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())
                })
                .transpose()?;
            crate::ttl::apply_model_ttl_change(
                &mut t,
                info,
                args.TTLEnable,
                args.TTLCronJobSchedule.clone(),
            )?;
        } else {
            t.TTLInfo = None;
        }
        version = update_version_and_table(&mut meta, job, &mut t)?;
        table = Some(t);
        Ok(Vec::new())
    })?;
    let table = table.unwrap();
    let result = if table.TTLInfo.as_ref().is_some_and(|ttl| ttl.Enable) {
        context.register_create_table_ttl(&table)
    } else {
        context.delete_drop_table_ttl(table.ID)
    };
    result.map_err(|error| {
        job.state = JobState::Cancelled;
        error
    })?;
    job.finish_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(table),
    );
    Ok(version)
}
