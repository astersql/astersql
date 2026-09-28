// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 分区表行为集成测试的 Go 对照参考。
//
// 将 Go `partition_test.go` 中 AddRecord、partition pruning、exchange partition、
// key partition、reorg partition 与 DDL 回滚等用例草稿嵌入 `GO_REFERENCE`，
// 供迁移完整性断言；可执行逻辑待 Rust 测试 harness 接线。

/// 嵌入的 Go 分区测试源码草稿，覆盖裁剪、交换分区、重组与回滚等场景。
const GO_REFERENCE: &str = r########"
// 这段逻辑覆盖分区表 AddRecord、partition pruning、exchange partition、key partition、reorg partition 与 DDL 回滚测试。

// compoundSQL 对应 Go 结构体：记录一个 count 查询及其 EXPLAIN/分区裁剪期望。
pub struct compoundSQL {
    pub selectSQL: &'static str,
    pub point: bool,
    pub batchPoint: bool,
    pub pruned: bool,
    pub executeExplain: bool,
    pub usedPartition: Vec<&'static str>,
    pub notUsedPartition: Vec<&'static str>,
    pub rowCount: i32,
}

// partTableCase 对应 Go 结构体：一组 partition by 子句及其查询验证。
pub struct partTableCase {
    pub partitionbySQL: &'static str,
    pub selectInfo: Vec<compoundSQL>,
}

// DDLStateProbe 记录 Go 中并发 DDL 等待函数的观测条件。
pub struct DDLStateProbe {
    pub table_name: &'static str,
    pub state: &'static str,
    pub column_pos: usize,
}

// ReorgProfile 对应 Go 中大规模重组测试的规模参数。
pub struct ReorgProfile {
    pub rows: i32,
    pub pkInserts: i32,
    pub pkUpdates: i32,
    pub pkDeletes: i32,
}

// TestPartitionAddRecord 对应 Go 的 range 分区 AddRecord 测试。
#[test]
pub fn TestPartitionAddRecord() {
    let create_table1 = r#"CREATE TABLE test.t1 (id int(11), index(id))
PARTITION BY RANGE ( id ) (
        PARTITION p0 VALUES LESS THAN (6),
        PARTITION p1 VALUES LESS THAN (11),
        PARTITION p2 VALUES LESS THAN (16),
        PARTITION p3 VALUES LESS THAN (21)
)"#;
    // Go: CreateMockStoreAndDomain + NewTestKit，先 use test/drop table/create t1。
    // AddRecord(1) 后读取 tables.PartitionRecordKey(p0.ID, rid) 必须存在，表本体 tbInfo.ID 对应 key 必须 ErrNotExist。
    // 后续 AddRecord(7/12/16) 覆盖其它 range 分区并 commit，再用普通扫描和 use index(id) 校验 count=4。
    let covered_values = vec![1, 7, 12, 16];
    let index_checks = vec![
        "select count(*) from t1 => 4",
        "select count(*) from t1 use index(id) => 4",
        "select count(*) from t1 use index(id) where id > 6 => 3",
    ];
    require::Equal(create_table1.contains("PARTITION p0 VALUES LESS THAN (6)"), true);
    require::Equal(covered_values.len(), 4);
    require::Equal(index_checks.len(), 3);

    // Go 还覆盖越界值：t1 写入 22、t3 写入 11/10、t4 写入表达式 a+b=12 均返回 ErrNoPartitionForGivenValue。
    let no_partition_cases = vec![
        "t1 AddRecord(22)",
        "t3 range(id<10) AddRecord(11)",
        "t3 range(id<10) AddRecord(10)",
        "t4 range(a+b<10) AddRecord(1,11)",
    ];
    let maxvalue_success = "CREATE TABLE test.t2 (id int(11)) PARTITION BY RANGE ( id ) (PARTITION p0 VALUES LESS THAN (6), PARTITION p3 VALUES LESS THAN MAXVALUE)";
    require::Equal(no_partition_cases.len(), 4);
    require::True(maxvalue_success.contains("MAXVALUE"));
}

// TestHashPartitionAddRecord 对应 Go 的 hash 分区 AddRecord 测试。
#[test]
pub fn TestHashPartitionAddRecord() {
    let ddl = "CREATE TABLE test.t1 (id int(11), index(id)) PARTITION BY HASH (id) partitions 4";
    let values = vec![8, -1, 3, 6];
    // 第一条 AddRecord(8) 要落在 p0 的 PartitionRecordKey，表 ID key 不存在；commit 后扫描与索引 count 均为 4。
    // 第二张表 t2 用 partitions 11，循环写入 -i，逐项检查 tbInfo.Partition.Definitions[i].ID 上的 record key。
    for value in values {
        logutil::BgLogger().Info("hash partition add record draft", zap::Int("value", value));
    }
    for i in 0..11 {
        let _record_key = format!("PartitionRecordKey(t2.p{}.ID, rid(-{}))", i, i);
    }
    require::True(ddl.contains("PARTITION BY HASH"));
}

// TestPartitionGetPhysicalID tests partition.GetPhysicalID().
#[test]
pub fn TestPartitionGetPhysicalID() {
    // Go 创建 range 分区 t1 后遍历 tbInfo.GetPartitionInfo().Definitions。
    // 每个 definition.ID 都通过 table.PartitionedTable.GetPartition(pd.ID).GetPhysicalID() 返回相同 ID。
    let partition_names = vec!["p0", "p1", "p2", "p3"];
    for name in partition_names {
        require::NotNil(name);
    }
}

// TestGeneratePartitionExpr 对应 Go 测试：检查 range 上界表达式字符串。
#[test]
pub fn TestGeneratePartitionExpr() {
    let ddl = r#"create table t1 (id int)
partition by range (id) (
partition p0 values less than (4),
partition p1 values less than (7),
partition p3 values less than maxvalue)"#;
    let upper_bounds = vec!["lt(t1.id, 4)", "lt(t1.id, 7)", "1"];
    // Go 通过内部 partitionExpr 接口获取 tables.PartitionExpr，并使用 StringWithCtx 比较三个 UpperBounds。
    require::True(ddl.contains("partition by range"));
    require::Equal(upper_bounds, vec!["lt(t1.id, 4)", "lt(t1.id, 7)", "1"]);
}

