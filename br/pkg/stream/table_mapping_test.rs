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

//! Go-equivalent tests for `br/pkg/stream/table_mapping_test.go`.
//! 验证 TableMappingManager 的 proto 往返、合并基线映射、FilteredOut 透传、
//! meta KV 解析与临时 ID 复用；MockMetaInfoCollector 仅保留更新的 commit_ts。
//! 与 Go 单测场景一一对应，不改断言语义。

// 上游 ID 在还原前可为负临时值，合并基线后应采纳已分配正 ID。
// Proto 往返必须保持分区映射与 FilteredOut，避免过滤信息丢失。
// Meta KV 解析需同时覆盖 DefaultCF 与 WriteCF，才能写入 ID 映射。
// TableHistory 乱序到达时以最大 commit_ts 为准，防止旧名回写。
// Mock 收集器按时间戳门禁，模拟真实 collector 的幂等更新。
// 本文件只解释测试意图与断言依据，不改任何可执行逻辑。
// Go 对照文件：br/pkg/stream/table_mapping_test.go。
// 错误槽 CleanError/ReportIfError 保证无错误时放行。
// 预分配区间为 ReplaceTemporaryIDs 预留，替换后所有已使用负 ID 必须消失。
// DBReplaceMap 键始终为上游库 ID，值为下游替换描述。
// 分区映射 PartitionMap 的键/值同为上下游物理表 ID。
// FilteredOut 在库与表两级均可设置，ToProto 需双层透传。
// WriteCF 值以 'P' 开头并编码 start_ts，对齐 TiKV 事务写记录。
// EncodeTxnMetaKey 构造 meta 事务键，供 ParseMetaKv 识别 DB 条目。
// 合并空基线时不得清空现有临时映射。
// 合并入空现有映射时结果应等于基线快照。
// 断言深度相等以验证 serde/proto 字段无遗漏。
// 历史管理器与映射管理器分工：前者记名，后者记 ID 替换。

use std::collections::HashMap;

use astersql_br_pkg_utils_consts::{DefaultCF, WriteCF};

use crate::stubs::{
    DBReplace, DownstreamID, NewDBReplace, NewTableReplace, TableReplace, TableSimpleInfo,
    UpstreamID, meta, model, utils::EncodeTxnMetaKey,
};
use crate::table_history::NewTableHistoryManager;
use crate::table_mapping::{
    DatabaseSchemaLookup, FromDBMapProto, MetaInfoCollector, NewTableMappingManager,
    PiTRIdTrackerLookup, TableMappingManager,
};

#[derive(Default)]
struct TestTracker {
    dbs: std::collections::HashSet<i64>,
    tables: std::collections::HashSet<(i64, i64)>,
}

impl PiTRIdTrackerLookup for TestTracker {
    fn ContainsDB(&self, db_id: i64) -> bool {
        self.dbs.contains(&db_id)
    }

    fn ContainsDBAndTableId(&self, db_id: i64, table_id: i64) -> bool {
        self.tables.contains(&(db_id, table_id))
    }
}

struct TestInfoSchema(HashMap<String, i64>);

impl DatabaseSchemaLookup for TestInfoSchema {
    fn SchemaIDByName(&self, name: &str) -> Option<i64> {
        self.0.get(&name.to_ascii_lowercase()).copied()
    }
}

/// 内存版 MetaInfoCollector：按 commit_ts 取新，忽略乱序旧事件。
/// 用于 ParseMetaKvAndUpdateIdMapping 路径的副作用观测。
struct MockMetaInfoCollector {
    db_infos: HashMap<i64, model::DBInfo>,
    db_timestamps: HashMap<i64, u64>,
    table_infos: HashMap<i64, HashMap<i64, model::TableInfo>>,
    table_timestamps: HashMap<i64, HashMap<i64, u64>>,
}

/// 构造空收集器。
impl MockMetaInfoCollector {
    fn new() -> Self {
        Self {
            db_infos: HashMap::new(),
            db_timestamps: HashMap::new(),
            table_infos: HashMap::new(),
            table_timestamps: HashMap::new(),
        }
    }
}

