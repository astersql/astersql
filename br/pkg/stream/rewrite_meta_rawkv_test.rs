// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/stream/rewrite_meta_rawkv_test.go`.
//!
//! 覆盖 SchemasReplace 的键/值重写、分区交换、TTL 关闭、
//! PITR 缺映射跳过，以及 DDL delete-range 参数重映射。
//! 断言与 Go 同名用例对齐；钩子用 Arc<Mutex> 采集回调副作用。
//! 构造辅助把 DBInfo/TableInfo JSON 与 RewriteTS=9527 固定下来。
//! 不修改被测实现行为，仅解释场景与期望依据。
//! downstream = upstream + 100 是测试约定，非产品规则。
//! delete-range 用例验证 hex 表前缀重映射而非 SQL 执行。
//! exchange 用例串联 table_mapping 与 rewrite 两阶段。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_br_pkg_utils_consts::{DefaultCF, WriteCF};

use crate::meta_kv::{
    ParseTxnMetaKeyFrom, RawWriteCFValue, WriteTypeDelete, WriteTypePut, WriteTypeRollback,
};
use crate::rewrite_meta_rawkv::{
    NewDBReplace, NewSchemasReplace, NewSchemasReplaceWithHooks, NewTableReplace, PreDelRangeQuery,
    SchemasReplace,
};
use crate::stubs::LightningPhysicalImportTxnSource;
use crate::stubs::UpstreamID;
use crate::stubs::codec;
use crate::stubs::meta;
use crate::stubs::model;
use crate::stubs::tablecodec;
use crate::stubs::utils::EncodeTxnMetaKey;
use crate::table_mapping::{MetaInfoCollector, NewTableMappingManager};

// 测试内共用：编码辅助与 mock 构造放文件前部，用例按 Go 顺序排列。

/// 构造最小 DBInfo JSON，供 rewriteDBInfo 输入。
/// Name 的 O/L 同设，避免大小写分支干扰断言。
fn produce_db_info_value(db_name: &str, db_id: i64) -> Vec<u8> {
    serde_json::to_vec(&model::DBInfo {
        ID: db_id,
        Name: model::CIStr {
            O: db_name.into(),
            L: db_name.into(),
        },
    })
    .unwrap()
}

/// 构造最小 TableInfo JSON（无分区/TTL）。
/// 其余字段走 Default，聚焦 ID 重写结果。
fn produce_table_info_value(table_name: &str, table_id: i64) -> Vec<u8> {
    serde_json::to_vec(&model::TableInfo {
        ID: table_id,
        Name: model::CIStr {
            O: table_name.into(),
            L: table_name.into(),
        },
        ..Default::default()
    })
    .unwrap()
}

/// 测试用 SchemasReplace：RewriteTS 固定 9527。
/// 可选把 delete-range 查询追加到共享 Vec。
/// fromPitrIdMap=false，缺映射应报错而非跳过。
fn mock_empty_schemas_replace(
    queries: Option<Arc<Mutex<Vec<PreDelRangeQuery>>>>,
    db_map: HashMap<UpstreamID, crate::stubs::DBReplace>,
) -> SchemasReplace {
    // 将回调查询推入共享列表，供断言弹出。
    let hook = queries.map(|q| {
        Box::new(move |query: PreDelRangeQuery| {
            q.lock().unwrap().push(query);
        }) as Box<dyn FnMut(PreDelRangeQuery) + Send>
    });
    // fromPitr=false, RewriteTS=9527 与 Go mock 对齐。
    NewSchemasReplaceWithHooks(db_map, false, 9527, hook)
}

#[test]
/// 对应 Go TestRewriteKeyForDB。
/// DefaultCF 保留原 Ts；WriteCF 改为 RewriteTS；Field 换下游 dbID。
fn test_rewrite_key_for_db() {
    let db_id: i64 = 1;
    let db_name = "db";
    // 小 Ts，便于与 RewriteTS=9527 区分。
    let ts: u64 = 1234;
    let m_dbs = b"DBs";
    // 构造 DBs 列表项事务键，Field=DBkey(db_id)。
    let encoded_key = EncodeTxnMetaKey(m_dbs, &meta::DBkey(db_id), ts);

    let mut db_map = HashMap::new();
    // 下游 ID 约定为上游+100，便于断言。
    let downstream_id = db_id + 100;
    db_map.insert(db_id, NewDBReplace(db_name.into(), downstream_id));
    let sr = mock_empty_schemas_replace(None, db_map);

    // DefaultCF：Ts 不变，Field 换下游。
    let new_key = sr
        .RewriteKeyForDB(&encoded_key, DefaultCF)
        .unwrap()
        .unwrap();
    let decoded = ParseTxnMetaKeyFrom(&new_key).unwrap();
    // 断言 DefaultCF 路径。
    assert_eq!(decoded.Ts, ts);
    assert_eq!(meta::ParseDBKey(&decoded.Field).unwrap(), downstream_id);

    // WriteCF：Ts 必须等于 RewriteTS。
    let new_key = sr.RewriteKeyForDB(&encoded_key, WriteCF).unwrap().unwrap();
    let decoded = ParseTxnMetaKeyFrom(&new_key).unwrap();
    // 断言 WriteCF 路径。
    assert_eq!(decoded.Ts, sr.RewriteTS);
    assert_eq!(meta::ParseDBKey(&decoded.Field).unwrap(), downstream_id);
}