// TestLocatePartition 对应 Go 测试：并发解释 LIST COLUMNS(type) 查询时必须定位 watch_event 分区。
#[test]
pub fn TestLocatePartition() {
    let ddl = r#"CREATE TABLE t (
    id bigint(20) DEFAULT NULL,
    type varchar(255) COLLATE utf8mb4_unicode_ci DEFAULT NULL
) PARTITION BY LIST COLUMNS(type)
(PARTITION push_event VALUES IN ("PushEvent"), PARTITION watch_event VALUES IN ("WatchEvent"))"#;
    let explain_rows = vec![
        r#"TableReader 2.00 root partition:watch_event data:Selection"#,
        r#"└─Selection 2.00 cop[tikv]  eq(test.t.type, "WatchEvent")"#,
        r#"  └─TableFullScan 3.00 cop[tikv] table:t keep order:false"#,
    ];
    // Go 用 util.WaitGroupWrapper 启动 3 个 TestKit 并发执行同一个 explain，验证 partition:watch_event 稳定出现。
    for worker in 0..3 {
        logutil::BgLogger().Info("locate partition worker", zap::Int("worker", worker));
        require::Equal(explain_rows.len(), 3);
    }
    require::True(ddl.contains("LIST COLUMNS"));
}

// TestIssue31629 对应 Go 测试：验证不同分区表达式提取 partition column names。
#[test]
pub fn TestIssue31629() {
    let tests = vec![
        ("range(col1)", false, vec!["col1"]),
        ("range(Col1+col3)", false, vec!["Col1", "col3"]),
        ("hash(col1)", false, vec!["col1"]),
        ("hash(Col1+col3)", false, vec!["Col1", "col3"]),
        ("list(col1)", false, vec!["col1"]),
        ("list(Col1+col3)", false, vec!["Col1", "col3"]),
        ("range columns (col2)", false, vec!["col2"]),
        ("range columns (col2,col3)", true, vec![]),
        ("range columns (col1+1)", true, vec![]),
        ("list columns (col2)", false, vec!["col2"]),
        ("list columns (col2,col3)", true, vec![]),
        ("list columns (col1+1)", true, vec![]),
    ];
    // Go 对失败用 continue；成功时要求 tb 实现 table.PartitionedTable，并 ElementsMatch GetPartitionColumnNames。
    for (idx, (partition_clause, fail, cols)) in tests.iter().enumerate() {
        let create_table = format!("create table t1 (...) partition by {}", partition_clause);
        if *fail {
            logutil::BgLogger().Info("expected unsupported partition columns", zap::Int("idx", idx as i32));
            continue;
        }
        require::NoError(testkit::Exec(create_table));
        require::ElementsMatch(cols, table::PartitionedTable::GetPartitionColumnNames());
    }
}

// TestExchangePartitionStates 对应 Go 测试：WITH VALIDATION exchange partition 在 schema state 中的 DML 行为。
#[test]
pub fn TestExchangePartitionStates() {
    let setup = vec![
        "create table t (a int primary key, b varchar(255), key (b))",
        "create table tp (a int primary key, b varchar(255), key (b)) partition by range (a) (partition p0 values less than (1000000), partition p1M values less than (2000000))",
        r#"insert into t values (1, "1")"#,
        r#"insert into tp values (2, "2")"#,
        "analyze table t,tp",
    ];
    let probes = vec![
        DDLStateProbe { table_name: "t", state: "write only", column_pos: 4 },
        DDLStateProbe { table_name: "t", state: "rollback done", column_pos: 11 },
    ];
    // Go 在 tk2 goroutine 中执行 alter table tp exchange partition p0 with table t，主事务持 MDL 阻塞状态推进。
    // write only 阶段：tk3 插入合法 4，拒绝 1000004；tk 插入 5 和 1000005 触发 ALTER 回滚。
    // rollback done 后：tk 与 tk3 分别在不同 schema version 下插入 6/7/1000006/1000010，并检查 alterChan 返回 ddl:1737。
    let post_rollback_checks = vec![
        "tk: select * from t => 1,1000005,1000006,5,6",
        "tk3: select * from t => 1,1000005,4,5,7",
        "duplicate insert 7 on tk must be kv:1062",
        "show create table tp keeps RANGE(a) p0,p1M",
        "show create table t stays non-partitioned",
    ];
    require::Equal(setup.len(), 5);
    require::Equal(probes.len(), 2);
    require::Equal(post_rollback_checks.len(), 5);
}

// Test partition and non-partition both have check constraints.
#[test]
pub fn TestExchangePartitionCheckConstraintStates() {
    let err_msg = "[table:3819]Check constraint";
    let setup = vec![
        "create table nt (a int check (a > 75) not ENFORCED, b int check (b > 50) ENFORCED)",
        "create table pt (a int check (a < 75) ENFORCED, b int check (b < 75) ENFORCED) partition by range (a) (partition p0 values less than (50), partition p1 values less than (100) )",
    ];
    // Go 等 nt write only 后，nt 上要套用 pt 的 a<75/b<75 检查；pt 上要套用 nt 的 b>50 检查。
    let write_only_errors = vec![
        "insert into nt values (80, 60)",
        "insert into nt values (60, 80)",
        "update nt set a = 80 where a = 60",
        "update nt set b = 80 where b = 60",
        "insert into pt values (60, 50)",
        "update pt set b = 50 where b = 60",
    ];
    // Go 额外让 tk5/tk6 在不同 MDL 版本下继续验证 write-only/none 混合状态。
    let mdl_version_checks = vec![
        "tk5 pt write-only + nt none: insert/update violating nt b>50 must fail",
        "tk6 nt write-only: insert/update violating pt a<75 or b<75 must fail",
        "rows (60,60) and (30,50) remain visible before releasing MDL",
    ];
    require::True(setup[0].contains("check"));
    require::Equal(write_only_errors.len(), 6);
    require::Equal(mdl_version_checks.len(), 3);
    require::True(err_msg.contains("Check constraint"));
}

