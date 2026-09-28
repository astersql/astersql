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

//! Go-equivalent tests for `br/pkg/metautil/debug_test.go`.
//!
//! 对齐 Go `TestDecodeMetaFile`：用内存 Storage 写入加密后的 MetaFile / StatsFile，
//! 再调用 `DecodeMetaFile` 验证其会把叶子元数据与统计块解码到 `jsons/*.json`。
//! 场景覆盖两类叶子：仅含 SST 数据文件索引，以及含 Schema + StatsFileIndex 的 schema 元数据。
//! 断言依据是解码后 protobuf 字段与构造 fixture 时写入的明文一致（含 cipher_iv / sha256）。
//! 使用 Plaintext 密码器，避免测试依赖真实 AES 密钥协商，仍走 Encrypt/Decrypt 完整路径。
//! 根索引以 `MetaFile.meta_files` 挂叶子描述（name/sha256/size/iv），与 v2 布局一致。
//! 统计旁路文件名固定为 `jsons/stats.json`，由 debug 解码路径跟 Schema 索引写出。
//! 本文件不改动 DecodeMetaFile 行为，只固定 Go 对照夹具与字段级期望。

use std::sync::Arc;

use astersql_br_pkg_utils::json::{UnmarshalMetaFile, UnmarshalStatsFile};
use astersql_objstore::azblob::MemoryStorage;
use astersql_objstore_storeapi::{Context, Storage};

use crate::debug::DecodeMetaFile;
use crate::metafile::{Encrypt, sha256_bytes};
use crate::stubs::kvproto::brpb::{
    CipherInfo, File, MetaFile, Schema, StatsBlock, StatsFile, StatsFileIndex,
};
use crate::stubs::kvproto::encryptionpb::EncryptionMethod;
use crate::stubs::protobuf::Message;

/// 构造与 Go 测试相同的明文 CipherInfo，确保 Encrypt 直通且 IV 为空。
fn plaintext_cipher() -> CipherInfo {
    let mut cipher = CipherInfo::new();
    cipher.set_cipher_type(EncryptionMethod::Plaintext);
    cipher
}

/// flushMetaFile: serialize MetaFile, encrypt, write to storage, return File index.
///
/// 对照 Go `flushMetaFile`：先序列化再加密，索引里的 sha256/size 取自明文，
/// cipher_iv 取自 Encrypt 返回值，供后续 DecodeMetaFile 按叶子节点解密校验。
fn flush_meta_file(
    ctx: &Context,
    fname: &str,
    meta_file: &MetaFile,
    storage: &Arc<dyn Storage + Send + Sync>,
    cipher: &CipherInfo,
) -> File {
    let content = meta_file.write_to_bytes().expect("marshal metafile");
    // 加密后写入对象存储；索引仍记录明文摘要，与 Go 备份索引布局一致。
    let (encrypted, iv) = Encrypt(content.clone(), Some(cipher)).expect("encrypt");
    storage
        .WriteFile(ctx, fname, &encrypted)
        .expect("write metafile");
    let mut file = File::new();
    file.set_name(fname.to_string());
    file.set_sha256(sha256_bytes(&content));
    file.set_size(content.len() as u64);
    file.set_cipher_iv(iv);
    file
}

/// flushStatsFile: write encrypted StatsFile, return StatsFileIndex fixture.
///
/// 对照 Go `flushStatsFile`：同时记录密文长度 `size_enc` 与明文长度 `size_ori`，
/// 并塞入固定 `inline_data`，验证 Schema 透传 StatsFileIndex 时字段不丢失。
fn flush_stats_file(
    ctx: &Context,
    fname: &str,
    stats_file: &StatsFile,
    storage: &Arc<dyn Storage + Send + Sync>,
    cipher: &CipherInfo,
) -> StatsFileIndex {
    let content = stats_file.write_to_bytes().expect("marshal stats");
    // 校验和必须对明文计算，否则 DecodeMetaFile 下载后会报 checksum mismatch。
    let checksum = sha256_bytes(&content);
    let size_ori = content.len() as u64;
    let (encrypted, iv) = Encrypt(content, Some(cipher)).expect("encrypt");
    storage
        .WriteFile(ctx, fname, &encrypted)
        .expect("write stats");
    let mut index = StatsFileIndex::new();
    index.set_name(fname.to_string());
    index.set_sha256(checksum);
    index.set_size_enc(encrypted.len() as u64);
    index.set_size_ori(size_ori);
    index.set_cipher_iv(iv);
    // Go 测试用 strconv.Itoa(42) 作为 inline 占位，此处保持同值。
    index.set_inline_data(format!("{}", 42_i32).into_bytes());
    index
}

