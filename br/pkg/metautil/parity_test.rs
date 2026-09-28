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

//! Parity checks proving the Go public contract of `br/pkg/metautil`
//! (debug.go/load.go/metafile.go/statsfile.go) is reflected by the Rust port.
//!
//! 契约级回归：常量名、统计文件命名、SHA-256、Encrypt 明文直通与非法 cipher 报错。
//! 第二测覆盖 StatsWriter 刷盘阈值、downloadStats 物理 ID 重写，以及 JSON 往返字段名。
//! 阈值通过原子全局 `maxStatsJsonTableSize`/`inlineSize` 临时压到 1，结束后由 Guard 还原。
//! 缺少 rewrite 规则时必须浮现 `ErrRestoreInvalidRewrite`，与 Go 错误串约定一致。
//! 本文件不测调试解码或 LoadBackupTables；那些由 debug_test/load_test 覆盖。
//! sync_channel(8) 缓冲下载任务，避免生产者在断言前阻塞。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use crate::stubs::{
    DecodeTableID, JSONTable, Key, kvproto::brpb::CipherInfo,
    kvproto::encryptionpb::EncryptionMethod, protobuf::Message,
};
use astersql_objstore::azblob::MemoryStorage;
use astersql_objstore_storeapi::{Context, Storage};

use crate::metafile::{Encrypt, LockFile, MetaJSONFile, MetaV1, MetaV2, sha256_bytes};
use crate::statsfile::{
    downloadStats, getStatsFileName, inlineSize, marshalStatsJSONTable, maxStatsJsonTableSize,
    newStatsWriter, unmarshalStatsJSONTable,
};
use crate::statsfile_test_support::StatsConfigTestGuard;

/// 明文 CipherInfo：Encrypt 应原样返回内容且 IV 为空。
fn plaintext_cipher() -> CipherInfo {
    let mut cipher = CipherInfo::new();
    cipher.set_cipher_type(EncryptionMethod::Plaintext);
    cipher
}

/// 构造可辨识的 JSONTable 夹具；`magic` 驱动 Count/ModifyCount/Version 等差字段。
fn sample_json_table(magic: i64, table: &str) -> JSONTable {
    JSONTable {
        Columns: HashMap::new(),
        Indices: HashMap::new(),
        Partitions: HashMap::new(),
        DatabaseName: "test-schema".to_string(),
        TableName: table.to_string(),
        PredicateColumns: Vec::new(),
        Count: magic,
        ModifyCount: magic + 1,
        Version: magic as u64 + 2,
        IsHistoricalStats: false,
    }
}

