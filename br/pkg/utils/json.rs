//! 设计约束：
//! 1. 不修改 protobuf 消息语义，仅转换展示层编码。
//! 2. 十六进制始终小写，与历史备份文件及测试夹具一致。
//! 3. Base64 使用标准字母表与 '=' padding。
//! 4. 往返后 JSON 逻辑相等即可，键顺序由 serde_json Map 决定。
//! 5. 解析错误统一走 SharedError，便于上层 Annotate。
//! 6. File.name/cf 为空时省略，避免噪声字段。
//! 7. total_kvs/total_bytes/crc64xor 为零同样省略。
//! 8. start_version/end_version 用于增量备份边界。
//! 9. cluster_version/br_version 保留换行，因历史字符串含多行 banner。
//! 10. 本文件无 I/O；读写外部存储由 metautil 等调用方负责。
//! 11. make_* 与 from_* 成对出现，便于对照 Go 同名逻辑。
//! 12. insert_* 辅助函数集中 omitempty 规则，避免各处分叉。
//! 13. b64_* 自实现以避免额外依赖与编解码差异。
//! 14. 整数字段保持精确类型；不接受会损失精度的浮点表示。
//! 15. Trace 仅用于解码路径，序列化路径直接 SharedError::new。
//! 16. MetaFile.backup_ranges 与 File 键编码刻意不同，移植时勿混用。
//! 17. Schema.tiflash_replicas 以 u32 存，JSON 侧按 u64 读写。
//! 18. StatsFile 结构最简，便于统计分片独立落盘。
//! 19. 单元测试夹具来自 Go json_test.go 的真实备份片段。
//! 20. 变更字段布局时需同步更新 Go 与夹具，否则往返测试失败。
//! 21. Marshal* 返回 UTF-8 JSON 字节，调用方可直接写入对象存储。
//! 22. Unmarshal* 不要求字段按固定顺序出现。
//! 23. 加密相关字段缺失视为未加密，不推导默认 IV。
//! 24. raw_ranges 可仅含 cf，表示整 CF 范围。
//! 25. files 数组顺序通常反映备份生成顺序，本模块不重排。
//! 26. schemas 中 db 与 table 可为部分填充（仅库备份）。
//! 27. crc64xor 与 SST 校验联动，JSON 层只透传数值。
//! 28. size 字段表示压缩后文件大小（字节）。
//! 29. 本模块线程安全：无共享可变状态，纯函数式转换。
//! 30. 与 key::hex_* 共用编解码，避免 hex 实现分叉。
//! 31. 错误信息不本地化，保持与 Go 测试字符串可对照。
//! 32. Value::Object 使用 serde_json Map，键序不属于公开契约。
//! 33. 大文件元数据应走流式 MetaFile，而非单份 BackupMeta。
//! 34. StatsBlock.physical_id 标识表/分区物理 ID。
//! 35. 若 JSON 根不是 object，from_* 将在字段访问时失败。
//! 36. insert_hex 对空切片跳过，避免写出空字符串键。
//! 37. b64_decode 对非法字符立即返回 invalid base64。
//! 38. 完成往返后应使用逻辑 JSON 相等比较，而非字节相等。
//! 39. 版本字段 meta.version 演进时需保持向后可读。
//! 40. 该文件无异步；CPU 开销主要在 serde 与 hex/base64。
//! 41. 调用方负责保证输入 protobuf 字段已填充完整。
//! 42. 与 Go 不一致时优先以 Go 测试夹具为真源。
// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! JSON helpers for backup metadata, ported from `br/pkg/utils/json.go`.
//!
//! 备份元数据 JSON 编解码：BackupMeta / MetaFile / StatsFile 与 Go json.go 字段布局对齐。
//! 二进制键与校验和用十六进制；cipher_iv / backup_ranges 键用标准 Base64。
//! schema.db/table/stats 与 ddls 以嵌套 JSON 对象保留，往返时再序列化为字节。
//! 零值与空切片字段在序列化时省略，以匹配 Go encoding/json omitempty 习惯。
//! 反序列化容忍 JSON number 以 i64/f64 出现，经 as_u64 统一转 u64。