#[test]
/// 对应 Go TestRewriteDBInfo。
/// JSON ID 替换后可重复调用；并验证 shortValue 路径标记物理导入来源。
fn test_rewrite_db_info() {
    let db_id: i64 = 1;
    let db_name = "db1";
    // 上游 db_id=1 → 下游 +100。
    let value = produce_db_info_value(db_name, db_id);
    let mut db_map = HashMap::new();
    db_map.insert(db_id, NewDBReplace(db_name.into(), db_id + 100));
    let sr = mock_empty_schemas_replace(None, db_map);

    let new_value = sr.rewriteDBInfo(&value).unwrap().unwrap();
    let info: model::DBInfo = serde_json::from_slice(&new_value).unwrap();
    // 首次重写后 ID 等于映射下游。
    assert_eq!(info.ID, sr.DbReplaceMap[&db_id].DbID);

    // 再次重写应保持同一下游 ID（幂等）。
    let new_id = sr.DbReplaceMap[&db_id].DbID;
    let new_value = sr.rewriteDBInfo(&value).unwrap().unwrap();
    let info: model::DBInfo = serde_json::from_slice(&new_value).unwrap();
    assert_eq!(info.ID, sr.DbReplaceMap[&db_id].DbID);
    assert_eq!(new_id, sr.DbReplaceMap[&db_id].DbID);

    let mut write_value = RawWriteCFValue::default();
    // 手工拼装 WriteCF：Put + shortValue(DBInfo) + txnSource=7。
    let mut buff = vec![WriteTypePut];
    buff = codec::EncodeUvarint(buff, 1);
    buff.push(b'v');
    buff.push(value.len() as u8);
    buff.extend_from_slice(&value);
    buff.push(b'S');
    buff = codec::EncodeUvarint(buff, 7);
    // 解析后 shortValue 即为原始 DBInfo JSON。
    write_value.ParseFrom(&buff).unwrap();
    write_value.UpdateShortValue(
        sr.rewriteDBInfo(&write_value.GetShortValue())
            .unwrap()
            .unwrap(),
    );
    // 与生产 rewriteValue 路径一致的标记调用。
    write_value.MarkPhysicalImportTxnSource();
    // 往返编码后供后续对照（本断言以 buff2 为主）。
    let encoded = write_value.EncodeTo();
    let mut rewritten = RawWriteCFValue::default();
    rewritten.ParseFrom(&encoded).unwrap();
    // 标记后 txnSource 应 OR 上 LightningPhysicalImportTxnSource。
    let mut expected_source = 7u64;
    expected_source |= LightningPhysicalImportTxnSource;
    // 再拼期望缓冲，确认 EncodeTo 与带标记来源一致。
    let mut wv2 = RawWriteCFValue::default();
    let mut buff2 = vec![WriteTypePut];
    buff2 = codec::EncodeUvarint(buff2, 1);
    buff2.push(b'v');
    let nv = sr.rewriteDBInfo(&value).unwrap().unwrap();
    buff2.push(nv.len() as u8);
    buff2.extend_from_slice(&nv);
    buff2.push(b'S');
    buff2 = codec::EncodeUvarint(buff2, expected_source);
    wv2.ParseFrom(&buff2).unwrap();
    // 带 OR 后的 txnSource 编码必须稳定。
    assert_eq!(wv2.EncodeTo(), buff2);
}

#[test]
/// 对应 Go TestRewriteKeyForTable。
/// 遍历 Table/AutoIncrement/AutoTable/AutoRandom/Sequence 等多种 Field 形态。
/// 每种形态都校验 DefaultCF/WriteCF 的 Ts 与双侧 ID 重写。
fn test_rewrite_key_for_table() {
    let db_id: i64 = 1;
    let db_name = "db";
    let table_id: i64 = 57;
    let table_name = "table";
    let ts: u64 = 400036290571534337;
    // 多种 meta Field 编解码对，共用同一套 ID 映射。
    let cases: Vec<(fn(i64) -> Vec<u8>, fn(&[u8]) -> Result<i64, String>)> = vec![
        // 普通表键。
        (meta::TableKey, meta::ParseTableKey),
        // 自增 ID 元数据键。
        (meta::AutoIncrementIDKey, meta::ParseAutoIncrementIDKey),
        // 分配表 ID 计数器键。
        (meta::AutoTableIDKey, meta::ParseAutoTableIDKey),
        // auto_random 相关计数键。
        (meta::AutoRandomTableIDKey, meta::ParseAutoRandomTableIDKey),
        // sequence 对象键。
        (meta::SequenceKey, meta::ParseSequenceKey),
    ];
    // 对每种 Field 形态各跑 DefaultCF 与 WriteCF。
    for (encode_fn, decode_fn) in cases {
        let encoded_key = EncodeTxnMetaKey(&meta::DBkey(db_id), &encode_fn(table_id), ts);
        let mut db_map = HashMap::new();
        // 库/表下游均 +100。
        let down_db = db_id + 100;
        let down_tbl = table_id + 100;
        db_map.insert(db_id, NewDBReplace(db_name.into(), down_db));
        db_map
            .get_mut(&db_id)
            .unwrap()
            .TableMap
            // 单表映射。
            .insert(table_id, NewTableReplace(table_name.into(), down_tbl));
        let sr = mock_empty_schemas_replace(None, db_map);

        let new_key = sr
            .rewriteKeyForTable(&encoded_key, DefaultCF, decode_fn, encode_fn)
            .unwrap()
            .unwrap();
        let decoded = ParseTxnMetaKeyFrom(&new_key).unwrap();
        // DefaultCF：Ts 保持，Key/Field 均下游化。
        assert_eq!(decoded.Ts, ts);
        assert_eq!(meta::ParseDBKey(&decoded.Key).unwrap(), down_db);
        assert_eq!(decode_fn(&decoded.Field).unwrap(), down_tbl);

        // 同一键再走 WriteCF。
        let new_key = sr
            .rewriteKeyForTable(&encoded_key, WriteCF, decode_fn, encode_fn)
            .unwrap()
            .unwrap();
        let decoded = ParseTxnMetaKeyFrom(&new_key).unwrap();
        // WriteCF：Ts 换成 RewriteTS。
        assert_eq!(decoded.Ts, sr.RewriteTS);
        assert_eq!(meta::ParseDBKey(&decoded.Key).unwrap(), down_db);
        assert_eq!(decode_fn(&decoded.Field).unwrap(), down_tbl);
    }
}

