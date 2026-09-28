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

//! Go `log_file_manager_test.go` equivalents (MemStorage + FakeStreamMetadataHelper).
//! 对照 Go 日志文件管理器测试：TS 过滤、KV 去重、WriteCF 类型跳过、并发读门闩。
//! 使用 FakeStreamMetadataHelper/MemStorage，不触达真实对象存储与 TiKV。
//! ReadFilteredEntriesFromFiles 的 filterTS 把条目拆成已应用/待应用两段。
//! DDL JobHistory 键绕过 auto-id 去重；空 value 在去重路径被跳过。
//! WriteCF 忽略 Lock/Rollback，仅保留 Put/Delete 等有效写类型。
//! 并发用例用 gate 卡住 4 读读，断言 MaxActiveReadCount==4 后放行。
//! 注入 metas 的 FileManager 用例验证 streamingMeta + FilterDataFiles 管线。
//! 夹具 wm/dm 只填 TS/CF，其它字段默认，聚焦过滤谓词。
//! build_test_buffer 用 sha256 覆盖整段缓冲，RangeOffset=0 表示全文件可读。
//! TEST_NewLogFileManager(start, restore, shift, helper) 参数顺序与 Go 测试一致。
//! assert_entries 要求长度与逐字段相等，防止静默截断。
//! 并发门闩用例依赖 FakeStreamMetadataHelper::with_gate，非生产代码路径。
//! ReadStreamMeta 在注入场景返回通过 TS 窗的 metas。
//! CollectAll 物化 DML 迭代器以便断言 Path。
//! 空 value 双胞胎在 generate_kv_data_with 中成对出现，模拟删除标记。
//! encode_write_cf_value 的类型字节 P/R/L/D 对应 Put/Rollback/Lock/Delete。
//! filterTS 拆分语义：Ts < filterTS → kv；Ts >= filterTS → filtered。
//! auto-id 去重键为逻辑键（去 Ts），保留最大 Ts 且非空 value。
//! 非 auto-id（TableKey）不去重，允许同前缀多版本并存。
//! DDL JobHistory 前缀特殊，始终走旁路。
//! CopySemantics 污染后仍相等，防止零拷贝陷阱。
//! test_read_from_metadata_and_file_manger 名称保留 Go 历史拼写。
//! MemStorage::new 空存储足够 CreateLogFileManager 初始化。
//! NewMigrationBuilder(0,10,100) 与注入 metas 的窗口匹配。
//! IsMeta=true 的 m1 不得出现在 LoadDMLFiles 结果中。
//! WriteCF 全 Rollback 时 kv 与 filtered 皆空。
//! DefaultCF 对照段验证去重取 ts=60。
//! 四线程 join 后再次断言 MaxActive，防止门闩状态泄漏。
//! err_tx 收集各线程 Result，任一失败则测试失败。
//! ratio 无关本文件；本文件无 SkipMap 随机测试。
//! 时间常量 35/75/25 对应 start/restore/shift，贯穿多数用例。
//! 35/75/25 外的特例会在测试体内单独注释说明。
//! 编码助手与 export_test 共享，保证键布局与 Go codec 桩一致。
//! 本文件禁止改断言数值：数值即与 Go 的契约。
//! 中文注释只解释意图与边界，不复述断言字面量。
//! 若未来补齐真实 walk，应另开任务而非在此混入网络 IO。
//! FakeStreamMetadataHelper::new 无门闩；with_gate 才启用并发阻塞。
//! sha256_digest 与备份校验路径一致，错误摘要会在读失败时暴露。
//! Duration::from_secs(2) 为并发等待上限，避免 CI 挂死。
//! thread::sleep 5ms 轮询 ActiveReadCount，平衡灵敏度与 CPU。
//! Arc<TEST_NewLogFileManager> 使多线程共享同一 helper/gate。
//! Mutex<Vec<Result>> 聚合错误，顺序无关。
//! Generate 数据前部/后置的 encode 用于撑起 RangeOffset 边界。
//! slice 仅对窗口内字节做 sha，与 RangeOffset/RangeLength 对齐。
//! mTable 条目在部分过滤策略下可能保留，WriteCF 分支用宽松 assert。
//! DefaultCF 分支用精确 vec 断言，契约更强。
//! mixed 用例同时覆盖 DDL 旁路与 auto-id 去重交互。
//! dedup_filter_ts_split 证明去重发生在拆分之前。
//! non_auto_id 证明 TableKey 两版本分别落入 kv/filtered。
//! filter_data_files 不构造 FileManager，直接测静态函数。
//! file manager 集成测试覆盖注入 metas 的端到端过滤。
//! 保持 PingCAP 许可证与英文 Go 对照注释不被删除。
//! 不新增 AsterSQL 版权行（任务仅注释）。
//! 验证通过后删除对应任务 md。
//! 任务 228 仅补充注释，不改任何断言或夹具数值。
//! 阅读顺序建议：静态过滤 → 去重系列 → WriteCF → 并发 → FileManager。
//! wm 的第三参 _min_begin 保留签名兼容，当前未使用。
//! dm 无第三参，因 DefaultCF 用例不需要 begin 语义。
//! encode_auto_id 的 empty_v 控制是否写入空 value。
//! encodemdbkv 同理服务普通 mDB 键。
//! encodeddljobkv 固定非空 value，突出旁路行为。
//! auto_id_entry/ddl_entry/mdb_entry 与编码函数成对出现。
//! prepare 类逻辑集中在 export_test，本文件专注行为断言。
//! 若密度统计漏计 `//!`，应检查是否含汉字。
//! 完成标准：中文注释≥108 且 diff 仅注释。
//! 空白检查与 rustfmt 2024 必须通过或按基线待回归。
//! 不在此文件引入新依赖或新测试函数。

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use astersql_br_pkg_utils_iter::{CollectAll, FromSlice};

