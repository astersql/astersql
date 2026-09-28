// Copyright 2026 AsterSQL.

//! Stream 包跨模块 Go/Rust 公开契约冒烟测试。
//! 单测串联 decode_kv、meta_kv、rewrite、search、migration、
//! table_mapping/history 与 logging_helper，校验关键不变量与字节布局。
//! 不替代各文件细粒度单测；失败时优先对照对应 Go 包公开 API。
//! 使用内存 Storage 与手工拼装缓冲，避免依赖真实集群。
//! 固定样例 ID/时间戳，便于与 Go 侧字面量逐项 diff。

use std::collections::HashMap;
use std::sync::Arc;

use astersql_br_pkg_utils_consts::{DefaultCF, WriteCF};
use sha2::Digest;

use crate::decode_kv::{DecodeKVEntry, EncodeKVEntry, EventIterator, Iterator, NewEventIterator};
use crate::logging_helper::LogDBReplaceMap;
use crate::meta_kv::{ParseTxnMetaKeyFrom, RawWriteCFValue, WriteTypePut};
use crate::rewrite_meta_rawkv::{NewDBReplace, NewSchemasReplace, SchemasReplace};
use crate::search::{EncodeSearchKey, NewStartWithComparator, NewStreamBackupSearch};
use crate::stream_metas::{
    MergeMigrations, NewMigration, UpdateShiftTS, UpdateShiftTSFromMetadata, isEmptyEdition,
    isInsane, migIdOf, nameOf,
};
use crate::stubs::backuppb::{DataFileGroup, DataFileInfo, MetaEdit, Metadata, Migration};
use crate::stubs::model;
use crate::stubs::utils::EncodeTxnMetaKey;
use crate::stubs::{MemStorage, NewTableReplace, Storage, meta};
use crate::table_history::{LogBackupTableHistoryManager, NewTableHistoryManager};
use crate::table_mapping::{MetaInfoCollector, NewTableMappingManager, TableSimpleInfo};

/// 采集 DB/表 meta 回调，供 table_mapping 路径断言。
/// 仅记录参数元组，不模拟下游副作用。
/// 与 Go 测试中的 collector 桩角色相同。
struct TestCollector {
    dbs: Vec<(i64, String, u64)>,
    tables: Vec<(i64, i64, String, u64)>,
}

impl MetaInfoCollector for TestCollector {
    /// 记录数据库信息回调。
    fn OnDatabaseInfo(&mut self, dbId: i64, dbName: String, commitTs: u64) {
        self.dbs.push((dbId, dbName, commitTs));
    }

    /// 记录表信息回调；名称取自 TableSimpleInfo。
    fn OnTableInfo(
        &mut self,
        dbID: i64,
        tableId: i64,
        tableSimpleInfo: &TableSimpleInfo,
        commitTs: u64,
    ) {
        self.tables
            .push((dbID, tableId, tableSimpleInfo.Name.clone(), commitTs));
    }
}