use serde_json::{Map, Number, Value};

use astersql_errors::{SharedError, Trace};

use crate::kvproto::brpb::{
    BackupMeta, BackupRange, File, MetaFile, RawRange, Schema, StatsBlock, StatsFile,
};

use super::key::{hex_decode_string, hex_encode};

// 为解析错误补 Trace 包装，栈信息对齐 Go errors.Trace。
fn trace_err(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}

/// 将 BackupMeta 编为 JSON 字节；键字段已转为可读 hex/base64。
pub fn MarshalBackupMeta(meta: &BackupMeta) -> Result<Vec<u8>, SharedError> {
    let result = make_json_backup_meta(meta)?;
    serde_json::to_vec(&result).map_err(SharedError::new)
}

/// 从 JSON 还原 BackupMeta；缺省字段保持 protobuf 默认零值。
pub fn UnmarshalBackupMeta(data: &[u8]) -> Result<BackupMeta, SharedError> {
    let j_meta: Value = serde_json::from_slice(data).map_err(SharedError::new)?;
    validate_backup_meta(&j_meta)?;
    from_json_backup_meta(j_meta)
}

/// 序列化 MetaFile（含 data_files / schemas / backup_ranges）。
pub fn MarshalMetaFile(meta: &MetaFile) -> Result<Vec<u8>, SharedError> {
    let result = make_json_meta_file(meta)?;
    serde_json::to_vec(&result).map_err(SharedError::new)
}

/// 反序列化 MetaFile；backup_ranges 的键按 Base64 解码。
pub fn UnmarshalMetaFile(data: &[u8]) -> Result<MetaFile, SharedError> {
    let j_meta: Value = serde_json::from_slice(data).map_err(SharedError::new)?;
    validate_meta_file(&j_meta)?;
    from_json_meta_file(j_meta)
}

/// 序列化统计文件 blocks 列表。
pub fn MarshalStatsFile(meta: &StatsFile) -> Result<Vec<u8>, SharedError> {
    let result = make_json_stats_file(meta)?;
    serde_json::to_vec(&result).map_err(SharedError::new)
}

/// 反序列化统计文件；json_table 以嵌套对象往返。
pub fn UnmarshalStatsFile(data: &[u8]) -> Result<StatsFile, SharedError> {
    let j_meta: Value = serde_json::from_slice(data).map_err(SharedError::new)?;
    validate_stats_file(&j_meta)?;
    from_json_stats_file(j_meta)
}

fn invalid_json_type(field: &str) -> SharedError {
    SharedError::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("invalid JSON type for field {field}"),
    ))
}

fn validate_field(
    obj: &Map<String, Value>,
    field: &str,
    valid: impl Fn(&Value) -> bool,
) -> Result<(), SharedError> {
    if let Some(value) = obj.get(field)
        && !value.is_null()
        && !valid(value)
    {
        return Err(invalid_json_type(field));
    }
    Ok(())
}

fn object<'a>(value: &'a Value, field: &str) -> Result<&'a Map<String, Value>, SharedError> {
    value.as_object().ok_or_else(|| invalid_json_type(field))
}

fn validate_file(value: &Value) -> Result<(), SharedError> {
    let obj = object(value, "file")?;
    for field in ["name", "cf", "sha256", "start_key", "end_key", "cipher_iv"] {
        validate_field(obj, field, Value::is_string)?;
    }
    for field in [
        "start_version",
        "end_version",
        "total_kvs",
        "total_bytes",
        "crc64xor",
        "size",
    ] {
        validate_field(obj, field, |v| v.as_u64().is_some())?;
    }
    Ok(())
}

fn validate_raw_range(value: &Value) -> Result<(), SharedError> {
    let obj = object(value, "raw_range")?;
    for field in ["start_key", "end_key", "cf"] {
        validate_field(obj, field, Value::is_string)?;
    }
    Ok(())
}

