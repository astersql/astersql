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

//! MODIFY COLUMN metadata and masking writes share the durable worker transaction.
use crate::job_worker::JobExecutionContext;
use astersql_meta::TransactionMutator;
use astersql_meta_model::{
    ColumnInfo, SchemaState, TableInfo,
    group_3::{Job, JobState, JobVersion},
};
use astersql_parser_ast as ast;
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Args {
    column: Option<Box<ColumnInfo>>,
    old_column_id: i64,
    old_column_name: astersql_meta_model::ast::CIStr,
    position: Option<serde_json::Value>,
    modify_column_type: u8,
    new_shard_bits: u64,
    changing_column: Option<Box<ColumnInfo>>,
    changing_indexes: Vec<astersql_meta_model::IndexInfo>,
    removed_idxs: Vec<i64>,
    index_ids: Vec<i64>,
    new_index_ids: Vec<i64>,
    partition_ids: Vec<i64>,
}
fn decode(job: &Job) -> Result<Args, String> {
    if job.version != JobVersion::V1 {
        return serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string());
    }
    let v: Vec<serde_json::Value> =
        serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string())?;
    let keys = [
        "column",
        "old_column_name",
        "position",
        "modify_column_type",
        "new_shard_bits",
        "changing_column",
        "changing_indexes",
        "removed_idxs",
        "old_column_id",
    ];
    let mut map = serde_json::Map::new();
    for (key, value) in keys.iter().zip(v) {
        if !value.is_null() {
            map.insert((*key).into(), value);
        }
    }
    serde_json::from_value(serde_json::Value::Object(map)).map_err(|e| e.to_string())
}
pub(crate) fn finished_range_ids(job: &Job) -> Result<(Vec<i64>, Vec<i64>), String> {
    if job.version == JobVersion::V1 {
        let values: Vec<serde_json::Value> =
            serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string())?;
        let ids = serde_json::from_value(
            values
                .first()
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|e| e.to_string())?;
        let partitions = serde_json::from_value(
            values
                .get(1)
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|e| e.to_string())?;
        return Ok((ids, partitions));
    }
    let args = decode(job)?;
    Ok((args.index_ids, args.partition_ids))
}

/// Go noReorgDataStrict over canonical persisted column types.
pub fn no_reorg_data_strict(table: &TableInfo, old: &ColumnInfo, new: &ColumnInfo) -> bool {
    let unsigned = |c: &ColumnInfo| c.GetFlag() & astersql_parser_mysql::r#type::UnsignedFlag != 0;
    let truncation = (new.GetFlen() > 0
        && (new.GetFlen() < old.GetFlen() || new.GetDecimal() < old.GetDecimal()))
        || unsigned(old) != unsigned(new);
    let string = |tp| matches!(tp, 15 | 253 | 254 | 249 | 250 | 251 | 252);
    let integer = |tp| matches!(tp, 1 | 2 | 3 | 8 | 9);
    if old.GetType() == new.GetType() {
        match old.GetType() {
            246 => {
                return old.GetFlen() == new.GetFlen()
                    && old.GetDecimal() == new.GetDecimal()
                    && unsigned(old) == unsigned(new);
            }
            247 | 248 => {
                return new.GetElems().len() >= old.GetElems().len()
                    && old
                        .GetElems()
                        .iter()
                        .zip(new.GetElems())
                        .all(|(a, b)| a == b);
            }
            1 | 2 | 3 | 8 | 9 => return unsigned(old) == unsigned(new),
            254 if old.GetCharset() == "binary" => return new.GetFlen() == old.GetFlen(),
            astersql_parser_mysql::r#type::TypeTiDBVectorFloat32 => {
                return new.GetFlen() == -1 || old.GetFlen() == new.GetFlen();
            }
            _ => {}
        }
        return !truncation;
    }
    if matches!(old.GetType(), 15 | 253) && new.GetType() == 254 {
        return false;
    }
    if old.GetType() == 254
        && matches!(new.GetType(), 15 | 253)
        && table
            .Indices
            .iter()
            .any(|i| i.Columns.iter().any(|c| c.Name.L == old.Name.L))
    {
        return false;
    }
    if string(old.GetType()) && string(new.GetType()) {
        return !truncation;
    }
    if integer(old.GetType()) && integer(new.GetType()) {
        let (old_len, _) =
            astersql_parser_mysql::util::GetDefaultFieldLengthAndDecimal(old.GetType());
        let (new_len, _) =
            astersql_parser_mysql::util::GetDefaultFieldLengthAndDecimal(new.GetType());
        return !(new_len > 0 && new_len < old_len) && unsigned(old) == unsigned(new);
    }
    false
}
pub fn choose_type(
    table: &TableInfo,
    old: &ColumnInfo,
    new: &ColumnInfo,
    sql_mode: u64,
    compat: u8,
) -> u8 {
    let notnull = |c: &ColumnInfo| c.GetFlag() & astersql_parser_mysql::r#type::NotNullFlag != 0;
    if no_reorg_data_strict(table, old, new) {
        return if !notnull(old) && notnull(new) { 2 } else { 1 };
    }
    if compat == 6
        || table.Partition.is_some()
        || table.TiFlashReplica.as_ref().is_some_and(|r| r.Count > 0)
    {
        return 4;
    }
    let integer = |tp| matches!(tp, 1 | 2 | 3 | 8 | 9 | 13);
    let chars = |tp| matches!(tp, 15 | 253 | 254);
    let sign = |c: &ColumnInfo| c.GetFlag() & astersql_parser_mysql::r#type::UnsignedFlag;
    if integer(old.GetType()) && integer(new.GetType()) && sign(old) != sign(new)
        || chars(old.GetType())
            && chars(new.GetType())
            && !astersql_util_collate::CompatibleCollate(&old.GetCollate(), &new.GetCollate())
    {
        return 4;
    }
    if !astersql_parser_mysql::r#const::SQLMode(sql_mode as i64).HasStrictMode() {
        return 4;
    }
    let row = !(integer(old.GetType()) && integer(new.GetType()))
        && (!(chars(old.GetType()) && chars(new.GetType()))
            || old.GetCharset() == "binary"
            || new.GetCharset() == "binary");
    if row {
        return 4;
    }
    let indexed = table
        .Indices
        .iter()
        .any(|i| i.Columns.iter().any(|c| c.Name.L == old.Name.L));
    if !indexed
        || astersql_types::metadata::NeedRestoredData(&old.FieldType)
            == astersql_types::metadata::NeedRestoredData(&new.FieldType)
    {
        2
    } else {
        3
    }
}

