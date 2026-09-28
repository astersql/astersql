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

//! Debug helpers to decode backup meta/stats files to JSON, ported from `br/pkg/metautil/debug.go`.
//!
//! 调试辅助：把备份 meta/stats 文件解密校验后落成 JSON，对齐 Go `debug.go`。
//! 路径约定写入 `jsons/<name>.json`；校验失败返回 ErrInvalidMetaFile。
//! DecodeMetaFile 只处理一层子 meta，若再嵌套 meta_files 则视为非法层级。

use std::sync::Arc;

use crate::stubs::{
    kvproto::brpb::{self, CipherInfo, MetaFile, Schema},
    protobuf::Message,
};
use astersql_br_pkg_errors::ErrInvalidMetaFile;
use astersql_br_pkg_utils::encryption::Decrypt;
use astersql_errors::{ErrorArg, SharedError, Trace};
use astersql_objstore_storeapi::{Context, Storage};
use serde_json::{Map, Value};

use crate::metafile::{hex_encode, sha256_bytes};

// 为 I/O 等错误附加 Trace 栈，便于 debug 排障。
fn trace_err(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}

fn marshal_meta_file_json(meta: &MetaFile) -> Result<Vec<u8>, SharedError> {
    let mut out = Map::new();
    if !meta.get_data_files().is_empty() {
        out.insert(
            "data_files".into(),
            Value::Array(meta.get_data_files().iter().map(file_json).collect()),
        );
    }
    if !meta.get_raw_ranges().is_empty() {
        out.insert(
            "raw_ranges".into(),
            Value::Array(
                meta.get_raw_ranges()
                    .iter()
                    .map(|range| {
                        let mut item = Map::new();
                        insert_hex(&mut item, "start_key", range.get_start_key());
                        insert_hex(&mut item, "end_key", range.get_end_key());
                        Value::Object(item)
                    })
                    .collect(),
            ),
        );
    }
    if !meta.get_schemas().is_empty() {
        out.insert(
            "schemas".into(),
            Value::Array(
                meta.get_schemas()
                    .iter()
                    .map(schema_json)
                    .collect::<Result<_, _>>()?,
            ),
        );
    }
    if !meta.get_ddls().is_empty() {
        out.insert(
            "ddls".into(),
            Value::Array(
                meta.get_ddls()
                    .iter()
                    .map(|ddl| serde_json::from_slice(ddl).map_err(SharedError::new))
                    .collect::<Result<_, _>>()?,
            ),
        );
    }
    serde_json::to_vec(&Value::Object(out)).map_err(SharedError::new)
}

fn marshal_stats_file_json(stats: &brpb::StatsFile) -> Result<Vec<u8>, SharedError> {
    let blocks = stats
        .get_blocks()
        .iter()
        .map(|block| {
            let mut item = Map::new();
            let table: Value =
                serde_json::from_slice(block.get_json_table()).map_err(SharedError::new)?;
            if !table.is_null() {
                item.insert("json_table".into(), table);
            }
            if block.get_physical_id() != 0 {
                item.insert("physical_id".into(), block.get_physical_id().into());
            }
            Ok(Value::Object(item))
        })
        .collect::<Result<Vec<_>, SharedError>>()?;
    serde_json::to_vec(&serde_json::json!({ "blocks": blocks })).map_err(SharedError::new)
}

fn file_json(file: &brpb::File) -> Value {
    let mut item = Map::new();
    insert_string(&mut item, "name", file.get_name());
    insert_string(&mut item, "cf", file.get_cf());
    insert_hex(&mut item, "sha256", file.get_sha256());
    insert_hex(&mut item, "start_key", file.get_start_key());
    insert_hex(&mut item, "end_key", file.get_end_key());
    insert_u64(&mut item, "start_version", file.get_start_version());
    insert_u64(&mut item, "end_version", file.get_end_version());
    insert_u64(&mut item, "total_kvs", file.get_total_kvs());
    insert_u64(&mut item, "total_bytes", file.get_total_bytes());
    insert_u64(&mut item, "crc64xor", file.get_crc64xor());
    insert_u64(&mut item, "size", file.get_size());
    insert_base64(&mut item, "cipher_iv", file.get_cipher_iv());
    Value::Object(item)
}