/// 仅当 commit_ts 更新时覆盖库/表信息，对齐 Go mock 行为。
impl MetaInfoCollector for MockMetaInfoCollector {
    /// 库级元信息回调：较旧时间戳直接丢弃。
    fn OnDatabaseInfo(&mut self, db_id: i64, db_name: String, commit_ts: u64) {
        if self
            .db_timestamps
            .get(&db_id)
            .map(|ts| commit_ts > *ts)
            .unwrap_or(true)
        {
            self.db_infos.insert(
                db_id,
                model::DBInfo {
                    ID: db_id,
                    Name: model::CIStr {
                        O: db_name.clone(),
                        L: db_name,
                    },
                },
            );
            self.db_timestamps.insert(db_id, commit_ts);
        }
    }

    /// 表级元信息回调：按 db_id/table_id 双层映射，仅保留更新 TS。
    fn OnTableInfo(
        &mut self,
        db_id: i64,
        table_id: i64,
        table_simple_info: &TableSimpleInfo,
        commit_ts: u64,
    ) {
        self.table_infos.entry(db_id).or_default();
        self.table_timestamps.entry(db_id).or_default();
        let newer = self.table_timestamps[&db_id]
            .get(&table_id)
            .map(|ts| commit_ts > *ts)
            .unwrap_or(true);
        if newer {
            self.table_infos.get_mut(&db_id).unwrap().insert(
                table_id,
                model::TableInfo {
                    ID: table_id,
                    Name: model::CIStr {
                        O: table_simple_info.Name.clone(),
                        L: table_simple_info.Name.clone(),
                    },
                    ..Default::default()
                },
            );
            self.table_timestamps
                .get_mut(&db_id)
                .unwrap()
                .insert(table_id, commit_ts);
        }
    }
}

#[test]
/// 覆盖 FromDBReplaceMap→ToProto→FromDBMapProto 往返，含分区与 FilteredOut。
fn test_to_proto() {
    let db_name = "db1";
    let tbl_name = "t1";
    let old_db: UpstreamID = 100;
    let new_db: DownstreamID = 200;
    let old_tbl: UpstreamID = 101;
    let old_p1: UpstreamID = 102;
    let old_p2: UpstreamID = 103;
    let new_tbl: DownstreamID = 201;
    let new_p1: DownstreamID = 202;
    let new_p2: DownstreamID = 203;

    // 构造含两个分区映射且 FilteredOut 的表替换。
    let mut tr = NewTableReplace(tbl_name.into(), new_tbl);
    // 上游分区 → 下游分区映射。
    tr.PartitionMap.insert(old_p1, new_p1);
    tr.PartitionMap.insert(old_p2, new_p2);
    tr.FilteredOut = true;
    // 库替换挂载表映射，同样标记 FilteredOut。
    let mut dr = NewDBReplace(db_name.into(), new_db);
    dr.TableMap.insert(old_tbl, tr);
    dr.FilteredOut = true;
    let mut drs = HashMap::new();
    // 以上游库 ID 为键挂入映射。
    drs.insert(old_db, dr);

    let mut tm = NewTableMappingManager();
    tm.FromDBReplaceMap(Some(drs.clone())).unwrap();
    let db_map = tm.ToProto();
    assert_eq!(db_map.len(), 1);
    assert_eq!(db_map[0].Name, db_name);
    assert_eq!(db_map[0].IdMap.UpstreamId, old_db);
    assert_eq!(db_map[0].IdMap.DownstreamId, new_db);
    assert!(db_map[0].FilteredOut);
    assert_eq!(db_map[0].Tables.len(), 1);
    assert_eq!(db_map[0].Tables[0].Name, tbl_name);
    assert_eq!(db_map[0].Tables[0].IdMap.UpstreamId, old_tbl);
    assert_eq!(db_map[0].Tables[0].IdMap.DownstreamId, new_tbl);
    assert!(db_map[0].Tables[0].FilteredOut);
    // 两个分区映射都应进入 proto。
    assert_eq!(db_map[0].Tables[0].Partitions.len(), 2);

    // proto 反序列化后应与原始 DBReplace 映射深度相等。
    let drs2 = FromDBMapProto(db_map);
    assert_eq!(drs2, drs);
}