/// TestDecodeMetaFile — Go `TestDecodeMetaFile`.
///
/// 两段断言：先解码 data 叶子到 `jsons/data.json` 校验 SST File 字段；
/// 再解码 schema 叶子，校验 Schema 本体、内嵌 StatsFileIndex，以及旁路写出的 `jsons/stats.json`。
#[test]
fn test_decode_meta_file() {
    let storage: Arc<dyn Storage + Send + Sync> = Arc::new(MemoryStorage::default());
    let ctx = Context::default();
    let cipher = plaintext_cipher();

    // —— 构造含单个 SST 描述的 data MetaFile 叶子 ——
    // 下列 File 字段刻意取易辨识字面量，便于解码后逐字段比对。
    let mut data_file = File::new();
    data_file.set_name("1.sst".to_string());
    data_file.set_sha256(b"1.sst".to_vec());
    data_file.set_start_key(b"start".to_vec());
    data_file.set_end_key(b"end".to_vec());
    data_file.set_end_version(1);
    data_file.set_crc64xor(1);
    data_file.set_total_kvs(2);
    data_file.set_total_bytes(3);
    // CF 与 cipher_iv 也写入可辨识值，防止解码路径漏字段。
    data_file.set_cf("write".to_string());
    data_file.set_cipher_iv(b"1.sst".to_vec());

    let mut data_meta = MetaFile::new();
    data_meta.set_data_files(vec![data_file]);
    // 对象键为 "data"，旁路输出应为 jsons/data.json。
    let file1 = flush_meta_file(&ctx, "data", &data_meta, &storage, &cipher);

    // —— 构造 StatsFile（两块）并挂到 Schema.stats_index ——
    // 两个 physical_id 用于验证 DecodeMetaFile 展开全部 StatsBlock。
    let mut block1 = StatsBlock::new();
    block1.set_physical_id(1);
    block1.set_json_table(b"1".to_vec());
    let mut block2 = StatsBlock::new();
    block2.set_physical_id(2);
    block2.set_json_table(b"2".to_vec());
    let mut stats_file = StatsFile::new();
    stats_file.set_blocks(vec![block1, block2]);
    let stats = flush_stats_file(&ctx, "stats", &stats_file, &storage, &cipher);

    // Schema 内嵌库表 JSON 字节与统计索引；DecodeMetaFile 应原样落到 jsons/schema.json。
    // db/table 字节保持 Go 测试中的 JSON 形态（含 L/O 大小写字段）。
    let mut schema = Schema::new();
    schema.set_db(br#"{"db_name":{"L":"test","O":"test"},"id":1,"state":5}"#.to_vec());
    schema.set_table(br#"{"id":2,"state":5}"#.to_vec());
    schema.set_crc64xor(1);
    schema.set_total_kvs(2);
    schema.set_total_bytes(3);
    schema.set_tiflash_replicas(4);
    schema.set_stats(br#"{"a":1}"#.to_vec());
    // 挂上刚 flush 的 StatsFileIndex，驱动 stats 旁路解码。
    schema.set_stats_index(vec![stats.clone()]);
    let mut schema_meta = MetaFile::new();
    schema_meta.set_schemas(vec![schema]);
    let file2 = flush_meta_file(&ctx, "schema", &schema_meta, &storage, &cipher);

    {
        // 根索引只挂 data 叶子；DecodeMetaFile 按索引名写出 jsons/<name>.json。
        let mut index = MetaFile::new();
        let mut entry = MetaFile::new();
        entry.set_name(file1.get_name().to_string());
        entry.set_sha256(file1.get_sha256().to_vec());
        entry.set_size(file1.get_size());
        entry.set_cipher_iv(file1.get_cipher_iv().to_vec());
        index.mut_meta_files().push(entry);

        DecodeMetaFile(&ctx, storage.clone(), Some(&cipher), Some(&index)).expect("decode data");
        // 旁路路径固定前缀 jsons/，基名取叶子对象键。
        let content = storage
            .ReadFile(&ctx, "jsons/data.json")
            .expect("read json");
        let json: serde_json::Value =
            serde_json::from_slice(&content).expect("decoded meta output must be JSON");
        assert_eq!(json["data_files"][0]["sha256"], "312e737374");
        assert_eq!(json["data_files"][0]["start_key"], "7374617274");
        // 解析旁路 JSON：字段必须与 flush 前明文 MetaFile 完全一致。
        let meta = UnmarshalMetaFile(&content).expect("parse meta json");
        assert_eq!(meta.get_data_files().len(), 1);
        let f = &meta.get_data_files()[0];
        assert_eq!(f.get_name(), "1.sst");
        assert_eq!(f.get_sha256(), b"1.sst");
        assert_eq!(f.get_start_key(), b"start");
        assert_eq!(f.get_end_key(), b"end");
        assert_eq!(f.get_end_version(), 1);
        assert_eq!(f.get_crc64xor(), 1);
        assert_eq!(f.get_total_kvs(), 2);
        assert_eq!(f.get_total_bytes(), 3);
        assert_eq!(f.get_cf(), "write");
        assert_eq!(f.get_cipher_iv(), b"1.sst");
    }

    {
        // 第二段：schema 叶子；除 Schema 外还需验证连带写出的 stats 块文件。
        let mut index = MetaFile::new();
        let mut entry = MetaFile::new();
        entry.set_name(file2.get_name().to_string());
        entry.set_sha256(file2.get_sha256().to_vec());
        entry.set_size(file2.get_size());
        entry.set_cipher_iv(file2.get_cipher_iv().to_vec());
        index.mut_meta_files().push(entry);

        DecodeMetaFile(&ctx, storage.clone(), Some(&cipher), Some(&index)).expect("decode schema");

        let content = storage
            .ReadFile(&ctx, "jsons/schema.json")
            .expect("read schema json");
        let json: serde_json::Value =
            serde_json::from_slice(&content).expect("decoded schema output must be JSON");
        assert_eq!(json["schemas"][0]["db"]["db_name"]["L"], "test");
        assert_eq!(json["schemas"][0]["table"]["id"], 2);
        assert_eq!(json["schemas"].as_array().unwrap().len(), 1);
        let s = &json["schemas"][0];
        // 库表 JSON 与聚合统计字段必须原样保留（Go 对照断言）。
        assert_eq!(
            s["db"],
            serde_json::json!({"db_name":{"L":"test","O":"test"},"id":1,"state":5})
        );
        assert_eq!(s["table"], serde_json::json!({"id":2,"state":5}));
        assert_eq!(s["crc64xor"], 1);
        assert_eq!(s["total_kvs"], 2);
        assert_eq!(s["total_bytes"], 3);
        assert_eq!(s["tiflash_replicas"], 4);
        assert_eq!(s["stats"], serde_json::json!({"a":1}));
        let stats_index = s["stats_index"].as_array().unwrap();
        assert_eq!(stats_index.len(), 1);
        // StatsFileIndex 全字段透传：名称、摘要、密文/明文长度、IV、inline。
        assert_eq!(stats_index[0]["name"], stats.get_name());
        assert_eq!(stats_index[0]["size_enc"], stats.get_size_enc());
        assert_eq!(stats_index[0]["size_ori"], stats.get_size_ori());
        assert_eq!(stats_index[0]["inline_data"], "NDI=");

        // DecodeMetaFile 会跟随 Schema 索引再写出统计文件 JSON。
        let content = storage
            .ReadFile(&ctx, "jsons/stats.json")
            .expect("read stats json");
        let json: serde_json::Value =
            serde_json::from_slice(&content).expect("decoded stats output must be JSON");
        assert_eq!(json["blocks"][0]["json_table"], 1);
        assert_eq!(json["blocks"][1]["json_table"], 2);
        let stats_blocks = UnmarshalStatsFile(&content).expect("parse stats");
        assert_eq!(stats_blocks.get_blocks().len(), 2);
        assert_eq!(stats_blocks.get_blocks()[0].get_physical_id(), 1);
        assert_eq!(stats_blocks.get_blocks()[0].get_json_table(), b"1");
        assert_eq!(stats_blocks.get_blocks()[1].get_physical_id(), 2);
        assert_eq!(stats_blocks.get_blocks()[1].get_json_table(), b"2");
    }
}