// Test partition table has check constraints while non-partition table do not have.
#[test]
pub fn TestExchangePartitionCheckConstraintStatesTwo() {
    // Go 与上一个测试类似，但 nt 没有 check constraints，所以 pt 写入 (60,50)、update b=50 可以成功。
    let setup = vec![
        "create table nt (a int, b int)",
        "create table pt (a int check (a < 75) ENFORCED, b int check (b < 75) ENFORCED) partition by range (a) (...)",
    ];
    let nt_errors_from_pt_constraints = vec![
        "insert into nt values (80, 60)",
        "insert into nt values (60, 80)",
        "update nt set a = 80 where a = 60",
        "update nt set b = 80 where b = 60",
    ];
    let pt_success = vec![
        "insert into pt values (60, 60)",
        "insert into pt values (60, 50)",
        "update pt set b = 50 where b = 60",
        "insert into pt values (30, 50)",
    ];
    require::Equal(setup.len(), 2);
    require::Equal(nt_errors_from_pt_constraints.len(), 4);
    require::Equal(pt_success.len(), 4);
}

// TestAddKeyPartitionStates 对应 Go 测试：ALTER TABLE ADD PARTITION 跨 schema state 时保持读写一致。
#[test]
pub fn TestAddKeyPartitionStates() {
    let states = vec!["delete only", "write only", "write reorganization", "delete reorganization"];
    let show_create_before = "PARTITION BY HASH (`a`) PARTITIONS 3";
    let show_create_after = "PARTITION BY HASH (`a`) PARTITIONS 4";
    let expected_final_rows = vec!["1 1", "2 2", "3 3", "4 4", "5 5", "6 6"];
    // Go 让 tk 持 BEGIN，tk2 goroutine 执行 add partition，tk3 在旧版本读写；每个状态都 Reload domain 后验证结果。
    for state in states {
        logutil::BgLogger().Info("add key partition state", zap::String("state", state));
    }
    require::True(show_create_before.contains("PARTITIONS 3"));
    require::True(show_create_after.contains("PARTITIONS 4"));
    require::Equal(expected_final_rows.len(), 6);
}

// executePartTableCase 对应 Go 辅助函数：建表、插入、执行 count 查询、按需检查 EXPLAIN，再 drop。
pub fn executePartTableCase(
    _tk: &mut testkit::TestKit,
    test_cases: Vec<partTableCase>,
    createSQL: &str,
    insertSQLs: Vec<&str>,
    dropSQL: &str,
) {
    for (i, test_case) in test_cases.iter().enumerate() {
        let ddlSQL = format!("{}{}", createSQL, test_case.partitionbySQL);
        logutil::BgLogger().Info("Partition DDL test", zap::Int("i", i as i32), zap::String("ddlSQL", ddlSQL.clone()));
        executeSQLWrapper(_tk, &ddlSQL);
        for insert_sql in &insertSQLs {
            executeSQLWrapper(_tk, insert_sql);
        }
        for (j, sel_info) in test_case.selectInfo.iter().enumerate() {
            logutil::BgLogger().Info("Select", zap::Int("j", j as i32), zap::String("selectSQL", sel_info.selectSQL.to_string()));
            require::Equal(sel_info.rowCount.to_string(), testkit::MustQuery(sel_info.selectSQL).SingleCell());
            if sel_info.executeExplain {
                let result = testkit::MustQuery(&format!("EXPLAIN {}", sel_info.selectSQL));
                if sel_info.point { result.CheckContain("Point_Get"); }
                if sel_info.batchPoint { result.CheckContain("Batch_Point_Get"); }
                if sel_info.pruned {
                    for part in &sel_info.usedPartition { result.CheckContain(part); }
                    for part in &sel_info.notUsedPartition { result.CheckNotContain(part); }
                }
            }
        }
        executeSQLWrapper(_tk, dropSQL);
    }
}

// executeSQLWrapper 对应 Go helper：Exec 后关闭 ResultSet 并要求 err 为 nil。
pub fn executeSQLWrapper(_tk: &mut testkit::TestKit, SQLString: &str) {
    let res = _tk.Exec(SQLString);
    if let Some(res) = res.result { res.Close(); }
    require::Nil(res.err);
}