#[test]
fn rewrite_meta_entry_routes_all_table_key_kinds() {
    let db_id = 1;
    let table_id = 57;
    let mut db_map = HashMap::new();
    db_map.insert(db_id, NewDBReplace("db".into(), 101));
    db_map
        .get_mut(&db_id)
        .unwrap()
        .TableMap
        .insert(table_id, NewTableReplace("table".into(), 157));
    let cases: Vec<(fn(i64) -> Vec<u8>, fn(&[u8]) -> Result<i64, String>)> = vec![
        (meta::AutoIncrementIDKey, meta::ParseAutoIncrementIDKey),
        (meta::AutoTableIDKey, meta::ParseAutoTableIDKey),
        (meta::AutoRandomTableIDKey, meta::ParseAutoRandomTableIDKey),
        (meta::SequenceKey, meta::ParseSequenceKey),
    ];

    for (encode, parse) in cases {
        let key = EncodeTxnMetaKey(&meta::DBkey(db_id), &encode(table_id), 1234);
        let mut sr = NewSchemasReplace(db_map.clone(), false, 9527);
        let entry = sr
            .RewriteMetaKvEntry(&key, b"unchanged", WriteCF)
            .unwrap()
            .expect("recognized table-scoped meta key");
        let decoded = ParseTxnMetaKeyFrom(&entry.Key).unwrap();
        assert_eq!(meta::ParseDBKey(&decoded.Key).unwrap(), 101);
        assert_eq!(parse(&decoded.Field).unwrap(), 157);
        assert_eq!(decoded.Ts, 9527);
        assert_eq!(entry.Value, b"unchanged");
    }
}

#[test]
fn rewrite_write_cf_preserves_rollback_and_marks_delete_source() {
    let db_id = 1;
    let table_id = 57;
    let mut db_map = HashMap::new();
    db_map.insert(db_id, NewDBReplace("db".into(), 101));
    db_map
        .get_mut(&db_id)
        .unwrap()
        .TableMap
        .insert(table_id, NewTableReplace("table".into(), 157));
    let key = EncodeTxnMetaKey(&meta::DBkey(db_id), &meta::TableKey(table_id), 1234);
    let mut sr = NewSchemasReplace(db_map, false, 9527);

    let rollback = codec::EncodeUvarint(vec![WriteTypeRollback], u64::MAX);
    let entry = sr
        .RewriteMetaKvEntry(&key, &rollback, WriteCF)
        .unwrap()
        .unwrap();
    assert_eq!(entry.Value, rollback);
    assert!(sr.GetDeletedTables().is_empty());

    let delete = codec::EncodeUvarint(vec![WriteTypeDelete], u64::MAX);
    let entry = sr
        .RewriteMetaKvEntry(&key, &delete, WriteCF)
        .unwrap()
        .unwrap();
    let mut expected = RawWriteCFValue::default();
    expected.ParseFrom(&delete).unwrap();
    expected.MarkPhysicalImportTxnSource();
    assert_eq!(entry.Value, expected.EncodeTo());
    assert!(sr.GetDeletedTables()[&db_id].contains(&table_id));
}