fn validate_schema(value: &Value) -> Result<(), SharedError> {
    let obj = object(value, "schema")?;
    for field in ["crc64xor", "total_kvs", "total_bytes"] {
        validate_field(obj, field, |v| v.as_u64().is_some())?;
    }
    validate_field(obj, "tiflash_replicas", |v| {
        v.as_u64().is_some_and(|v| u32::try_from(v).is_ok())
    })?;
    validate_field(obj, "is_merge_option_allowed", Value::is_boolean)
}

fn validate_array(
    obj: &Map<String, Value>,
    field: &str,
    item: impl Fn(&Value) -> Result<(), SharedError>,
) -> Result<(), SharedError> {
    if let Some(value) = obj.get(field) {
        if value.is_null() {
            return Ok(());
        }
        for value in value.as_array().ok_or_else(|| invalid_json_type(field))? {
            item(value)?;
        }
    }
    Ok(())
}

fn validate_backup_meta(value: &Value) -> Result<(), SharedError> {
    let obj = object(value, "backup_meta")?;
    for field in ["cluster_version", "br_version", "new_collations_enabled"] {
        validate_field(obj, field, Value::is_string)?;
    }
    for field in ["cluster_id", "start_version", "end_version"] {
        validate_field(obj, field, |v| v.as_u64().is_some())?;
    }
    validate_field(obj, "version", |v| {
        v.as_i64().is_some_and(|v| i32::try_from(v).is_ok())
    })?;
    validate_field(obj, "is_raw_kv", Value::is_boolean)?;
    validate_array(obj, "files", validate_file)?;
    validate_array(obj, "raw_ranges", validate_raw_range)?;
    validate_array(obj, "schemas", validate_schema)
}

fn validate_meta_file(value: &Value) -> Result<(), SharedError> {
    let obj = object(value, "meta_file")?;
    validate_array(obj, "data_files", validate_file)?;
    validate_array(obj, "raw_ranges", validate_raw_range)?;
    validate_array(obj, "schemas", validate_schema)?;
    validate_array(obj, "ddls", |_| Ok(()))?;
    validate_array(obj, "backup_ranges", |range| {
        let range = object(range, "backup_range")?;
        validate_field(range, "start_key", Value::is_string)?;
        validate_field(range, "end_key", Value::is_string)
    })
}

fn validate_stats_file(value: &Value) -> Result<(), SharedError> {
    let obj = object(value, "stats_file")?;
    validate_array(obj, "blocks", |block| {
        let block = object(block, "stats_block")?;
        validate_field(block, "physical_id", |v| v.as_i64().is_some())
    })
}

// File → JSON：标量 omitempty，sha256/start/end 强制 hex。
fn make_json_file(file: &File) -> Value {
    let mut obj = file_to_map(file);
    insert_hex(&mut obj, "sha256", file.get_sha256());
    // 校验和与区间键对人可读，便于 debug 元数据。
    insert_hex(&mut obj, "start_key", file.get_start_key());
    insert_hex(&mut obj, "end_key", file.get_end_key());
    if !file.get_cipher_iv().is_empty() {
        // IV 用 Base64：与 Go json 中 []byte 默认编码一致。
        obj.insert(
            "cipher_iv".into(),
            Value::String(b64_encode(file.get_cipher_iv())),
        );
    }
    Value::Object(obj)
}

// JSON → File：cipher_iv 走 Base64，其余键字段走 hex。
fn from_json_file(value: &Value) -> Result<File, SharedError> {
    let mut file = map_to_file(value_as_object(value)?)?;
    if let Some(v) = value.get("sha256").and_then(Value::as_str) {
        file.set_sha256(hex_decode_string(v).map_err(trace_err)?);
        // hex 解码失败经 Trace 包装后上抛。
    }
    if let Some(v) = value.get("start_key").and_then(Value::as_str) {
        file.set_start_key(hex_decode_string(v).map_err(trace_err)?);
    }
    if let Some(v) = value.get("end_key").and_then(Value::as_str) {
        file.set_end_key(hex_decode_string(v).map_err(trace_err)?);
    }
    if let Some(v) = value.get("cipher_iv").and_then(Value::as_str) {
        // 加密文件才有 cipher_iv；缺失表示明文 SST。
        file.set_cipher_iv(b64_decode(v).map_err(trace_err)?);
    }
    Ok(file)
}