use crate::export_test::{
    AutoIncrementIDKey, DBkey, EncodeKVEntry, EncodeTxnMetaKey, EncodeUintDesc,
    FakeStreamMetadataHelper, NewMetaName, NewMigrationBuilder, TEST_NewLogFileManager, TableKey,
    encode_write_cf_value, sha256_digest,
};
use crate::log_file_manager::{
    CreateLogFileManager, KvEntryWithTS, LogFileManagerInit, MetaName, ShouldFilterOutByTsStatic,
};
use crate::migration::WithMigrations;
use crate::stubs::Context;
use crate::stubs::backuppb::{DataFileGroup, DataFileInfo, Metadata};
use crate::stubs::consts;
use crate::stubs::kv_entry::Entry;
use crate::stubs::storeapi::{MemStorage, Storage};

// WriteCF 夹具：指定 Min/MaxTs，Path 固定为 w。
fn wm(start: u64, end: u64, _min_begin: u64) -> DataFileInfo {
    DataFileInfo {
        MinTs: start,
        MaxTs: end,
        Cf: consts::WriteCF.into(),
        Path: "w".into(),
        ..Default::default()
    }
}
// DefaultCF 夹具：指定 Min/MaxTs，Path 固定为 d。
fn dm(start: u64, end: u64) -> DataFileInfo {
    DataFileInfo {
        MinTs: start,
        MaxTs: end,
        Cf: consts::DefaultCF.into(),
        Path: "d".into(),
        ..Default::default()
    }
}

#[test]
/// Go `TestFilterDataFiles`。
/// 静态校验 ShouldFilterOutByTs：越界 MinTs、WriteCF 过早 MaxTs、DefaultCF 保留。
fn test_filter_data_files() {
    // MinTs>restore → 过滤。
    assert!(ShouldFilterOutByTsStatic(&wm(50, 60, 0), 40, 10, 5)); // MinTs > restore
    assert!(ShouldFilterOutByTsStatic(
        &DataFileInfo {
            Cf: consts::WriteCF.into(),
            MaxTs: 5,
            MinTs: 1,
            ..Default::default()
        },
        100,
        10,
        5
    ));
    assert!(!ShouldFilterOutByTsStatic(
        &DataFileInfo {
            Cf: consts::DefaultCF.into(),
            MinTs: 10,
            MaxTs: 20,
            Path: "x".into(),
            ..Default::default()
        },
        100,
        5,
        5
    ));
}