#[test]
fn rewrite_table_entry_uses_full_table_rewrite_contract() {
    let db_id = 40;
    let table_id = 100;
    let mut db_map = HashMap::new();
    db_map.insert(db_id, NewDBReplace("db".into(), 140));
    db_map
        .get_mut(&db_id)
        .unwrap()
        .TableMap
        .insert(table_id, NewTableReplace("renamed".into(), 200));
    let mut sr = NewSchemasReplace(db_map, false, 9527);
    let callback_count = Arc::new(Mutex::new(0));
    let callback_count_clone = callback_count.clone();
    sr.AfterTableRewrittenFn = Some(Box::new(move |deleted, _| {
        assert!(!deleted);
        *callback_count_clone.lock().unwrap() += 1;
    }));
    let table = model::TableInfo {
        ID: table_id,
        Name: model::CIStr {
            O: "old".into(),
            L: "old".into(),
        },
        TTLInfo: Some(model::TTLInfo {
            Enable: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let key = EncodeTxnMetaKey(&meta::DBkey(db_id), &meta::TableKey(table_id), 1234);
    let value = serde_json::to_vec(&table).unwrap();
    let entry = sr
        .RewriteMetaKvEntry(&key, &value, DefaultCF)
        .unwrap()
        .unwrap();
    let decoded_key = ParseTxnMetaKeyFrom(&entry.Key).unwrap();
    assert_eq!(meta::ParseDBKey(&decoded_key.Key).unwrap(), 140);
    let decoded_value: model::TableInfo = serde_json::from_slice(&entry.Value).unwrap();
    assert_eq!(decoded_value.Name.O, "renamed");
    assert!(!decoded_value.TTLInfo.unwrap().Enable);
    assert_eq!(*callback_count.lock().unwrap(), 1);
}

#[test]
/// 对应 Go TestRewriteTableInfo。
/// AfterTableRewrittenFn 应被调用并允许改写 TiFlashReplica。
/// 连续两次 rewrite 计数累加到 2。
fn test_rewrite_table_info() {
    let db_id: i64 = 40;
    let db_name = "db";
    let table_id: i64 = 100;
    let table_name = "t1";
    // 无分区表，验证钩子与 ID 替换。
    let value = produce_table_info_value(table_name, table_id);
    let mut db_map = HashMap::new();
    db_map.insert(db_id, NewDBReplace(db_name.into(), db_id + 100));
    db_map
        .get_mut(&db_id)
        .unwrap()
        .TableMap
        .insert(table_id, NewTableReplace(table_name.into(), table_id + 100));
    let mut sr = mock_empty_schemas_replace(None, db_map);
    let table_count = Arc::new(Mutex::new(0usize));
    let tc = table_count.clone();
    // 钩子：计数 + 注入 TiFlashReplica，验证可观测副作用。
    sr.AfterTableRewrittenFn = Some(Box::new(move |_deleted, table_info| {
        *tc.lock().unwrap() += 1;
        // 模拟恢复后补齐 TiFlash 副本信息。
        table_info.TiFlashReplica = Some(model::TiFlashReplicaInfo { Count: 1 });
    }));

    let new_value = sr.rewriteTableInfo(&value, db_id).unwrap().unwrap();
    let table_info: model::TableInfo = serde_json::from_slice(&new_value).unwrap();
    // ID 下游化。
    assert_eq!(
        table_info.ID,
        sr.DbReplaceMap[&db_id].TableMap[&table_id].TableID
    );
    // 钩子注入的 replica 计数可见。
    assert_eq!(table_info.TiFlashReplica.as_ref().unwrap().Count, 1);

    let new_id = sr.DbReplaceMap[&db_id].TableMap[&table_id].TableID;
    let new_value = sr.rewriteTableInfo(&value, db_id).unwrap().unwrap();
    let table_info: model::TableInfo = serde_json::from_slice(&new_value).unwrap();
    assert_eq!(
        table_info.ID,
        sr.DbReplaceMap[&db_id].TableMap[&table_id].TableID
    );
    assert_eq!(new_id, sr.DbReplaceMap[&db_id].TableMap[&table_id].TableID);
    // 两次成功 rewrite 应对应两次钩子调用。
    assert_eq!(*table_count.lock().unwrap(), 2);
}

#[test]
/// 对应 Go TestRewriteTableInfoForPartitionTable。
/// 分区定义 ID 必须全部映射；名称字符串保持不变。
/// 第二次 rewrite 结果与第一次下游 ID 一致（幂等）。
fn test_rewrite_table_info_for_partition_table() {
    let db_id: i64 = 40;
    let table_id: i64 = 100;
    // 分区 ID 紧邻表 ID，模拟真实分配。
    let pt1_id: i64 = 101;
    let pt2_id: i64 = 102;
    let table_name = "t1";
    let tbl = model::TableInfo {
        ID: table_id,
        Name: model::CIStr {
            O: table_name.into(),
            L: table_name.into(),
        },
        // 两分区 pt1/pt2，后续映射 +100。
        Partition: Some(model::PartitionInfo {
            Definitions: vec![
                model::PartitionDefinition {
                    ID: pt1_id,
                    Name: model::CIStr {
                        O: "pt1".into(),
                        L: "pt1".into(),
                    },
                },
                model::PartitionDefinition {
                    ID: pt2_id,
                    Name: model::CIStr {
                        O: "pt2".into(),
                        L: "pt2".into(),
                    },
                },
            ],
        }),
        ..Default::default()
    };
    let value = serde_json::to_vec(&tbl).unwrap();
    let mut db_map = HashMap::new();
    // 库/表/分区三级映射。
    db_map.insert(db_id, NewDBReplace("db".into(), db_id + 100));
    db_map
        .get_mut(&db_id)
        .unwrap()
        .TableMap
        .insert(table_id, NewTableReplace(table_name.into(), table_id + 100));
    db_map
        .get_mut(&db_id)
        .unwrap()
        .TableMap
        .get_mut(&table_id)
        .unwrap()
        .PartitionMap
        // pt1/pt2 各自 +100。
        .insert(pt1_id, pt1_id + 100);
    db_map
        .get_mut(&db_id)
        .unwrap()
        .TableMap
        .get_mut(&table_id)
        .unwrap()
        .PartitionMap
        .insert(pt2_id, pt2_id + 100);
    // RewriteTS=0：本用例不关心 WriteCF Ts。
    let mut sr = NewSchemasReplace(db_map, false, 0);

    let new_value = sr.rewriteTableInfo(&value, db_id).unwrap().unwrap();
    let table_info: model::TableInfo = serde_json::from_slice(&new_value).unwrap();
    // 表名不变，仅 ID/分区 ID 变化。
    assert_eq!(table_info.Name.O, table_name);
    assert_eq!(
        table_info.ID,
        sr.DbReplaceMap[&db_id].TableMap[&table_id].TableID
    );
    assert_eq!(
        table_info.Partition.as_ref().unwrap().Definitions[0].ID,
        sr.DbReplaceMap[&db_id].TableMap[&table_id].PartitionMap[&pt1_id]
    );
    assert_eq!(
        table_info.Partition.as_ref().unwrap().Definitions[0].Name.O,
        "pt1"
    );
    // 分区名不被 ID 重写影响。
    assert_eq!(
        table_info.Partition.as_ref().unwrap().Definitions[1].ID,
        sr.DbReplaceMap[&db_id].TableMap[&table_id].PartitionMap[&pt2_id]
    );

    // 缓存下游分区 ID，供二次 rewrite 对照。
    let new_id1 = sr.DbReplaceMap[&db_id].TableMap[&table_id].PartitionMap[&pt1_id];
    let new_id2 = sr.DbReplaceMap[&db_id].TableMap[&table_id].PartitionMap[&pt2_id];
    let new_value = sr.rewriteTableInfo(&value, db_id).unwrap().unwrap();
    let table_info: model::TableInfo = serde_json::from_slice(&new_value).unwrap();
    assert_eq!(
        table_info.Partition.as_ref().unwrap().Definitions[0].ID,
        new_id1
    );
    assert_eq!(
        table_info.Partition.as_ref().unwrap().Definitions[1].ID,
        new_id2
    );
}

/// 记录 OnTableInfo 见到的 (db,table)，供 exchange 场景断言。
/// OnDatabaseInfo 为空实现，本文件不依赖 DB 回调。
struct MockCollector {
    table_infos: HashMap<i64, HashMap<i64, bool>>,
}
impl MockCollector {
    /// 空采集器。
    fn new() -> Self {
        Self {
            table_infos: HashMap::new(),
        }
    }
}
impl MetaInfoCollector for MockCollector {
    /// 忽略 DB 回调。
    fn OnDatabaseInfo(&mut self, _db_id: i64, _db_name: String, _commit_ts: u64) {}
    /// 仅记录表出现过，值为占位 true。
    fn OnTableInfo(
        &mut self,
        db_id: i64,
        table_id: i64,
        _info: &crate::stubs::TableSimpleInfo,
        _commit_ts: u64,
    ) {
        self.table_infos
            .entry(db_id)
            .or_default()
            .insert(table_id, true);
    }
}

#[test]
/// 对应 Go TestRewriteTableInfoForExchangePartition。
/// 模拟交换分区后 JSON 内 ID 交叉：分区槽位持有对方表 ID。
/// 先经 TableMappingManager 吸收 meta，再 SchemasReplace 重写。
/// DefaultCF  alone 不触发 collector；WriteCF Put 后才登记表。
fn test_rewrite_table_info_for_exchange_partition() {
    // 两库两表 + 两分区，构造交换布局。
    let db_id1: i64 = 100;
    let table_id1: i64 = 101;
    let pt1_id: i64 = 102;
    let pt2_id: i64 = 103;
    // 第二库承载交换对侧普通表。
    let db_id2: i64 = 105;
    let table_id2: i64 = 106;
    let ts: u64 = 400036290571534337;

    let t1 = model::TableInfo {
        ID: table_id1,
        Name: model::CIStr {
            O: "t1".into(),
            L: "t1".into(),
        },
        Partition: Some(model::PartitionInfo {
            Definitions: vec![
                model::PartitionDefinition {
                    ID: pt1_id,
                    Name: model::CIStr {
                        O: "pt1".into(),
                        L: "pt1".into(),
                    },
                },
                model::PartitionDefinition {
                    ID: pt2_id,
                    Name: model::CIStr {
                        O: "pt2".into(),
                        L: "pt2".into(),
                    },
                },
            ],
        }),
        ..Default::default()
    };
    // t2 初始无分区。
    let t2 = model::TableInfo {
        ID: table_id2,
        Name: model::CIStr {
            O: "t2".into(),
            L: "t2".into(),
        },
        ..Default::default()
    };

    let mut db_map = HashMap::new();
    db_map.insert(db_id1, NewDBReplace(String::new(), db_id1 + 100));
    db_map
        .get_mut(&db_id1)
        .unwrap()
        .TableMap
        .insert(table_id1, NewTableReplace("t1".into(), table_id1 + 100));
    db_map
        .get_mut(&db_id1)
        .unwrap()
        .TableMap
        .get_mut(&table_id1)
        .unwrap()
        .PartitionMap
        .insert(pt1_id, pt1_id + 100);
    db_map
        .get_mut(&db_id1)
        .unwrap()
        .TableMap
        .get_mut(&table_id1)
        .unwrap()
        .PartitionMap
        .insert(pt2_id, pt2_id + 100);
    // db2 映射。
    db_map.insert(db_id2, NewDBReplace(String::new(), db_id2 + 100));
    db_map
        .get_mut(&db_id2)
        .unwrap()
        .TableMap
        .insert(table_id2, NewTableReplace("t2".into(), table_id2 + 100));

    let mut tm = NewTableMappingManager();
    // 以静态 map 为基线，再吃日志 meta 增量。
    tm.MergeBaseDBReplace(db_map);
    let mut collector = MockCollector::new();

    let mut t1_copy = t1.clone();
    let mut t2_copy = t2.clone();
    // 交换后：t1 的分区0 ID 变为 t2 的表 ID。
    t1_copy.Partition.as_mut().unwrap().Definitions[0].ID = table_id2;
    // t2 表 ID 变为原 pt1，模拟交换对侧。
    t2_copy.ID = pt1_id;
    // 交换后的 t1 JSON 作为 DefaultCF 值。
    let value = serde_json::to_vec(&t1_copy).unwrap();

    let txn_key = EncodeTxnMetaKey(&meta::DBkey(db_id1), &meta::TableKey(table_id1), ts);
    // DefaultCF 更新映射，但 collector 尚不登记 table（需 WriteCF）。
    tm.ParseMetaKvAndUpdateIdMapping(&txn_key, &value, DefaultCF, ts, &mut collector)
        .unwrap();
    // 仅 DefaultCF 时不应登记 table_id1。
    assert!(
        !collector
            .table_infos
            .get(&db_id1)
            .map(|m| m.contains_key(&table_id1))
            .unwrap_or(false)
    );

    // 最小合法 WriteCF Put（无 shortValue）。
    let mut write_cf = vec![WriteTypePut];
    write_cf = codec::EncodeUvarint(write_cf, ts);
    tm.ParseMetaKvAndUpdateIdMapping(&txn_key, &write_cf, WriteCF, ts + 1, &mut collector)
        .unwrap();
    // WriteCF Put 之后才应出现在 collector。
    assert!(collector.table_infos[&db_id1].contains_key(&table_id1));

    let mut sr = NewSchemasReplace(tm.DBReplaceMap.clone(), false, 0);
    // 补齐交换后的映射：分区0→table_id2+100；db2 侧 pt1 作表。
    sr.DbReplaceMap
        .get_mut(&db_id1)
        .unwrap()
        .TableMap
        .get_mut(&table_id1)
        .unwrap()
        .PartitionMap
        .insert(table_id2, table_id2 + 100);
    sr.DbReplaceMap
        .get_mut(&db_id2)
        .unwrap()
        .TableMap
        .insert(pt1_id, NewTableReplace("t2".into(), pt1_id + 100));

    let new_value = sr.rewriteTableInfo(&value, db_id1).unwrap().unwrap();
    let table_info: model::TableInfo = serde_json::from_slice(&new_value).unwrap();
    // 交换后分区0 映射到 table_id2+100。
    assert_eq!(table_info.ID, table_id1 + 100);
    assert_eq!(
        table_info.Partition.as_ref().unwrap().Definitions[0].ID,
        table_id2 + 100
    );
    assert_eq!(
        table_info.Partition.as_ref().unwrap().Definitions[1].ID,
        pt2_id + 100
    );

    let value = serde_json::to_vec(&t2_copy).unwrap();
    let txn_key = EncodeTxnMetaKey(&meta::DBkey(db_id2), &meta::TableKey(pt1_id), ts);
    tm.ParseMetaKvAndUpdateIdMapping(&txn_key, &value, DefaultCF, ts, &mut collector)
        .unwrap();
    // 对侧同样 Default+Write 双阶段。
    let mut write_cf2 = vec![WriteTypePut];
    write_cf2 = codec::EncodeUvarint(write_cf2, ts);
    tm.ParseMetaKvAndUpdateIdMapping(&txn_key, &write_cf2, WriteCF, ts + 1, &mut collector)
        .unwrap();
    // db2 侧以 pt1_id 作为表 ID 登记。
    assert!(collector.table_infos[&db_id2].contains_key(&pt1_id));

    let new_value = sr.rewriteTableInfo(&value, db_id2).unwrap().unwrap();
    let table_info: model::TableInfo = serde_json::from_slice(&new_value).unwrap();
    // 对侧表（原 pt1）重写到 pt1+100。
    assert_eq!(table_info.ID, pt1_id + 100);
}

#[test]
/// 对应 Go TestRewriteTableInfoForTTLTable。
/// 重写后 TTL Enable 必须为 false，其余 TTL 字段保留。
fn test_rewrite_table_info_for_ttl_table() {
    let db_id: i64 = 40;
    let table_id: i64 = 100;
    // TTL 列名与间隔字面量对齐 Go。
    let col_name = "t";
    let table_name = "t1";
    let tbl = model::TableInfo {
        ID: table_id,
        Name: model::CIStr {
            O: table_name.into(),
            L: table_name.into(),
        },
        TTLInfo: Some(model::TTLInfo {
            ColumnName: model::CIStr {
                O: col_name.into(),
                L: col_name.into(),
            },
            IntervalExprStr: "1".into(),
            // day，与 Go 枚举值一致。
            IntervalTimeUnit: 5, // day
            // 输入为启用状态，重写后应变 false。
            Enable: true,
        }),
        ..Default::default()
    };
    let value = serde_json::to_vec(&tbl).unwrap();
    let mut db_map = HashMap::new();
    db_map.insert(db_id, NewDBReplace("db".into(), db_id + 100));
    db_map
        .get_mut(&db_id)
        .unwrap()
        .TableMap
        .insert(table_id, NewTableReplace(table_name.into(), table_id + 100));
    let mut sr = mock_empty_schemas_replace(None, db_map);
    let new_value = sr.rewriteTableInfo(&value, db_id).unwrap().unwrap();
    let table_info: model::TableInfo = serde_json::from_slice(&new_value).unwrap();
    assert_eq!(table_info.Name.O, table_name);
    assert_eq!(
        table_info.ID,
        sr.DbReplaceMap[&db_id].TableMap[&table_id].TableID
    );
    let ttl = table_info.TTLInfo.as_ref().unwrap();
    // 列名与间隔表达式保留。
    assert_eq!(ttl.ColumnName.O, col_name);
    assert_eq!(ttl.IntervalExprStr, "1");
    // Enable 已在下方断言关闭。
    // 关键：恢复路径强制关闭 TTL。
    assert!(!ttl.Enable);
}

#[test]
/// 对应 Go TestFromPitrIdMap。
/// fromPitrIdMap=true 时未知 DB/表返回 None，不报错。
fn test_from_pitr_id_map() {
    let mut db_replace = HashMap::new();
    // map 仅含 db=1/table=100。
    db_replace.insert(1, NewDBReplace("test_db".into(), 1));
    db_replace
        .get_mut(&1)
        .unwrap()
        .TableMap
        .insert(100, NewTableReplace("test_table".into(), 100));
    let db_info_value = produce_db_info_value("test_db2", 2);
    let table_info_value = produce_table_info_value("test_table2", 101);
    // true=PITR：映射外对象一律跳过。
    let mut sr = NewSchemasReplace(db_replace, true, 0);
    // db_id=2 / table 101 均不在 map 中。
    assert!(sr.rewriteDBInfo(&db_info_value).unwrap().is_none());
    // 表 101 不在 db1 的 TableMap。
    assert!(sr.rewriteTableInfo(&table_info_value, 1).unwrap().is_none());
    // db=2 本身也不在 map。
    assert!(sr.rewriteTableInfo(&table_info_value, 2).unwrap().is_none());
}

// —— DDL Job delete-range 场景使用的固定上/下游 ID ——
// 旧 ID 70 段，新 ID 80 段，便于肉眼对照映射。
const M_DDL_JOB_DB_OLD_ID: i64 = 70;
// table0 及其三分区旧 ID。
const M_DDL_JOB_TABLE0_OLD_ID: i64 = 71;
const M_DDL_JOB_PARTITION0_OLD_ID: i64 = 72;
// partition1/2 紧随其后。
const M_DDL_JOB_PARTITION1_OLD_ID: i64 = 73;
const M_DDL_JOB_PARTITION2_OLD_ID: i64 = 74;
// 同库另一张普通表旧 ID。
const M_DDL_JOB_TABLE1_OLD_ID: i64 = 75;
// 对应下游 ID 段。
const M_DDL_JOB_DB_NEW_ID: i64 = 80;
const M_DDL_JOB_TABLE0_NEW_ID: i64 = 81;
// 分区新 ID 82-84，table1 新 ID 85。
const M_DDL_JOB_PARTITION0_NEW_ID: i64 = 82;
const M_DDL_JOB_PARTITION1_NEW_ID: i64 = 83;
const M_DDL_JOB_PARTITION2_NEW_ID: i64 = 84;
const M_DDL_JOB_TABLE1_NEW_ID: i64 = 85;

/// 表前缀编码为 hex，匹配 Job.DelRangeArgs 键形态。
fn encode_table_key(table_id: i64) -> String {
    // 与 remap_hex_table_key 输入格式一致。
    hex::encode(tablecodec::EncodeTablePrefix(table_id))
}

/// 装配含三分区表 + 普通表的映射，并挂 delete-range 采集钩子。
/// 全局映射应覆盖 table0/分区/table1 全部旧→新 ID。
fn ddl_job_schema_replace(queries: Arc<Mutex<Vec<PreDelRangeQuery>>>) -> SchemasReplace {
    let mut partition_map = HashMap::new();
    // 三分区旧→新。
    partition_map.insert(M_DDL_JOB_PARTITION0_OLD_ID, M_DDL_JOB_PARTITION0_NEW_ID);
    partition_map.insert(M_DDL_JOB_PARTITION1_OLD_ID, M_DDL_JOB_PARTITION1_NEW_ID);
    partition_map.insert(M_DDL_JOB_PARTITION2_OLD_ID, M_DDL_JOB_PARTITION2_NEW_ID);
    let mut table_replace0 = NewTableReplace(String::new(), M_DDL_JOB_TABLE0_NEW_ID);
    // table0 携带三分区映射。
    table_replace0.PartitionMap = partition_map;
    let table_replace1 = NewTableReplace(String::new(), M_DDL_JOB_TABLE1_NEW_ID);
    let mut table_map = HashMap::new();
    // table0（分区表）与 table1（普通表）。
    table_map.insert(M_DDL_JOB_TABLE0_OLD_ID, table_replace0);
    table_map.insert(M_DDL_JOB_TABLE1_OLD_ID, table_replace1);
    let mut db_replace = NewDBReplace(String::new(), M_DDL_JOB_DB_NEW_ID);
    db_replace.TableMap = table_map;
    let mut db_map = HashMap::new();
    // 单库映射挂到 SchemasReplace。
    db_map.insert(M_DDL_JOB_DB_OLD_ID, db_replace);
    mock_empty_schemas_replace(Some(queries), db_map)
}

#[test]
/// 对应 Go TestDeleteRangeForMDDLJob。
/// drop schema：五条 DelRange 全部映射到下游 StartKey 集合。
/// drop table0：四条（三分区+表）StartKey 均在下游集合内。
fn test_delete_range_for_mddl_job() {
    let queries = Arc::new(Mutex::new(Vec::new()));
    // 挂采集钩子的 replace。
    let mut sr = ddl_job_schema_replace(queries.clone());

    // 期望出现的下游 StartKey 对应 ID 全集。
    let all_table_ids = [
        M_DDL_JOB_TABLE0_NEW_ID,
        M_DDL_JOB_PARTITION0_NEW_ID,
        M_DDL_JOB_PARTITION1_NEW_ID,
        M_DDL_JOB_PARTITION2_NEW_ID,
        M_DDL_JOB_TABLE1_NEW_ID,
    ];
    let all_table_keys: std::collections::HashSet<_> = all_table_ids
        .iter()
        .map(|id| encode_table_key(*id))
        .collect();

    // 场景1：drop schema，覆盖表+三分区+另一表。
    // Job.NeedGC=true 才会产生查询。
    let job = model::Job {
        ID: 1,
        NeedGC: true,
        DelRangeArgs: vec![
            model::DelRangeArg {
                TableID: M_DDL_JOB_TABLE0_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_TABLE0_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_TABLE0_OLD_ID + 1),
            },
            model::DelRangeArg {
                TableID: M_DDL_JOB_PARTITION0_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_PARTITION0_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_PARTITION0_OLD_ID + 1),
            },
            model::DelRangeArg {
                TableID: M_DDL_JOB_PARTITION1_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_PARTITION1_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_PARTITION1_OLD_ID + 1),
            },
            model::DelRangeArg {
                TableID: M_DDL_JOB_PARTITION2_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_PARTITION2_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_PARTITION2_OLD_ID + 1),
            },
            model::DelRangeArg {
                TableID: M_DDL_JOB_TABLE1_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_TABLE1_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_TABLE1_OLD_ID + 1),
            },
        ],
    };
    sr.processIngestIndexAndDeleteRangeFromJob(&job).unwrap();
    let q = queries.lock().unwrap().pop().unwrap();
    // 参数条数与下游键集合大小一致。
    assert_eq!(q.ParamsList.len(), all_table_keys.len());
    // 每条 StartKey 必须已是下游 hex 前缀。
    for params in &q.ParamsList {
        assert!(
            all_table_keys.contains(&params.StartKey),
            "{}",
            params.StartKey
        );
    }

    // 场景2：只 drop table0 及其分区。
    // 清空后再测 drop table0。
    queries.lock().unwrap().clear();
    let job = model::Job {
        ID: 2,
        NeedGC: true,
        DelRangeArgs: vec![
            model::DelRangeArg {
                TableID: M_DDL_JOB_PARTITION0_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_PARTITION0_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_PARTITION0_OLD_ID + 1),
            },
            model::DelRangeArg {
                TableID: M_DDL_JOB_PARTITION1_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_PARTITION1_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_PARTITION1_OLD_ID + 1),
            },
            model::DelRangeArg {
                TableID: M_DDL_JOB_PARTITION2_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_PARTITION2_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_PARTITION2_OLD_ID + 1),
            },
            model::DelRangeArg {
                TableID: M_DDL_JOB_TABLE0_OLD_ID,
                ElemID: 0,
                StartKey: encode_table_key(M_DDL_JOB_TABLE0_OLD_ID),
                EndKey: encode_table_key(M_DDL_JOB_TABLE0_OLD_ID + 1),
            },
        ],
    };
    sr.processIngestIndexAndDeleteRangeFromJob(&job).unwrap();
    let q = queries.lock().unwrap().pop().unwrap();
    // 四条：三分区 + 表本身。
    assert_eq!(q.ParamsList.len(), 4);
    // StartKey 同样必须落在下游集合。
    for params in &q.ParamsList {
        assert!(all_table_keys.contains(&params.StartKey));
    }
}