// RawRange：仅 start/end/cf；用于 raw KV 备份元数据。
fn make_json_raw_range(raw: &RawRange) -> Value {
    let mut obj = Map::new();
    insert_hex(&mut obj, "start_key", raw.get_start_key());
    insert_hex(&mut obj, "end_key", raw.get_end_key());
    insert_string(&mut obj, "cf", raw.get_cf());
    Value::Object(obj)
}

// 缺字段保持默认空；cf 为普通字符串。
fn from_json_raw_range(value: &Value) -> Result<RawRange, SharedError> {
    let mut raw = RawRange::default();
    if let Some(v) = value.get("start_key").and_then(Value::as_str) {
        raw.set_start_key(hex_decode_string(v).map_err(trace_err)?);
    }
    if let Some(v) = value.get("end_key").and_then(Value::as_str) {
        raw.set_end_key(hex_decode_string(v).map_err(trace_err)?);
    }
    if let Some(v) = value.get("cf").and_then(Value::as_str) {
        raw.set_cf(v.to_string());
    }
    Ok(raw)
}

// Schema：db/table/stats 字节载荷解析为嵌套 JSON 再嵌入。
// 空字节跳过，避免写入 null/空串破坏 omitempty 约定。
fn make_json_schema(schema: &Schema) -> Result<Value, SharedError> {
    let mut obj = schema_to_map(schema);
    let db: Value = serde_json::from_slice(schema.get_db()).map_err(SharedError::new)?;
    if !db.is_null() {
        obj.insert("db".into(), db);
    }
    if !schema.get_table().is_empty() {
        obj.insert(
            "table".into(),
            serde_json::from_slice(schema.get_table()).map_err(SharedError::new)?,
        );
    }
    if !schema.get_stats().is_empty() {
        obj.insert(
            "stats".into(),
            serde_json::from_slice(schema.get_stats()).map_err(SharedError::new)?,
        );
    }
    Ok(Value::Object(obj))
}

// 嵌套对象再 to_vec 写回 Schema 字节字段；table/stats 允许显式 null 跳过。
fn from_json_schema(value: &Value) -> Result<Schema, SharedError> {
    let mut schema = Schema::default();
    if let Some(v) = value.get("crc64xor").and_then(as_u64) {
        schema.set_crc64xor(v);
    }
    if let Some(v) = value.get("total_kvs").and_then(as_u64) {
        schema.set_total_kvs(v);
    }
    if let Some(v) = value.get("total_bytes").and_then(as_u64) {
        schema.set_total_bytes(v);
    }
    if let Some(v) = value.get("tiflash_replicas").and_then(as_u64) {
        schema.set_tiflash_replicas(v as u32);
    }
    if let Some(v) = value
        .get("is_merge_option_allowed")
        .and_then(Value::as_bool)
    {
        schema.set_is_merge_option_allowed(v);
    }
    let db = value.get("db").unwrap_or(&Value::Null);
    schema.set_db(serde_json::to_vec(db).map_err(SharedError::new)?);
    if let Some(table) = value.get("table") {
        // null table 表示仅库级 schema（无表信息）。
        if !table.is_null() {
            schema.set_table(serde_json::to_vec(table).map_err(SharedError::new)?);
        }
    }
    if let Some(stats) = value.get("stats") {
        // stats 可为 null；非 null 时整棵 JSON 树写回字节。
        if !stats.is_null() {
            schema.set_stats(serde_json::to_vec(stats).map_err(SharedError::new)?);
        }
    }
    Ok(schema)
}