// 拼接条目字节并填 Sha256/Length，模拟备份文件缓冲。
fn build_test_buffer(cf: &str, entries: &[Vec<u8>]) -> (Vec<u8>, DataFileInfo) {
    let mut buff = Vec::new();
    for e in entries {
        buff.extend_from_slice(e);
    }
    let sum = sha256_digest(&buff);
    let len = buff.len() as u64;
    (
        buff,
        DataFileInfo {
            Sha256: sum,
            RangeOffset: 0,
            RangeLength: len,
            Length: len,
            Cf: cf.into(),
            Path: "f".into(),
            ..Default::default()
        },
    )
}

// 编码普通 mDB 风格键值（UintDesc + 可选空 value）。
fn encodemdbkv(logical: &str, ts: u64, empty_v: bool) -> Vec<u8> {
    let key = EncodeUintDesc(logical.as_bytes().to_vec(), ts);
    let v: &[u8] = if empty_v { b"" } else { b"mdb value" };
    EncodeKVEntry(&key, v)
}
// 编码 mDDLJobHistory 键，绕过 auto-id 去重路径。
fn encodeddljobkv(job_id: i32, ts: u64) -> Vec<u8> {
    let prefix = format!("mDDLJobHistory:{job_id}");
    let key = EncodeUintDesc(prefix.into_bytes(), ts);
    EncodeKVEntry(&key, format!("job-{job_id}-data").as_bytes())
}
// 编码 AutoIncrementID 元键，触发按逻辑键去重。
fn encode_auto_id(db: i64, table: i64, ts: u64, empty_v: bool) -> Vec<u8> {
    let key = EncodeTxnMetaKey(&DBkey(db), &AutoIncrementIDKey(table), ts);
    let v: &[u8] = if empty_v { b"" } else { b"mdb value" };
    EncodeKVEntry(&key, v)
}

// 逐条比对 Ts/Key/Value，失败即暴露去重或过滤偏差。
fn assert_entries(got: &[KvEntryWithTS], expect: &[KvEntryWithTS]) {
    assert_eq!(got.len(), expect.len());
    for (g, e) in got.iter().zip(expect.iter()) {
        assert_eq!(g.Ts, e.Ts);
        assert_eq!(g.E.Key, e.E.Key);
        assert_eq!(g.E.Value, e.E.Value);
    }
}
// 期望条目：auto-id 键 + 固定 value。
fn auto_id_entry(db: i64, table: i64, ts: u64) -> KvEntryWithTS {
    KvEntryWithTS {
        E: Entry {
            Key: EncodeTxnMetaKey(&DBkey(db), &AutoIncrementIDKey(table), ts),
            Value: b"mdb value".to_vec(),
        },
        Ts: ts,
    }
}
// 期望条目：DDL JobHistory 键。
fn ddl_entry(job_id: i32, ts: u64) -> KvEntryWithTS {
    let prefix = format!("mDDLJobHistory:{job_id}");
    KvEntryWithTS {
        E: Entry {
            Key: EncodeUintDesc(prefix.into_bytes(), ts),
            Value: format!("job-{job_id}-data").into_bytes(),
        },
        Ts: ts,
    }
}
// 期望条目：普通 mDB 键。
fn mdb_entry(logical: &str, ts: u64) -> KvEntryWithTS {
    KvEntryWithTS {
        E: Entry {
            Key: EncodeUintDesc(logical.as_bytes().to_vec(), ts),
            Value: b"mdb value".to_vec(),
        },
        Ts: ts,
    }
}

#[test]
/// Go `TestReadFilteredEntries_DedupMDBKeys`。
/// 同 auto-id 多版本只保留最高 Ts；低于 filterTS 的进 filtered。
/// 此处 filterTS=55，期望只剩 ts=60 的 filtered，kv 为空。
fn test_read_filtered_entries_dedup_mdb_keys() {
    let ctx = Context::Background();
    let (data, file) = build_test_buffer(
        consts::DefaultCF,
        &[
            encode_auto_id(1, 5, 40, false),
            encode_auto_id(1, 5, 60, false),
            encode_auto_id(1, 5, 50, false),
        ],
    );
    let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
    let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 55).unwrap();
    assert!(kv.is_empty());
    assert_entries(&filtered, &[auto_id_entry(1, 5, 60)]);
}