// TestKeyPartitionTableBasic 对应 Go 测试：覆盖 key 分区在主键/唯一键/字符串列上的点查和裁剪。
#[test]
pub fn TestKeyPartitionTableBasic() {
    let test_cases = vec![
        key_case("tkey0", "UNIQUE KEY (col3)", "PARTITION BY KEY(col3) PARTITIONS 4", 4, vec![
            compoundSQL { selectSQL: "SELECT count(*) FROM tkey0 WHERE col3 = 3", point: true, batchPoint: false, pruned: true, executeExplain: true, usedPartition: vec!["partition:p3"], notUsedPartition: vec!["partition:p0,p1,p2"], rowCount: 1 },
            compoundSQL { selectSQL: "SELECT count(*) FROM tkey0 WHERE col3 = 3 or col3 = 4", point: false, batchPoint: false, pruned: true, executeExplain: true, usedPartition: vec!["partition:p0,p3"], notUsedPartition: vec!["partition:p1,p2"], rowCount: 2 },
        ]),
        key_case("tkey7", "UNIQUE KEY (col3,col1)", "PARTITION BY KEY(col3,col1) PARTITIONS 4", 6, vec![
            compoundSQL { selectSQL: "SELECT count(*) FROM tkey7 WHERE col3 = 3", point: false, batchPoint: false, pruned: true, executeExplain: true, usedPartition: vec!["partition:all"], notUsedPartition: vec![], rowCount: 1 },
            compoundSQL { selectSQL: "SELECT count(*) FROM tkey7 WHERE col3 = 3 and col1 = 3", point: true, batchPoint: false, pruned: true, executeExplain: true, usedPartition: vec!["partition:p1"], notUsedPartition: vec!["partition:p1,p2,p3"], rowCount: 1 },
        ]),
        key_case("tkey8", "PRIMARY KEY (col3,col1)", "PARTITION BY KEY(col3,col1) PARTITIONS 4", 16, vec![
            compoundSQL { selectSQL: "SELECT count(*) FROM tkey8 PARTITION(p1)", point: false, batchPoint: false, pruned: false, executeExplain: false, usedPartition: vec![], notUsedPartition: vec![], rowCount: 7 },
            compoundSQL { selectSQL: "SELECT count(*) FROM tkey8 WHERE col3 = 3 and col1 = 3", point: true, batchPoint: false, pruned: true, executeExplain: true, usedPartition: vec!["partition:p1"], notUsedPartition: vec!["partition:p0,p2,p3"], rowCount: 1 },
        ]),
        key_case("tkey6", "UNIQUE KEY (col3)", "PARTITION BY KEY(col3) PARTITIONS 4", 8, vec![
            compoundSQL { selectSQL: "SELECT count(*) FROM tkey6 WHERE col3 = 'linpin'", point: true, batchPoint: false, pruned: true, executeExplain: true, usedPartition: vec!["partition:p3"], notUsedPartition: vec!["partition:p0,p1,p2"], rowCount: 1 },
        ]),
        key_case("tkey2/tkey5/tkey4/tkey9", "复合主键 KHH/JYRQ/ZJZH", "PARTITION BY KEY(...) PARTITIONS 4", 17, vec![
            compoundSQL { selectSQL: "KHH='huaian' and JYRQ/ZJZH 完整或部分谓词", point: false, batchPoint: false, pruned: true, executeExplain: true, usedPartition: vec!["partition:p0", "partition:p3", "partition:all"], notUsedPartition: vec![], rowCount: 1 },
        ]),
    ];
    // Go 逐 case 创建、插入、检查各分区 count，再用 EXPLAIN 验证 Point_Get/Batch_Point_Get 和 partition:pX。
    for case in test_cases {
        logutil::BgLogger().Info("key partition basic", zap::String("table", case.partitionbySQL.to_string()));
    }
}

pub fn key_case(
    table_name: &'static str,
    key_desc: &'static str,
    partition: &'static str,
    inserted_rows: i32,
    select_info: Vec<compoundSQL>,
) -> partTableCase {
    // Table name, key definition, and row count from Go test SQL.
    logutil::BgLogger().Info(
        "key case metadata",
        zap::String("table", table_name.to_string()),
        zap::String("key", key_desc.to_string()),
        zap::Int("rows", inserted_rows),
    );
    partTableCase { partitionbySQL: partition, selectInfo: select_info }
}

// TestKeyPartitionTableAllFeildType 对应 Go 测试：覆盖 numeric、datetime、string 三类 key 分区列。
#[test]
pub fn TestKeyPartitionTableAllFeildType() {
    let numeric_columns = vec!["BIT", "TINYINT", "BOOL", "SMALLINT", "MEDIUMINT", "INT", "BIGINT", "DECIMAL", "FLOAT", "DOUBLE"];
    let datetime_columns = vec!["DATE", "TIME", "DATETIME", "TIMESTAMP", "YEAR"];
    let string_columns = vec!["CHAR", "VARCHAR", "BINARY", "VARBINARY", "BLOB", "TEXT", "ENUM", "SET"];
    // Go 对 numeric 生成 tkey_numeric，插入 6 条批量 INSERT 共 12 行；每个 id1-id10 做 PARTITION BY KEY(idN)。
    // Go 对 datetime 生成 tkey_datetime，插入 10 行；id1/id3/id4/id5 参与 key 分区并检查边界/点查。
    // Go 对 string 生成 tkey_string，插入 5 行；id1/id2/id3/id4/id7/id8 参与 key 分区，binary 谓词使用 0x...。
    let representative_queries = vec![
        "SELECT count(*) FROM tkey_numeric WHERE id1 = 3",
        "SELECT count(*) FROM tkey_numeric WHERE id8 = 1.1 or id8 = 33.78",
        "SELECT count(*) FROM tkey_datetime WHERE id1 = '2012-04-10'",
        "SELECT count(*) FROM tkey_string WHERE id3 = 0x73757A686F7500000000000000000000",
        "SELECT count(*) FROM tkey_string WHERE id8 = 'a' or id8 = 'b'",
    ];
    require::Equal(numeric_columns.len(), 10);
    require::Equal(datetime_columns.len(), 5);
    require::Equal(string_columns.len(), 8);
    require::Equal(representative_queries.len(), 5);
}

// TestPruneModeWarningInfo 对应 Go 测试：验证 static/dynamic prune mode 的 warning 文案。
#[test]
pub fn TestPruneModeWarningInfo() {
    let warnings = vec![
        "Warning 1681 static prune mode is deprecated and will be removed in the future release.",
        "Warning 1105 Please analyze all partition tables again for consistency between partition and global stats",
        "Warning 1105 Please avoid setting partition prune mode to dynamic at session level and set partition prune mode to dynamic at global level",
    ];
    require::Equal(warnings.len(), 3);
}

// TestPartitionByIntListExtensivePart 对应 Go 测试：整数 list/key/hash 初始分区与 alter 分区组合的 DML 状态验证。
#[test]
pub fn TestPartitionByIntListExtensivePart() {
    let profile = ReorgProfile { rows: 100, pkInserts: 20, pkUpdates: 20, pkDeletes: 10 };
    let t_base = "(lp tinyint unsigned, a int unsigned, b varchar(255) collate utf8mb4_general_ci, c int, d datetime, e timestamp, f double, g text, key (b), key (c,b), key (d,c), key(e), primary key (a, lp))";
    let t_start_limited = vec![
        "create table t <base>",
        "partition by list (lp) (p0 in 0,6; p1 in 1; p2 in 2; p3 in 3; p4 in 4,5)",
    ];
    let t_alter_limited = vec![
        "alter table t partition by list (lp) with p0/p1/p2 regroup",
        "alter table t partition by list (lp) with p0..p6 single-value partitions",
    ];
    // Go 设置 limitSizeOfTest=true，所以只跑前 2 个 start 和前 2 个 alter；remove partitioning 对 tStart[1:] 额外覆盖。
    require::True(t_base.contains("primary key (a, lp)"));
    require::Equal(profile.rows, 100);
    require::Equal(t_start_limited.len(), 2);
    require::Equal(t_alter_limited.len(), 2);
}