/// 一次跑完 stream 包核心公开面的契约检查。
/// 分段注释对应各子模块；任一段失败即整体失败。
/// 顺序刻意与常见恢复流水线一致：解码→历史→meta→迁移→映射→搜索。
#[test]
fn go_rust_public_contract_matches() {
    // —— decode_kv：KV 条目编解码往返与超大缓冲拒绝 ——
    let k = b"key".to_vec();
    let v = b"value".to_vec();
    let encoded = EncodeKVEntry(&k, &v);
    let (dk, dv, n) = DecodeKVEntry(&encoded).unwrap();
    assert_eq!(dk, k);
    assert_eq!(dv, v);
    // 消费长度应等于整段编码。
    assert_eq!(n as usize, encoded.len());
    // 过短输入必须报错。
    assert!(DecodeKVEntry(b"short").is_err());

    // 缓冲区超过 u32 上限时 Valid=false 且带明确错误文案。
    let mut oversize = NewEventIterator(vec![0u8; (u32::MAX as usize) + 1]);
    assert!(!oversize.Valid());
    assert!(oversize.GetError().unwrap().contains("buffer too large"));

    // —— table_history：同 ID 较新 ts 覆盖旧名 ——
    let mut hist = NewTableHistoryManager();
    hist.AddTableHistory(1, "t1", 10, 100);
    hist.AddTableHistory(1, "t1_new", 10, 200);
    let h = hist.GetTableHistory().get(&1).unwrap();
    // 索引 1 对应 table_id 槽位上的最新记录。
    assert_eq!(h[1].TableName, "t1_new");
    assert_eq!(h[1].Timestamp, 200);
    // DB 名映射同样按 ts 取新。
    hist.RecordDBIdToName(10, "db_old", 100);
    hist.RecordDBIdToName(10, "db_new", 200);
    assert_eq!(hist.GetDBNameByID(10), Some("db_new"));

    // —— WriteCF：Put + txnSource 无 shortValue 的解析往返 ——
    let ts: u64 = 400036290571534337;
    let txn_source: u64 = 9527;
    // 'S' 即 txnSource flag，与 meta_kv 常量一致。
    let mut buff = vec![WriteTypePut];
    buff = crate::stubs::codec::EncodeUvarint(buff, ts);
    buff.push(b'S');
    buff = crate::stubs::codec::EncodeUvarint(buff, txn_source);
    let mut v = RawWriteCFValue::default();
    v.ParseFrom(&buff).unwrap();
    assert!(v.IsPut());
    assert!(!v.HasShortValue());
    assert_eq!(v.GetStartTs(), ts);
    assert_eq!(buff, v.EncodeTo());

    // —— search：前缀比较器与 EncodeSearchKey 布局 ——
    let cmp = NewStartWithComparator();
    let raw = b"prefix-key";
    let enc = EncodeSearchKey(raw);
    // 编码键应能被“以原始前缀开头”的比较命中。
    assert!(cmp.Compare(&enc, &enc[..raw.len().max(1)]));
    // 裸前缀比较：abc 以 ab 开头。
    assert!(NewStartWithComparator().Compare(b"abc", b"ab"));

    // —— meta_kv：DBs 列表项键往返 ——
    let mDbs = b"DBs".to_vec();
    let txn_key = EncodeTxnMetaKey(&mDbs, &meta::DBkey(1), ts);
    let raw_meta = ParseTxnMetaKeyFrom(&txn_key).unwrap();
    // Field 解析出的 db_id 必须为 1。
    assert_eq!(meta::ParseDBKey(&raw_meta.Field).unwrap(), 1);
    assert_eq!(raw_meta.EncodeMetaKey(), txn_key);

    // —— stream_metas：合并 migration、ID 解析、路径合法性 ——
    let mut m1 = NewMigration();
    m1.TruncatedTo = 10;
    m1.EditMeta.push(MetaEdit {
        Path: "p1".into(),
        DeletePhysicalFiles: vec!["f1".into()],
        ..Default::default()
    });
    let mut m2 = NewMigration();
    m2.TruncatedTo = 20;
    m2.EditMeta.push(MetaEdit {
        Path: "p1".into(),
        DeletePhysicalFiles: vec!["f2".into()],
        ..Default::default()
    });
    // 同 Path 合并：TruncatedTo 取较大，删除文件列表并集。
    let merged = MergeMigrations(&m1, &m2);
    assert_eq!(merged.TruncatedTo, 20);
    assert_eq!(merged.EditMeta.len(), 1);
    assert_eq!(merged.EditMeta[0].DeletePhysicalFiles.len(), 2);
    assert_eq!(migIdOf("BASE").unwrap(), 0);
    assert_eq!(migIdOf("00000001_suffix").unwrap(), 1);
    assert!(migIdOf("bad").is_err());
    // 空串与路径穿越视为 insane；合法 backupmeta 路径通过。
    assert!(isInsane(""));
    assert!(isInsane("../escape"));
    assert!(isInsane("logs/.."));
    assert!(isInsane("v1/backupmeta/../.."));
    assert!(!isInsane("v1/backupmeta/foo"));
    assert!(isEmptyEdition(&MetaEdit::default()));
    let mig = NewMigration();
    // 名称前缀带零填充序号。
    assert!(nameOf(&mig, 1).starts_with("00000001_"));

    // —— UpdateShiftTS：从 metadata 的 DefaultCF MinBeginTs 推 shift ——
    let md = Metadata {
        MinTs: 5,
        MaxTs: 20,
        FileGroups: vec![DataFileGroup {
            DataFilesInfo: vec![DataFileInfo {
                Cf: WriteCF.into(),
                MinTs: 6,
                MaxTs: 15,
                MinBeginTsInDefaultCf: 7,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let (shift, ok) = UpdateShiftTSFromMetadata(&md, 6, 15);
    assert!(ok);
    assert_eq!(shift, 7);
    // 文件名编码的 store/range 亦应能成功更新。
    let (_, ok2) = UpdateShiftTS(
        "000000000000000a000000000000000b-d000000000000000cl000000000000000du000000000000000e.meta",
        &md,
        0xd,
        0xe,
    );
    assert!(ok2);

    // —— table_mapping + SchemasReplace：ID 映射与键重写 ——
    let db_id: i64 = 1;
    let downstream: i64 = 101;
    let db_name = "db";
    let db_value = serde_json::to_vec(&model::DBInfo {
        ID: db_id,
        Name: model::CIStr {
            O: db_name.into(),
            L: db_name.into(),
        },
    })
    .unwrap();
    let default_key = EncodeTxnMetaKey(b"DBs", &meta::DBkey(db_id), 100);
    let write_key = EncodeTxnMetaKey(b"DBs", &meta::DBkey(db_id), 200);

    let mut mgr = NewTableMappingManager();
    let mut collector = TestCollector {
        dbs: Vec::new(),
        tables: Vec::new(),
    };
    // DefaultCF 携带 DBInfo JSON，应触发 OnDatabaseInfo。
    mgr.ParseMetaKvAndUpdateIdMapping(&default_key, &db_value, DefaultCF, 100, &mut collector)
        .unwrap();
    let mut write_val = vec![WriteTypePut];
    write_val = crate::stubs::codec::EncodeUvarint(write_val, 100);
    // WriteCF 值至少 9 字节才通过 RawWriteCFValue 校验。
    while write_val.len() < 9 {
        write_val.push(0xff);
    }
    mgr.ParseMetaKvAndUpdateIdMapping(&write_key, &write_val, WriteCF, 200, &mut collector)
        .unwrap();
    assert_eq!(collector.dbs.len(), 1);
    assert!(mgr.DBReplaceMap.contains_key(&db_id));
    // 上游临时映射 ID 为负，表示待分配下游 ID。
    assert!(mgr.DBReplaceMap[&db_id].DbID < 0);

    let mut db_map = HashMap::new();
    db_map.insert(db_id, NewDBReplace(db_name.into(), downstream));
    // genTS=9999：WriteCF 重写后 Ts 被替换；DefaultCF 保留原 Ts。
    let sr = NewSchemasReplace(db_map, false, 9999);
    let new_key = sr
        .RewriteKeyForDB(&default_key, DefaultCF)
        .unwrap()
        .unwrap();
    let decoded = ParseTxnMetaKeyFrom(&new_key).unwrap();
    assert_eq!(meta::ParseDBKey(&decoded.Field).unwrap(), downstream);
    assert_eq!(decoded.Ts, 100);
    let new_write = sr.RewriteKeyForDB(&write_key, WriteCF).unwrap().unwrap();
    let decoded_w = ParseTxnMetaKeyFrom(&new_write).unwrap();
    assert_eq!(decoded_w.Ts, 9999);

    // —— logging_helper：DBReplaceMap 文本摘要 ——
    let mut lines = Vec::new();
    LogDBReplaceMap(
        "title",
        &HashMap::from([(db_id, NewDBReplace("db".into(), downstream))]),
        Some(&mut lines),
    );
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("downstreamId=101"));

    // —— search：内存存储上按前缀检索数据文件 ——
    let mem = MemStorage::new();
    let search_raw = b"search-prefix";
    let file_key = EncodeSearchKey(search_raw);
    let file_key_with_ts = crate::stubs::codec::EncodeUintDesc(file_key.clone(), 50);
    let entry = EncodeKVEntry(&file_key_with_ts, b"val");
    // Sha256 与文件内容一致，Search 才会接受该 DataFileInfo。
    let checksum = sha2::Sha256::digest(&entry);
    mem.insert("data/file1", entry);
    let data_file = DataFileInfo {
        Path: "data/file1".into(),
        StartKey: file_key.clone(),
        EndKey: crate::stubs::codec::EncodeUintDesc(file_key.clone(), u64::MAX),
        MinTs: 1,
        MaxTs: 100,
        Cf: DefaultCF.into(),
        Sha256: checksum.to_vec(),
        ..Default::default()
    };
    let meta_json = serde_json::to_vec(&Metadata {
        Files: vec![data_file.clone()],
        ..Default::default()
    })
    .unwrap();
    mem.insert("v1/backupmeta/test.meta", meta_json);
    let storage = Arc::new(mem) as Arc<dyn Storage>;
    let search = NewStreamBackupSearch(
        storage.clone(),
        NewStartWithComparator(),
        search_raw.to_vec(),
    );
    // 前缀命中后应返回非空结果集。
    let found = search.Search().unwrap();
    // 至少包含刚插入的 data/file1 条目。
    assert!(!found.is_empty());
    // 对齐 Go export_test.go：测试可直接调用单文件搜索入口。
    let from_data_file = search.SearchFromDataFileForTest(&data_file).unwrap();
    assert_eq!(from_data_file.len(), 1);
    assert_eq!(from_data_file[0].Value, "dmFs");
}