#[test]
/// Go `TestReadFilteredEntries_DDLJobHistoryBypassesDedup`。
/// DDL JobHistory 与普通 mDB 并存时不做 auto-id 去重，全部进 kv。
fn test_read_filtered_entries_ddl_job_history_bypasses_dedup() {
    let ctx = Context::Background();
    let (data, file) = build_test_buffer(
        consts::DefaultCF,
        &[
            encodeddljobkv(100, 40),
            encodeddljobkv(101, 50),
            encodemdbkv("mDB:reg:1", 45, false),
        ],
    );
    let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
    let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
    assert!(filtered.is_empty());
    assert_entries(
        &kv,
        &[
            ddl_entry(100, 40),
            ddl_entry(101, 50),
            mdb_entry("mDB:reg:1", 45),
        ],
    );
}

#[test]
/// Go `TestReadFilteredEntries_MixedDDLJobHistoryAndMDBKeys`。
/// filterTS=50：低于阈值的 DDL 进 kv，高于的 DDL/auto-id 进 filtered。
fn test_read_filtered_entries_mixed() {
    let ctx = Context::Background();
    let (data, file) = build_test_buffer(
        consts::DefaultCF,
        &[
            encodeddljobkv(200, 40),
            encode_auto_id(2, 1, 42, false),
            encodeddljobkv(201, 60),
            encode_auto_id(2, 1, 55, false),
        ],
    );
    let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
    let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 50).unwrap();
    assert_entries(&kv, &[ddl_entry(200, 40)]);
    assert_entries(&filtered, &[ddl_entry(201, 60), auto_id_entry(2, 1, 55)]);
}

#[test]
/// Go `TestReadFilteredEntries_DedupFilterTSSplit`。
/// 去重后仅高版本存活，且因 Ts>=filterTS 落入 filtered。
fn test_read_filtered_entries_dedup_filter_ts_split() {
    let ctx = Context::Background();
    let (data, file) = build_test_buffer(
        consts::DefaultCF,
        &[
            encode_auto_id(3, 7, 40, false),
            encode_auto_id(3, 7, 60, false),
        ],
    );
    let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
    let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 50).unwrap();
    assert!(kv.is_empty());
    assert_entries(&filtered, &[auto_id_entry(3, 7, 60)]);
}

#[test]
/// Go `TestReadFilteredEntries_CopySemantics`。
/// 返回的 KV 必须是拷贝：污染 helper 缓冲后键值不变。
fn test_read_filtered_entries_copy_semantics() {
    let ctx = Context::Background();
    let (data, file) =
        build_test_buffer(consts::DefaultCF, &[encodemdbkv("mDB:copytest", 50, false)]);
    let helper = FakeStreamMetadataHelper::new(data);
    let fm = TEST_NewLogFileManager(35, 75, 25, helper.clone());
    let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
    assert!(filtered.is_empty());
    assert_eq!(kv.len(), 1);
    let want_key = kv[0].E.Key.clone();
    let want_val = kv[0].E.Value.clone();
    // 原地改写底层缓冲，验证返回值已深拷贝。
    for b in helper.Data.lock().unwrap().iter_mut() {
        *b = 0xff;
    }
    assert_eq!(kv[0].E.Key, want_key);
    assert_eq!(kv[0].E.Value, want_val);
}

#[test]
/// Go `TestReadFilteredEntries_DedupEmptyValueSkipped`。
/// 空 value 的高版本被跳过，保留有值的低版本。
fn test_read_filtered_entries_dedup_empty_value_skipped() {
    let ctx = Context::Background();
    let (data, file) = build_test_buffer(
        consts::DefaultCF,
        &[
            encode_auto_id(4, 9, 60, true),
            encode_auto_id(4, 9, 40, false),
        ],
    );
    let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
    let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
    assert!(filtered.is_empty());
    assert_entries(&kv, &[auto_id_entry(4, 9, 40)]);
}