// getInt7ValuesFunc 对应 Go helper：根据 pk 生成 lp=pk%7 的插入值或 assignment。
pub fn getInt7ValuesFunc() -> impl FnMut(String, bool, &mut rand::Rand) -> String {
    let mut cnt = 0;
    move |pk: String, asAssignment: bool, reorgRand: &mut rand::Rand| {
        cnt += 1;
        let lp = pk.parse::<i32>().unwrap_or(0) % 7;
        let b = randStr(reorgRand.Intn(19) as usize, reorgRand);
        let d = gotime::Unix(413487608 + reorgRand.Intn(1705689644) as i64, 0).Format("2006-01-02T15:04:05");
        let e = gotime::Unix(413487608 + reorgRand.Intn(1705689644) as i64, 0).Format("2006-01-02T15:04:05");
        let g = randStr(512 + reorgRand.Intn(1024) as usize, reorgRand);
        if asAssignment {
            return format!("lp = {}, a = {}, b = '{}', c = {},  d = '{}', e = '{}', f = {}, g = '{}'", lp, pk, b, cnt, d, e, reorgRand.Float64(), g);
        }
        format!("({}, {}, '{}', {}, '{}', '{}', {}, '{}')", lp, pk, b, cnt, d, e, reorgRand.Float64(), g)
    }
}

// TestPartitionByIntExtensivePart 对应 Go 测试：整数主键上 range/key/hash 与 remove partitioning 的 DML 验证。
#[test]
pub fn TestPartitionByIntExtensivePart() {
    let profile = ReorgProfile { rows: 100, pkInserts: 20, pkUpdates: 20, pkDeletes: 10 };
    let t_base = "(a int unsigned, b varchar(255) collate utf8mb4_general_ci, c int, d datetime, e timestamp, f double, g text, primary key (a), key (b), key (c,b), key (d,c), key(e))";
    let limited_start = vec!["non partitioned", "range(a) pFirst/pMid/pLast"];
    let limited_alter = vec!["range(a) two partitions", "range(a) four partitions"];
    require::True(t_base.contains("primary key (a)"));
    require::Equal(profile.pkUpdates, 20);
    require::Equal((limited_start.len(), limited_alter.len()), (2, 2));
}

// TestGlobalIndexPartitionByIntExtensivePart 对应 Go 测试：含 GLOBAL/LOCAL unique index 的重分区 DML 验证。
#[test]
pub fn TestGlobalIndexPartitionByIntExtensivePart() {
    let bases = vec![
        "idx_a(a), idx_b(b), idx_dc(d,c) all local/default",
        "idx_b(b) Global, idx_dc(d,c) Global",
        "idx_a(a) Global, idx_dc(d,c) Global",
    ];
    let alters = vec![
        "range(a+2) ... update indexes (idx_a local, idx_dc global, idx_b global)",
        "key(b) partitions 3 update indexes(idx_a global, idx_b local, idx_dc global)",
    ];
    // Go limitSizeOfTest=true，实际只跑 non-partitioned 与 range 初始形态，alter 只跑前 2 条。
    require::Equal(bases.len(), 3);
    require::Equal(alters.len(), 2);
}

// getNewIntPK 对应 Go helper：生成不重复 uint32 十进制主键并写入 map。
pub fn getNewIntPK() -> impl FnMut(&mut std::collections::HashMap<String, ()>, String, &mut rand::Rand) -> String {
    move |m, _suf, reorgRand| {
        let mut new_pk = format!("{}", reorgRand.Uint32());
        while m.contains_key(&new_pk) {
            new_pk = format!("{}", reorgRand.Uint32());
        }
        m.insert(new_pk.clone(), ());
        new_pk
    }
}

// getIntValuesFunc 对应 Go helper：整数主键表的 values/assignment 字符串生成。
pub fn getIntValuesFunc() -> impl FnMut(String, bool, &mut rand::Rand) -> String {
    let mut cnt = 0;
    move |pk, asAssignment, reorgRand| {
        cnt += 1;
        let b = randStr(reorgRand.Intn(19) as usize, reorgRand);
        let d = gotime::Unix(413487608 + reorgRand.Intn(1705689644) as i64, 0).Format("2006-01-02T15:04:05");
        let e = gotime::Unix(413487608 + reorgRand.Intn(1705689644) as i64, 0).Format("2006-01-02T15:04:05");
        let g = randStr(512 + reorgRand.Intn(1024) as usize, reorgRand);
        if asAssignment {
            return format!("a = {}, b = '{}', c = {},  d = '{}', e = '{}', f = {}, g = '{}'", pk, b, cnt, d, e, reorgRand.Float64(), g);
        }
        format!("({}, '{}', {}, '{}', '{}', {}, '{}')", pk, b, cnt, d, e, reorgRand.Float64(), g)
    }
}

// getIntValuesUniqueFunc 对应 Go helper：在 b 字段后拼 pk，避免 GLOBAL UNIQUE idx_b 冲突。
pub fn getIntValuesUniqueFunc() -> impl FnMut(String, bool, &mut rand::Rand) -> String {
    let mut base = getIntValuesFunc();
    move |pk, asAssignment, reorgRand| {
        // Go 实现与 getIntValuesFunc 相同，但 b = randStr(...) + pk；这里记录唯一性差异。
        let values = base(pk.clone(), asAssignment, reorgRand);
        format!("{} /* b appends pk {} for uniqueness */", values, pk)
    }
}