#[test]
/// MergeBaseDBReplace：空现有/空基线两种合并路径，期望与 Go table-driven 一致。
fn test_merge_base_db_replace() {
    let cases: Vec<(
        &str,
        HashMap<i64, DBReplace>,
        HashMap<i64, DBReplace>,
        HashMap<i64, DBReplace>,
    )> = vec![
        (
            // 现有映射为空时，结果应等于基线映射。
            "merge into empty existing map",
            HashMap::new(),
            HashMap::from([(
                1,
                DBReplace {
                    Name: "db1".into(),
                    DbID: 1000,
                    TableMap: HashMap::from([(
                        10,
                        TableReplace {
                            TableID: 1010,
                            Name: "table1".into(),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            )]),
            HashMap::from([(
                1,
                DBReplace {
                    Name: "db1".into(),
                    DbID: 1000,
                    TableMap: HashMap::from([(
                        10,
                        TableReplace {
                            TableID: 1010,
                            Name: "table1".into(),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            )]),
        ),
        (
            // 基线为空时，现有临时 ID 映射应原样保留。
            "merge empty base map",
            HashMap::from([(
                1,
                DBReplace {
                    Name: "db1".into(),
                    DbID: -1,
                    TableMap: HashMap::from([(
                        10,
                        TableReplace {
                            TableID: -10,
                            Name: "table1".into(),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            )]),
            HashMap::new(),
            HashMap::from([(
                1,
                DBReplace {
                    Name: "db1".into(),
                    DbID: -1,
                    TableMap: HashMap::from([(
                        10,
                        TableReplace {
                            TableID: -10,
                            Name: "table1".into(),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            )]),
        ),
    ];
    // 逐案合并后比对 DBReplaceMap。
    for (name, existing, base, expected) in cases {
        let mut tm = NewTableMappingManager();
        tm.FromDBReplaceMap(Some(existing)).unwrap();
        tm.MergeBaseDBReplace(base);
        assert_eq!(tm.DBReplaceMap, expected, "{name}");
    }
}

#[test]
/// FilteredOut 标志应进入 proto，供下游过滤已排除的库表。
fn test_filter_db_replace_map() {
    let tr = NewTableReplace("t".into(), 2);
    let tr2 = NewTableReplace("t2".into(), 3);
    let mut dr = NewDBReplace("db".into(), 1);
    dr.TableMap.insert(10, tr);
    dr.TableMap.insert(11, tr2);
    let mut tm = NewTableMappingManager();
    tm.FromDBReplaceMap(Some(HashMap::from([(100i64, dr)])))
        .unwrap();
    let tracker = TestTracker {
        dbs: std::collections::HashSet::from([100]),
        tables: std::collections::HashSet::from([(100, 10)]),
    };
    tm.ApplyFilterToDBReplaceMap(&tracker);
    assert!(!tm.DBReplaceMap[&100].FilteredOut);
    assert!(!tm.DBReplaceMap[&100].TableMap[&10].FilteredOut);
    assert!(tm.DBReplaceMap[&100].TableMap[&11].FilteredOut);
}

#[test]
/// 临时负 ID 按 -1、-10 的降序顺序分配并回写。
fn test_replace_temporary_i_ds() {
    let mut tm = NewTableMappingManager();
    // 预分配 [1000,2000) 供临时 ID 替换使用。
    tm.SetPreallocatedRange(1000, 2000);
    let mut dr = NewDBReplace("db".into(), -1);
    dr.TableMap.insert(10, NewTableReplace("t".into(), -10));
    tm.FromDBReplaceMap(Some(HashMap::from([(1i64, dr)])))
        .unwrap();
    fn allocate(n: usize) -> Result<Vec<i64>, crate::stubs::errors::Error> {
        Ok((0..n).map(|i| 1000 + i as i64).collect())
    }
    tm.ReplaceTemporaryIDs(allocate).unwrap();
    assert_eq!(tm.DBReplaceMap[&1].DbID, 1000);
    assert_eq!(tm.DBReplaceMap[&1].TableMap[&10].TableID, 1001);
    assert_eq!(tm.tempIDCounter, 0);
}

#[test]
fn duplicate_temporary_id_for_different_upstreams_is_rejected() {
    let mut dr = NewDBReplace("db".into(), -1);
    dr.TableMap.insert(10, NewTableReplace("t".into(), -1));
    let mut tm = NewTableMappingManager();
    tm.FromDBReplaceMap(Some(HashMap::from([(1, dr)]))).unwrap();
    let err = tm
        .ReplaceTemporaryIDs(|_| Ok(vec![1000]))
        .expect_err("duplicate temporary IDs must be rejected");
    assert!(err.to_string().contains("duplicate temporary ID -1"));
}

#[test]
/// DefaultCF 写入 DBInfo，WriteCF 提交记录后应注册临时 DbID（负值）。
fn test_parse_meta_kv_and_update_id_mapping() {
    let db_id: i64 = 1;
    let db_name = "db";
    let db_value = serde_json::to_vec(&model::DBInfo {
        ID: db_id,
        Name: model::CIStr {
            O: db_name.into(),
            L: db_name.into(),
        },
    })
    .unwrap();
    // DefaultCF：start_ts=100 的事务 meta 键。
    let default_key = EncodeTxnMetaKey(b"DBs", &meta::DBkey(db_id), 100);
    // WriteCF：commit_ts=200 的提交记录键。
    let write_key = EncodeTxnMetaKey(b"DBs", &meta::DBkey(db_id), 200);
    let mut mgr = NewTableMappingManager();
    // 旁路收集器验证 OnDatabaseInfo 副作用。
    let mut collector = MockMetaInfoCollector::new();
    mgr.ParseMetaKvAndUpdateIdMapping(&default_key, &db_value, DefaultCF, 100, &mut collector)
        .unwrap();
    // 构造 Put 类型 write 值，嵌入 start_ts=100。
    let mut write_val = vec![b'P'];
    write_val = crate::stubs::codec::EncodeUvarint(write_val, 100);
    while write_val.len() < 9 {
        write_val.push(0xff);
    }
    mgr.ParseMetaKvAndUpdateIdMapping(&write_key, &write_val, WriteCF, 200, &mut collector)
        .unwrap();
    // collector 应收到恰好一条库信息。
    assert_eq!(collector.db_infos.len(), 1);
    assert!(mgr.DBReplaceMap.contains_key(&db_id));
    // 新发现的库尚未分配下游 ID，保持负临时值。
    assert!(mgr.DBReplaceMap[&db_id].DbID < 0);
}

#[test]
/// 表/库名历史：乱序较小 TS 不得覆盖最新名；GetDBNameByID 取最大 TS。
fn test_table_history_manager_out_of_order_ts() {
    let mut hist = NewTableHistoryManager();
    hist.AddTableHistory(1, "t1", 10, 100);
    // 较旧 TS=50 不得覆盖最新表名。
    hist.AddTableHistory(1, "t1_new", 10, 50); // older ts ignored for latest
    hist.AddTableHistory(1, "t1_newer", 10, 200);
    let h = hist.GetTableHistory().get(&1).unwrap();
    // 最新 TS=200 的表名胜出。
    assert_eq!(h[1].TableName, "t1_newer");
    assert_eq!(h[1].Timestamp, 200);
    hist.RecordDBIdToName(10, "db_old", 100);
    // 乱序较小 TS 的库名应被忽略。
    hist.RecordDBIdToName(10, "db_new", 50);
    hist.RecordDBIdToName(10, "db_newest", 200);
    // 库名同样按最大 TS 取值。
    assert_eq!(hist.GetDBNameByID(10), Some("db_newest"));
}

#[test]
/// CleanError 后 ReportIfError 应成功（无待报告错误）。
fn test_report_error() {
    let mut tm = NewTableMappingManager();
    tm.noDefaultKVErrorMap
        .insert(1, crate::stubs::errors::Error::new("test"));
    assert!(tm.ReportIfError().is_err());
    tm.CleanError(2);
    assert!(tm.ReportIfError().is_err());
    tm.CleanError(1);
    assert!(tm.ReportIfError().is_ok());
}

#[test]
/// 合并基线正 ID 到现有临时映射时，应复用基线 DbID=1000。
fn test_reuse_existing_database_i_ds() {
    let mut tm = NewTableMappingManager();
    let mut filtered = NewDBReplace("db2".into(), -2);
    filtered.FilteredOut = true;
    let existing = HashMap::from([
        (1, NewDBReplace("DB1".into(), -1)),
        (2, filtered),
        (3, NewDBReplace("db3".into(), 30)),
    ]);
    tm.FromDBReplaceMap(Some(existing)).unwrap();
    let schemas = TestInfoSchema(HashMap::from([
        ("db1".into(), 100),
        ("db2".into(), 200),
        ("db3".into(), 300),
    ]));
    tm.ReuseExistingDatabaseIDs(&schemas);
    assert_eq!(tm.DBReplaceMap[&1].DbID, 100);
    assert!(tm.DBReplaceMap[&1].Reused);
    assert_eq!(tm.DBReplaceMap[&2].DbID, -2);
    assert!(!tm.DBReplaceMap[&2].Reused);
    assert_eq!(tm.DBReplaceMap[&3].DbID, 30);
    assert!(!tm.DBReplaceMap[&3].Reused);
}