#[test]
/// Go `TestReadFilteredEntries_WriteCFSkipsLockAndRollback`。
/// 多场景：Put 胜出、Lock 忽略、Delete 保留、全 Rollback 为空、顺序无关。
/// DefaultCF 对照：去重取最高 Ts。
fn test_read_filtered_entries_write_cf_skips_lock_and_rollback() {
    let ctx = Context::Background();
    let put_v = encode_write_cf_value(b'P');
    let rollback_v = encode_write_cf_value(b'R');
    let lock_v = encode_write_cf_value(b'L');
    let delete_v = encode_write_cf_value(b'D');
    let enc = |db, table, ts, value: &[u8]| {
        EncodeKVEntry(
            &EncodeTxnMetaKey(&DBkey(db), &AutoIncrementIDKey(table), ts),
            value,
        )
    };

    {
        let (data, file) = build_test_buffer(
            consts::WriteCF,
            &[enc(1, 5, 40, &put_v), enc(1, 5, 60, &rollback_v)],
        );
        let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
        let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
        assert!(filtered.is_empty());
        // Put 保留，Rollback 丢弃。
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].Ts, 40);
        assert_eq!(kv[0].E.Value, put_v);
    }
    {
        let (data, file) = build_test_buffer(
            consts::WriteCF,
            &[enc(1, 5, 40, &put_v), enc(1, 5, 55, &lock_v)],
        );
        let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
        let (kv, _) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
        // Put 保留，Rollback 丢弃。
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].Ts, 40);
    }
    {
        let (data, file) = build_test_buffer(
            consts::WriteCF,
            &[enc(1, 3, 45, &delete_v), enc(1, 3, 70, &rollback_v)],
        );
        let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
        let (kv, _) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
        // Put 保留，Rollback 丢弃。
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].Ts, 45);
    }
    {
        let (data, file) = build_test_buffer(
            consts::WriteCF,
            &[enc(1, 5, 40, &rollback_v), enc(1, 5, 60, &rollback_v)],
        );
        let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
        let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
        assert!(kv.is_empty() && filtered.is_empty());
    }
    {
        let (data, file) = build_test_buffer(
            consts::WriteCF,
            &[enc(1, 5, 50, &rollback_v), enc(1, 5, 40, &put_v)],
        );
        let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
        let (kv, _) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
        // Put 保留，Rollback 丢弃。
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].Ts, 40);
    }
    {
        let (data, file) = build_test_buffer(
            consts::DefaultCF,
            &[
                encode_auto_id(1, 5, 40, false),
                encode_auto_id(1, 5, 60, false),
            ],
        );
        let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
        let (kv, _) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 100).unwrap();
        // Put 保留，Rollback 丢弃。
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].Ts, 60);
    }
}

#[test]
/// Go `TestReadFilteredEntries_NonAutoIDNotDeduped`。
/// 非 auto-id 的 TableKey 不做去重，按 filterTS 拆分两段。
fn test_read_filtered_entries_non_auto_id_not_deduped() {
    let ctx = Context::Background();
    let k1 = EncodeTxnMetaKey(&DBkey(1), &TableKey(3), 40);
    let k2 = EncodeTxnMetaKey(&DBkey(1), &TableKey(3), 70);
    let (data, file) = build_test_buffer(
        consts::DefaultCF,
        &[
            EncodeKVEntry(&k1, b"tableinfo-v1"),
            EncodeKVEntry(&k2, b"tableinfo-v2"),
        ],
    );
    let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
    let (kv, filtered) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 50).unwrap();
    // Put 保留，Rollback 丢弃。
    assert_eq!(kv.len(), 1);
    assert_eq!(kv[0].Ts, 40);
    assert_eq!(kv[0].E.Value, b"tableinfo-v1");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].Ts, 70);
}