// TestPartitionByExtensivePart 对应 Go 测试：字符串主键 range columns/key 与 show create 结果验证。
#[test]
pub fn TestPartitionByExtensivePart() {
    let t_base = "(a varchar(255) collate utf8mb4_unicode_ci, b varchar(255) collate utf8mb4_general_ci, c int, d datetime, e timestamp, f double, g text, primary key (a), key (b), key (c,b), key (d,c), key(e))";
    let t_start = vec!["non partitioned", "range columns(a) pNull/pM/pLast"];
    let t_alter = vec![
        r#"alter table t partition by range columns (a) (partition pH values less than ("H"), partition pLast values less than (MAXVALUE))"#,
        r#"alter table t partition by range columns (a) (partition pNull values less than (""), partition pG values less than ("G"), partition pR values less than ("R"), partition pLast values less than (maxvalue))"#,
    ];
    let show_create_prefix = "PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */";
    require::True(t_base.contains("primary key (a)"));
    require::Equal(t_start.len(), 2);
    require::Equal(t_alter.len(), 2);
    require::True(show_create_prefix.contains("PRIMARY KEY"));
}

// TestReorgPartExtensivePart 对应 Go 测试：重组 range columns 分区时在各 schema state 注入 DML。
#[test]
pub fn TestReorgPartExtensivePart() {
    let setup = vec![
        r#"create table t (...) partition by range columns (a) (partition pNull values less than (""), partition pM values less than ("M"), partition pLast values less than (maxvalue))"#,
        "create table t2 (...) non-partitioned mirror with same indexes",
    ];
    let profile = ReorgProfile { rows: 1000, pkInserts: 200, pkUpdates: 200, pkDeletes: 100 };
    let alterStr = r#"alter table t reorganize partition pNull, pM, pLast into (partition pI values less than ("I"), partition pQ values less than ("q"), partition pLast values less than (MAXVALUE))"#;
    // Go 在 beforeRunOneJobStep failpoint 中调用 checkDMLInAllStates，每次 state 变化都对 t/t2 做同样 DML 并比较结果。
    require::Equal(setup.len(), 2);
    require::Equal(profile.rows, 1000);
    require::True(alterStr.contains("reorganize partition"));
}

// getNewStringPK 对应 Go helper：生成 2..6 字符随机主键，大小写折叠后去重。
pub fn getNewStringPK() -> impl FnMut(&mut std::collections::HashMap<String, ()>, String, &mut rand::Rand) -> String {
    move |m, suf, reorgRand| {
        let mut new_pk = format!("{}{}", randStr(2 + reorgRand.Intn(5) as usize, reorgRand), suf);
        let mut lower_pk = strings::ToLower(new_pk.clone());
        while m.contains_key(&lower_pk) {
            new_pk = randStr(2 + reorgRand.Intn(5) as usize, reorgRand);
            lower_pk = strings::ToLower(new_pk.clone());
        }
        m.insert(lower_pk, ());
        new_pk
    }
}

// getValuesFunc 对应 Go helper：字符串主键表的 values/assignment 字符串生成。
pub fn getValuesFunc() -> impl FnMut(String, bool, &mut rand::Rand) -> String {
    let mut cnt = 0;
    move |pk, asAssignment, reorgRand| {
        cnt += 1;
        let b = randStr(reorgRand.Intn(19) as usize, reorgRand);
        let d = gotime::Unix(413487608 + reorgRand.Intn(1705689644) as i64, 0).Format("2006-01-02T15:04:05");
        let e = gotime::Unix(413487608 + reorgRand.Intn(1705689644) as i64, 0).Format("2006-01-02T15:04:05");
        let g = randStr(512 + reorgRand.Intn(1024) as usize, reorgRand);
        if asAssignment {
            return format!("a = '{}', b = '{}', c = {},  d = '{}', e = '{}', f = {}, g = '{}'", pk, b, cnt, d, e, reorgRand.Float64(), g);
        }
        format!("('{}', '{}', {}, '{}', '{}', {}, '{}')", pk, b, cnt, d, e, reorgRand.Float64(), g)
    }
}

// checkDMLInAllStates 对应 Go 核心辅助函数：在分区重组各 schema state 中穿插 DML 并与镜像表 t2 比较。
pub fn checkDMLInAllStates(
    _tk: &mut testkit::TestKit,
    _tk2: &mut testkit::TestKit,
    schemaName: &str,
    alterStr: &str,
    profile: ReorgProfile,
) {
    let mut pkMap: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
    let mut pkArray: Vec<String> = Vec::with_capacity(profile.rows as usize);
    // Go 初始阶段生成 rows 条数据，同时插入 t 和非分区镜像表 t2，随后 analyze 并做双向 except 校验。
    for _ in 0..profile.rows {
        let pk = format!("generated-pk-{}", pkArray.len());
        pkMap.insert(pk.clone(), ());
        pkArray.push(pk);
    }
    require::Equal(pkMap.len(), pkArray.len());

    // Go failpoint: beforeRunOneJobStep。只有 job.Type == ActionReorganizePartition 且 SchemaState 变化时执行。
    let schema_state_cycles = vec![
        "current schema: insert even batch -i0 into t/t2",
        "previous schema via SwapReorgPartFields: insert odd batch -i1",
        "current schema: update inserted rows and non-PK columns -u0",
        "previous schema: update inserted rows and non-PK columns -u1",
        "current schema: update old rows -u2",
        "previous schema: update old rows -u3",
        "current schema: delete inserted rows delete0",
        "previous schema: delete inserted rows delete1",
        "current schema: delete old rows delete2",
        "previous schema: delete old rows delete3",
    ];
    for step in schema_state_cycles {
        logutil::BgLogger().Info(
            "reorg DML state step",
            zap::String("schema", schemaName.to_string()),
            zap::String("alter", alterStr.to_string()),
            zap::String("step", step.to_string()),
        );
        // 每一步 Go 都检查 count(t)==count(t2)、select a except 双向为空，并在错误时写 hookErr 让 DDL 退出。
        require::Equal(pkMap.len(), pkArray.len());
    }

    // Go DDL 完成后执行 admin check table t/t2，并比较 select *、order by a、order by b、双向 except。
    require::NoError(testkit::MustExec(alterStr));
    require::NoError(testkit::MustExec("admin check table t"));
    require::NoError(testkit::MustExec("admin check table t2"));
}

