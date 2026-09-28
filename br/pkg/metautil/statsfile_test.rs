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

//! Go-equivalent tests for `br/pkg/metautil/statsfile_test.go`.
//!
//! 本文件对齐 Go `TestStatsWriter`：覆盖明文与多种 AES-CTR 密钥下的 stats 备份/恢复闭环。
//! 断言重点是 rewrite 后的 physicalID、库表名与 Count，而不是完整直方图内容。
//! 通过临时压低 `maxStatsJsonTableSize`/`inlineSize` 强制走远端写文件路径。
//! 使用 MemoryStorage，避免依赖真实对象存储；并发下载结果经 channel 收集后排序比对。
//! 密钥长度必须匹配各 AES-CTR 变体，否则 Encrypt/Decrypt 会失败。
//! 同一 storage 上串行跑多组 cipher，验证不同加密配置互不污染断言。
//! 任务列表按 PhysicalID 排序后再比对，消除并发完成顺序抖动。
//! StatsConfigTestGuard 确保阈值在 panic 路径也会恢复，避免污染后续测试。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::thread;

use astersql_objstore::azblob::MemoryStorage;
use astersql_objstore_storeapi::{Context, Storage};

use crate::statsfile::{downloadStats, inlineSize, maxStatsJsonTableSize, newStatsWriter};
use crate::statsfile_test_support::StatsConfigTestGuard;
use crate::stubs::JSONTable;
use crate::stubs::kvproto::brpb::CipherInfo;
use crate::stubs::kvproto::encryptionpb::EncryptionMethod;

/// 一组加密算法与合法密钥长度的测试用例，覆盖 Plaintext 与 AES-128/192/256-CTR。
struct EncryptTest {
    method: EncryptionMethod,
    right_key: &'static str,
}

/// 构造可区分的 JSONTable 样本；magic 写入 Count，便于 rewrite 后回查。
fn sample_json_table(magic: i64, table: &str) -> JSONTable {
    // Go fills Columns/Indices with tipb histograms; arbitrary JSON values retain
    // the complete field shape at this dependency boundary.
    JSONTable {
        Columns: HashMap::from([("test".to_string(), serde_json::json!({"null_count": magic}))]),
        Indices: HashMap::from([(
            "test".to_string(),
            serde_json::json!({"null_count": magic + 1}),
        )]),
        Partitions: HashMap::new(),
        DatabaseName: "test-schema".to_string(),
        TableName: table.to_string(),
        PredicateColumns: Vec::new(),
        Count: magic,
        ModifyCount: 0,
        Version: 0,
        IsHistoricalStats: false,
    }
}

/// TestStatsWriter — Go `TestStatsWriter`.
/// 验证：强制刷盘后 index 可被 downloadStats 读回，且 rewrite 后字段一致。
#[test]
fn test_stats_writer() {
    // Guard 保证阈值即使断言失败也会恢复默认值。
    let _stats_config_guard = StatsConfigTestGuard::acquire();
    let ctx = Context::default();
    let test_cases = [
        EncryptTest {
            method: EncryptionMethod::Plaintext,
            right_key: "",
        },
        EncryptTest {
            method: EncryptionMethod::Aes128Ctr,
            right_key: "0123456789012345",
        },
        EncryptTest {
            method: EncryptionMethod::Aes192Ctr,
            right_key: "012345678901234567890123",
        },
        EncryptTest {
            method: EncryptionMethod::Aes256Ctr,
            right_key: "01234567890123456789012345678901",
        },
    ];
    let fake_json_tables: HashMap<i64, JSONTable> = HashMap::from([
        (1, sample_json_table(1, "test-table")),
        (2, sample_json_table(2, "test-table-1")),
    ]);
    // 正向 rewrite：旧 physicalID -> 新 ID；反向用于断言时找回原始样本。
    let rewrite_ids = HashMap::from([(1_i64, 10_i64), (2_i64, 20_i64)]);
    let rerewrite_ids = HashMap::from([(10_i64, 1_i64), (20_i64, 2_i64)]);

    let storage: Arc<dyn Storage + Send + Sync> = Arc::new(MemoryStorage::default());
    for v in test_cases {
        let mut cipher = CipherInfo::new();
        cipher.set_cipher_type(v.method);
        // 每轮用例重置 cipher，避免串用上一轮密钥。
        cipher.set_cipher_key(v.right_key.as_bytes().to_vec());
        let mut stats_writer = newStatsWriter(storage.clone(), Some(cipher.clone()));

        // 阈值压到 1：第一张表必然远端落盘且无法内联。
        maxStatsJsonTableSize.store(1, Ordering::SeqCst);
        inlineSize.store(1, Ordering::SeqCst);
        stats_writer
            .BackupStats(&ctx, Some(&fake_json_tables[&1]), 1)
            .expect("backup stats 1");

        // 恢复默认阈值后再写第二张，覆盖“已有 index 后不再内联”分支。
        maxStatsJsonTableSize.store(32 * 1024 * 1024, Ordering::SeqCst);
        inlineSize.store(8 * 1024, Ordering::SeqCst);
        stats_writer
            .BackupStats(&ctx, Some(&fake_json_tables[&2]), 2)
            .expect("backup stats 2");
        let indexes = stats_writer.BackupStatsDone(&ctx).expect("done");

        let (tx, rx) = mpsc::sync_channel(8);
        let download = {
            let storage = storage.clone();
            let ctx = ctx.clone();
            let cipher = cipher.clone();
            let indexes = indexes.clone();
            let rewrite_ids = rewrite_ids.clone();
            thread::spawn(move || {
                downloadStats(&ctx, storage, Some(cipher), indexes, rewrite_ids, tx)
            })
        };
        // 收集全部任务后再 join，避免下载端阻塞在满缓冲上。
        let mut tasks: Vec<_> = rx.into_iter().collect();
        download.join().expect("join").expect("download");
        tasks.sort_by_key(|t| t.PhysicalID);
        // 两张表应各产生一个加载任务。
        assert_eq!(tasks.len(), 2);
        for task in tasks {
            let orig_id = rerewrite_ids[&task.PhysicalID];
            let expected = &fake_json_tables[&orig_id];
            let got = task.JSONTable.as_ref().expect("json table");
            // Go compares the complete JSONTable, including histogram payloads.
            assert_eq!(got.as_ref(), expected);
        }
    }
}

/// Go uses encoding/json on the complete JSONTable value, so every exported
/// field must survive the stats-file round trip.
#[test]
fn test_stats_json_table_round_trip_preserves_all_fields() {
    let table = JSONTable {
        Columns: HashMap::from([(
            "column".to_string(),
            serde_json::json!({"histogram": {"ndv": 7}, "null_count": 3}),
        )]),
        Indices: HashMap::from([(
            "index".to_string(),
            serde_json::json!({"cm_sketch": {"default_value": 11}}),
        )]),
        Partitions: HashMap::from([(
            "p0".to_string(),
            serde_json::json!({"count": 19, "table_name": "partition"}),
        )]),
        DatabaseName: "database".to_string(),
        TableName: "table".to_string(),
        PredicateColumns: vec![serde_json::json!({
            "id": 23,
            "last_used_at": "2026-08-13T00:00:00Z"
        })],
        Count: 29,
        ModifyCount: 31,
        Version: 37,
        IsHistoricalStats: true,
    };

    let encoded = crate::statsfile::marshalStatsJSONTable(&table).expect("marshal full table");
    let decoded =
        crate::statsfile::unmarshalStatsJSONTable(&encoded).expect("unmarshal full table");
    assert_eq!(decoded, table);
}