fn schema_json(schema: &Schema) -> Result<Value, SharedError> {
    let mut item = Map::new();
    let db: Value = serde_json::from_slice(schema.get_db()).map_err(SharedError::new)?;
    if !db.is_null() {
        item.insert("db".into(), db);
    }
    for (name, bytes) in [("table", schema.get_table()), ("stats", schema.get_stats())] {
        if !bytes.is_empty() {
            item.insert(
                name.into(),
                serde_json::from_slice(bytes).map_err(SharedError::new)?,
            );
        }
    }
    insert_u64(&mut item, "crc64xor", schema.get_crc64xor());
    insert_u64(&mut item, "total_kvs", schema.get_total_kvs());
    insert_u64(&mut item, "total_bytes", schema.get_total_bytes());
    insert_u64(
        &mut item,
        "tiflash_replicas",
        schema.get_tiflash_replicas() as u64,
    );
    if schema.get_is_merge_option_allowed() {
        item.insert("is_merge_option_allowed".into(), Value::Bool(true));
    }
    if !schema.get_stats_index().is_empty() {
        item.insert(
            "stats_index".into(),
            Value::Array(
                schema
                    .get_stats_index()
                    .iter()
                    .map(|index| {
                        let mut value = Map::new();
                        insert_string(&mut value, "name", index.get_name());
                        insert_base64(&mut value, "sha256", index.get_sha256());
                        insert_u64(&mut value, "size_enc", index.get_size_enc());
                        insert_u64(&mut value, "size_ori", index.get_size_ori());
                        insert_base64(&mut value, "cipher_iv", index.get_cipher_iv());
                        insert_base64(&mut value, "inline_data", index.get_inline_data());
                        Value::Object(value)
                    })
                    .collect(),
            ),
        );
    }
    if !schema.get_partition_merge_option_allowed().is_empty() {
        item.insert(
            "partition_merge_option_allowed".into(),
            serde_json::to_value(schema.get_partition_merge_option_allowed())
                .map_err(SharedError::new)?,
        );
    }
    Ok(Value::Object(item))
}