// BackupMeta 聚合：files/raw_ranges/schemas 数组 + ddls 嵌套 JSON。
fn make_json_backup_meta(meta: &BackupMeta) -> Result<Value, SharedError> {
    let mut obj = backup_meta_to_map(meta);
    if !meta.get_files().is_empty() {
        obj.insert(
            "files".into(),
            Value::Array(meta.get_files().iter().map(make_json_file).collect()),
        );
    }
    if !meta.get_raw_ranges().is_empty() {
        obj.insert(
            "raw_ranges".into(),
            Value::Array(
                meta.get_raw_ranges()
                    .iter()
                    .map(make_json_raw_range)
                    .collect(),
            ),
        );
    }
    if !meta.get_schemas().is_empty() {
        let mut schemas = Vec::new();
        for schema in meta.get_schemas() {
            schemas.push(make_json_schema(schema)?);
        }
        obj.insert("schemas".into(), Value::Array(schemas));
    }
    // Go 始终解析 DDL 载荷；空字节不是合法 JSON，必须报错。
    let ddls: Value = serde_json::from_slice(meta.get_ddls()).map_err(SharedError::new)?;
    if !ddls.is_null() {
        obj.insert("ddls".into(), ddls);
    }
    Ok(Value::Object(obj))
}

// 逐字段填充；version 以 i64 读入再转 i32，兼容 JSON 数字类型。
fn from_json_backup_meta(value: Value) -> Result<BackupMeta, SharedError> {
    let mut meta = BackupMeta::default();
    if let Some(v) = value.get("cluster_id").and_then(as_u64) {
        meta.set_cluster_id(v);
    }
    if let Some(v) = value.get("cluster_version").and_then(Value::as_str) {
        meta.set_cluster_version(v.to_string());
    }
    if let Some(v) = value.get("br_version").and_then(Value::as_str) {
        meta.set_br_version(v.to_string());
    }
    if let Some(v) = value.get("start_version").and_then(as_u64) {
        meta.set_start_version(v);
    }
    if let Some(v) = value.get("end_version").and_then(as_u64) {
        meta.set_end_version(v);
    }
    if let Some(v) = value.get("version").and_then(Value::as_i64) {
        meta.set_version(v as i32);
    }
    if let Some(v) = value.get("is_raw_kv").and_then(Value::as_bool) {
        // raw KV 备份标记，影响恢复路径选择。
        meta.set_is_raw_kv(v);
    }
    if let Some(v) = value.get("new_collations_enabled").and_then(Value::as_str) {
        // 排序规则开关以字符串 "True"/"False" 存盘，保持历史兼容。
        meta.set_new_collations_enabled(v.to_string());
    }
    if let Some(files) = value.get("files").and_then(Value::as_array) {
        for file in files {
            meta.mut_files().push(from_json_file(file)?);
            // 任一 file 解析失败即中止，避免部分成功。
        }
    }
    if let Some(raw_ranges) = value.get("raw_ranges").and_then(Value::as_array) {
        for raw_range in raw_ranges {
            meta.mut_raw_ranges().push(from_json_raw_range(raw_range)?);
        }
    }
    if let Some(schemas) = value.get("schemas").and_then(Value::as_array) {
        for schema in schemas {
            meta.mut_schemas().push(from_json_schema(schema)?);
            // schemas 与 files 独立数组，顺序保持 JSON 原序。
        }
    }
    let ddls = value.get("ddls").unwrap_or(&Value::Null);
    meta.set_ddls(serde_json::to_vec(ddls).map_err(SharedError::new)?);
    Ok(meta)
}