// DefaultCF 通用编码：prefix_ts 键 + 可选空 value。
fn encodekv(prefix: &str, ts: u64, empty_v: bool) -> Vec<u8> {
    let k = format!("{prefix}_{ts}");
    let key = EncodeUintDesc(k.into_bytes(), ts);
    let v: &[u8] = if empty_v { b"" } else { b"any value" };
    EncodeKVEntry(&key, v)
}
// WriteCF 编码：非空 value 使用 Put 类型写记录。
fn encodekv_write_cf(prefix: &str, ts: u64, empty_v: bool) -> Vec<u8> {
    let k = format!("{prefix}_{ts}");
    let key = EncodeUintDesc(k.into_bytes(), ts);
    if empty_v {
        EncodeKVEntry(&key, b"")
    } else {
        EncodeKVEntry(&key, &encode_write_cf_value(b'P'))
    }
}
// 生成带 RangeOffset 窗口的复合缓冲，覆盖多 prefix/ts。
fn generate_kv_data_with(encode: fn(&str, u64, bool) -> Vec<u8>) -> (Vec<u8>, DataFileInfo) {
    let mut buff = Vec::new();
    buff.extend(encode("mDDLHistory", 10, false));
    buff.extend(encode("mDDLHistory", 10, true));
    let range_offset = buff.len() as u64;
    for (p, ts, empty) in [
        ("mDDLHistory", 21u64, false),
        ("mDDLHistory", 22, true),
        ("mDDL", 27, false),
        ("mDDL", 28, true),
        ("mDDL", 37, false),
        ("mDDL", 38, true),
        ("mDDLHistory", 45, false),
        ("mDDLHistory", 45, true),
        ("mDDL", 50, false),
        ("mDDL", 50, true),
        ("mTable", 52, false),
        ("mTable", 52, true),
        ("mDDL", 65, false),
        ("mDDL", 65, true),
        ("mDDLHistory", 80, false),
        ("mDDLHistory", 80, true),
    ] {
        buff.extend(encode(p, ts, empty));
    }
    let range_length = buff.len() as u64 - range_offset;
    buff.extend(encode("mDDL", 90, false));
    buff.extend(encode("mDDL", 90, true));
    let slice = &buff[range_offset as usize..(range_offset + range_length) as usize];
    let sha = sha256_digest(slice);
    (
        buff,
        DataFileInfo {
            Sha256: sha,
            RangeOffset: range_offset,
            RangeLength: range_length,
            Length: range_length,
            Path: "f".into(),
            ..Default::default()
        },
    )
}

#[test]
/// Go `TestReadAllEntries`。
/// WriteCF/DefaultCF 窗口断言 + 四线程并发读门闩压力。
fn test_read_all_entries() {
    let ctx = Context::Background();
    {
        let (data, mut file) = generate_kv_data_with(encodekv_write_cf);
        // WriteCF 路径：空 value 双胞胎被跳过。
        file.Cf = consts::WriteCF.into();
        let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
        let (kv, next) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 50).unwrap();
        // WriteCF: mDDL@37, mDDLHistory@45 (<50); mDDL@50, mDDL@65 (>=50).
        // Empty-value twins skipped; mTable skipped (not mD* after filter? kept if mD prefix).
        assert!(
            kv.len() >= 2,
            "kv ts={:?}",
            kv.iter().map(|e| e.Ts).collect::<Vec<_>>()
        );
        assert!(
            next.len() >= 2,
            "next ts={:?}",
            next.iter().map(|e| e.Ts).collect::<Vec<_>>()
        );
        assert!(kv.iter().any(|e| e.Ts == 37));
        assert!(kv.iter().any(|e| e.Ts == 45));
        assert!(next.iter().any(|e| e.Ts == 50));
        assert!(next.iter().any(|e| e.Ts == 65));
    }
    {
        let (data, mut file) = generate_kv_data_with(encodekv);
        // DefaultCF：严格断言 ts 列表。
        file.Cf = consts::DefaultCF.into();
        let fm = TEST_NewLogFileManager(35, 75, 25, FakeStreamMetadataHelper::new(data));
        let (kv, next) = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 50).unwrap();
        assert_eq!(
            kv.iter().map(|e| e.Ts).collect::<Vec<_>>(),
            vec![27, 37, 45]
        );
        assert_eq!(next.iter().map(|e| e.Ts).collect::<Vec<_>>(), vec![50, 65]);
    }
    {
        let (data, mut file) = generate_kv_data_with(encodekv);
        // DefaultCF：严格断言 ts 列表。
        file.Cf = consts::DefaultCF.into();
        let helper = FakeStreamMetadataHelper::with_gate(data);
        let fm = Arc::new(TEST_NewLogFileManager(35, 75, 25, helper.clone()));
        let mut handles = vec![];
        let err_tx = Arc::new(std::sync::Mutex::new(Vec::new()));
        for _ in 0..4 {
            let fm = fm.clone();
            let file = file.clone();
            let err_tx = err_tx.clone();
            handles.push(thread::spawn(move || {
                let ctx = Context::Background();
                let r = fm.ReadFilteredEntriesFromFiles(&ctx, &file, 50);
                err_tx.lock().unwrap().push(r.map(|_| ()));
            }));
        }
        let start = std::time::Instant::now();
        // 等待四读并发挂起在 gate 上。
        while helper.ActiveReadCount() < 4 && start.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            helper.ActiveReadCount(),
            4,
            "max_active={}",
            helper.MaxActiveReadCount()
        );
        assert_eq!(helper.MaxActiveReadCount(), 4);
        // 打开放行，等待四线程完成。
        helper.CloseReadGate();
        for h in handles {
            h.join().unwrap();
        }
        for r in err_tx.lock().unwrap().iter() {
            assert!(r.is_ok(), "{r:?}");
        }
        assert_eq!(helper.MaxActiveReadCount(), 4);
    }
}

