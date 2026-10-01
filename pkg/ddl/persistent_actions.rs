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
    matches!(action, 1 | 10 | 17 | 26 | 39 | 55 | 75 | 76)
}

pub fn step(
    context: &mut dyn crate::job_worker::JobExecutionContext,
    job: &mut Job,
) -> Result<i64, String> {
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

/// Go updateVersionAndTableInfo for a single public metadata-only table action.
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