// MetaFile：ddls 为字节数组逐条解析；backup_ranges 键用 Base64。
// 注意与 BackupMeta.ddls（单块 JSON）编码形态不同，需保持分支分离。
fn make_json_meta_file(meta: &MetaFile) -> Result<Value, SharedError> {
    let mut obj = Map::new();
    if !meta.get_data_files().is_empty() {
        obj.insert(
            "data_files".into(),
            Value::Array(meta.get_data_files().iter().map(make_json_file).collect()),
        );
    }
    if !meta.get_raw_ranges().is_empty() {
        obj.insert(
            "raw_ranges".into(),
            Value::Array(
                meta.get_raw_ranges()
                    .iter()
                    .map(make_json_raw_range)
                    .collect(),
            ),
        );
    }
    if !meta.get_schemas().is_empty() {
        let mut schemas = Vec::new();
        for schema in meta.get_schemas() {
            schemas.push(make_json_schema(schema)?);
        }
        obj.insert("schemas".into(), Value::Array(schemas));
    }
    if !meta.get_ddls().is_empty() {
        let mut ddls = Vec::new();
        for ddl in meta.get_ddls() {
            ddls.push(serde_json::from_slice(ddl).map_err(SharedError::new)?);
            // MetaFile：每条 ddl 字节独立解析为 JSON 值。
        }
        obj.insert("ddls".into(), Value::Array(ddls));
    }
    if !meta.get_backup_ranges().is_empty() {
        let mut ranges = Vec::new();
        for r in meta.get_backup_ranges() {
            let mut m = Map::new();
            if !r.get_start_key().is_empty() {
                // backup_ranges 与 File 键不同：此处用 Base64 而非 hex。
                m.insert(
                    "start_key".into(),
                    Value::String(b64_encode(r.get_start_key())),
                );
            }
            if !r.get_end_key().is_empty() {
                m.insert("end_key".into(), Value::String(b64_encode(r.get_end_key())));
            }
            ranges.push(Value::Object(m));
        }
        obj.insert("backup_ranges".into(), Value::Array(ranges));
    }
    Ok(Value::Object(obj))
}

// 还原 MetaFile；backup_ranges 缺失 start/end 时保持空键。
fn from_json_meta_file(value: Value) -> Result<MetaFile, SharedError> {
    let mut meta = MetaFile::default();
    if let Some(files) = value.get("data_files").and_then(Value::as_array) {
        for file in files {
            meta.mut_data_files().push(from_json_file(file)?);
        }
    }
    if let Some(raw_ranges) = value.get("raw_ranges").and_then(Value::as_array) {
        for raw_range in raw_ranges {
            meta.mut_raw_ranges().push(from_json_raw_range(raw_range)?);
        }
    }
    if let Some(schemas) = value.get("schemas").and_then(Value::as_array) {
        for schema in schemas {
            meta.mut_schemas().push(from_json_schema(schema)?);
        }
    }
    if let Some(ddls) = value.get("ddls").and_then(Value::as_array) {
        for ddl in ddls {
            meta.mut_ddls()
                .push(serde_json::to_vec(ddl).map_err(SharedError::new)?);
            // 数组元素写回字节切片。
        }
    }
    if let Some(ranges) = value.get("backup_ranges").and_then(Value::as_array) {
        for r in ranges {
            let mut br = BackupRange::default();
            if let Some(v) = r.get("start_key").and_then(Value::as_str) {
                br.set_start_key(b64_decode(v).map_err(trace_err)?);
            }
            if let Some(v) = r.get("end_key").and_then(Value::as_str) {
                br.set_end_key(b64_decode(v).map_err(trace_err)?);
            }
            meta.mut_backup_ranges().push(br);
        }
    }
    Ok(meta)
}

// StatsBlock：json_table 嵌套对象；physical_id 为零则省略。
fn make_json_stats_block(stats_block: &StatsBlock) -> Result<Value, SharedError> {
    let mut obj = Map::new();
    let json_table: Value =
        serde_json::from_slice(stats_block.get_json_table()).map_err(SharedError::new)?;
    if !json_table.is_null() {
        obj.insert("json_table".into(), json_table);
    }
    if stats_block.get_physical_id() != 0 {
        obj.insert(
            "physical_id".into(),
            Value::Number(stats_block.get_physical_id().into()),
        );
    }
    Ok(Value::Object(obj))
}