fn destination(
    table: &TableInfo,
    old: &ColumnInfo,
    position: Option<&serde_json::Value>,
) -> Result<usize, String> {
    let Some(position) = position else {
        return Ok(old.Offset as usize);
    };
    let kind = position
        .get("Tp")
        .or_else(|| position.get("tp"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if kind == 1 {
        return Ok(0);
    }
    if kind != 2 {
        return Ok(old.Offset as usize);
    }
    let relative = position
        .get("RelativeColumn")
        .or_else(|| position.get("relative_column"))
        .and_then(|v| v.get("Name"))
        .and_then(|v| v.get("L").or_else(|| v.get("l")))
        .and_then(|v| v.as_str())
        .ok_or("invalid column position")?;
    let column = table
        .Columns
        .iter()
        .find(|c| c.Name.L == relative && c.State == SchemaState::Public && c.ID != old.ID)
        .ok_or_else(|| {
            format!(
                "[schema:1054]Unknown column '{}' in '{}'",
                relative, table.Name.O
            )
        })?;
    Ok(column.Offset as usize + usize::from(column.Offset < old.Offset))
}

fn rewrite(expr: &str, old: &str, new: &str) -> Result<String, String> {
    if old.eq_ignore_ascii_case(new) {
        return Ok(expr.into());
    }
    let statement = astersql_parser::New()
        .ParseOneStmt(&format!("SELECT {expr}"), "", "")
        .map_err(|e| e.to_string())?;
    let mut select = statement
        .into_any()
        .downcast::<ast::SelectStmt>()
        .map_err(|_| "invalid masking policy expression")?;
    if select.Fields.Fields.len() != 1 {
        return Err("invalid masking policy expression".into());
    }
    let expression = select.Fields.Fields[0]
        .Expr
        .as_mut()
        .ok_or("invalid masking policy expression")?;
    struct Rename<'a>(&'a str, &'a str);
    impl ast::InPlaceVisitor for Rename<'_> {
        fn enter(&mut self, node: &mut dyn ast::Node) -> bool {
            if let Some(expr) = node.as_any_mut().downcast_mut::<ast::ExprNode>() {
                if let ast::ExprKind::Column(column) = &mut expr.Kind {
                    if column.Name.L.eq_ignore_ascii_case(self.0) {
                        column.Name = ast::NewCIStr(self.1);
                    }
                }
            }
            false
        }
        fn leave(&mut self, _: &mut dyn ast::Node) -> bool {
            true
        }
    }
    if !ast::Walk(expression, &mut Rename(old, new)) {
        return Err("failed to rewrite masking policy expression".into());
    }
    ast::sql_restore::restore_expr(expression)
}
pub fn sync_policy(
    context: &mut dyn JobExecutionContext,
    table: &TableInfo,
    old: &ColumnInfo,
    new: &ColumnInfo,
) -> Result<(), String> {
    let policies = crate::persistent_masking_actions::policies_on_table(context, table.ID)?;
    for policy in policies {
        if policy[4].parse::<i64>().map_err(|e| e.to_string())? != table.ID {
            continue;
        }
        if policy[6].parse::<i64>().map_err(|e| e.to_string())? != old.ID
            && !policy[5].eq_ignore_ascii_case(&old.Name.L)
            && !policy[5].eq_ignore_ascii_case(&new.Name.L)
        {
            continue;
        }
        if new.IsGenerated() {
            return Err(astersql_util_dbterror::ErrUnsupportedOnGeneratedColumn
                .GenWithStackByArgs(&["masking policy on generated column".into()])
                .to_string());
        }
        let tp = new.GetType();
        if !matches!(
            tp,
            1 | 2
                | 3
                | 4
                | 5
                | 7
                | 8
                | 9
                | 10
                | 11
                | 12
                | 13
                | 15
                | 16
                | 246
                | 249
                | 250
                | 251
                | 252
                | 253
                | 254
        ) {
            return Err(astersql_util_dbterror::ErrGeneralUnsupportedDDL
                .GenWithStackByArgs(&["masking policy on unsupported column type".into()])
                .to_string());
        }
        let expression = rewrite(&policy[7], &policy[5], &new.Name.O)?;
        let now = context.masking_policy_timestamp()?;
        let quote = crate::table_mode::sql_text;
        let masking_type = policy[9].trim().to_uppercase();
        let masking_type = match masking_type.as_str() {
            "MASK_FULL" | "MASK_PARTIAL" | "MASK_NULL" | "MASK_DATE" | "CUSTOM" => {
                masking_type.as_str()
            }
            _ => "CUSTOM",
        };
        let status = if matches!(
            policy[8].trim().to_uppercase().as_str(),
            "ENABLE" | "ENABLED"
        ) {
            "ENABLED"
        } else {
            "DISABLED"
        };
        let tokens: Vec<_> = policy[10]
            .split(',')
            .map(|token| token.trim().to_uppercase())
            .collect();
        let restrict: Vec<_> = [
            "INSERT_INTO_SELECT",
            "UPDATE_SELECT",
            "DELETE_SELECT",
            "CTAS",
        ]
        .into_iter()
        .filter(|name| tokens.iter().any(|token| token == name))
        .collect();
        let restrict = if restrict.is_empty() {
            "NONE".to_string()
        } else {
            restrict.join(",")
        };
        context.query(&format!("UPDATE mysql.tidb_masking_policy SET policy_name={},db_name={},table_name={},table_id={},column_id={},column_name={},expression={},status={},masking_type={},restrict_on={},updated_at={} WHERE policy_id={}",quote(&policy[1]),quote(&policy[2]),quote(&table.Name.O),table.ID,new.ID,quote(&new.Name.O),quote(&expression),quote(status),quote(masking_type),quote(&restrict),quote(&now),policy[0]),"update-masking-policy")?;
    }
    Ok(())
}
fn save_args(job: &mut Job, args: &Args) -> Result<(), String> {
    job.raw_args = if job.version == JobVersion::V1 {
        if args.changing_column.is_none() && args.changing_indexes.is_empty() {
            serde_json::to_vec(&serde_json::json!([
                args.column,
                args.old_column_name,
                args.position,
                args.modify_column_type,
                args.new_shard_bits
            ]))
        } else {
            serde_json::to_vec(&serde_json::json!([
                args.column,
                args.old_column_name,
                args.position,
                args.modify_column_type,
                args.new_shard_bits,
                args.changing_column,
                args.changing_indexes,
                args.removed_idxs,
                args.old_column_id
            ]))
        }
    } else {
        serde_json::to_vec(args)
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}
fn temporary_gc_ids(job: &Job, indexes: impl IntoIterator<Item = i64>) -> Vec<i64> {
    if job
        .reorg_meta
        .as_ref()
        .is_some_and(|meta| meta.ReorgTp.NeedMergeProcess())
    {
        indexes
            .into_iter()
            .map(|id| id | astersql_tablecodec::TempIndexPrefix)
            .collect()
    } else {
        Vec::new()
    }
}
fn write_table(
    context: &mut dyn JobExecutionContext,
    job: &mut Job,
    table: &mut TableInfo,
) -> Result<i64, String> {
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.set_table_schema_diff(job, version)?;
        meta.update_table(job.schema_id, table)?;
        Ok(Vec::new())
    })?;
    Ok(version)
}
fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("invalid modify-column checkpoint".into());
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|p| {
            std::str::from_utf8(p)
                .map_err(|e| e.to_string())
                .and_then(|p| u8::from_str_radix(p, 16).map_err(|e| e.to_string()))
        })
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn initialize_cursor(
    context: &mut dyn JobExecutionContext,
    job: &Job,
    id: i64,
    element: i64,
    index: bool,
    merging: bool,
) -> Result<(), String> {
    let (start, end) = if merging {
        (
            astersql_tablecodec::EncodeTableIndexPrefix(id, astersql_tablecodec::TempIndexPrefix).0,
            astersql_tablecodec::EncodeTableIndexPrefix(
                id,
                astersql_tablecodec::TempIndexPrefix + astersql_tablecodec::IndexIDMask,
            )
            .0,
        )
    } else {
        let start = astersql_tablecodec::GenTableRecordPrefix(id).0;
        let end = astersql_kv::Key(start.clone()).PrefixNext().0;
        (start, end)
    };
    context.query(
        &format!("DELETE FROM mysql.tidb_ddl_reorg WHERE job_id={}", job.id),
        "init_handle",
    )?;
    context.query(&format!("INSERT INTO mysql.tidb_ddl_reorg (job_id,ele_id,ele_type,start_key,end_key,physical_id) VALUES ({},{},X'{}',X'{}',X'{}',{})",job.id,element,hex(if index {b"_idx_"}else{b"_col_"}),hex(&start),hex(&end),id),"init_handle")?;
    Ok(())
}
fn data_error(old: &ColumnInfo, row: &[String]) -> String {
    if row.get(1).is_some_and(|value| value == "1") {
        "[ddl:1138]Invalid use of NULL value".into()
    } else {
        format!(
            "[types:1265]Data truncated for column '{}', value is '{}'",
            old.Name.L, row[0]
        )
    }
}
fn check_data(
    context: &mut dyn JobExecutionContext,
    job: &Job,
    table: &TableInfo,
    old: &ColumnInfo,
    new: &ColumnInfo,
) -> Result<Vec<Vec<String>>, String> {
    let quoted = |s: &str| format!("`{}`", s.replace('`', "``"));
    let name = quoted(&old.Name.O);
    let mut conditions = Vec::new();
    if !no_reorg_data_strict(&table, &old, &new) {
        if matches!(old.GetType(), 1 | 2 | 3 | 8 | 9 | 13)
            && matches!(new.GetType(), 1 | 2 | 3 | 8 | 9 | 13)
        {
            if new.GetFlag() & astersql_parser_mysql::r#type::UnsignedFlag != 0 {
                conditions.push(format!(
                    "({name} < 0 OR {name} > {})",
                    astersql_types::scalar::IntegerUnsignedUpperBound(new.GetType())
                ));
            } else {
                conditions.push(format!(
                    "({name} < {} OR {name} > {})",
                    astersql_types::scalar::IntegerSignedLowerBound(new.GetType()),
                    astersql_types::scalar::IntegerSignedUpperBound(new.GetType())
                ));
            }
        } else {
            conditions.push(format!("LENGTH({name}) > {}", new.GetFlen()));
            if matches!(old.GetType(), 15 | 253) && new.GetType() == 254 {
                conditions.push(format!("{name} LIKE '% '"));
            }
        }
    }
    if old.GetFlag() & astersql_parser_mysql::r#type::NotNullFlag == 0
        && new.GetFlag() & astersql_parser_mysql::r#type::NotNullFlag != 0
        && !(old.GetType() != 7 && new.GetType() == 7)
    {
        conditions.push(format!("{name} IS NULL"));
    }
    if !conditions.is_empty() {
        let rows = context.query(
            &format!(
                "SELECT {name}, {name} IS NULL FROM {}.{} WHERE {} LIMIT 1",
                quoted(&job.schema_name),
                quoted(&table.Name.O),
                conditions.join(" OR ")
            ),
            "check-modify-column",
        )?;
        return Ok(rows);
    }
    Ok(Vec::new())
}
fn advance_reorg(
    context: &mut dyn JobExecutionContext,
    job: &mut Job,
    table: &mut TableInfo,
    args: &mut Args,
    old: &ColumnInfo,
    new: &ColumnInfo,
) -> Result<i64, String> {
    use astersql_meta_model::group_3::{DDLReorgMeta, ReorgStage, ReorgType};
    let index_only = args.modify_column_type == 3;
    if args.changing_column.is_none() && (!index_only || job.schema_state == SchemaState::None) {
        let mut column = if index_only { old.clone() } else { new.clone() };
        if !index_only {
            table.MaxColumnID += 1;
            column.ID = table.MaxColumnID;
            column.Offset = table.Columns.len() as isize;
            column.Name = astersql_meta_model::ast::NewCIStr(
                &astersql_meta_model::GenUniqueChangingColumnName(table, old),
            );
            column.State = SchemaState::None;
            column.ChangeStateInfo = Some(astersql_meta_model::ChangeStateInfo {
                DependencyColumnOffset: old.Offset,
            });
        }
        let related: Vec<_> = table
            .Indices
            .iter()
            .filter(|i| i.Columns.iter().any(|c| c.Name.L == old.Name.L))
            .cloned()
            .collect();
        for mut index in related {
            let original = index.clone();
            table.MaxIndexID += 1;
            index.ID = table.MaxIndexID;
            index.Name = astersql_meta_model::ast::NewCIStr(
                &astersql_meta_model::GenUniqueChangingIndexName(table, &original),
            );
            index.State = SchemaState::None;
            for part in &mut index.Columns {
                if part.Name.L == old.Name.L {
                    part.Name = column.Name.clone();
                    part.Offset = column.Offset;
                    part.UseChangingType = index_only;
                }
            }
            table.Indices.push(index);
        }
        if index_only {
            table.Columns[old.Offset as usize].ChangingFieldType = Some(new.FieldType.clone());
        } else {
            table.Columns.push(column.clone());
            args.changing_column = Some(Box::new(column));
        }
        args.old_column_id = old.ID;
        job.reorg_meta
            .get_or_insert_with(DDLReorgMeta::default)
            .SQLMode = job.sql_mode;
    }
    let changing_id = if index_only {
        old.ID
    } else {
        args.changing_column.as_ref().unwrap().ID
    };
    let offset = table
        .Columns
        .iter()
        .position(|c| c.ID == changing_id)
        .ok_or("changing column not found")?;
    let index_ids: Vec<_> = table
        .Indices
        .iter()
        .filter(|i| i.IsChanging() && i.HasColumnInIndexColumns(table, changing_id))
        .map(|i| i.ID)
        .collect();
    args.changing_indexes = table
        .Indices
        .iter()
        .filter(|i| index_ids.contains(&i.ID))
        .cloned()
        .collect();
    let state = if index_only {
        job.schema_state
    } else {
        table.Columns[offset].State
    };
    let next = match state {
        SchemaState::None => {
            crate::persistent_actions::initialize_reorg_indexes(
                context,
                job,
                &mut args.changing_indexes,
            )
            .map_err(|e| {
                job.state = JobState::Rollingback;
                e
            })?;
            job.reorg_meta.as_mut().unwrap().Stage = ReorgStage::ReorgStageModifyColumnUpdateColumn;
            if old.GetFlag() & astersql_parser_mysql::r#type::NotNullFlag == 0
                && new.GetFlag() & astersql_parser_mysql::r#type::NotNullFlag != 0
            {
                let old_col = table.Columns.iter_mut().find(|c| c.ID == old.ID).unwrap();
                old_col.SetFlag(
                    old_col.GetFlag() | astersql_parser_mysql::r#type::PreventNullInsertFlag,
                );
            }
            SchemaState::DeleteOnly
        }
        SchemaState::DeleteOnly => {
            if index_only {
                let rows = check_data(context, job, table, old, new)?;
                if let Some(row) = rows.first() {
                    job.state = JobState::Rollingback;
                    return Err(data_error(old, row));
                }
            }
            SchemaState::WriteOnly
        }
        SchemaState::WriteOnly => {
            job.snapshot_ver = 0;
            SchemaState::WriteReorganization
        }
        SchemaState::WriteReorganization => {
            let meta = job
                .reorg_meta
                .as_ref()
                .ok_or("modify-column reorg metadata missing")?;
            if meta.Stage != ReorgStage::ReorgStageModifyColumnCompleted {
                if index_only && meta.Stage == ReorgStage::ReorgStageModifyColumnUpdateColumn {
                    job.snapshot_ver = 0;
                    job.reorg_meta.as_mut().unwrap().Stage =
                        ReorgStage::ReorgStageModifyColumnRecreateIndex;
                    save_args(job, args)?;
                    return Ok(0);
                }
                let indexes = meta.Stage == ReorgStage::ReorgStageModifyColumnRecreateIndex;
                let merge_backend = indexes && meta.ReorgTp.NeedMergeProcess();
                let backfill_state = index_ids
                    .first()
                    .and_then(|id| table.Indices.iter().find(|index| index.ID == *id))
                    .map(|index| index.BackfillState)
                    .unwrap_or_default();
                if merge_backend && backfill_state == astersql_meta_model::BackfillStateReadyToMerge
                {
                    for index in table
                        .Indices
                        .iter_mut()
                        .filter(|index| index_ids.contains(&index.ID))
                    {
                        index.BackfillState = astersql_meta_model::BackfillStateMerging;
                    }
                    job.snapshot_ver = 0;
                    save_args(job, args)?;
                    return write_table(context, job, table);
                }
                let merging =
                    merge_backend && backfill_state == astersql_meta_model::BackfillStateMerging;
                let physical_ids: Vec<_> = table
                    .GetPartitionInfo()
                    .map(|p| p.Definitions.iter().map(|d| d.ID).collect())
                    .unwrap_or_else(|| vec![table.ID]);
                let rows=context.query(&format!("select ele_id,HEX(ele_type),HEX(start_key),HEX(end_key),physical_id from mysql.tidb_ddl_reorg where job_id={}",job.id),"get_handle")?;
                if rows.is_empty() {
                    initialize_cursor(
                        context,
                        job,
                        physical_ids[0],
                        if indexes {
                            *index_ids.first().ok_or("missing changing index")?
                        } else {
                            changing_id
                        },
                        indexes,
                        merging,
                    )?;
                    if job.snapshot_ver == 0 {
                        context.with_transaction(&mut |txn| {
                            job.snapshot_ver = txn.StartTS();
                            Ok(Vec::new())
                        })?;
                    }
                    save_args(job, args)?;
                    return Ok(0);
                }
                let row = &rows[0];
                let start = decode_hex(&row[2])?;
                let end = decode_hex(&row[3])?;
                let physical = row[4].parse::<i64>().map_err(|e| e.to_string())?;
                let batch = job.reorg_meta.as_ref().unwrap().GetBatchSize().max(1) as usize;
                let (next, count) = if indexes {
                    let request = crate::backfilling::IndexBackfillBatch {
                        schema_id: job.schema_id,
                        table_id: table.ID,
                        index_ids: index_ids.clone(),
                        task: crate::backfilling::ReorgBackfillTask {
                            physical_table_id: physical,
                            start_key: start,
                            end_key: end.clone(),
                            ..Default::default()
                        },
                        batch_size: batch,
                        resource_group: job.reorg_meta.as_ref().unwrap().ResourceGroupName.clone(),
                        sql_mode: job.sql_mode as i64,
                    };
                    let result = if merging {
                        context.merge_modified_indexes(request, job)?
                    } else if job.reorg_meta.as_ref().unwrap().ReorgTp == ReorgType::ReorgTypeIngest
                    {
                        context.ingest_modified_indexes(request, job)?
                    } else {
                        context.backfill_prepared_indexes(request)?
                    };
                    (result.next_key, result.scan_count)
                } else {
                    context.backfill_modified_column(
                        table,
                        old,
                        &table.Columns[offset],
                        physical,
                        &start,
                        &end,
                        batch,
                        job.sql_mode,
                        job.reorg_meta
                            .as_ref()
                            .and_then(|meta| meta.Location.as_deref()),
                    )?
                };
                job.set_row_count(job.get_row_count() + count);
                context.query(
                    &format!(
                        "UPDATE mysql.tidb_ddl_reorg SET start_key=X'{}' WHERE job_id={}",
                        hex(&next),
                        job.id
                    ),
                    "update_handle",
                )?;
                if next < end {
                    save_args(job, args)?;
                    return Ok(0);
                }
                let position = physical_ids
                    .iter()
                    .position(|id| *id == physical)
                    .ok_or("invalid modify-column physical ID")?;
                if let Some(next) = physical_ids.get(position + 1) {
                    initialize_cursor(
                        context,
                        job,
                        *next,
                        if indexes { index_ids[0] } else { changing_id },
                        indexes,
                        merging,
                    )?;
                    save_args(job, args)?;
                    return Ok(0);
                }
                context.query(
                    &format!("DELETE FROM mysql.tidb_ddl_reorg WHERE job_id={}", job.id),
                    "clean_handle",
                )?;
                if merge_backend {
                    for index in table
                        .Indices
                        .iter_mut()
                        .filter(|index| index_ids.contains(&index.ID))
                    {
                        index.BackfillState = if merging {
                            astersql_meta_model::BackfillStateInapplicable
                        } else {
                            astersql_meta_model::BackfillStateReadyToMerge
                        };
                    }
                    if !merging {
                        save_args(job, args)?;
                        return write_table(context, job, table);
                    }
                    job.reorg_meta.as_mut().unwrap().Stage =
                        ReorgStage::ReorgStageModifyColumnCompleted;
                    save_args(job, args)?;
                    return write_table(context, job, table);
                }
                job.reorg_meta.as_mut().unwrap().Stage = if !indexes && !index_ids.is_empty() {
                    ReorgStage::ReorgStageModifyColumnRecreateIndex
                } else {
                    ReorgStage::ReorgStageModifyColumnCompleted
                };
                save_args(job, args)?;
                return Ok(0);
            }
            if job.reorg_meta.as_ref().unwrap().AnalyzeState
                == astersql_meta_model::group_3::AnalyzeStateNone
            {
                // The parent performs ANALYZE for a multi-schema change.
                let enable = job
                    .session_vars
                    .get(astersql_sessionctx_vardef::TiDBEnableDDLAnalyze)
                    .map(|value| value.eq_ignore_ascii_case("ON") || value == "1")
                    .unwrap_or(astersql_sessionctx_vardef::DefTiDBEnableDDLAnalyze);
                let version = job
                    .session_vars
                    .get(astersql_sessionctx_vardef::TiDBAnalyzeVersion)
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(astersql_sessionctx_vardef::DefTiDBAnalyzeVersion);
                let analyze = job.multi_schema_info.is_none()
                    && enable
                    && version == 2
                    && table.Partition.is_none()
                    && table
                        .Indices
                        .iter()
                        .any(|index| index.State == SchemaState::WriteReorganization);
                job.reorg_meta.as_mut().unwrap().AnalyzeState = if analyze {
                    astersql_meta_model::group_3::AnalyzeStateRunning
                } else {
                    astersql_meta_model::group_3::AnalyzeStateSkipped
                };
                if !analyze {
                    if let Some(info) = job.multi_schema_info.as_mut() {
                        info.revertible = false;
                    }
                }
                save_args(job, args)?;
                return Ok(0);
            }
            if job.reorg_meta.as_ref().unwrap().AnalyzeState
                == astersql_meta_model::group_3::AnalyzeStateRunning
            {
                let state = context.analyze_modified_table(job, table)?;
                job.reorg_meta.as_mut().unwrap().AnalyzeState = state;
                save_args(job, args)?;
                return Ok(0);
            }
            let old_offset = table.Columns.iter().position(|c| c.ID == old.ID).unwrap();
            if index_only {
                let mut replacement = new.clone();
                replacement.ID = old.ID;
                replacement.Offset = old.Offset;
                replacement.State = SchemaState::Public;
                replacement.ChangingFieldType = Some(old.FieldType.clone());
                table.Columns[old_offset] = replacement;
            } else {
                table.Columns[old_offset].State = SchemaState::WriteOnly;
                table.Columns[old_offset].Name = astersql_meta_model::ast::NewCIStr(
                    &astersql_meta_model::GenRemovingObjName(&old.Name.O),
                );
                table.Columns[offset].Name = new.Name.clone();
                table.Columns[offset].State = SchemaState::Public;
                table.Columns[offset].ChangeStateInfo = None;
            }
            let dest = destination(table, old, args.position.as_ref())?;
            table.MoveColumnInfo(offset, dest);
            let new_offset = table
                .Columns
                .iter()
                .position(|c| c.ID == changing_id)
                .unwrap();
            for index in &mut table.Indices {
                if index_ids.contains(&index.ID) {
                    index.Name = astersql_meta_model::ast::NewCIStr(&index.GetChangingOriginName());
                    index.State = SchemaState::Public;
                    for part in &mut index.Columns {
                        if part.Name.L
                            == if index_only {
                                old.Name.L.as_str()
                            } else {
                                args.changing_column.as_ref().unwrap().Name.L.as_str()
                            }
                        {
                            part.Name = new.Name.clone();
                            part.Offset = new_offset as isize;
                            part.UseChangingType = false;
                        }
                    }
                } else if index.Columns.iter().any(|part| part.Name.L == old.Name.L) {
                    index.State = SchemaState::WriteOnly;
                    if index_only {
                        for part in &mut index.Columns {
                            if part.Name.L == old.Name.L {
                                part.Name = new.Name.clone();
                                part.Offset = new_offset as isize;
                                part.UseChangingType = true;
                            }
                        }
                    }
                    index.Name = astersql_meta_model::ast::NewCIStr(
                        &astersql_meta_model::GenRemovingObjName(&index.Name.O),
                    );
                }
            }
            if let Some(ttl) = table
                .TTLInfo
                .as_mut()
                .filter(|ttl| ttl.ColumnName.L == old.Name.L)
            {
                ttl.ColumnName = new.Name.clone();
            }
            job.schema_state = SchemaState::Public;
            if !index_only {
                args.changing_column = Some(Box::new(
                    table
                        .Columns
                        .iter()
                        .find(|c| c.ID == changing_id)
                        .unwrap()
                        .clone(),
                ));
            }
            save_args(job, args)?;
            return write_table(context, job, table);
        }
        other => return Err(format!("invalid changing column state {other}")),
    };
    if !index_only {
        table.Columns[offset].State = next;
    }
    for index in &mut table.Indices {
        if let Some(updated) = args.changing_indexes.iter().find(|i| i.ID == index.ID) {
            *index = updated.clone();
            index.State = next;
        }
    }
    if !index_only {
        args.changing_column = Some(Box::new(table.Columns[offset].clone()));
    }
    let version = write_table(context, job, table)?;
    job.schema_state = next;
    save_args(job, args)?;
    Ok(version)
}

pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let mut args = decode(job).map_err(|e| {
        job.state = JobState::Cancelled;
        e
    })?;
    let mut table = None;
    context.with_transaction(&mut |txn| {
        table = Some(crate::persistent_actions::public_table(
            &TransactionMutator::new(txn),
            job,
        )?);
        Ok(Vec::new())
    })?;
    let mut table = table.unwrap();
    let offset = table
        .Columns
        .iter()
        .position(|c| {
            if args.old_column_id != 0 {
                c.ID == args.old_column_id
            } else {
                c.Name.L == args.old_column_name.L
            }
        })
        .ok_or_else(|| {
            job.state = JobState::Cancelled;
            format!("unknown column {}", args.old_column_name.O)
        })?;
    let old = table.Columns[offset].clone();
    let mut new = *args
        .column
        .clone()
        .ok_or("missing modify column definition")?;
    if args.modify_column_type == 0 || args.modify_column_type == 6 {
        args.modify_column_type =
            choose_type(&table, &old, &new, job.sql_mode, args.modify_column_type);
        if matches!(old.GetType(), 15 | 253)
            && new.GetType() == 254
            && matches!(args.modify_column_type, 2 | 3)
        {
            args.modify_column_type = 5;
        }
        save_args(job, &args)?;
    }
    if job.is_rollingback() {
        let flag = table.Columns[offset].GetFlag();
        table.Columns[offset].SetFlag(flag & !astersql_parser_mysql::r#type::PreventNullInsertFlag);
        table.Columns[offset].ChangingFieldType = None;
        args.index_ids.clear();
        if args.modify_column_type == 3 {
            args.index_ids = table
                .Indices
                .iter()
                .filter(|index| index.IsChanging() && index.HasColumnInIndexColumns(&table, old.ID))
                .map(|index| index.ID)
                .collect();
            table
                .Indices
                .retain(|index| !args.index_ids.contains(&index.ID));
        }
        if let Some(changing) = &args.changing_column {
            args.index_ids = table
                .Indices
                .iter()
                .filter(|i| {
                    i.Columns.iter().any(|c| {
                        table
                            .Columns
                            .get(c.Offset as usize)
                            .is_some_and(|column| column.ID == changing.ID)
                    })
                })
                .map(|i| i.ID)
                .collect();
            table.Indices.retain(|i| !args.index_ids.contains(&i.ID));
            table.Columns.retain(|c| c.ID != changing.ID);
        }
        args.index_ids
            .extend(temporary_gc_ids(job, args.index_ids.clone()));
        args.partition_ids = table
            .GetPartitionInfo()
            .map(|p| p.Definitions.iter().map(|d| d.ID).collect())
            .unwrap_or_default();
        let version = write_table(context, job, &mut table)?;
        job.raw_args = if job.version == JobVersion::V1 {
            serde_json::to_vec(&serde_json::json!([args.index_ids, args.partition_ids, []]))
        } else {
            serde_json::to_vec(
                &serde_json::json!({"index_ids":args.index_ids,"partition_ids":args.partition_ids}),
            )
        }
        .map_err(|e| e.to_string())?;
        job.finish_table_job(
            JobState::RollbackDone,
            SchemaState::None,
            version,
            std::sync::Arc::new(table),
        );
        return Ok(version);
    }
    if args.modify_column_type == 3 && job.schema_state == SchemaState::Public {
        let removed: Vec<_> = table
            .Indices
            .iter()
            .filter(|index| index.IsRemoving())
            .map(|index| index.ID)
            .collect();
        if table
            .Indices
            .iter()
            .any(|index| index.IsRemoving() && index.State == SchemaState::WriteOnly)
        {
            for index in table.Indices.iter_mut().filter(|index| index.IsRemoving()) {
                index.State = SchemaState::DeleteOnly;
            }
            return write_table(context, job, &mut table);
        }
        table.Indices.retain(|index| !removed.contains(&index.ID));
        table.Columns[offset].ChangingFieldType = None;
        table.Columns[offset]
            .SetFlag(old.GetFlag() & !astersql_parser_mysql::r#type::PreventNullInsertFlag);
        args.index_ids = removed;
        let temporary = temporary_gc_ids(
            job,
            table
                .Indices
                .iter()
                .filter(|index| index.HasColumnInIndexColumns(&table, old.ID))
                .map(|index| index.ID),
        );
        args.index_ids.extend(temporary);
        args.index_ids.extend(&args.removed_idxs);
        args.new_index_ids.clear();
        args.partition_ids = table
            .GetPartitionInfo()
            .map(|p| p.Definitions.iter().map(|p| p.ID).collect())
            .unwrap_or_default();
        let version = write_table(context, job, &mut table)?;
        job.raw_args = if job.version == JobVersion::V1 {
            serde_json::to_vec(&serde_json::json!([args.index_ids, args.partition_ids, []]))
        } else {
            serde_json::to_vec(&args)
        }
        .map_err(|e| e.to_string())?;
        job.finish_table_job(
            JobState::Done,
            SchemaState::Public,
            version,
            std::sync::Arc::new(table),
        );
        return Ok(version);
    }
    if args.modify_column_type == 4 && job.schema_state == SchemaState::Public {
        let changing_id = args
            .changing_column
            .as_ref()
            .ok_or("missing changing column")?
            .ID;
        let mut target = table
            .Columns
            .iter()
            .find(|column| column.ID == changing_id)
            .cloned()
            .ok_or("changing column not found")?;
        target.Name = new.Name.clone();
        let related_ids: Vec<_> = table
            .Indices
            .iter()
            .filter(|index| {
                index.Columns.iter().any(|column| {
                    table
                        .Columns
                        .get(column.Offset as usize)
                        .is_some_and(|c| c.ID == old.ID)
                })
            })
            .map(|index| index.ID)
            .collect();
        match old.State {
            SchemaState::WriteOnly => {
                table.Columns[offset].State = SchemaState::DeleteOnly;
                for index in table
                    .Indices
                    .iter_mut()
                    .filter(|index| related_ids.contains(&index.ID))
                {
                    index.State = SchemaState::DeleteOnly;
                }
            }
            SchemaState::DeleteOnly => {
                table.Columns.remove(offset);
                table
                    .Indices
                    .retain(|index| !related_ids.contains(&index.ID));
                for (offset, column) in table.Columns.iter_mut().enumerate() {
                    column.Offset = offset as isize;
                }
                for index in &mut table.Indices {
                    for column in &mut index.Columns {
                        if let Some(c) = table.Columns.iter().find(|c| c.Name.L == column.Name.L) {
                            column.Offset = c.Offset;
                        }
                    }
                }
                sync_policy(context, &table, &old, &target)?;
                crate::persistent_actions::async_notify_event(
                    context,
                    job,
                    -1,
                    astersql_ddl_notifier::NewModifyColumnEvent(
                        Some(Box::new(table.clone())),
                        vec![Box::new(target.clone())],
                        false,
                    ),
                )?;
                args.index_ids = related_ids;
                args.index_ids.extend(temporary_gc_ids(
                    job,
                    table
                        .Indices
                        .iter()
                        .filter(|index| index.HasColumnInIndexColumns(&table, changing_id))
                        .map(|index| index.ID),
                ));
                args.index_ids.extend(&args.removed_idxs);
                args.partition_ids = table
                    .GetPartitionInfo()
                    .map(|p| p.Definitions.iter().map(|d| d.ID).collect())
                    .unwrap_or_default();
                args.new_index_ids = table
                    .Indices
                    .iter()
                    .filter(|index| index.HasColumnInIndexColumns(&table, changing_id))
                    .map(|index| index.ID)
                    .collect();
            }
            state => {
                return Err(format!(
                    "unexpected column state {state} in modify column job"
                ));
            }
        }
        let mut version = 0;
        context.with_transaction(&mut |txn| {
            let mut meta = TransactionMutator::new(txn);
            version = meta.gen_schema_version()?;
            meta.set_table_schema_diff(job, version)?;
            meta.update_table(job.schema_id, &mut table)?;
            Ok(Vec::new())
        })?;
        if old.State == SchemaState::DeleteOnly {
            job.raw_args = if job.version == JobVersion::V1 {
                serde_json::to_vec(&serde_json::json!([
                    args.index_ids,
                    args.partition_ids,
                    args.new_index_ids
                ]))
                .map_err(|e| e.to_string())?
            } else {
                serde_json::to_vec(&args).map_err(|e| e.to_string())?
            };
            job.finish_table_job(
                JobState::Done,
                SchemaState::Public,
                version,
                std::sync::Arc::new(table),
            );
        }
        return Ok(version);
    }
    if matches!(args.modify_column_type, 3 | 4) {
        return advance_reorg(context, job, &mut table, &mut args, &old, &new);
    }
    if matches!(args.modify_column_type, 2 | 5) {
        let prevent = astersql_parser_mysql::r#type::PreventNullInsertFlag;
        if old.ChangingFieldType.is_none() && old.GetFlag() & prevent == 0 {
            if old.GetFlag() & astersql_parser_mysql::r#type::NotNullFlag == 0
                && new.GetFlag() & astersql_parser_mysql::r#type::NotNullFlag != 0
            {
                table.Columns[offset].SetFlag(old.GetFlag() | prevent);
            }
            if !no_reorg_data_strict(&table, &old, &new) {
                table.Columns[offset].ChangingFieldType = Some(new.FieldType.clone());
            }
            return write_table(context, job, &mut table);
        }
        let rows = check_data(context, job, &table, &old, &new)?;
        {
            if let Some(row) = rows.first() {
                if args.modify_column_type == 5 {
                    table.Columns[offset].SetFlag(old.GetFlag() & !prevent);
                    table.Columns[offset].ChangingFieldType = None;
                    args.modify_column_type = 4;
                    save_args(job, &args)?;
                    return write_table(context, job, &mut table);
                }
                job.state = JobState::Rollingback;
                return Err(if row.get(1).is_some_and(|v| v == "1") {
                    "[ddl:1138]Invalid use of NULL value".into()
                } else {
                    format!(
                        "[types:1265]Data truncated for column '{}', value is '{}'",
                        old.Name.L, row[0]
                    )
                });
            }
        }
        if args.modify_column_type == 5 {
            args.modify_column_type = choose_type(&table, &old, &new, job.sql_mode, 0);
            save_args(job, &args)?;
            return Ok(0);
        }
        args.modify_column_type = 1;
    }
    if args.modify_column_type != 1 {
        return Err("MODIFY COLUMN reorganization handler unavailable".into());
    }
    if table
        .Columns
        .iter()
        .any(|c| c.ID != old.ID && c.Name.L == new.Name.L)
    {
        job.state = JobState::Rollingback;
        return Err(format!("duplicate column {}", new.Name.O));
    }
    let dest = destination(&table, &old, args.position.as_ref())?;
    new.ID = old.ID;
    new.Offset = old.Offset;
    new.State = SchemaState::Public;
    new.ChangingFieldType = None;
    new.SetFlag(new.GetFlag() & !astersql_parser_mysql::r#type::PreventNullInsertFlag);
    table.Columns[offset] = new.clone();
    for index in &mut table.Indices {
        for col in &mut index.Columns {
            if col.Name.L == old.Name.L {
                col.Name = new.Name.clone();
            }
        }
    }
    for fk in &mut table.ForeignKeys {
        for col in &mut fk.Cols {
            if col.L == old.Name.L {
                *col = new.Name.clone();
            }
        }
    }
    table.MoveColumnInfo(offset, dest);
    if let Some(ttl) = table
        .TTLInfo
        .as_mut()
        .filter(|ttl| ttl.ColumnName.L == old.Name.L)
    {
        ttl.ColumnName = new.Name.clone();
    }
    if let Err(error) = sync_policy(context, &table, &old, &new) {
        job.state = JobState::Rollingback;
        return Err(error);
    }
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.set_table_schema_diff(job, version)?;
        meta.update_table(job.schema_id, &mut table)?;
        Ok(Vec::new())
    })?;
    args.index_ids.clear();
    args.partition_ids.clear();
    args.new_index_ids.clear();
    job.raw_args = if job.version == JobVersion::V1 {
        b"[[],[],[]]".to_vec()
    } else {
        b"{}".to_vec()
    };
    job.finish_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(table),
    );
    Ok(version)
}