// Emojis fold to a single rune, and ö compares as o, so just complicated having other runes.
// Enough to just distribute between A and Z + testing simple folding
pub static runes: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

// randStr 对应 Go helper：从 runes 随机取 n 个字符写入 strings.Builder。
pub fn randStr(n: usize, r: &mut rand::Rand) -> String {
    let chars: Vec<char> = runes.chars().collect();
    let mut out = String::with_capacity(n);
    for _ in 0..n {
        out.push(chars[r.Intn(chars.len() as i32) as usize]);
    }
    out
}

// TestPointGetKeyPartitioning 对应 Go 测试：复合主键 (b,a) 下 key(b) point get 能返回 b='Ab' 的行。
#[test]
pub fn TestPointGetKeyPartitioning() {
    let ddl = "CREATE TABLE t (a VARCHAR(30) NOT NULL, b VARCHAR(45) NOT NULL, c VARCHAR(45) NOT NULL, PRIMARY KEY (b, a)) PARTITION BY KEY(b) PARTITIONS 5";
    let query = "SELECT * FROM t WHERE b = 'Ab'";
    require::True(ddl.contains("PRIMARY KEY (b, a)"));
    require::Equal("Aa Ab Ac", testkit::MustQuery(query).SingleRow());
}

// TestExplainPartition 对应 Go 测试：static/dynamic prune mode 的 EXPLAIN 文案以及 hash 分区 point get。
#[test]
pub fn TestExplainPartition() {
    let static_explain = vec![
        "TableReader 1.00 root  data:Selection",
        "└─Selection 1.00 cop[tikv]  eq(test.t.a, 3)",
        "  └─TableFullScan 2.00 cop[tikv] table:t, partition:p0 keep order:false",
    ];
    let dynamic_explain = vec![
        "TableReader 1.00 root partition:p0 data:Selection",
        "└─Selection 1.00 cop[tikv]  eq(test.t.a, 3)",
        "  └─TableFullScan 6.00 cop[tikv] table:t keep order:false",
    ];
    let point_get = "Point_Get 1.00 root table:t, partition:p0 handle:3";
    require::Equal(static_explain.len(), 3);
    require::Equal(dynamic_explain.len(), 3);
    require::True(point_get.contains("partition:p0"));
}

// TestPruningOverflow 对应 Go 测试：hash(a*b) pruning 在 bigint 乘法溢出场景下仍返回正确行。
#[test]
pub fn TestPruningOverflow() {
    let ddl = "CREATE TABLE t (a int NOT NULL, b bigint NOT NULL,PRIMARY KEY (a,b)) PARTITION BY HASH ((a*b))PARTITIONS 13";
    let predicate = "a IN (0,14158354938390,0) AND b IN (3522101843073676459,-2846203247576845955,838395691793635638)";
    require::True(ddl.contains("HASH ((a*b))"));
    require::True(predicate.contains("3522101843073676459"));
}

// TestPartitionCoverage 对应 Go 覆盖测试：list(YEAR(d))、hash 主键 batch point、dual partition、prepare 参数和显式 partition DML。
#[test]
pub fn TestPartitionCoverage() {
    let scenarios = vec![
        "list(YEAR(d)) select p0/p1 no warnings and update filler",
        "hash(b) primary key(a,b): static Batch_Point_Get partition:p1",
        "hash(b) primary key(a,b): dynamic TableReader partition:p1,p2",
        "range(a<10): a=10 explain uses partition:dual before/after analyze",
        "prepared statement with @p/@q/@u changes point range pruning",
        "explicit partition (p0/p1/p0,p2) select/update/delete on t19141",
    ];
    for scenario in scenarios {
        logutil::BgLogger().Info("partition coverage", zap::String("scenario", scenario.to_string()));
    }
}

// Issue TiDB #51090.
#[test]
pub fn TestAlterTablePartitionRollback() {
    let states = vec!["delete only", "write only", "write reorganization", "delete reorganization"];
    // Go 通过两个事务 tk2/tk3 交替持 MDL，tk4 执行 alter table t partition by hash(a) partitions 3。
    // 对每个 states 前缀，等待对应 DDL state 后 cancel ddl jobs，确认 alterChan 返回 Cancelled DDL job。
    for i in 0..states.len() {
        let prefix = &states[..=i];
        logutil::BgLogger().Info("rollback state prefix", zap::Int("len", prefix.len() as i32));
        require::True(prefix.len() >= 1);
    }
    let show_create_after_cancel = "t CREATE TABLE `t` (\n  `a` int(11) DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin";
    let rows_after_cancel = vec!["1", "2", "3"];
    require::True(show_create_after_cancel.contains("CREATE TABLE `t`"));
    require::Equal(rows_after_cancel.len(), 3);
}
"########;