// physical_id 以 i64 读入，对齐 Go int64 JSON 数字。
fn from_json_stats_block(value: &Value) -> Result<StatsBlock, SharedError> {
    let mut block = StatsBlock::default();
    let json_table = value.get("json_table").unwrap_or(&Value::Null);
    block.set_json_table(serde_json::to_vec(json_table).map_err(SharedError::new)?);
    if let Some(v) = value.get("physical_id").and_then(Value::as_i64) {
        block.set_physical_id(v);
    }
    Ok(block)
}

// StatsFile 仅含 blocks 数组，结构刻意保持扁平。
fn make_json_stats_file(stats_file: &StatsFile) -> Result<Value, SharedError> {
    let mut blocks = Vec::new();
    for block in stats_file.get_blocks() {
        blocks.push(make_json_stats_block(block)?);
    }
    Ok(Value::Object(Map::from_iter([(
        "blocks".into(),
        Value::Array(blocks),
    )])))
}

// 无 blocks 字段时返回空 StatsFile。
fn from_json_stats_file(value: Value) -> Result<StatsFile, SharedError> {
    let mut meta = StatsFile::default();
    if let Some(blocks) = value.get("blocks").and_then(Value::as_array) {
        for block in blocks {
            meta.mut_blocks().push(from_json_stats_block(block)?);
        }
    }
    Ok(meta)
}

// 期望 JSON object；否则 InvalidData，避免后续 get 静默失败。
fn value_as_object(value: &Value) -> Result<&Map<String, Value>, SharedError> {
    value.as_object().ok_or_else(|| {
        SharedError::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected object",
        ))
    })
}

// 空串省略，模拟 Go omitempty。
fn insert_string(map: &mut Map<String, Value>, key: &str, value: &str) {
    if !value.is_empty() {
        map.insert(key.into(), Value::String(value.to_string()));
    }
}

// 零值省略；非零写入 Number。
fn insert_u64(map: &mut Map<String, Value>, key: &str, value: u64) {
    if value != 0 {
        map.insert(key.into(), Value::Number(Number::from(value)));
        // serde_json::Number 保持整数精确表示。
    }
}

// 非空字节写小写 hex 字符串。
fn insert_hex(map: &mut Map<String, Value>, key: &str, bytes: &[u8]) {
    if !bytes.is_empty() {
        map.insert(key.into(), Value::String(hex_encode(bytes)));
    }
}

// File 标量字段映射；不含 sha256/键/iv（由上层补）。
fn file_to_map(file: &File) -> Map<String, Value> {
    let mut obj = Map::new();
    insert_string(&mut obj, "name", file.get_name());
    insert_string(&mut obj, "cf", file.get_cf());
    insert_u64(&mut obj, "total_kvs", file.get_total_kvs());
    insert_u64(&mut obj, "total_bytes", file.get_total_bytes());
    insert_u64(&mut obj, "crc64xor", file.get_crc64xor());
    insert_u64(&mut obj, "end_version", file.get_end_version());
    insert_u64(&mut obj, "start_version", file.get_start_version());
    insert_u64(&mut obj, "size", file.get_size());
    obj
}

// 从 JSON object 还原 File 标量；缺省保持默认。
fn map_to_file(map: &Map<String, Value>) -> Result<File, SharedError> {
    let mut file = File::default();
    if let Some(v) = map.get("name").and_then(Value::as_str) {
        file.set_name(v.to_string());
    }
    if let Some(v) = map.get("cf").and_then(Value::as_str) {
        file.set_cf(v.to_string());
    }
    if let Some(v) = map.get("total_kvs").and_then(as_u64) {
        file.set_total_kvs(v);
    }
    if let Some(v) = map.get("total_bytes").and_then(as_u64) {
        file.set_total_bytes(v);
    }
    if let Some(v) = map.get("crc64xor").and_then(as_u64) {
        file.set_crc64xor(v);
    }
    if let Some(v) = map.get("end_version").and_then(as_u64) {
        file.set_end_version(v);
    }
    if let Some(v) = map.get("start_version").and_then(as_u64) {
        file.set_start_version(v);
    }
    if let Some(v) = map.get("size").and_then(as_u64) {
        file.set_size(v);
    }
    Ok(file)
}