/// 公开常量、命名规则、摘要与 Encrypt 边界的 Go/Rust 契约对照。
#[test]
fn go_rust_public_contract_matches() {
    // Constants keep the exact Go values.
    // 锁文件与旁路 JSON 路径、MetaV1/V2 数值必须与 Go 字面量完全一致。
    assert_eq!(LockFile, "backup.lock");
    assert_eq!(MetaJSONFile, "jsons/backupmeta.json");
    assert_eq!(MetaV1, 0);
    assert_eq!(MetaV2, 1);

    // getStatsFileName keeps the Go 9-digit zero padded naming rule.
    // 小于 1e9 补零到 9 位；更大则原样十进制，避免截断。
    assert_eq!(getStatsFileName(5), "backupmeta.schema.stats.000000005");
    assert_eq!(
        getStatsFileName(1234567890),
        "backupmeta.schema.stats.1234567890"
    );

    // sha256 helper is a real SHA-256 (Go crypto/sha256 parity).
    // 固定向量 "abc" 的十六进制摘要，防止桩实现退化成非加密哈希。
    assert_eq!(
        crate::metafile::hex_encode(&sha256_bytes(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );

    // Encrypt: plaintext cipher passes content through without IV.
    // 明文模式不改内容、不生成 IV，与 Go berrors 路径前的短路径一致。
    let (out, iv) = Encrypt(b"payload".to_vec(), Some(&plaintext_cipher())).expect("encrypt");
    assert_eq!(out, b"payload");
    assert!(iv.is_empty());

    // Encrypt: invalid cipher type is an error, mirroring Go's berrors path.
    // Unknown 必须失败，禁止静默当明文处理。
    let mut bad = CipherInfo::new();
    bad.set_cipher_type(EncryptionMethod::Unknown);
    assert!(Encrypt(b"payload".to_vec(), Some(&bad)).is_err());
}

/// Go `tablecodec.DecodeTableID` accepts API V2 keys after stripping their
/// four-byte mode/keyspace prefix before decoding the table key.
#[test]
fn decode_table_id_accepts_api_v2_keyspace_prefixes() {
    let encoded_table_id = (42_u64 ^ (1_u64 << 63)).to_be_bytes();
    let table_key = [[b't'].as_slice(), encoded_table_id.as_slice()].concat();

    for mode in [b'x', b'r'] {
        let api_v2_key = [[mode, 0x01, 0x02, 0x03].as_slice(), &table_key].concat();
        assert_eq!(DecodeTableID(Key(api_v2_key)), 42);
    }

    assert_eq!(DecodeTableID(Key(b"x\x01\x02".to_vec())), 0);
}

/// StatsWriter 刷盘、downloadStats 重写与 JSON 往返的端到端契约。
#[test]
fn stats_writer_inline_and_file_flush_matches_go() {
    // Guard 串行化全局阈值修改，避免并行测试互相污染。
    let _stats_config_guard = StatsConfigTestGuard::acquire();
    let storage: Arc<dyn Storage + Send + Sync> = Arc::new(MemoryStorage::default());
    let ctx = Context::default();

    let mut writer = newStatsWriter(storage.clone(), Some(plaintext_cipher()));

    // Force the flush path as the Go test does with tiny thresholds.
    // 阈值压到 1：第一次 BackupStats 必落真实文件且无 inline。
    maxStatsJsonTableSize.store(1, Ordering::SeqCst);
    inlineSize.store(1, Ordering::SeqCst);
    writer
        .BackupStats(&ctx, Some(&sample_json_table(1, "test-table")), 1)
        .expect("backup stats table 1");
    // 恢复默认阈值后再写第二张表，仍按序号命名独立文件。
    maxStatsJsonTableSize.store(32 * 1024 * 1024, Ordering::SeqCst);
    inlineSize.store(8 * 1024, Ordering::SeqCst);
    writer
        .BackupStats(&ctx, Some(&sample_json_table(2, "test-table-1")), 2)
        .expect("backup stats table 2");
    // Done 冲刷缓冲区并返回全部 StatsFileIndex。
    let indexes = writer.BackupStatsDone(&ctx).expect("backup stats done");
    // 两次 BackupStats 应对应两条索引，顺序与写入序号一致。
    assert_eq!(indexes.len(), 2);

    // First flush exceeded inlineSize=1, so it became a real stats file.
    // 校验落盘名、空 inline、密文摘要与 size_enc 一致。
    assert_eq!(indexes[0].get_name(), getStatsFileName(1));
    assert!(indexes[0].get_inline_data().is_empty());
    let content = storage
        .ReadFile(&ctx, indexes[0].get_name())
        .expect("stats file written");
    assert_eq!(sha256_bytes(&content), indexes[0].get_sha256());
    assert_eq!(content.len() as u64, indexes[0].get_size_enc());

    // Second flush is small and not first: still a named file per Go logic.
    // Go 对非首个块即使可 inline 仍写独立文件名。
    assert_eq!(indexes[1].get_name(), getStatsFileName(2));

    // Restore path: download rewrites physical IDs and yields the JSON tables.
    // rewrite 1→10、2→20；任务按 PhysicalID 排序后断言。
    let rewrite: HashMap<i64, i64> = HashMap::from([(1, 10), (2, 20)]);
    let (tx, rx) = mpsc::sync_channel(8);
    let cipher = Some(plaintext_cipher());
    let download = std::thread::spawn({
        let storage = storage.clone();
        let ctx = ctx.clone();
        let indexes = indexes.clone();
        move || downloadStats(&ctx, storage, cipher, indexes, rewrite, tx)
    });
    // 收齐任务后再 join，保证 channel 关闭语义与 Go 一致。
    let mut tasks: Vec<_> = rx.into_iter().collect();
    download.join().expect("no panic").expect("download stats");
    // 下载完成顺序不确定，按重写后的 PhysicalID 排序再断言。
    tasks.sort_by_key(|task| task.PhysicalID);
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].PhysicalID, 10);
    assert_eq!(tasks[1].PhysicalID, 20);
    let json0 = tasks[0].JSONTable.as_ref().expect("json table");
    assert_eq!(json0.DatabaseName, "test-schema");
    assert_eq!(json0.TableName, "test-table");
    assert_eq!(json0.Count, 1);

    // Missing rewrite rule surfaces ErrRestoreInvalidRewrite like Go.
    // 空 rewrite 映射必须失败；先排干 channel 再取错误，避免死锁。
    let (tx, rx) = mpsc::sync_channel(8);
    let bad_download = std::thread::spawn({
        let storage = storage.clone();
        let ctx = ctx.clone();
        let indexes = indexes.clone();
        move || {
            downloadStats(
                &ctx,
                storage,
                Some(plaintext_cipher()),
                indexes,
                HashMap::new(),
                tx,
            )
        }
    });
    let _drain: Vec<_> = rx.into_iter().collect();
    let err = bad_download
        .join()
        .expect("no panic")
        .expect_err("missing rewrite rule must fail");
    assert!(
        err.to_string().contains("ErrRestoreInvalidRewrite"),
        "unexpected error: {err}"
    );

    // JSON round trip preserves the Go field names and values.
    // 序列化字段名与 Go statistics JSON 契约对齐（database_name 等在 marshal 内处理）。
    let bytes = marshalStatsJSONTable(&sample_json_table(7, "json-table")).expect("marshal");
    let parsed = unmarshalStatsJSONTable(&bytes).expect("unmarshal");
    assert_eq!(parsed.DatabaseName, "test-schema");
    assert_eq!(parsed.TableName, "json-table");
    assert_eq!(parsed.Count, 7);
    assert_eq!(parsed.ModifyCount, 8);
    assert_eq!(parsed.Version, 9);
}