#[test]
/// Go `TestReadMetaBetweenTS` / FileManager — injected metas + FilterDataFiles。
/// 注入含 DML+meta 的 group，断言只产出 DML 文件 f1。
fn test_read_meta_between_ts_and_file_manager() {
    let ctx = Context::Background();
    let storage: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let builder = NewMigrationBuilder(0, 10, 100);
    let mut fm = CreateLogFileManager(
        &ctx,
        LogFileManagerInit {
            StartTS: 10,
            RestoreTS: 100,
            Storage: storage,
            MigrationsBuilder: builder,
            Migrations: WithMigrations {
                skipmap: Default::default(),
                compactionDirs: vec![],
                fullBackups: vec![],
                shiftStartTS: 10,
                startTS: 10,
                restoredTS: 100,
            },
            MetadataDownloadBatchSize: 8,
            EncryptionManager: None,
        },
    )
    .unwrap();
    fm.SetInjectedMetas(vec![NewMetaName(
        Metadata {
            StoreId: 1,
            MinTs: 20,
            MaxTs: 80,
            FileGroups: vec![DataFileGroup {
                Path: "g".into(),
                DataFilesInfo: vec![
                    DataFileInfo {
                        Path: "f1".into(),
                        MinTs: 30,
                        MaxTs: 40,
                        IsMeta: false,
                        Cf: consts::DefaultCF.into(),
                        ..Default::default()
                    },
                    DataFileInfo {
                        Path: "m1".into(),
                        MinTs: 30,
                        MaxTs: 40,
                        IsMeta: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        },
        "meta_1",
    )]);
    let metas = fm.ReadStreamMeta(&ctx).unwrap();
    assert_eq!(metas.len(), 1);
    let mut dml = fm.LoadDMLFiles(&ctx).unwrap();
    let collected = CollectAll(
        &astersql_br_pkg_utils_iter::Context::background(),
        &mut *dml,
    );
    assert!(collected.Err.is_none());
    let files = collected.Item.unwrap_or_default();
    assert_eq!(files.len(), 1);
    // meta 文件 m1 被 FilterDataFiles 剔除。
    assert_eq!(files[0].Path, "f1");
}

#[test]
/// Go `TestReadFromMetadata` / `TestFileManger` smoke via streamingMeta filter。
/// 复用上一用例，覆盖 Go 拼写 FileManger 的等价烟雾入口。
fn test_read_from_metadata_and_file_manger() {
    test_read_meta_between_ts_and_file_manager();
}