fn insert_string(out: &mut Map<String, Value>, name: &str, value: &str) {
    if !value.is_empty() {
        out.insert(name.into(), value.into());
    }
}
fn insert_u64(out: &mut Map<String, Value>, name: &str, value: u64) {
    if value != 0 {
        out.insert(name.into(), value.into());
    }
}
fn insert_hex(out: &mut Map<String, Value>, name: &str, value: &[u8]) {
    if !value.is_empty() {
        out.insert(name.into(), hex_encode(value).into());
    }
}
fn insert_base64(out: &mut Map<String, Value>, name: &str, value: &[u8]) {
    if !value.is_empty() {
        out.insert(name.into(), base64_encode(value).into());
    }
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (chunk.get(1).copied().map(u32::from).unwrap_or(0) << 8)
            | chunk.get(2).copied().map(u32::from).unwrap_or(0);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 解码产物路径格式；实际拼接亦使用 `jsons/{}.json`。
/// JSON file name format for decoded meta/stats dumps.
pub const JSONFileFormat: &str = "jsons/%s.json";

/// 遍历 schema 的 stats_index：读文件→解密→SHA256 校验→解析→写 JSON。
/// DecodeStatsFile decodes stats files referenced by schemas into JSON objects.
pub fn DecodeStatsFile(
    ctx: &Context,
    s: Arc<dyn Storage>,
    cipher: Option<&CipherInfo>,
    schemas: &[Schema],
) -> Result<(), SharedError> {
    // 每个 schema 可能引用多个 stats 文件。
    for schema in schemas {
        for statsIndex in schema.get_stats_index() {
            // 空名跳过，避免读无效对象键。
            if statsIndex.get_name().is_empty() {
                continue;
            }
            // 对象存储读取失败包装为可 Trace 的 SharedError。
            let content = s.ReadFile(ctx, statsIndex.get_name()).map_err(|err| {
                trace_err(SharedError::new(std::io::Error::other(err.to_string())))
            })?;
            // 按索引携带的 IV 与可选 cipher 解密。
            let decryptContent = Decrypt(content, cipher, statsIndex.get_cipher_iv())?;

            // 明文校验；与索引内 sha256 不一致则拒。
            let checksum = sha256_bytes(&decryptContent);
            // 校验失败：期望/实际 hex 写入错误参数。
            if statsIndex.get_sha256() != checksum {
                return Err(
                    ErrInvalidMetaFile.GenWithStackByArgs(&[ErrorArg::String(format!(
                        "checksum mismatch expect {}, got {}",
                        hex_encode(statsIndex.get_sha256()),
                        hex_encode(&checksum)
                    ))]),
                );
            }

            // 解析为 StatsFile，再序列化为“JSON 友好”字节写出。
            let statsFileBlocks =
                crate::stubs::protobuf::parse_from_bytes::<brpb::StatsFile>(&decryptContent)
                    .map_err(SharedError::new)?;
            let jsonContent = marshal_stats_file_json(&statsFileBlocks)?;
            // 输出名与源 stats 文件名对应。
            let json_name = format!("jsons/{}.json", statsIndex.get_name());
            s.WriteFile(ctx, &json_name, &jsonContent).map_err(|err| {
                trace_err(SharedError::new(std::io::Error::other(err.to_string())))
            })?;
        }
    }
    Ok(())
}

/// 解码子 MetaFile：校验后写 JSON，并递归调用 DecodeStatsFile 处理 schemas。
/// DecodeMetaFile decodes child meta files into JSON and their stats sidecars.
pub fn DecodeMetaFile(
    ctx: &Context,
    s: Arc<dyn Storage>,
    cipher: Option<&CipherInfo>,
    metaIndex: Option<&MetaFile>,
) -> Result<(), SharedError> {
    // 无索引则空操作成功，对齐 Go 对 nil 的容忍。
    let Some(metaIndex) = metaIndex else {
        return Ok(());
    };

    // 逐个 meta 节点处理；支持取消。
    for node in metaIndex.get_meta_files() {
        // 取消时返回 Interrupted，消息对齐 context canceled。
        if ctx.is_cancelled() {
            return Err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "context canceled",
            )));
        }
        // 读取子 meta 原始内容。
        let content = s
            .ReadFile(ctx, node.get_name())
            .map_err(|err| trace_err(SharedError::new(std::io::Error::other(err.to_string()))))?;
        let decryptContent = Decrypt(content, cipher, node.get_cipher_iv())?;

        let checksum = sha256_bytes(&decryptContent);
        // 校验失败返回 ErrInvalidMetaFile。
        if node.get_sha256() != checksum {
            return Err(
                ErrInvalidMetaFile.GenWithStackByArgs(&[ErrorArg::String(format!(
                    "checksum mismatch expect {}, got {}",
                    hex_encode(node.get_sha256()),
                    hex_encode(&checksum)
                ))]),
            );
        }

        // 解析子 MetaFile；禁止再包含 meta_files（仅一层）。
        let mut child = crate::stubs::protobuf::parse_from_bytes::<MetaFile>(&decryptContent)
            .map_err(SharedError::new)?;
        // 意外多层：直接报 InvalidData，避免静默丢层级。
        if !child.get_meta_files().is_empty() {
            return Err(trace_err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("the metafile has unexpected level: {child:?}"),
            ))));
        }

        // 写出 JSON 后取出 schemas 解码 stats 侧车。
        let jsonContent = marshal_meta_file_json(&child)?;
        let json_name = format!("jsons/{}.json", node.get_name());
        s.WriteFile(ctx, &json_name, &jsonContent)
            .map_err(|err| trace_err(SharedError::new(std::io::Error::other(err.to_string()))))?;

        // take 移出 schemas，避免二次借用冲突。
        let schemas = child.take_schemas();
        // 复用同一 Storage/cipher 解码统计文件。
        DecodeStatsFile(ctx, s.clone(), cipher, &schemas)?;
    }
    Ok(())
}