// Schema 标量；is_merge_option_allowed 仅在 true 时写出。
fn schema_to_map(schema: &Schema) -> Map<String, Value> {
    let mut obj = Map::new();
    insert_u64(&mut obj, "crc64xor", schema.get_crc64xor());
    insert_u64(&mut obj, "total_kvs", schema.get_total_kvs());
    insert_u64(&mut obj, "total_bytes", schema.get_total_bytes());
    insert_u64(
        &mut obj,
        "tiflash_replicas",
        schema.get_tiflash_replicas() as u64,
    );
    if schema.get_is_merge_option_allowed() {
        // false 时省略字段，减小元数据体积。
        obj.insert("is_merge_option_allowed".into(), Value::Bool(true));
    }
    obj
}

// BackupMeta 标量头；is_raw_kv 仅 true 时出现。
fn backup_meta_to_map(meta: &BackupMeta) -> Map<String, Value> {
    let mut obj = Map::new();
    insert_u64(&mut obj, "cluster_id", meta.get_cluster_id());
    insert_string(&mut obj, "cluster_version", meta.get_cluster_version());
    insert_u64(&mut obj, "start_version", meta.get_start_version());
    insert_u64(&mut obj, "end_version", meta.get_end_version());
    insert_string(&mut obj, "br_version", meta.get_br_version());
    if meta.get_version() != 0 {
        // version=0 表示旧格式缺省，不写出。
        obj.insert(
            "version".into(),
            Value::Number(Number::from(meta.get_version())),
        );
    }
    if meta.get_is_raw_kv() {
        // 与 Go omitempty bool 一致：false 不出现在 JSON。
        obj.insert("is_raw_kv".into(), Value::Bool(true));
    }
    insert_string(
        &mut obj,
        "new_collations_enabled",
        meta.get_new_collations_enabled(),
    );
    obj
}

// 内部取值辅助；公开入口已按 Go 的整数类型规则完成严格校验。
fn as_u64(value: &Value) -> Option<u64> {
    value.as_u64()
}

// 标准 Base64（含 padding），无依赖外部 crate，保证与 Go encoding/base64 一致。
fn b64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        // 每 3 字节 → 4 字符。
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = data.len() - i;
    // 处理末尾 1/2 字节剩余并补 '='。
    if rem == 1 {
        let n = (data[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

// 对齐 Go encoding/base64.StdEncoding：只忽略 CR/LF，且要求正确 padding。
fn b64_decode(text: &str) -> Result<Vec<u8>, SharedError> {
    fn val(c: u8) -> Result<u8, SharedError> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid base64",
            ))),
        }
    }
    let bytes: Vec<u8> = text
        .bytes()
        .filter(|b| !matches!(b, b'\r' | b'\n'))
        .collect();
    if bytes.len() % 4 != 0 {
        return Err(SharedError::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid base64 length",
        )));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let last = i + 4 == bytes.len();
        let padding = match (bytes[i + 2], bytes[i + 3]) {
            (b'=', b'=') => 2,
            (_, b'=') => 1,
            _ => 0,
        };
        if (!last && padding != 0) || bytes[i] == b'=' || bytes[i + 1] == b'=' {
            return Err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid base64 padding",
            )));
        }
        if padding == 0 && bytes[i + 2] == b'=' {
            return Err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid base64 padding",
            )));
        }
        let n = ((val(bytes[i])? as u32) << 18)
            | ((val(bytes[i + 1])? as u32) << 12)
            | if padding < 2 {
                (val(bytes[i + 2])? as u32) << 6
            } else {
                0
            }
            | if padding == 0 {
                val(bytes[i + 3])? as u32
            } else {
                0
            };
        out.push(((n >> 16) & 0xff) as u8);
        if padding < 2 {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if padding == 0 {
            out.push((n & 0xff) as u8);
        }
        i += 4;
    }
    Ok(out)
}