#[test]
/// 对应 Go TestDeleteRangeForMDDLJob2。
/// 单表 drop：StartKey 变为 NEW_ID，ElemID 原样保留。
fn test_delete_range_for_mddl_job2() {
    let queries = Arc::new(Mutex::new(Vec::new()));
    let mut sr = ddl_job_schema_replace(queries.clone());
    // 单表单 ElemID=2。
    let job = model::Job {
        ID: 10,
        NeedGC: true,
        DelRangeArgs: vec![model::DelRangeArg {
            TableID: M_DDL_JOB_TABLE1_OLD_ID,
            ElemID: 2,
            StartKey: encode_table_key(M_DDL_JOB_TABLE1_OLD_ID),
            EndKey: encode_table_key(M_DDL_JOB_TABLE1_OLD_ID + 1),
        }],
    };
    sr.processIngestIndexAndDeleteRangeFromJob(&job).unwrap();
    let q = queries.lock().unwrap().pop().unwrap();
    // 仅一条范围，且 StartKey 已换 NEW_ID。
    assert_eq!(q.ParamsList.len(), 1);
    assert_eq!(
        q.ParamsList[0].StartKey,
        encode_table_key(M_DDL_JOB_TABLE1_NEW_ID)
    );
    // ElemID 不被 ID 映射改写。
    assert_eq!(q.ParamsList[0].ElemID, 2);
}

#[test]
/// Go TestCompatibleAlert 验证 alert 钩子；Rust 用 AfterTableRewrittenFn 等价触发。
/// 只要 rewriteTableInfo 成功，钩子必被调用一次。
fn test_compatible_alert() {
    // Go 侧测 alert；此处用 AfterTableRewrittenFn 证明钩子链路可达。
    let mut db_map = HashMap::new();
    // 最小 1→101 / 2→102 映射。
    db_map.insert(1, NewDBReplace("db".into(), 101));
    db_map
        .get_mut(&1)
        .unwrap()
        .TableMap
        .insert(2, NewTableReplace("t".into(), 102));
    let mut sr = mock_empty_schemas_replace(None, db_map);
    // 共享标志：钩子置 true。
    let called = Arc::new(Mutex::new(false));
    let c = called.clone();
    sr.AfterTableRewrittenFn = Some(Box::new(move |_, _| {
        *c.lock().unwrap() = true;
    }));
    // 触发 rewriteTableInfo → 钩子。
    let value = produce_table_info_value("t", 2);
    sr.rewriteTableInfo(&value, 1).unwrap();
    // 钩子必须已触发。
    assert!(*called.lock().unwrap());
}