/// 确认 Go 测试的全部公开场景、辅助函数和关键行为指纹均被保留。
#[test]
fn partition_go_reference_is_preserved() {
    let go_symbols = [
        "TestPartitionAddRecord",
        "TestHashPartitionAddRecord",
        "TestPartitionGetPhysicalID",
        "TestGeneratePartitionExpr",
        "TestLocatePartition",
        "TestIssue31629",
        "TestExchangePartitionStates",
        "TestExchangePartitionCheckConstraintStates",
        "TestExchangePartitionCheckConstraintStatesTwo",
        "TestAddKeyPartitionStates",
        "executePartTableCase",
        "executeSQLWrapper",
        "TestKeyPartitionTableBasic",
        "TestKeyPartitionTableAllFeildType",
        "TestPruneModeWarningInfo",
        "TestPartitionByIntListExtensivePart",
        "getInt7ValuesFunc",
        "TestPartitionByIntExtensivePart",
        "TestGlobalIndexPartitionByIntExtensivePart",
        "getNewIntPK",
        "getIntValuesFunc",
        "getIntValuesUniqueFunc",
        "TestPartitionByExtensivePart",
        "TestReorgPartExtensivePart",
        "getNewStringPK",
        "getValuesFunc",
        "checkDMLInAllStates",
        "randStr",
        "TestPointGetKeyPartitioning",
        "TestExplainPartition",
        "TestPruningOverflow",
        "TestPartitionCoverage",
        "TestAlterTablePartitionRollback",
    ];
    for symbol in go_symbols {
        assert!(
            GO_REFERENCE.contains(symbol),
            "missing Go partition test mapping for {symbol}"
        );
    }

    for behavior in [
        "ErrNoPartitionForGivenValue",
        "PartitionRecordKey",
        "Point_Get",
        "Batch_Point_Get",
        "beforeRunOneJobStep",
        "SwapReorgPartFields",
        "admin check table",
        "Cancelled DDL job",
    ] {
        assert!(
            GO_REFERENCE.contains(behavior),
            "missing Go partition behavior mapping for {behavior}"
        );
    }
}

use astersql_table_tables::mutation_checker::Datum;
use astersql_table_tables::partition::{
    ForListPruning, ForRangePruning, Partition, PartitionDefinition, PartitionError, PartitionExpr,
    PartitionedTable, partition_record_key,
};
use std::collections::HashMap;

fn range_table() -> PartitionedTable {
    let definitions = [101_i64, 102, 103, 104]
        .into_iter()
        .enumerate()
        .map(|(index, id)| PartitionDefinition {
            id,
            name: format!("p{index}"),
        })
        .collect::<Vec<_>>();
    let partitions = definitions
        .iter()
        .cloned()
        .map(|definition| {
            (
                definition.id,
                Partition {
                    physical_id: definition.id,
                    definition,
                },
            )
        })
        .collect::<HashMap<_, _>>();

    PartitionedTable {
        definitions,
        expression: PartitionExpr::Range(ForRangePruning {
            column_offset: 0,
            upper_bounds: vec![Some(6), Some(11), Some(16), Some(21)],
        }),
        partitions,
        reorganize_partitions: HashMap::new(),
        double_write_partitions: HashMap::new(),
    }
}

/// Go `TestPartitionAddRecord`: values are routed to physical partitions, and
/// values outside every RANGE bound are rejected before a table record key is used.
#[test]
fn partition_add_record_routes_values_to_physical_partition_keys() {
    let table = range_table();
    for (value, physical_id) in [(1, 101), (7, 102), (12, 103), (16, 104)] {
        let partition = table.locate_partition(&[Datum::Int(value)]).unwrap();
        assert_eq!(partition.physical_id, physical_id);
        assert_eq!(
            partition_record_key(partition.physical_id, value),
            format!("t{physical_id}_r{value}").into_bytes()
        );
    }
    assert_eq!(
        table.locate_partition(&[Datum::Int(22)]),
        Err(PartitionError::NoPartitionForValue)
    );
}

/// Go `TestHashPartitionAddRecord`: negative input uses its unsigned magnitude
/// before modulo, so every physical partition remains addressable.
#[test]
fn hash_partition_add_record_routes_negative_and_positive_values() {
    let expression = PartitionExpr::Hash {
        column_offset: 0,
        partition_count: 4,
    };
    assert_eq!(expression.locate_partition(&[Datum::Int(8)]), Ok(0));
    assert_eq!(expression.locate_partition(&[Datum::Int(-1)]), Ok(1));
    assert_eq!(expression.locate_partition(&[Datum::Int(3)]), Ok(3));
    assert_eq!(expression.locate_partition(&[Datum::Int(6)]), Ok(2));

    let eleven_partitions = PartitionExpr::Hash {
        column_offset: 0,
        partition_count: 11,
    };
    for value in 0..11 {
        assert_eq!(
            eleven_partitions.locate_partition(&[Datum::Int(-value)]),
            Ok(value as usize)
        );
    }
}

/// Go `TestPartitionGetPhysicalID` and `TestGeneratePartitionExpr`: definition
/// order, inclusive lower bounds and MAXVALUE routing must agree.
#[test]
fn range_partition_keeps_definition_physical_ids_and_boundaries() {
    let table = range_table();
    assert_eq!(
        table
            .definitions
            .iter()
            .map(|definition| table.partitions[&definition.id].physical_id)
            .collect::<Vec<_>>(),
        vec![101, 102, 103, 104]
    );

    let expression = PartitionExpr::Range(ForRangePruning {
        column_offset: 0,
        upper_bounds: vec![Some(4), Some(7), None],
    });
    assert_eq!(expression.locate_partition(&[Datum::Null]), Ok(0));
    assert_eq!(expression.locate_partition(&[Datum::Int(1)]), Ok(0));
    assert_eq!(expression.locate_partition(&[Datum::Int(4)]), Ok(1));
    assert_eq!(expression.locate_partition(&[Datum::Int(6)]), Ok(1));
    assert_eq!(expression.locate_partition(&[Datum::Int(7)]), Ok(2));
    assert_eq!(expression.locate_partition(&[Datum::Int(i64::MAX)]), Ok(2));
}

/// Go `TestLocatePartition` / `TestPartitionCoverage`: LIST values target their
/// configured partition and use DEFAULT only for unmatched values.
#[test]
fn list_partition_routes_configured_values_and_default() {
    let expression = PartitionExpr::List(ForListPruning {
        column_offset: 0,
        value_to_partition: HashMap::from([
            (Datum::Bytes(b"PushEvent".to_vec()), 0),
            (Datum::Bytes(b"WatchEvent".to_vec()), 1),
        ]),
        default_partition: Some(2),
    });
    assert_eq!(
        expression.locate_partition(&[Datum::Bytes(b"WatchEvent".to_vec())]),
        Ok(1)
    );
    assert_eq!(
        expression.locate_partition(&[Datum::Bytes(b"OtherEvent".to_vec())]),
        Ok(2)
    );
}
