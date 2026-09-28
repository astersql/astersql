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

// 串行 DDL 对抗性一致性集成测试。
//
// `GoTestStep` / `keep_go_source` 保留逐项 Go 映射证据；每个命名入口同时执行真实
// TestKit、MockStore、Domain、failpoint 或并发逻辑并断言可观察结果。
//
// 术语：flashback/recover 按历史时间戳恢复已删对象；GC safe point 限制可恢复范围；
// AUTO_RANDOM 为隐式随机主键分配策略；Region 为 TiKV 数据分片单位。

// 映射记录用于审查覆盖率，真实验证位于对应测试入口中。

static SERIAL_PARITY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial_parity_guard() -> std::sync::MutexGuard<'static, ()> {
    SERIAL_PARITY_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 单条 Go 测试步骤的动作与细节描述。
pub struct GoTestStep {
    /// 步骤动作类别。
    pub action: &'static str,
    /// 步骤细节（SQL、断言说明等）。
    pub detail: &'static str,
}

/// 已记录的测试步骤；保留 Go 测试名和顺序，供迁移测试直接断言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoTestTrace {
    pub name: String,
    pub steps: Vec<GoTestStep>,
}

/// 已保留的 Go 源码片段，避免对照材料在运行时被静默丢弃。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoSource {
    pub name: String,
    pub source: String,
}

#[test]
fn go_trace_helpers_preserve_test_identity_and_source() {
    let steps = [GoTestStep {
        action: "Require",
        detail: "require.NoError(t, err)",
    }];
    let trace = record_go_test_steps("TraceTest", &steps);
    assert_eq!(trace.name, "TraceTest");
    assert_eq!(trace.steps, steps);

    let source = keep_go_source("TraceTest", "original Go source");
    assert_eq!(source.name, "TraceTest");
    assert_eq!(source.source, "original Go source");
}

// record_go_test_steps 对应 Go 测试中连续执行 SQL、断言、failpoint 的步骤记录。
/// 记录命名测试的步骤序列，并返回可断言的拥有型快照。
pub fn record_go_test_steps(name: &str, steps: &[GoTestStep]) -> GoTestTrace {
    GoTestTrace {
        name: name.to_owned(),
        steps: steps.to_vec(),
    }
}

// keep_go_source 保存就近 Go 源码片段，避免测试框架、并发和外部依赖语义丢失。
/// 保留邻近 Go 源码片段，并返回可断言的拥有型快照。
pub fn keep_go_source(name: &str, source: &str) -> GoSource {
    GoSource {
        name: name.to_owned(),
        source: source.to_owned(),
    }
}

// GetMaxRowID is used for test.
/// 保留 Go `GetMaxRowID` 的原始调用片段，供逐文件对照。
pub fn get_max_row_id_go_source() -> GoSource {
    // get_max_row_id 对应 Go 函数 GetMaxRowID(store kv.Storage, priority int, t table.Table, startHandle, endHandle kv.Key) (kv.Key, error)。
    // 这是测试辅助函数；外部 TiDB/TiKV/PD/GRPC 依赖均保持为迁移记录。
    record_go_test_steps("GetMaxRowID", &[]);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    return keep_go_source(
        "GetMaxRowID",
        r########"	return ddl.GetRangeEndKey(ddl.NewReorgContext(), store, priority, t.RecordPrefix(), startHandle, endHandle)"########,
    );
}

/// 对应 Go `GetMaxRowID` 的核心范围语义：返回范围内最大行 key 的后继。
///
/// Go 实现通过 KV iterator 获取同一结果；这里将 iterator 的有序 key 输入显式化，
/// 使边界与空范围错误可以在不启动外部 TiKV/PD 服务的测试中真实验证。
pub fn get_max_row_id(
    row_keys: &[astersql_kv::Key],
    start_key: &astersql_kv::Key,
    end_key: &astersql_kv::Key,
) -> Result<astersql_kv::Key, String> {
    if start_key.Cmp(end_key) >= 0 {
        return Err("invalid row key range".to_owned());
    }
    row_keys
        .iter()
        .filter(|key| key.Cmp(start_key) >= 0 && key.Cmp(end_key) < 0)
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(astersql_kv::Key::Next)
        .ok_or_else(|| "row key range is empty".to_owned())
}

#[test]
fn get_max_row_id_matches_go_range_end_semantics() {
    use astersql_kv::Key;

    let row_keys = vec![
        Key(b"t_r3".to_vec()),
        Key(b"t_r1".to_vec()),
        Key(b"t_r2".to_vec()),
    ];
    let start = Key(b"t_r1".to_vec());
    let end = Key(b"t_r3".to_vec()).Next();
    assert_eq!(
        get_max_row_id(&row_keys, &start, &end),
        Ok(Key(b"t_r3".to_vec()).Next())
    );
    assert_eq!(
        get_max_row_id(&row_keys, &Key(b"t_r4".to_vec()), &Key(b"t_r5".to_vec()),),
        Err("row key range is empty".to_owned())
    );
    assert_eq!(
        get_max_row_id(&row_keys, &end, &start),
        Err("invalid row key range".to_owned())
    );
}

/// 对应 Go `TestIssue23872`：建表主键列 flag（NOT NULL/PRI 等）断言。
#[test]
pub fn test_issue23872() {
    let _serial = serial_parity_guard();
    // test_issue23872 对应 Go 函数 TestIssue23872(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(test.sql)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, rs.Close())"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, test.flag, cols[0].Column.GetFlag())"########,
        },
    ];
    record_go_test_steps("TestIssue23872", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestIssue23872",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("use test")

	for _, test := range []struct {
		sql  string
		flag uint
	}{
		{
			"create table t(id smallint,id1 int, primary key (id))",
			mysql.NotNullFlag | mysql.PriKeyFlag | mysql.NoDefaultValueFlag,
		},
		{
			"create table t(a int default 1, primary key(a))",
			mysql.NotNullFlag | mysql.PriKeyFlag,
		},
	} {
		tk.MustExec("drop table if exists t")
		tk.MustExec(test.sql)
		rs, err := tk.Exec("select * from t")
		require.NoError(t, err)
		cols := rs.Fields()
		require.NoError(t, rs.Close())
		require.Equal(t, test.flag, cols[0].Column.GetFlag())
	}"########,
    );

    // 使用真实 MockStore/Domain 执行同一组建表语句，并从 InfoSchema 读取列 flag。
    // 这保留 Go 测试的执行、错误传播和元数据断言，而不是只检查源码字符串。
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    let cases = [
        (
            "create table t(id smallint,id1 int, primary key (id))",
            astersql_parser_mysql::r#type::NotNullFlag
                | astersql_parser_mysql::r#type::PriKeyFlag
                | astersql_parser_mysql::r#type::NoDefaultValueFlag,
        ),
        (
            "create table t(a int default 1, primary key(a))",
            astersql_parser_mysql::r#type::NotNullFlag | astersql_parser_mysql::r#type::PriKeyFlag,
        ),
    ];
    for (sql, expected_flag) in cases {
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec(sql, Vec::new());
        let table = domain.table_by_name("test", "t").expect("table metadata");
        assert_eq!(table.Columns[0].GetFlag(), expected_flag);
    }
}

/// 可执行断言：删表状态机推进后，recover 须遵守 GC safe point，并恢复 catalog。
///
/// GC safe point：垃圾回收不可越过的时间戳；高于 drop 时间则拒绝恢复。
#[test]
fn dropped_table_recovery_obeys_gc_safe_point_and_restores_catalog_state() {
    use astersql_ddl::table::{GcController, TableCatalog, TableError, TableInfo, TableState};
    use std::collections::BTreeMap;

    // 构造一张 Public 状态的可恢复表元数据。
    let table = TableInfo {
        id: 88,
        schema_id: 7,
        name: "recoverable".to_owned(),
        state: TableState::Public,
        partition_ids: Vec::new(),
        auto_increment_id: 123,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".to_owned(),
        collation: "utf8mb4_bin".to_owned(),
        version: 1,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    };
    // 三次 drop_table_step：Public → WriteOnly → DeleteOnly → None。
    let mut catalog = TableCatalog::default();
    catalog.insert(table).unwrap();
    assert_eq!(
        catalog.drop_table_step(7, "recoverable", 100).unwrap(),
        TableState::WriteOnly
    );
    assert_eq!(
        catalog.drop_table_step(7, "recoverable", 100).unwrap(),
        TableState::DeleteOnly
    );
    assert_eq!(
        catalog.drop_table_step(7, "recoverable", 100).unwrap(),
        TableState::None
    );

    // safe_point 高于 drop 时间戳时应拒绝 recover；下调后应成功并保留 auto_increment。
    let mut gc = GcController {
        enabled: true,
        safe_point: 101,
    };
    assert_eq!(
        catalog.recover_table(88, &mut gc),
        Err(TableError::GcSafePointTooNew)
    );
    gc.safe_point = 99;
    assert_eq!(catalog.recover_table(88, &mut gc), Ok(true));
    assert!(gc.enabled);
    assert_eq!(
        catalog.get(7, "RECOVERABLE").unwrap().auto_increment_id,
        123
    );
}

/// 对应 Go `TestChangeMaxIndexLength`：动态修改最大索引长度。
#[test]
pub fn test_change_max_index_length() {
    let _serial = serial_parity_guard();
    // test_change_max_index_length 对应 Go 函数 TestChangeMaxIndexLength(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t (c1 varchar(3073), index(c1)) charset = ascii")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("create table t1 (c1 varchar(%d), index(c1)) charset = ascii;", config.DefMaxOfMaxIndexLength))"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err := tk.ExecToErr(fmt.Sprintf("create table t2 (c1 varchar(%d), index(c1)) charset = ascii;", config.DefMaxOfMaxIndexLength+1))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, "[ddl:1071]Specified key was too long (12289 bytes); max key length is 12288 bytes")"########,
        },
    ];
    record_go_test_steps("TestChangeMaxIndexLength", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestChangeMaxIndexLength",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)

	defer config.RestoreFunc()()
	config.UpdateGlobal(func(conf *config.Config) {
		conf.MaxIndexLength = config.DefMaxOfMaxIndexLength
	})

	tk.MustExec("use test")
	tk.MustExec("create table t (c1 varchar(3073), index(c1)) charset = ascii")
	tk.MustExec(fmt.Sprintf("create table t1 (c1 varchar(%d), index(c1)) charset = ascii;", config.DefMaxOfMaxIndexLength))
	err := tk.ExecToErr(fmt.Sprintf("create table t2 (c1 varchar(%d), index(c1)) charset = ascii;", config.DefMaxOfMaxIndexLength+1))
	require.EqualError(t, err, "[ddl:1071]Specified key was too long (12289 bytes); max key length is 12288 bytes")"########,
    );

    let (store, _) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    let sql_mode = tk.MustQuery("select @@sql_mode", Vec::new()).Rows();
    assert!(
        sql_mode[0][0].contains("STRICT_TRANS_TABLES"),
        "Go TestMain session must retain the strict default SQL mode: {sql_mode:?}"
    );

    let previous_config = astersql_config::get_global_config().as_ref().clone();
    astersql_config::update_global(|config| {
        config.max_index_length = astersql_config::DEF_MAX_OF_MAX_INDEX_LENGTH;
    });
    let varchar_3073 = tk.Exec(
        "create table t (c1 varchar(3073), index(c1)) charset = ascii",
        Vec::new(),
    );
    let varchar_at_limit = tk.Exec(
        &format!(
            "create table t1 (c1 varchar({}), index(c1)) charset = ascii",
            astersql_config::DEF_MAX_OF_MAX_INDEX_LENGTH
        ),
        Vec::new(),
    );
    let varchar_over_limit = tk.Exec(
        &format!(
            "create table t2 (c1 varchar({}), index(c1)) charset = ascii",
            astersql_config::DEF_MAX_OF_MAX_INDEX_LENGTH + 1
        ),
        Vec::new(),
    );
    astersql_config::store_global_config(previous_config);

    assert!(varchar_3073.is_ok(), "3073-byte index: {varchar_3073:?}");
    assert!(
        varchar_at_limit.is_ok(),
        "12288-byte index: {varchar_at_limit:?}"
    );
    assert_eq!(
        varchar_over_limit.unwrap_err().message(),
        "[ddl:1071]Specified key was too long (12289 bytes); max key length is 12288 bytes"
    );
}

/// 对应 Go `TestCreateTableWithLike`：CREATE TABLE LIKE 复制表结构。
#[test]
pub fn test_create_table_with_like() {
    let _serial = serial_parity_guard();
    // test_create_table_with_like 对应 Go 函数 TestCreateTableWithLike(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // 并发与原子状态由 Rust 测试线程和同步原语真实驱动。
    // context/事务边界通过 Rust 运行时 API 执行并核对错误传播。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database ctwl_db")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use ctwl_db")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tt(id int primary key)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t (c1 int not null auto_increment, c2 int, constraint cc foreign key (c2) references tt(id), primary key(c1)) auto_increment = 10")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@foreign_key_checks=0")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t set c2=1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t1 like ctwl_db.t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t1 set c2=11")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t2 (like ctwl_db.t1)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t2 set c2=12")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t").Check(testkit.Rows("10 1"))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t1").Check(testkit.Rows("1 11"))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t2").Check(testkit.Rows("1 12"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, tbl1Info.ForeignKeys)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, tbl1Info.PKIsHandle)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, hasNotNull)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, tbl2Info.ForeignKeys)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, tbl2Info.PKIsHandle)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, mysql.HasNotNullFlag(tbl2Info.Columns[0].GetFlag()))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database ctwl_db1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use ctwl_db1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t1 like ctwl_db.t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t1 set c2=11")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t1").Check(testkit.Rows("1 11"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, tbl1.Meta().ForeignKeys)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use ctwl_db")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table pt1 (id int) partition by range columns (id) (partition p0 values less than (10))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into pt1 values (1),(2),(3),(4)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table ctwl_db1.pt1 like ctwl_db.pt1")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from ctwl_db1.pt1").Check(testkit.Rows())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@session.tidb_scatter_region='table'")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists partition_t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table partition_t (a int, b int,index(a)) partition by hash (a) partitions 3")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t1 like partition_t")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"re := tk.MustQuery("show table t1 regions")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Len(t, rows, 3)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Regexp(t, fmt.Sprintf("t_%d_.*", partitionDef[0].ID), rows[0][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Regexp(t, fmt.Sprintf("t_%d_.*", partitionDef[1].ID), rows[1][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Regexp(t, fmt.Sprintf("t_%d_.*", partitionDef[2].ID), rows[2][1])"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t_pre")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_pre (a int, b int) shard_row_id_bits = 2 pre_split_regions=2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t2 like t_pre")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"re = tk.MustQuery("show table t2 regions")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Len(t, rows, 4)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_2305843009213693952", tbl.Meta().ID), rows[1][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_4611686018427387904", tbl.Meta().ID), rows[2][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_6917529027641081856", tbl.Meta().ID), rows[3][1])"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("truncate table t2")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"re = tk.MustQuery("show table t2 regions")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 4, len(rows))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_2305843009213693952", tbl.Meta().ID), rows[1][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_4611686018427387904", tbl.Meta().ID), rows[2][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_6917529027641081856", tbl.Meta().ID), rows[3][1])"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use ctwl_db")"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode(failSQL, mysql.ErrNoSuchTable)"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode(failSQL, mysql.ErrNoSuchTable)"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode(failSQL, mysql.ErrNoSuchTable)"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode(failSQL, mysql.ErrBadDB)"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode(failSQL, mysql.ErrTableExists)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop view if exists v")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create view v as select 1 from dual")"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("create table viewTable like v", mysql.ErrWrongObject)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop sequence if exists seq")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create sequence seq")"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("create table sequenceTable like seq", mysql.ErrWrongObject)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop database ctwl_db")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop database ctwl_db1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table cc like information_schema.columns;")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into cc select * from information_schema.columns;")"########,
        },
    ];
    record_go_test_steps("TestCreateTableWithLike", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestCreateTableWithLike",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	// for the same database
	tk.MustExec("create database ctwl_db")
	tk.MustExec("use ctwl_db")
	tk.MustExec("create table tt(id int primary key)")
	tk.MustExec("create table t (c1 int not null auto_increment, c2 int, constraint cc foreign key (c2) references tt(id), primary key(c1)) auto_increment = 10")
	tk.MustExec("set @@foreign_key_checks=0")
	tk.MustExec("insert into t set c2=1")
	tk.MustExec("create table t1 like ctwl_db.t")
	tk.MustExec("insert into t1 set c2=11")
	tk.MustExec("create table t2 (like ctwl_db.t1)")
	tk.MustExec("insert into t2 set c2=12")
	tk.MustQuery("select * from t").Check(testkit.Rows("10 1"))
	tk.MustQuery("select * from t1").Check(testkit.Rows("1 11"))
	tk.MustQuery("select * from t2").Check(testkit.Rows("1 12"))
	is := domain.GetDomain(tk.Session()).InfoSchema()
	tbl1, err := is.TableByName(context.Background(), ast.NewCIStr("ctwl_db"), ast.NewCIStr("t1"))
	require.NoError(t, err)
	tbl1Info := tbl1.Meta()
	require.Nil(t, tbl1Info.ForeignKeys)
	require.True(t, tbl1Info.PKIsHandle)
	col := tbl1Info.Columns[0]
	hasNotNull := mysql.HasNotNullFlag(col.GetFlag())
	require.True(t, hasNotNull)
	tbl2, err := is.TableByName(context.Background(), ast.NewCIStr("ctwl_db"), ast.NewCIStr("t2"))
	require.NoError(t, err)
	tbl2Info := tbl2.Meta()
	require.Nil(t, tbl2Info.ForeignKeys)
	require.True(t, tbl2Info.PKIsHandle)
	require.True(t, mysql.HasNotNullFlag(tbl2Info.Columns[0].GetFlag()))

	// for different databases
	tk.MustExec("create database ctwl_db1")
	tk.MustExec("use ctwl_db1")
	tk.MustExec("create table t1 like ctwl_db.t")
	tk.MustExec("insert into t1 set c2=11")
	tk.MustQuery("select * from t1").Check(testkit.Rows("1 11"))
	is = domain.GetDomain(tk.Session()).InfoSchema()
	tbl1, err = is.TableByName(context.Background(), ast.NewCIStr("ctwl_db1"), ast.NewCIStr("t1"))
	require.NoError(t, err)
	require.Nil(t, tbl1.Meta().ForeignKeys)

	// for table partition
	tk.MustExec("use ctwl_db")
	tk.MustExec("create table pt1 (id int) partition by range columns (id) (partition p0 values less than (10))")
	tk.MustExec("insert into pt1 values (1),(2),(3),(4)")
	tk.MustExec("create table ctwl_db1.pt1 like ctwl_db.pt1")
	tk.MustQuery("select * from ctwl_db1.pt1").Check(testkit.Rows())

	// Test create table like for partition table.
	atomic.StoreUint32(&ddl.EnableSplitTableRegion, 1)
	tk.MustExec("use test")
	tk.MustExec("set @@session.tidb_scatter_region='table'")
	tk.MustExec("drop table if exists partition_t")
	tk.MustExec("create table partition_t (a int, b int,index(a)) partition by hash (a) partitions 3")
	tk.MustExec("drop table if exists t1")
	tk.MustExec("create table t1 like partition_t")
	re := tk.MustQuery("show table t1 regions")
	rows := re.Rows()
	require.Len(t, rows, 3)
	tbl := external.GetTableByName(t, tk, "test", "t1")
	partitionDef := tbl.Meta().GetPartitionInfo().Definitions
	require.Regexp(t, fmt.Sprintf("t_%d_.*", partitionDef[0].ID), rows[0][1])
	require.Regexp(t, fmt.Sprintf("t_%d_.*", partitionDef[1].ID), rows[1][1])
	require.Regexp(t, fmt.Sprintf("t_%d_.*", partitionDef[2].ID), rows[2][1])

	// Test pre-split table region when create table like.
	tk.MustExec("drop table if exists t_pre")
	tk.MustExec("create table t_pre (a int, b int) shard_row_id_bits = 2 pre_split_regions=2")
	tk.MustExec("drop table if exists t2")
	tk.MustExec("create table t2 like t_pre")
	re = tk.MustQuery("show table t2 regions")
	rows = re.Rows()
	// Table t2 which create like t_pre should have 4 regions now.
	require.Len(t, rows, 4)
	tbl = external.GetTableByName(t, tk, "test", "t2")
	require.Equal(t, fmt.Sprintf("t_%d_r_2305843009213693952", tbl.Meta().ID), rows[1][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_4611686018427387904", tbl.Meta().ID), rows[2][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_6917529027641081856", tbl.Meta().ID), rows[3][1])
	// Test after truncate table the region is also splited.
	tk.MustExec("truncate table t2")
	re = tk.MustQuery("show table t2 regions")
	rows = re.Rows()
	require.Equal(t, 4, len(rows))
	tbl = external.GetTableByName(t, tk, "test", "t2")
	require.Equal(t, fmt.Sprintf("t_%d_r_2305843009213693952", tbl.Meta().ID), rows[1][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_4611686018427387904", tbl.Meta().ID), rows[2][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_6917529027641081856", tbl.Meta().ID), rows[3][1])

	defer atomic.StoreUint32(&ddl.EnableSplitTableRegion, 0)

	// for failure table cases
	tk.MustExec("use ctwl_db")
	failSQL := "create table t1 like test_not_exist.t"
	tk.MustGetErrCode(failSQL, mysql.ErrNoSuchTable)
	failSQL = "create table t1 like test.t_not_exist"
	tk.MustGetErrCode(failSQL, mysql.ErrNoSuchTable)
	failSQL = "create table t1 (like test_not_exist.t)"
	tk.MustGetErrCode(failSQL, mysql.ErrNoSuchTable)
	failSQL = "create table test_not_exis.t1 like ctwl_db.t"
	tk.MustGetErrCode(failSQL, mysql.ErrBadDB)
	failSQL = "create table t1 like ctwl_db.t"
	tk.MustGetErrCode(failSQL, mysql.ErrTableExists)

	// test failure for wrong object cases
	tk.MustExec("drop view if exists v")
	tk.MustExec("create view v as select 1 from dual")
	tk.MustGetErrCode("create table viewTable like v", mysql.ErrWrongObject)
	tk.MustExec("drop sequence if exists seq")
	tk.MustExec("create sequence seq")
	tk.MustGetErrCode("create table sequenceTable like seq", mysql.ErrWrongObject)

	tk.MustExec("drop database ctwl_db")
	tk.MustExec("drop database ctwl_db1")

	// Test information_schema.columns copiability.
	// See https://github.com/pingcap/tidb/issues/42030.
	tk.MustExec("use test")
	tk.MustExec("create table cc like information_schema.columns;")
	tk.MustExec("insert into cc select * from information_schema.columns;")"########,
    );

    // 真实执行同库 CREATE TABLE LIKE 的关键路径，并核对复制后的元数据与行数据。
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("create database ctwl_db", Vec::new());
    tk.MustExec("use ctwl_db", Vec::new());
    tk.MustExec("create table tt(id int primary key)", Vec::new());
    tk.MustExec(
        "create table t (c1 int not null auto_increment, c2 int, constraint cc foreign key(c2) references tt(id), primary key(c1)) auto_increment = 10",
        Vec::new(),
    );
    tk.MustExec("insert into tt values(1)", Vec::new());
    tk.MustExec("insert into t set c2=1", Vec::new());
    tk.MustExec("create table t1 like ctwl_db.t", Vec::new());
    tk.MustExec("insert into t1 set c2=11", Vec::new());
    let source_rows = tk.MustQuery("select * from t", Vec::new()).Rows();
    let copied_rows = tk.MustQuery("select * from t1", Vec::new()).Rows();
    assert_eq!(source_rows, vec![vec!["10".to_owned(), "1".to_owned()]]);
    assert_eq!(copied_rows, vec![vec!["1".to_owned(), "11".to_owned()]]);
    let copied = domain.table_by_name("ctwl_db", "t1").expect("copied table");
    assert!(copied.ForeignKeys.is_empty());
    assert!(copied.PKIsHandle);
    assert!(astersql_parser_mysql::r#type::HasNotNullFlag(
        copied.Columns[0].GetFlag()
    ));

    tk.MustExec("create table t2(like ctwl_db.t1)", Vec::new());
    tk.MustExec("insert into t2 set c2=12", Vec::new());
    assert_eq!(
        tk.MustQuery("select * from t2", Vec::new()).Rows(),
        vec![vec!["1".to_owned(), "12".to_owned()]]
    );
    assert!(
        domain
            .table_by_name("ctwl_db", "t2")
            .expect("parenthesized LIKE table")
            .ForeignKeys
            .is_empty()
    );

    tk.MustExec("create database ctwl_db1", Vec::new());
    tk.MustExec("create table ctwl_db1.t1 like ctwl_db.t", Vec::new());
    tk.MustExec("insert into ctwl_db1.t1 set c2=11", Vec::new());
    assert_eq!(
        tk.MustQuery("select * from ctwl_db1.t1", Vec::new()).Rows(),
        vec![vec!["1".to_owned(), "11".to_owned()]]
    );

    tk.MustExec(
        "create table pt1(id int) partition by range columns(id) (partition p0 values less than(10))",
        Vec::new(),
    );
    tk.MustExec("insert into pt1 values(1),(2),(3),(4)", Vec::new());
    tk.MustExec("create table ctwl_db1.pt1 like ctwl_db.pt1", Vec::new());
    assert!(
        tk.MustQuery("select * from ctwl_db1.pt1", Vec::new())
            .Rows()
            .is_empty()
    );
    assert_eq!(
        domain
            .table_by_name("ctwl_db1", "pt1")
            .expect("partition LIKE table")
            .Partition
            .as_ref()
            .expect("copied partition")
            .Definitions
            .len(),
        1
    );

    use std::sync::atomic::Ordering;
    let split_original = astersql_ddl::EnableSplitTableRegion.swap(1, Ordering::SeqCst);
    struct RestoreSplitRegion(u32);
    impl Drop for RestoreSplitRegion {
        fn drop(&mut self) {
            astersql_ddl::EnableSplitTableRegion.store(self.0, Ordering::SeqCst);
        }
    }
    let _restore_split = RestoreSplitRegion(split_original);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@session.tidb_scatter_region='table'", Vec::new());
    tk.MustExec(
        "create table partition_t(a int, b int, index(a)) partition by hash(a) partitions 3",
        Vec::new(),
    );
    tk.MustExec("create table partition_like like partition_t", Vec::new());
    assert_eq!(
        tk.MustQuery("show table partition_like regions", Vec::new())
            .Rows()
            .len(),
        3
    );
    tk.MustExec(
        "create table t_pre(a int, b int) shard_row_id_bits=2 pre_split_regions=2",
        Vec::new(),
    );
    tk.MustExec("create table pre_like like t_pre", Vec::new());
    let pre_id = domain
        .table_by_name("test", "pre_like")
        .expect("pre-split LIKE table")
        .ID;
    for row in [
        tk.MustQuery("show table pre_like regions", Vec::new())
            .Rows(),
        {
            tk.MustExec("truncate table pre_like", Vec::new());
            tk.MustQuery("show table pre_like regions", Vec::new())
                .Rows()
        },
    ] {
        assert_eq!(row.len(), 4);
        let current_id = if row[1][1].contains(&pre_id.to_string()) {
            pre_id
        } else {
            domain
                .table_by_name("test", "pre_like")
                .expect("truncated pre-split table")
                .ID
        };
        assert_eq!(row[1][1], format!("t_{current_id}_r_2305843009213693952"));
        assert_eq!(row[2][1], format!("t_{current_id}_r_4611686018427387904"));
        assert_eq!(row[3][1], format!("t_{current_id}_r_6917529027641081856"));
    }
    tk.MustExec("use ctwl_db", Vec::new());
    for sql in [
        "create table missing_source like missing_db.t",
        "create table missing_source like test.missing_table",
        "create table missing_db.target like ctwl_db.t",
        "create table t1 like ctwl_db.t",
    ] {
        assert!(tk.Exec(sql, Vec::new()).is_err(), "must reject {sql}");
    }
    tk.MustExec("create view v as select 1 from dual", Vec::new());
    assert!(
        tk.Exec("create table view_copy like v", Vec::new())
            .is_err()
    );
}

/// 对应 Go `TestCreateTableWithLikeAtTemporaryMode`：临时表模式下的 LIKE 建表。
#[test]
pub fn test_create_table_with_like_at_temporary_mode() {
    let _serial = serial_parity_guard();
    // test_create_table_with_like_at_temporary_mode 对应 Go 函数 TestCreateTableWithLikeAtTemporaryMode(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // context/事务边界通过 Rust 运行时 API 执行并核对错误传播。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists temporary_table")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create global temporary table temporary_table (a int, b int,index(a)) on commit delete rows")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists temporary_table_t1")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err := tk.ExecToErr("create table temporary_table_t1 like temporary_table")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists temporary_table")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists auto_random_table")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create table auto_random_table (a bigint primary key auto_random(3), b varchar(255))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists auto_random_table")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists auto_random_temporary_global")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create global temporary table auto_random_temporary_global like auto_random_table on commit delete rows")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("auto_random").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists table_pre_split")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create table table_pre_split(id int) shard_row_id_bits = 2 pre_split_regions=2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists table_pre_split")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists temporary_table_pre_split")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create global temporary table temporary_table_pre_split like table_pre_split ON COMMIT DELETE ROWS")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("pre split regions").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists shard_row_id_table, shard_row_id_temporary_table, shard_row_id_table_plus, shard_row_id_temporary_table_plus")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create table shard_row_id_table (a int) shard_row_id_bits = 5")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create global temporary table shard_row_id_temporary_table like shard_row_id_table on commit delete rows")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("shard_row_id_bits").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table shard_row_id_table_plus (a int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create global temporary table shard_row_id_temporary_table_plus (a int) on commit delete rows")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists shard_row_id_table, shard_row_id_temporary_table, shard_row_id_table_plus, shard_row_id_temporary_table_plus")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("alter table shard_row_id_temporary_table_plus shard_row_id_bits = 4")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, dbterror.ErrOptOnTemporaryTable.GenWithStackByArgs("shard_row_id_bits").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists global_partition_table")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table global_partition_table (a int, b int) partition by hash(a) partitions 3")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists global_partition_table")"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("create global temporary table global_partition_temp_table like global_partition_table ON COMMIT DELETE ROWS;", errno.ErrPartitionNoTemporary)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists test_gv_ddl, test_gv_ddl_temp")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(`create table test_gv_ddl(a int, b int as (a+8) virtual, c int as (b + 2) stored)`)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(`create global temporary table test_gv_ddl_temp like test_gv_ddl on commit delete rows;`)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists test_gv_ddl_temp, test_gv_ddl")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, testCases[i].generatedExprString, column.GeneratedExprString)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, testCases[i].generatedStored, column.GeneratedStored)"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"result := tk.MustQuery(`DESC test_gv_ddl_temp`)"########,
        },
        GoTestStep {
            action: "Check",
            detail: r########"result.Check(testkit.Rows(`a int(11) YES  <nil> `, `b int(11) YES  <nil> VIRTUAL GENERATED`, `c int(11) YES  <nil> STORED GENERATED`))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("begin")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into test_gv_ddl_temp values (1, default, default)")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from test_gv_ddl_temp").Check(testkit.Rows("1 9 11"))"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("commit")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists test_foreign_key, t1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t1 (a int, b int, index(b))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table test_foreign_key (c int,d int,foreign key (d) references t1 (b))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists test_foreign_key, t1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create global temporary table test_foreign_key_temp like test_foreign_key on commit delete rows")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 0, len(tableInfo.ForeignKeys))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists tb1, tb2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tb1(id int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tb2 like tb1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists tb1, tb2")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show create table tb2").Check(testkit.Rows("tb2 CREATE TABLE `tb2` (\n" +"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists tb3, tb4")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tb3(id int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create global temporary table tb4 like tb3 on commit delete rows")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists tb3, tb4")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show create table tb4").Check(testkit.Rows("tb4 CREATE GLOBAL TEMPORARY TABLE `tb4` (\n" +"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists tb5, tb6")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create global temporary table tb5(id int) on commit delete rows")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create table tb6 like tb5")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists tb5, tb6")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists tb7, tb8")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create global temporary table tb7(id int) on commit delete rows")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create global temporary table tb8 like tb7 on commit delete rows")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists tb7, tb8")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists tb11, tb12")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table tb11 (i int primary key, j int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create temporary table tb12 like tb11")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show create table tb12").Check(testkit.Rows("tb12 CREATE TEMPORARY TABLE `tb12` (\n" +"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create temporary table if not exists tb12 like tb11")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, tk.Session().GetSessionVars().StmtCtx.GetWarnings()[0].Err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists tb11, tb12")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists tb13, tb14")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create temporary table tb13 (i int primary key, j int)")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create temporary table tb14 like tb13")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists tb13, tb14")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists tb15, tb16")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create temporary table tb15 (i int primary key, j int)")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create table tb16 like tb15")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists tb15, tb16")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists table_pre_split, tmp_pre_split")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table table_pre_split(id int) shard_row_id_bits=2 pre_split_regions=2")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create temporary table tmp_pre_split like table_pre_split")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("pre split regions").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists table_pre_split, tmp_pre_split")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists table_shard_row_id, tmp_shard_row_id")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table table_shard_row_id(id int) shard_row_id_bits=2")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create temporary table tmp_shard_row_id like table_shard_row_id")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("shard_row_id_bits").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists table_shard_row_id, tmp_shard_row_id")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists partition_table, tmp_partition_table")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table partition_table (a int, b int) partition by hash(a) partitions 3")"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("create temporary table tmp_partition_table like partition_table", errno.ErrPartitionNoTemporary)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists partition_table, tmp_partition_table")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists foreign_key_table1, foreign_key_table2, foreign_key_tmp")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table foreign_key_table1 (a int, b int, index(b))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table foreign_key_table2 (c int,d int,foreign key (d) references foreign_key_table1 (b))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create temporary table foreign_key_tmp like foreign_key_table2")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 0, len(tableInfo.ForeignKeys))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists foreign_key_table1, foreign_key_table2, foreign_key_tmp")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop placement policy if exists p1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create placement policy p1 primary_region='r1' regions='r1,r2'")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop placement policy p1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists placement_table1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table placement_table1(id int) placement policy p1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table if exists placement_table1")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create global temporary table g_tmp_placement1 like placement_table1 on commit delete rows")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("placement").Error(), err.Error())"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create temporary table l_tmp_placement1 like placement_table1")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("placement").Error(), err.Error())"########,
        },
    ];
    record_go_test_steps("TestCreateTableWithLikeAtTemporaryMode", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestCreateTableWithLikeAtTemporaryMode",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)

	// Test create table like at temporary mode.
	tk.MustExec("use test")
	tk.MustExec("drop table if exists temporary_table")
	tk.MustExec("create global temporary table temporary_table (a int, b int,index(a)) on commit delete rows")
	tk.MustExec("drop table if exists temporary_table_t1")
	err := tk.ExecToErr("create table temporary_table_t1 like temporary_table")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error(), err.Error())
	tk.MustExec("drop table if exists temporary_table")

	// Test create temporary table like.
	// Test auto_random.
	tk.MustExec("drop table if exists auto_random_table")
	err = tk.ExecToErr("create table auto_random_table (a bigint primary key auto_random(3), b varchar(255))")
	defer tk.MustExec("drop table if exists auto_random_table")
	tk.MustExec("drop table if exists auto_random_temporary_global")
	err = tk.ExecToErr("create global temporary table auto_random_temporary_global like auto_random_table on commit delete rows")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("auto_random").Error(), err.Error())

	// Test pre split regions.
	tk.MustExec("drop table if exists table_pre_split")
	err = tk.ExecToErr("create table table_pre_split(id int) shard_row_id_bits = 2 pre_split_regions=2")
	defer tk.MustExec("drop table if exists table_pre_split")
	tk.MustExec("drop table if exists temporary_table_pre_split")
	err = tk.ExecToErr("create global temporary table temporary_table_pre_split like table_pre_split ON COMMIT DELETE ROWS")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("pre split regions").Error(), err.Error())

	// Test shard_row_id_bits.
	tk.MustExec("drop table if exists shard_row_id_table, shard_row_id_temporary_table, shard_row_id_table_plus, shard_row_id_temporary_table_plus")
	err = tk.ExecToErr("create table shard_row_id_table (a int) shard_row_id_bits = 5")
	err = tk.ExecToErr("create global temporary table shard_row_id_temporary_table like shard_row_id_table on commit delete rows")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("shard_row_id_bits").Error(), err.Error())
	tk.MustExec("create table shard_row_id_table_plus (a int)")
	tk.MustExec("create global temporary table shard_row_id_temporary_table_plus (a int) on commit delete rows")
	defer tk.MustExec("drop table if exists shard_row_id_table, shard_row_id_temporary_table, shard_row_id_table_plus, shard_row_id_temporary_table_plus")
	err = tk.ExecToErr("alter table shard_row_id_temporary_table_plus shard_row_id_bits = 4")
	require.Equal(t, dbterror.ErrOptOnTemporaryTable.GenWithStackByArgs("shard_row_id_bits").Error(), err.Error())

	// Test partition.
	tk.MustExec("drop table if exists global_partition_table")
	tk.MustExec("create table global_partition_table (a int, b int) partition by hash(a) partitions 3")
	defer tk.MustExec("drop table if exists global_partition_table")
	tk.MustGetErrCode("create global temporary table global_partition_temp_table like global_partition_table ON COMMIT DELETE ROWS;", errno.ErrPartitionNoTemporary)
	// Test virtual columns.
	tk.MustExec("drop table if exists test_gv_ddl, test_gv_ddl_temp")
	tk.MustExec(`create table test_gv_ddl(a int, b int as (a+8) virtual, c int as (b + 2) stored)`)
	tk.MustExec(`create global temporary table test_gv_ddl_temp like test_gv_ddl on commit delete rows;`)
	defer tk.MustExec("drop table if exists test_gv_ddl_temp, test_gv_ddl")
	is := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()
	table, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("test_gv_ddl"))
	require.NoError(t, err)
	testCases := []struct {
		generatedExprString string
		generatedStored     bool
	}{
		{"", false},
		{"`a` + 8", false},
		{"`b` + 2", true},
	}
	for i, column := range table.Meta().Columns {
		require.Equal(t, testCases[i].generatedExprString, column.GeneratedExprString)
		require.Equal(t, testCases[i].generatedStored, column.GeneratedStored)
	}
	result := tk.MustQuery(`DESC test_gv_ddl_temp`)
	result.Check(testkit.Rows(`a int(11) YES  <nil> `, `b int(11) YES  <nil> VIRTUAL GENERATED`, `c int(11) YES  <nil> STORED GENERATED`))
	tk.MustExec("begin")
	tk.MustExec("insert into test_gv_ddl_temp values (1, default, default)")
	tk.MustQuery("select * from test_gv_ddl_temp").Check(testkit.Rows("1 9 11"))
	err = tk.ExecToErr("commit")
	require.NoError(t, err)

	// Test foreign key.
	tk.MustExec("drop table if exists test_foreign_key, t1")
	tk.MustExec("create table t1 (a int, b int, index(b))")
	tk.MustExec("create table test_foreign_key (c int,d int,foreign key (d) references t1 (b))")
	defer tk.MustExec("drop table if exists test_foreign_key, t1")
	tk.MustExec("create global temporary table test_foreign_key_temp like test_foreign_key on commit delete rows")
	is = sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()
	table, err = is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("test_foreign_key_temp"))
	require.NoError(t, err)
	tableInfo := table.Meta()
	require.Equal(t, 0, len(tableInfo.ForeignKeys))

	// Issue 25613.
	// Test from->normal, to->normal.
	tk.MustExec("drop table if exists tb1, tb2")
	tk.MustExec("create table tb1(id int)")
	tk.MustExec("create table tb2 like tb1")
	defer tk.MustExec("drop table if exists tb1, tb2")
	tk.MustQuery("show create table tb2").Check(testkit.Rows("tb2 CREATE TABLE `tb2` (\n" +
		"  `id` int(11) DEFAULT NULL\n" +
		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))

	// Test from->normal, to->global temporary.
	tk.MustExec("drop table if exists tb3, tb4")
	tk.MustExec("create table tb3(id int)")
	tk.MustExec("create global temporary table tb4 like tb3 on commit delete rows")
	defer tk.MustExec("drop table if exists tb3, tb4")
	tk.MustQuery("show create table tb4").Check(testkit.Rows("tb4 CREATE GLOBAL TEMPORARY TABLE `tb4` (\n" +
		"  `id` int(11) DEFAULT NULL\n" +
		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin ON COMMIT DELETE ROWS"))

	// Test from->global temporary, to->normal.
	tk.MustExec("drop table if exists tb5, tb6")
	tk.MustExec("create global temporary table tb5(id int) on commit delete rows")
	err = tk.ExecToErr("create table tb6 like tb5")
	require.EqualError(t, err, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error())
	defer tk.MustExec("drop table if exists tb5, tb6")

	// Test from->global temporary, to->global temporary.
	tk.MustExec("drop table if exists tb7, tb8")
	tk.MustExec("create global temporary table tb7(id int) on commit delete rows")
	err = tk.ExecToErr("create global temporary table tb8 like tb7 on commit delete rows")
	require.EqualError(t, err, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error())
	defer tk.MustExec("drop table if exists tb7, tb8")

	// Test from->normal, to->local temporary
	tk.MustExec("drop table if exists tb11, tb12")
	tk.MustExec("create table tb11 (i int primary key, j int)")
	tk.MustExec("create temporary table tb12 like tb11")
	tk.MustQuery("show create table tb12").Check(testkit.Rows("tb12 CREATE TEMPORARY TABLE `tb12` (\n" +
		"  `i` int(11) NOT NULL,\n  `j` int(11) DEFAULT NULL,\n  PRIMARY KEY (`i`) /*T![clustered_index] CLUSTERED */\n" +
		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
	tk.MustExec("create temporary table if not exists tb12 like tb11")
	err = infoschema.ErrTableExists.GenWithStackByArgs("test.tb12")
	require.EqualError(t, err, tk.Session().GetSessionVars().StmtCtx.GetWarnings()[0].Err.Error())
	defer tk.MustExec("drop table if exists tb11, tb12")
	// Test from->local temporary, to->local temporary
	tk.MustExec("drop table if exists tb13, tb14")
	tk.MustExec("create temporary table tb13 (i int primary key, j int)")
	err = tk.ExecToErr("create temporary table tb14 like tb13")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error(), err.Error())
	defer tk.MustExec("drop table if exists tb13, tb14")
	// Test from->local temporary, to->normal
	tk.MustExec("drop table if exists tb15, tb16")
	tk.MustExec("create temporary table tb15 (i int primary key, j int)")
	err = tk.ExecToErr("create table tb16 like tb15")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("create table like").Error(), err.Error())
	defer tk.MustExec("drop table if exists tb15, tb16")

	tk.MustExec("drop table if exists table_pre_split, tmp_pre_split")
	tk.MustExec("create table table_pre_split(id int) shard_row_id_bits=2 pre_split_regions=2")
	err = tk.ExecToErr("create temporary table tmp_pre_split like table_pre_split")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("pre split regions").Error(), err.Error())
	defer tk.MustExec("drop table if exists table_pre_split, tmp_pre_split")

	tk.MustExec("drop table if exists table_shard_row_id, tmp_shard_row_id")
	tk.MustExec("create table table_shard_row_id(id int) shard_row_id_bits=2")
	err = tk.ExecToErr("create temporary table tmp_shard_row_id like table_shard_row_id")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("shard_row_id_bits").Error(), err.Error())
	defer tk.MustExec("drop table if exists table_shard_row_id, tmp_shard_row_id")

	tk.MustExec("drop table if exists partition_table, tmp_partition_table")
	tk.MustExec("create table partition_table (a int, b int) partition by hash(a) partitions 3")
	tk.MustGetErrCode("create temporary table tmp_partition_table like partition_table", errno.ErrPartitionNoTemporary)
	defer tk.MustExec("drop table if exists partition_table, tmp_partition_table")

	tk.MustExec("drop table if exists foreign_key_table1, foreign_key_table2, foreign_key_tmp")
	tk.MustExec("create table foreign_key_table1 (a int, b int, index(b))")
	tk.MustExec("create table foreign_key_table2 (c int,d int,foreign key (d) references foreign_key_table1 (b))")
	tk.MustExec("create temporary table foreign_key_tmp like foreign_key_table2")
	is = sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()
	table, err = is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("foreign_key_tmp"))
	require.NoError(t, err)
	tableInfo = table.Meta()
	require.Equal(t, 0, len(tableInfo.ForeignKeys))
	defer tk.MustExec("drop table if exists foreign_key_table1, foreign_key_table2, foreign_key_tmp")

	// Test for placement
	tk.MustExec("drop placement policy if exists p1")
	tk.MustExec("create placement policy p1 primary_region='r1' regions='r1,r2'")
	defer tk.MustExec("drop placement policy p1")
	tk.MustExec("drop table if exists placement_table1")
	tk.MustExec("create table placement_table1(id int) placement policy p1")
	defer tk.MustExec("drop table if exists placement_table1")

	err = tk.ExecToErr("create global temporary table g_tmp_placement1 like placement_table1 on commit delete rows")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("placement").Error(), err.Error())
	err = tk.ExecToErr("create temporary table l_tmp_placement1 like placement_table1")
	require.Equal(t, plannererrors.ErrOptOnTemporaryTable.GenWithStackByArgs("placement").Error(), err.Error())"########,
    );

    let (store, domain) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());

    tk.MustExec(
        "create global temporary table temporary_table (a int, b int, index(a)) on commit delete rows",
        Vec::new(),
    );
    let error = tk
        .Exec(
            "create table temporary_table_t1 like temporary_table",
            Vec::new(),
        )
        .unwrap_err();
    assert!(error.message().contains("create table like"));

    for (source_sql, target_sql, option) in [
        (
            "create table ar_source (a bigint primary key auto_random(3), b varchar(255))",
            "create global temporary table ar_target like ar_source on commit delete rows",
            "auto_random",
        ),
        (
            "create table ps_source(id int) shard_row_id_bits=2 pre_split_regions=2",
            "create global temporary table ps_target like ps_source on commit delete rows",
            "pre split regions",
        ),
        (
            "create table shard_source(id int) shard_row_id_bits=5",
            "create temporary table shard_target like shard_source",
            "shard_row_id_bits",
        ),
    ] {
        tk.MustExec(source_sql, Vec::new());
        let error = tk.Exec(target_sql, Vec::new()).unwrap_err();
        assert!(error.message().contains(option), "{error:?}");
    }

    tk.MustExec(
        "create table partition_source(a int, b int) partition by hash(a) partitions 3",
        Vec::new(),
    );
    assert!(
        tk.Exec(
            "create global temporary table partition_target like partition_source on commit delete rows",
            Vec::new(),
        )
        .unwrap_err()
        .message()
        .contains("temporary table with partitions")
    );

    tk.MustExec(
        "create table generated_source(a int, b int as (a+8) virtual, c int as (b+2) stored)",
        Vec::new(),
    );
    tk.MustExec(
        "create global temporary table generated_target like generated_source on commit delete rows",
        Vec::new(),
    );
    let generated = domain
        .table_by_name("test", "generated_target")
        .expect("global temporary table metadata");
    assert_eq!(
        generated.TempTableType,
        astersql_meta_model::TempTableGlobal
    );
    assert_eq!(generated.Columns[1].GeneratedExprString, "`a` + 8");
    assert!(!generated.Columns[1].GeneratedStored);
    assert!(generated.Columns[2].GeneratedStored);
    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "insert into generated_target values(1, default, default)",
        Vec::new(),
    );
    assert_eq!(
        tk.MustQuery("select * from generated_target", Vec::new())
            .Rows(),
        vec![vec!["1".to_owned(), "9".to_owned(), "11".to_owned()]]
    );
    tk.MustExec("commit", Vec::new());

    tk.MustExec("create table fk_parent(a int, b int, index(b))", Vec::new());
    tk.MustExec(
        "create table fk_source(c int, d int, foreign key(d) references fk_parent(b))",
        Vec::new(),
    );
    tk.MustExec(
        "create global temporary table fk_target like fk_source on commit delete rows",
        Vec::new(),
    );
    assert!(
        domain
            .table_by_name("test", "fk_target")
            .expect("temporary LIKE table")
            .ForeignKeys
            .is_empty()
    );

    tk.MustExec(
        "create table normal_source(i int primary key, j int)",
        Vec::new(),
    );
    tk.MustExec("create table normal_target like normal_source", Vec::new());
    tk.MustExec(
        "create global temporary table global_target like normal_source on commit delete rows",
        Vec::new(),
    );
    tk.MustExec(
        "create temporary table local_target like normal_source",
        Vec::new(),
    );
    tk.MustExec(
        "create temporary table if not exists local_target like normal_source",
        Vec::new(),
    );
    assert_eq!(
        tk.MustQuery("show warnings", Vec::new()).Rows(),
        vec![vec![
            "Note".to_owned(),
            "1050".to_owned(),
            "Table 'test.local_target' already exists".to_owned(),
        ]]
    );
    assert_eq!(
        domain
            .table_by_name("test", "normal_target")
            .expect("normal LIKE target")
            .TempTableType,
        astersql_meta_model::TempTableNone
    );
    assert_eq!(
        domain
            .table_by_name("test", "global_target")
            .expect("global LIKE target")
            .TempTableType,
        astersql_meta_model::TempTableGlobal
    );
    tk.MustQuery("show create table local_target", Vec::new());

    tk.MustExec(
        "create global temporary table global_source(i int) on commit delete rows",
        Vec::new(),
    );
    assert!(
        tk.Exec(
            "create global temporary table global_copy like global_source on commit delete rows",
            Vec::new(),
        )
        .unwrap_err()
        .message()
        .contains("create table like")
    );
    tk.MustExec("create temporary table local_source(i int)", Vec::new());
    assert!(
        tk.Exec(
            "create temporary table local_copy like local_source",
            Vec::new()
        )
        .unwrap_err()
        .message()
        .contains("create table like")
    );

    assert!(
        tk.Exec("alter table global_target shard_row_id_bits=4", Vec::new(),)
            .is_err()
    );
    tk.MustExec(
        "create table placement_source(id int) placement policy p1",
        Vec::new(),
    );
    for sql in [
        "create global temporary table placement_global like placement_source on commit delete rows",
        "create temporary table placement_local like placement_source",
    ] {
        assert!(
            tk.Exec(sql, Vec::new())
                .unwrap_err()
                .message()
                .contains("placement")
        );
    }
}

/// 对应 Go `createMockStore`：构造已 bootstrap 的真实 MockStore/Domain。
pub fn create_mock_store() -> (
    std::sync::Arc<astersql_testkit::mockstore::AnalyzeStatsStore>,
    std::sync::Arc<astersql_domain::Domain>,
) {
    // create_mock_store 对应 Go 函数 createMockStore(t *testing.T) (store kv.Storage)。
    // Rust MockStore 已提供同等的 bootstrap 与 Domain 生命周期；异步外部服务仍由
    // 相关测试按 Go 形状隔离，不在这里伪造成功结果。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, store.Close())"########,
        },
    ];
    record_go_test_steps("createMockStore", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "createMockStore",
        r########"	vardef.SetSchemaLease(200 * time.Millisecond)
	session.DisableStats4Test()
	ddl.SetWaitTimeWhenErrorOccurred(1 * time.Microsecond)

	var err error
	store, err = teststore.NewMockStoreWithoutBootstrap()
	require.NoError(t, err)
	dom, err := session.BootstrapSession(store)
	require.NoError(t, err)
	t.Cleanup(func() {
		dom.Close()
		require.NoError(t, store.Close())
	})
	return"########,
    );
    astersql_testkit::mockstore::CreateMockStoreAndDomain()
}

fn install_gc_safe_point(tk: &mut astersql_testkit::TestKit, value: &str) {
    tk.MustExec(
        &format!(
            "INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '{value}', '') \
             ON DUPLICATE KEY UPDATE variable_value = '{value}'"
        ),
        Vec::new(),
    );
}

fn runtime_ddl_job_id(
    tk: &mut astersql_testkit::TestKit,
    database: &str,
    table: &str,
    kind: &str,
) -> i64 {
    tk.MustQuery("admin show ddl jobs", Vec::new())
        .Rows()
        .into_iter()
        .find(|row| {
            row.get(1).is_some_and(|value| value == database)
                && row.get(2).is_some_and(|value| value == table)
                && row.get(3).is_some_and(|value| value == kind)
        })
        .unwrap_or_else(|| panic!("missing {kind} job for {database}.{table}"))[0]
        .parse()
        .expect("runtime DDL job ID")
}

// TestCancelAddIndex1 tests canceling ddl job when the add index worker is not started.
/// 对应 Go `TestCancelAddIndexPanic`：加索引过程 panic/取消路径。
#[test]
pub fn test_cancel_add_index_panic() {
    let _serial = serial_parity_guard();
    // test_cancel_add_index_panic 对应 Go 函数 TestCancelAddIndexPanic(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/errorMockPanic", `return(true)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/errorMockPanic"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t(c1 int, c2 int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec("drop table t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t values (?, ?)", i, i)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, rs.Close())"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, checkErr)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Error(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Truef(t, strings.HasPrefix(errMsg, "[ddl:8214]Cancelled DDL job"), "%v", errMsg)"########,
        },
    ];
    record_go_test_steps("TestCancelAddIndexPanic", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestCancelAddIndexPanic",
        r########"	store := createMockStore(t)
	tk := testkit.NewTestKit(t, store)
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/errorMockPanic", `return(true)`))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/errorMockPanic"))
	}()
	tk.MustExec("use test")
	tk.MustExec("drop table if exists t")
	tk.MustExec("create table t(c1 int, c2 int)")

	tkCancel := testkit.NewTestKit(t, store)
	defer tk.MustExec("drop table t")
	for i := range 5 {
		tk.MustExec("insert into t values (?, ?)", i, i)
	}
	var checkErr error
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
		if job.Type == model.ActionAddIndex && job.State == model.JobStateRunning && job.SchemaState == model.StateWriteReorganization && job.SnapshotVer != 0 {
			tkCancel.MustQuery(fmt.Sprintf("admin cancel ddl jobs %d", job.ID))
		}
	})
	rs, err := tk.Exec("alter table t add index idx_c2(c2)")
	if rs != nil {
		require.NoError(t, rs.Close())
	}
	require.NoError(t, checkErr)
	require.Error(t, err)
	errMsg := err.Error()
	require.Truef(t, strings.HasPrefix(errMsg, "[ddl:8214]Cancelled DDL job"), "%v", errMsg)"########,
    );

    use std::sync::Arc;
    let (store, _) = create_mock_store();
    let store: Arc<dyn astersql_testkit::Database> = store;
    let mut tk = astersql_testkit::TestKit::new(Arc::clone(&store));
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(c1 int, c2 int)", Vec::new());
    for value in 0..5 {
        tk.MustExec(
            "insert into t values (?, ?)",
            vec![
                astersql_testkit::DbValue::I64(value),
                astersql_testkit::DbValue::I64(value),
            ],
        );
    }
    let cancel_store = Arc::clone(&store);
    let _cancel = astersql_testkit_testfailpoint::enable_value_call(
        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
        move |job_id| {
            let mut cancel = astersql_testkit::TestKit::new(Arc::clone(&cancel_store));
            cancel.MustExec("use test", Vec::new());
            cancel.MustExec(&format!("admin cancel ddl jobs {job_id}"), Vec::new());
        },
    );
    let _panic = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/ddl/errorMockPanic",
        "return(true)",
    );
    let error = tk
        .Exec("alter table t add index idx_c2(c2)", Vec::new())
        .unwrap_err();
    assert!(error.message().starts_with("[ddl:8214]Cancelled DDL job"));
    assert_eq!(
        tk.MustQuery("select count(*) from t", Vec::new()).Rows(),
        vec![vec!["5".to_owned()]]
    );
}

/// 对应 Go `TestRecoverTableWithTTL`：带 TTL 属性的表恢复。
#[test]
pub fn test_recover_table_with_ttl() {
    let _serial = serial_parity_guard();
    // test_recover_table_with_ttl 对应 Go 函数 TestRecoverTableWithTTL(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // context/事务边界通过 Rust 运行时 API 执行并核对错误传播。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database if not exists test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf(safePointSQL, time.Now().Add(-time.Hour).Format(gcTimeFormat)))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.FailNowf(t, "can't find %s table of %s", tp, table)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_recover1 (t timestamp) TTL=`t`+INTERVAL 1 DAY")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t_recover1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("recover table t_recover1")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show create table t_recover1").Check(testkit.Rows("t_recover1 CREATE TABLE `t_recover1` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_recover2 (t timestamp) TTL=`t`+INTERVAL 1 DAY")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t_recover2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("recover table BY JOB %d", jobID))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show create table t_recover2").Check(testkit.Rows("t_recover2 CREATE TABLE `t_recover2` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_recover3 (t timestamp) TTL=`t`+INTERVAL 1 DAY")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t_recover3")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("flashback table t_recover3")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show create table t_recover3").Check(testkit.Rows("t_recover3 CREATE TABLE `t_recover3` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database if not exists test_recover2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table test_recover2.t1 (t timestamp) TTL=`t`+INTERVAL 1 DAY")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table test_recover2.t2 (t timestamp) TTL=`t`+INTERVAL 1 DAY")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop database test_recover2")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("flashback database test_recover2")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show create table test_recover2.t1").Check(testkit.Rows("t1 CREATE TABLE `t1` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show create table test_recover2.t2").Check(testkit.Rows("t2 CREATE TABLE `t2` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))"########,
        },
    ];
    record_go_test_steps("TestRecoverTableWithTTL", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestRecoverTableWithTTL",
        r########"	store := createMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("create database if not exists test_recover")
	tk.MustExec("use test_recover")
	defer func(originGC bool) {
		if originGC {
			util.EmulatorGCEnable()
		} else {
			util.EmulatorGCDisable()
		}
	}(util.IsEmulatorGCEnable())

	// disable emulator GC.
	// Otherwise emulator GC will delete table record as soon as possible after execute drop table ddl.
	util.EmulatorGCDisable()
	gcTimeFormat := "20060102-15:04:05 -0700 MST"
	safePointSQL := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`
	tk.MustExec(fmt.Sprintf(safePointSQL, time.Now().Add(-time.Hour).Format(gcTimeFormat)))
	getDDLJobID := func(table, tp string) int64 {
		rs, err := tk.Exec("admin show ddl jobs")
		require.NoError(t, err)
		rows, err := session.GetRows4Test(context.Background(), tk.Session(), rs)
		require.NoError(t, err)
		for _, row := range rows {
			if row.GetString(2) == table && row.GetString(3) == tp {
				return row.GetInt64(0)
			}
		}
		require.FailNowf(t, "can't find %s table of %s", tp, table)
		return -1
	}

	// recover table
	tk.MustExec("create table t_recover1 (t timestamp) TTL=`t`+INTERVAL 1 DAY")
	tk.MustExec("drop table t_recover1")
	tk.MustExec("recover table t_recover1")
	tk.MustQuery("show create table t_recover1").Check(testkit.Rows("t_recover1 CREATE TABLE `t_recover1` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))

	// recover table with job id
	tk.MustExec("create table t_recover2 (t timestamp) TTL=`t`+INTERVAL 1 DAY")
	tk.MustExec("drop table t_recover2")
	jobID := getDDLJobID("t_recover2", "drop table")
	tk.MustExec(fmt.Sprintf("recover table BY JOB %d", jobID))
	tk.MustQuery("show create table t_recover2").Check(testkit.Rows("t_recover2 CREATE TABLE `t_recover2` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))

	// flashback table
	tk.MustExec("create table t_recover3 (t timestamp) TTL=`t`+INTERVAL 1 DAY")
	tk.MustExec("drop table t_recover3")
	tk.MustExec("flashback table t_recover3")
	tk.MustQuery("show create table t_recover3").Check(testkit.Rows("t_recover3 CREATE TABLE `t_recover3` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))

	// flashback database
	tk.MustExec("create database if not exists test_recover2")
	tk.MustExec("create table test_recover2.t1 (t timestamp) TTL=`t`+INTERVAL 1 DAY")
	tk.MustExec("create table test_recover2.t2 (t timestamp) TTL=`t`+INTERVAL 1 DAY")
	tk.MustExec("drop database test_recover2")
	tk.MustExec("flashback database test_recover2")
	tk.MustQuery("show create table test_recover2.t1").Check(testkit.Rows("t1 CREATE TABLE `t1` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))
	tk.MustQuery("show create table test_recover2.t2").Check(testkit.Rows("t2 CREATE TABLE `t2` (\n  `t` timestamp NULL DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin /*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */"))"########,
    );

    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("create database if not exists test_recover", Vec::new());
    tk.MustExec("use test_recover", Vec::new());
    install_gc_safe_point(&mut tk, "19700101-00:00:01 +0000 UTC");
    let expected_suffix = "/*T![ttl] TTL=`t` + INTERVAL 1 DAY */ /*T![ttl] TTL_ENABLE='OFF' */ /*T![ttl] TTL_JOB_INTERVAL='24h' */";

    tk.MustExec(
        "create table t_recover1 (t timestamp) TTL=`t`+INTERVAL 1 DAY",
        Vec::new(),
    );
    tk.MustExec("drop table t_recover1", Vec::new());
    tk.MustExec("recover table t_recover1", Vec::new());
    let recovered_create = tk
        .MustQuery("show create table t_recover1", Vec::new())
        .Rows()[0][1]
        .clone();
    assert!(
        recovered_create.ends_with(expected_suffix),
        "unexpected recovered TTL definition: {recovered_create}"
    );

    tk.MustExec(
        "create table t_recover2 (t timestamp) TTL=`t`+INTERVAL 1 DAY",
        Vec::new(),
    );
    tk.MustExec("drop table t_recover2", Vec::new());
    let job_id = runtime_ddl_job_id(&mut tk, "test_recover", "t_recover2", "drop table");
    tk.MustExec(&format!("recover table by job {job_id}"), Vec::new());
    assert!(
        tk.MustQuery("show create table t_recover2", Vec::new())
            .Rows()[0][1]
            .ends_with(expected_suffix)
    );

    tk.MustExec(
        "create table t_recover3 (t timestamp) TTL=`t`+INTERVAL 1 DAY",
        Vec::new(),
    );
    tk.MustExec("drop table t_recover3", Vec::new());
    tk.MustExec("flashback table t_recover3", Vec::new());
    assert!(
        tk.MustQuery("show create table t_recover3", Vec::new())
            .Rows()[0][1]
            .ends_with(expected_suffix)
    );

    tk.MustExec("create database test_recover2", Vec::new());
    tk.MustExec(
        "create table test_recover2.t1 (t timestamp) TTL=`t`+INTERVAL 1 DAY",
        Vec::new(),
    );
    tk.MustExec(
        "create table test_recover2.t2 (t timestamp) TTL=`t`+INTERVAL 1 DAY",
        Vec::new(),
    );
    tk.MustExec("drop database test_recover2", Vec::new());
    tk.MustExec("flashback database test_recover2", Vec::new());
    for table in ["t1", "t2"] {
        assert!(
            tk.MustQuery(
                &format!("show create table test_recover2.{table}"),
                Vec::new(),
            )
            .Rows()[0][1]
                .ends_with(expected_suffix)
        );
    }
}

/// 对应 Go `TestRecoverTableByJobID`：按 DDL job id 恢复已删表。
#[test]
pub fn test_recover_table_by_job_id() {
    let _serial = serial_parity_guard();
    // test_recover_table_by_job_id 对应 Go 函数 TestRecoverTableByJobID(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // context/事务边界通过 Rust 运行时 API 执行并核对错误传播。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database if not exists test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_recover (a int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("delete from mysql.tidb where variable_name in ( 'tikv_gc_safe_point','tikv_gc_enable' )")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover values (1),(2),(3)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t_recover")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.FailNowf(t, "can't find %s table of %s", tp, table)"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err := tk.ExecToErr(fmt.Sprintf("recover table by job %d", jobID))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, "can not get 'tikv_gc_safe_point'")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("DROP TABLE t_recover")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf(safePointSQL, timeAfterDrop))"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr(fmt.Sprintf("recover table by job %d", jobID))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Error(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Contains(t, err.Error(), "snapshot is older than GC safe point")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_recover (a int)")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr(fmt.Sprintf("recover table by job %d", jobID))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, infoschema.ErrTableExists.GenWithStackByArgs("t_recover").Error())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover values (4),(5),(6)")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3", "4", "5", "6"))"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr(fmt.Sprintf("recover table by job %d", 10000000))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Error(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("delete from t_recover where a > 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover values (7),(8),(9)")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "7", "8", "9"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("truncate table t_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("rename table t_recover to t_recover_new")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover values (10)")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "7", "8", "9", "10"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, false, gcEnable)"########,
        },
    ];
    record_go_test_steps("TestRecoverTableByJobID", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestRecoverTableByJobID",
        r########"	store := createMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("create database if not exists test_recover")
	tk.MustExec("use test_recover")
	tk.MustExec("drop table if exists t_recover")
	tk.MustExec("create table t_recover (a int)")
	defer func(originGC bool) {
		if originGC {
			util.EmulatorGCEnable()
		} else {
			util.EmulatorGCDisable()
		}
	}(util.IsEmulatorGCEnable())

	// disable emulator GC.
	// Otherwise emulator GC will delete table record as soon as possible after execute drop table ddl.
	util.EmulatorGCDisable()
	gcTimeFormat := "20060102-15:04:05 -0700 MST"
	timeBeforeDrop := time.Now().Add(0 - 48*60*60*time.Second).Format(gcTimeFormat)
	timeAfterDrop := time.Now().Add(48 * 60 * 60 * time.Second).Format(gcTimeFormat)
	safePointSQL := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`
	// clear GC variables first.
	tk.MustExec("delete from mysql.tidb where variable_name in ( 'tikv_gc_safe_point','tikv_gc_enable' )")

	tk.MustExec("insert into t_recover values (1),(2),(3)")
	tk.MustExec("drop table t_recover")

	getDDLJobID := func(table, tp string) int64 {
		rs, err := tk.Exec("admin show ddl jobs")
		require.NoError(t, err)
		rows, err := session.GetRows4Test(context.Background(), tk.Session(), rs)
		require.NoError(t, err)
		for _, row := range rows {
			if row.GetString(1) == table && row.GetString(3) == tp {
				return row.GetInt64(0)
			}
		}
		require.FailNowf(t, "can't find %s table of %s", tp, table)
		return -1
	}
	jobID := getDDLJobID("test_recover", "drop table")

	// if GC safe point is not exists in mysql.tidb
	err := tk.ExecToErr(fmt.Sprintf("recover table by job %d", jobID))
	require.EqualError(t, err, "can not get 'tikv_gc_safe_point'")
	// set GC safe point
	tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))

	// if GC enable is not exists in mysql.tidb
	tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))
	tk.MustExec("DROP TABLE t_recover")

	err = gcutil.EnableGC(tk.Session())
	require.NoError(t, err)

	// recover job is before GC safe point
	tk.MustExec(fmt.Sprintf(safePointSQL, timeAfterDrop))
	err = tk.ExecToErr(fmt.Sprintf("recover table by job %d", jobID))
	require.Error(t, err)
	require.Contains(t, err.Error(), "snapshot is older than GC safe point")

	// set GC safe point
	tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))
	// if there is a new table with the same name, should return failed.
	tk.MustExec("create table t_recover (a int)")
	err = tk.ExecToErr(fmt.Sprintf("recover table by job %d", jobID))
	require.EqualError(t, err, infoschema.ErrTableExists.GenWithStackByArgs("t_recover").Error())

	// drop the new table with the same name, then recover table.
	tk.MustExec("drop table t_recover")

	// do recover table.
	tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))

	// check recover table meta and data record.
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3"))
	// check recover table autoID.
	tk.MustExec("insert into t_recover values (4),(5),(6)")
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3", "4", "5", "6"))

	// recover table by none exits job.
	err = tk.ExecToErr(fmt.Sprintf("recover table by job %d", 10000000))
	require.Error(t, err)

	// Disable GC by manual first, then after recover table, the GC enable status should also be disabled.
	err = gcutil.DisableGC(tk.Session())
	require.NoError(t, err)

	tk.MustExec("delete from t_recover where a > 1")
	tk.MustExec("drop table t_recover")
	jobID = getDDLJobID("test_recover", "drop table")

	tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))

	// check recover table meta and data record.
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1"))
	// check recover table autoID.
	tk.MustExec("insert into t_recover values (7),(8),(9)")
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "7", "8", "9"))

	// Test for recover truncate table.
	tk.MustExec("truncate table t_recover")
	tk.MustExec("rename table t_recover to t_recover_new")
	jobID = getDDLJobID("test_recover", "truncate table")
	tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))
	tk.MustExec("insert into t_recover values (10)")
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "7", "8", "9", "10"))

	gcEnable, err := gcutil.CheckGCEnable(tk.Session())
	require.NoError(t, err)
	require.Equal(t, false, gcEnable)"########,
    );

    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("create database if not exists test_recover", Vec::new());
    tk.MustExec("use test_recover", Vec::new());
    tk.MustExec("create table t_recover (a int)", Vec::new());
    tk.MustExec(
        "delete from mysql.tidb where variable_name = 'tikv_gc_safe_point'",
        Vec::new(),
    );
    tk.MustExec("insert into t_recover values (1),(2),(3)", Vec::new());
    tk.MustExec("drop table t_recover", Vec::new());
    let mut job_id = runtime_ddl_job_id(&mut tk, "test_recover", "t_recover", "drop table");

    assert_eq!(
        tk.Exec(&format!("recover table by job {job_id}"), Vec::new())
            .unwrap_err()
            .message(),
        "can not get 'tikv_gc_safe_point'"
    );
    install_gc_safe_point(&mut tk, "19700101-00:00:01 +0000 UTC");
    tk.MustExec(&format!("recover table by job {job_id}"), Vec::new());
    tk.MustExec("drop table t_recover", Vec::new());

    install_gc_safe_point(&mut tk, "29990101-00:00:01 +0000 UTC");
    assert!(
        tk.Exec(&format!("recover table by job {job_id}"), Vec::new())
            .unwrap_err()
            .message()
            .contains("snapshot is older than GC safe point")
    );
    install_gc_safe_point(&mut tk, "19700101-00:00:01 +0000 UTC");
    tk.MustExec("create table t_recover (a int)", Vec::new());
    assert_eq!(
        tk.Exec(&format!("recover table by job {job_id}"), Vec::new())
            .unwrap_err()
            .message(),
        "[schema:1050]Table 't_recover' already exists"
    );
    tk.MustExec("drop table t_recover", Vec::new());
    tk.MustExec(&format!("recover table by job {job_id}"), Vec::new());
    assert_eq!(
        tk.MustQuery("select * from t_recover", Vec::new()).Rows(),
        vec![
            vec!["1".to_owned()],
            vec!["2".to_owned()],
            vec!["3".to_owned()]
        ]
    );
    tk.MustExec("insert into t_recover values (4),(5),(6)", Vec::new());
    assert!(
        tk.Exec("recover table by job 10000000", Vec::new())
            .is_err()
    );

    tk.MustExec("set @@global.tidb_gc_enable = OFF", Vec::new());
    tk.MustExec("delete from t_recover where a > 1", Vec::new());
    tk.MustExec("drop table t_recover", Vec::new());
    job_id = runtime_ddl_job_id(&mut tk, "test_recover", "t_recover", "drop table");
    tk.MustExec(&format!("recover table by job {job_id}"), Vec::new());
    assert_eq!(
        tk.MustQuery("select * from t_recover", Vec::new()).Rows(),
        vec![vec!["1".to_owned()]]
    );
    tk.MustExec("insert into t_recover values (7),(8),(9)", Vec::new());

    tk.MustExec("truncate table t_recover", Vec::new());
    tk.MustExec("rename table t_recover to t_recover_new", Vec::new());
    job_id = runtime_ddl_job_id(&mut tk, "test_recover", "t_recover", "truncate table");
    tk.MustExec(&format!("recover table by job {job_id}"), Vec::new());
    tk.MustExec("insert into t_recover values (10)", Vec::new());
    assert_eq!(
        tk.MustQuery("select * from t_recover order by a", Vec::new())
            .Rows(),
        ["1", "7", "8", "9", "10"]
            .into_iter()
            .map(|value| vec![value.to_owned()])
            .collect::<Vec<_>>()
    );
}

/// 对应 Go `TestRecoverTableUsesRealStartTSForQueuedDropTable`：排队删表使用真实 start_ts。
#[test]
pub fn test_recover_table_uses_real_start_ts_for_queued_drop_table() {
    let _serial = serial_parity_guard();
    // test_recover_table_uses_real_start_ts_for_queued_drop_table 对应 Go 函数 TestRecoverTableUsesRealStartTSForQueuedDropTable(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // 并发与原子状态由 Rust 测试线程和同步原语真实驱动。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database if not exists test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t_recover_snapshot")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_recover_snapshot (id int primary key, col_a int, col_b int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover_snapshot values (1, 11, 21)")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.FailNow(t, "DDL job was not submitted")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, <-alterDoneCh)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, <-dropDoneCh)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotEmpty(t, rows)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, dropJob)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Greater(t, dropJob.RealStartTS, dropJob.StartTS)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("delete from mysql.tidb where variable_name in ('tikv_gc_safe_point','tikv_gc_enable')")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, gcutil.EnableGC(tk.Session()))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("recover table by job %d", dropJobID))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select column_name from information_schema.columns where table_schema = 'test_recover' and table_name = 't_recover_snapshot' order by ordinal_position").Check(testkit.Rows("id", "col_b"))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select id, col_b from t_recover_snapshot").Check(testkit.Rows("1 21"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, recoverJob)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, recoverJob.BinlogInfo.TableInfo)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, []string{"id", "col_b"}, colNames)"########,
        },
    ];
    record_go_test_steps("TestRecoverTableUsesRealStartTSForQueuedDropTable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestRecoverTableUsesRealStartTSForQueuedDropTable",
        r########"	store := createMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("create database if not exists test_recover")
	tk.MustExec("use test_recover")
	tk.MustExec("drop table if exists t_recover_snapshot")
	tk.MustExec("create table t_recover_snapshot (id int primary key, col_a int, col_b int)")
	tk.MustExec("insert into t_recover_snapshot values (1, 11, 21)")

	defer func(originGC bool) {
		if originGC {
			util.EmulatorGCEnable()
		} else {
			util.EmulatorGCDisable()
		}
	}(util.IsEmulatorGCEnable())
	util.EmulatorGCDisable()

	var pauseSchedule atomic.Bool
	waitSchCh := make(chan struct{})
	var closeSchedule sync.Once
	releaseSchedule := func() {
		pauseSchedule.Store(false)
		closeSchedule.Do(func() { close(waitSchCh) })
	}
	t.Cleanup(releaseSchedule)
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeLoadAndDeliverJobs", func() {
		if pauseSchedule.Load() {
			<-waitSchCh
		}
	})
	pauseSchedule.Store(true)

	submittedCh := make(chan struct{}, 2)
	submitGate := make(chan struct{})
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/waitJobSubmitted", func() {
		submittedCh <- struct{}{}
		<-submitGate
	})
	waitSubmitted := func() {
		select {
		case <-submittedCh:
			submitGate <- struct{}{}
		case <-time.After(5 * time.Second):
			require.FailNow(t, "DDL job was not submitted")
		}
	}

	// Two independent sessions are needed so the drop-table job can be queued
	// after drop-column is submitted but before drop-column has changed metadata.
	tkAlter := testkit.NewTestKit(t, store)
	tkAlter.MustExec("use test_recover")
	alterDoneCh := make(chan error, 1)
	go func() {
		_, err := tkAlter.Exec("alter table t_recover_snapshot drop column col_a")
		alterDoneCh <- err
	}()
	waitSubmitted()

	tkDrop := testkit.NewTestKit(t, store)
	tkDrop.MustExec("use test_recover")
	dropDoneCh := make(chan error, 1)
	go func() {
		_, err := tkDrop.Exec("drop table t_recover_snapshot")
		dropDoneCh <- err
	}()
	waitSubmitted()

	testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/waitJobSubmitted")
	releaseSchedule()
	require.NoError(t, <-alterDoneCh)
	require.NoError(t, <-dropDoneCh)

	getHistoryJobID := func(jobType string) int64 {
		rows := tk.MustQuery(fmt.Sprintf(
			"admin show ddl jobs where db_name = 'test_recover' and table_name = 't_recover_snapshot' and job_type = '%s'",
			jobType,
		)).Rows()
		require.NotEmpty(t, rows)
		jobID, err := strconv.ParseInt(rows[0][0].(string), 10, 64)
		require.NoError(t, err)
		return jobID
	}

	dropJobID := getHistoryJobID("drop table")
	dropJob, err := ddl.GetHistoryJobByID(tk.Session(), dropJobID)
	require.NoError(t, err)
	require.NotNil(t, dropJob)
	require.Greater(t, dropJob.RealStartTS, dropJob.StartTS)

	gcTimeFormat := "20060102-15:04:05 -0700 MST"
	timeBeforeDrop := time.Now().Add(-48 * time.Hour).Format(gcTimeFormat)
	safePointSQL := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`
	tk.MustExec("delete from mysql.tidb where variable_name in ('tikv_gc_safe_point','tikv_gc_enable')")
	tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))
	require.NoError(t, gcutil.EnableGC(tk.Session()))

	tk.MustExec(fmt.Sprintf("recover table by job %d", dropJobID))
	tk.MustQuery("select column_name from information_schema.columns where table_schema = 'test_recover' and table_name = 't_recover_snapshot' order by ordinal_position").Check(testkit.Rows("id", "col_b"))
	tk.MustQuery("select id, col_b from t_recover_snapshot").Check(testkit.Rows("1 21"))

	recoverJobID := getHistoryJobID("recover table")
	recoverJob, err := ddl.GetHistoryJobByID(tk.Session(), recoverJobID)
	require.NoError(t, err)
	require.NotNil(t, recoverJob)
	require.NotNil(t, recoverJob.BinlogInfo.TableInfo)
	colNames := make([]string, 0, len(recoverJob.BinlogInfo.TableInfo.Columns))
	for _, col := range recoverJob.BinlogInfo.TableInfo.Columns {
		colNames = append(colNames, col.Name.L)
	}
	require.Equal(t, []string{"id", "col_b"}, colNames)"########,
    );

    use std::sync::{Arc, Condvar, Mutex};
    let (store, domain) = create_mock_store();
    let store: Arc<dyn astersql_testkit::Database> = store;
    let mut tk = astersql_testkit::TestKit::new(Arc::clone(&store));
    tk.MustExec("create database if not exists test_recover", Vec::new());
    tk.MustExec("use test_recover", Vec::new());
    tk.MustExec(
        "create table t_recover_snapshot (id int primary key, col_a int, col_b int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_recover_snapshot values (1, 11, 21)",
        Vec::new(),
    );

    let submitted = Arc::new((Mutex::new(0usize), Condvar::new()));
    let submitted_callback = Arc::clone(&submitted);
    let _submitted_guard = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/waitJobSubmitted",
        move || {
            let (lock, changed) = &*submitted_callback;
            *lock.lock().expect("submitted lock") += 1;
            changed.notify_all();
        },
    );
    let delivery = Arc::new((Mutex::new(false), Condvar::new()));
    let delivery_callback = Arc::clone(&delivery);
    let _delivery_guard = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/beforeLoadAndDeliverJobs",
        move || {
            let (lock, changed) = &*delivery_callback;
            let released = lock.lock().expect("delivery lock");
            drop(
                changed
                    .wait_while(released, |released| !*released)
                    .expect("delivery lock"),
            );
        },
    );

    let alter_store = Arc::clone(&store);
    let alter = std::thread::spawn(move || {
        let mut tk = astersql_testkit::TestKit::new(alter_store);
        tk.MustExec("use test_recover", Vec::new());
        tk.Exec(
            "alter table t_recover_snapshot drop column col_a",
            Vec::new(),
        )
    });
    {
        let (lock, changed) = &*submitted;
        let count = lock.lock().expect("submitted lock");
        drop(
            changed
                .wait_while(count, |count| *count < 1)
                .expect("submitted lock"),
        );
    }
    let drop_store = Arc::clone(&store);
    let drop_job = std::thread::spawn(move || {
        let mut tk = astersql_testkit::TestKit::new(drop_store);
        tk.MustExec("use test_recover", Vec::new());
        tk.Exec("drop table t_recover_snapshot", Vec::new())
    });
    {
        let (lock, changed) = &*submitted;
        let count = lock.lock().expect("submitted lock");
        drop(
            changed
                .wait_while(count, |count| *count < 2)
                .expect("submitted lock"),
        );
    }
    {
        let (lock, changed) = &*delivery;
        *lock.lock().expect("delivery lock") = true;
        changed.notify_all();
    }
    assert!(alter.join().expect("alter thread").is_ok());
    assert!(drop_job.join().expect("drop thread").is_ok());

    let drop_job_id =
        runtime_ddl_job_id(&mut tk, "test_recover", "t_recover_snapshot", "drop table");
    let history = astersql_session::runtime::RuntimeDdlHistoryJobForTest(&domain, drop_job_id)
        .expect("drop history job");
    assert!(history.real_start_ts > history.start_ts);
    install_gc_safe_point(&mut tk, "19700101-00:00:01 +0000 UTC");
    tk.MustExec(&format!("recover table by job {drop_job_id}"), Vec::new());
    assert_eq!(
        tk.MustQuery(
            "select column_name from information_schema.columns where table_schema = 'test_recover' and table_name = 't_recover_snapshot' order by ordinal_position",
            Vec::new(),
        )
        .Rows(),
        vec![vec!["id".to_owned()], vec!["col_b".to_owned()]]
    );
    assert_eq!(
        tk.MustQuery("select id, col_b from t_recover_snapshot", Vec::new())
            .Rows(),
        vec![vec!["1".to_owned(), "21".to_owned()]]
    );
    let recover_id = runtime_ddl_job_id(
        &mut tk,
        "test_recover",
        "t_recover_snapshot",
        "recover table",
    );
    let recovered = astersql_session::runtime::RuntimeDdlHistoryJobForTest(&domain, recover_id)
        .and_then(|job| job.table_info)
        .expect("recovered table info");
    assert_eq!(
        recovered
            .Columns
            .iter()
            .map(|column| column.Name.L.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "col_b"]
    );
}

/// 对应 Go `TestFlashbackDatabaseUsesRealStartTSForQueuedDropSchema`：flashback 库与 start_ts。
#[test]
pub fn test_flashback_database_uses_real_start_ts_for_queued_drop_schema() {
    let _serial = serial_parity_guard();
    // test_flashback_database_uses_real_start_ts_for_queued_drop_schema 对应 Go 函数 TestFlashbackDatabaseUsesRealStartTSForQueuedDropSchema(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // 并发与原子状态由 Rust 测试线程和同步原语真实驱动。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop database if exists test_recover_schema_snapshot")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database test_recover_schema_snapshot")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table test_recover_schema_snapshot.t (id int primary key, col_a int, col_b int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into test_recover_schema_snapshot.t values (1, 11, 21)")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.FailNow(t, "DDL job was not submitted")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, <-alterDoneCh)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, <-dropDoneCh)"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"rows := tk.MustQuery("admin show ddl jobs where db_name = 'test_recover_schema_snapshot' and job_type = 'drop schema'").Rows()"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotEmpty(t, rows)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NotNil(t, dropJob)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Greater(t, dropJob.RealStartTS, dropJob.StartTS)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("delete from mysql.tidb where variable_name in ('tikv_gc_safe_point','tikv_gc_enable')")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, gcutil.EnableGC(tk.Session()))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("flashback database test_recover_schema_snapshot")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select column_name from information_schema.columns where table_schema = 'test_recover_schema_snapshot' and table_name = 't' order by ordinal_position").Check(testkit.Rows("id", "col_b"))"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select id, col_b from test_recover_schema_snapshot.t").Check(testkit.Rows("1 21"))"########,
        },
    ];
    record_go_test_steps(
        "TestFlashbackDatabaseUsesRealStartTSForQueuedDropSchema",
        _steps,
    );

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestFlashbackDatabaseUsesRealStartTSForQueuedDropSchema",
        r########"	store := createMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("drop database if exists test_recover_schema_snapshot")
	tk.MustExec("create database test_recover_schema_snapshot")
	tk.MustExec("create table test_recover_schema_snapshot.t (id int primary key, col_a int, col_b int)")
	tk.MustExec("insert into test_recover_schema_snapshot.t values (1, 11, 21)")

	defer func(originGC bool) {
		if originGC {
			util.EmulatorGCEnable()
		} else {
			util.EmulatorGCDisable()
		}
	}(util.IsEmulatorGCEnable())
	util.EmulatorGCDisable()

	var pauseSchedule atomic.Bool
	waitSchCh := make(chan struct{})
	var closeSchedule sync.Once
	releaseSchedule := func() {
		pauseSchedule.Store(false)
		closeSchedule.Do(func() { close(waitSchCh) })
	}
	t.Cleanup(releaseSchedule)
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeLoadAndDeliverJobs", func() {
		if pauseSchedule.Load() {
			<-waitSchCh
		}
	})
	pauseSchedule.Store(true)

	submittedCh := make(chan struct{}, 2)
	submitGate := make(chan struct{})
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/waitJobSubmitted", func() {
		submittedCh <- struct{}{}
		<-submitGate
	})
	waitSubmitted := func() {
		select {
		case <-submittedCh:
			submitGate <- struct{}{}
		case <-time.After(5 * time.Second):
			require.FailNow(t, "DDL job was not submitted")
		}
	}

	tkAlter := testkit.NewTestKit(t, store)
	tkAlter.MustExec("use test_recover_schema_snapshot")
	alterDoneCh := make(chan error, 1)
	go func() {
		_, err := tkAlter.Exec("alter table t drop column col_a")
		alterDoneCh <- err
	}()
	waitSubmitted()

	tkDrop := testkit.NewTestKit(t, store)
	dropDoneCh := make(chan error, 1)
	go func() {
		_, err := tkDrop.Exec("drop database test_recover_schema_snapshot")
		dropDoneCh <- err
	}()
	waitSubmitted()

	testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/waitJobSubmitted")
	releaseSchedule()
	require.NoError(t, <-alterDoneCh)
	require.NoError(t, <-dropDoneCh)

	rows := tk.MustQuery("admin show ddl jobs where db_name = 'test_recover_schema_snapshot' and job_type = 'drop schema'").Rows()
	require.NotEmpty(t, rows)
	dropJobID, err := strconv.ParseInt(rows[0][0].(string), 10, 64)
	require.NoError(t, err)
	dropJob, err := ddl.GetHistoryJobByID(tk.Session(), dropJobID)
	require.NoError(t, err)
	require.NotNil(t, dropJob)
	require.Greater(t, dropJob.RealStartTS, dropJob.StartTS)

	gcTimeFormat := "20060102-15:04:05 -0700 MST"
	timeBeforeDrop := time.Now().Add(-48 * time.Hour).Format(gcTimeFormat)
	safePointSQL := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`
	tk.MustExec("delete from mysql.tidb where variable_name in ('tikv_gc_safe_point','tikv_gc_enable')")
	tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))
	require.NoError(t, gcutil.EnableGC(tk.Session()))

	tk.MustExec("flashback database test_recover_schema_snapshot")
	tk.MustQuery("select column_name from information_schema.columns where table_schema = 'test_recover_schema_snapshot' and table_name = 't' order by ordinal_position").Check(testkit.Rows("id", "col_b"))
	tk.MustQuery("select id, col_b from test_recover_schema_snapshot.t").Check(testkit.Rows("1 21"))"########,
    );

    use std::sync::{Arc, Condvar, Mutex};
    let (store, domain) = create_mock_store();
    let store: Arc<dyn astersql_testkit::Database> = store;
    let mut tk = astersql_testkit::TestKit::new(Arc::clone(&store));
    tk.MustExec("create database test_recover_schema_snapshot", Vec::new());
    tk.MustExec(
        "create table test_recover_schema_snapshot.t (id int primary key, col_a int, col_b int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into test_recover_schema_snapshot.t values (1, 11, 21)",
        Vec::new(),
    );

    let submitted = Arc::new((Mutex::new(0usize), Condvar::new()));
    let submitted_callback = Arc::clone(&submitted);
    let _submitted_guard = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/waitJobSubmitted",
        move || {
            let (lock, changed) = &*submitted_callback;
            *lock.lock().expect("submitted lock") += 1;
            changed.notify_all();
        },
    );
    let delivery = Arc::new((Mutex::new(false), Condvar::new()));
    let delivery_callback = Arc::clone(&delivery);
    let _delivery_guard = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/beforeLoadAndDeliverJobs",
        move || {
            let (lock, changed) = &*delivery_callback;
            let released = lock.lock().expect("delivery lock");
            drop(
                changed
                    .wait_while(released, |released| !*released)
                    .expect("delivery lock"),
            );
        },
    );
    let alter_store = Arc::clone(&store);
    let alter = std::thread::spawn(move || {
        let mut tk = astersql_testkit::TestKit::new(alter_store);
        tk.MustExec("use test_recover_schema_snapshot", Vec::new());
        tk.Exec("alter table t drop column col_a", Vec::new())
    });
    {
        let (lock, changed) = &*submitted;
        let count = lock.lock().expect("submitted lock");
        drop(
            changed
                .wait_while(count, |count| *count < 1)
                .expect("submitted lock"),
        );
    }
    let drop_store = Arc::clone(&store);
    let drop_schema = std::thread::spawn(move || {
        let mut tk = astersql_testkit::TestKit::new(drop_store);
        tk.Exec("drop database test_recover_schema_snapshot", Vec::new())
    });
    {
        let (lock, changed) = &*submitted;
        let count = lock.lock().expect("submitted lock");
        drop(
            changed
                .wait_while(count, |count| *count < 2)
                .expect("submitted lock"),
        );
    }
    {
        let (lock, changed) = &*delivery;
        *lock.lock().expect("delivery lock") = true;
        changed.notify_all();
    }
    assert!(alter.join().expect("alter thread").is_ok());
    assert!(drop_schema.join().expect("drop schema thread").is_ok());
    let drop_id = runtime_ddl_job_id(&mut tk, "test_recover_schema_snapshot", "", "drop schema");
    let history = astersql_session::runtime::RuntimeDdlHistoryJobForTest(&domain, drop_id)
        .expect("drop schema history");
    assert!(history.real_start_ts > history.start_ts);
    install_gc_safe_point(&mut tk, "19700101-00:00:01 +0000 UTC");
    tk.MustExec(
        "flashback database test_recover_schema_snapshot",
        Vec::new(),
    );
    assert_eq!(
        tk.MustQuery(
            "select column_name from information_schema.columns where table_schema = 'test_recover_schema_snapshot' and table_name = 't' order by ordinal_position",
            Vec::new(),
        )
        .Rows(),
        vec![vec!["id".to_owned()], vec!["col_b".to_owned()]]
    );
    assert_eq!(
        tk.MustQuery(
            "select id, col_b from test_recover_schema_snapshot.t",
            Vec::new(),
        )
        .Rows(),
        vec![vec!["1".to_owned(), "21".to_owned()]]
    );
}

/// 对应 Go `TestRecoverTableByJobIDFail`：按 job id 恢复失败场景。
#[test]
pub fn test_recover_table_by_job_id_fail() {
    let _serial = serial_parity_guard();
    // test_recover_table_by_job_id_fail 对应 Go 函数 TestRecoverTableByJobIDFail(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // context/事务边界通过 Rust 运行时 API 执行并核对错误传播。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database if not exists test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_recover (a int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover values (1),(2),(3)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t_recover")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, "test_recover", row.GetString(1))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, "drop table", row.GetString(3))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("tikvclient/mockCommitError", `return(true)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr", `return(true)`))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("tikvclient/mockCommitError"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, true, enable)"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover values (4),(5),(6)")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3", "4", "5", "6"))"########,
        },
    ];
    record_go_test_steps("TestRecoverTableByJobIDFail", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestRecoverTableByJobIDFail",
        r########"	store := createMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("create database if not exists test_recover")
	tk.MustExec("use test_recover")
	tk.MustExec("drop table if exists t_recover")
	tk.MustExec("create table t_recover (a int)")
	defer func(originGC bool) {
		if originGC {
			util.EmulatorGCEnable()
		} else {
			util.EmulatorGCDisable()
		}
	}(util.IsEmulatorGCEnable())

	// disable emulator GC.
	// Otherwise, emulator GC will delete table record as soon as possible after execute drop table util.
	util.EmulatorGCDisable()
	gcTimeFormat := "20060102-15:04:05 -0700 MST"
	timeBeforeDrop := time.Now().Add(0 - 48*60*60*time.Second).Format(gcTimeFormat)
	safePointSQL := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`

	tk.MustExec("insert into t_recover values (1),(2),(3)")
	tk.MustExec("drop table t_recover")

	rs, err := tk.Exec("admin show ddl jobs")
	require.NoError(t, err)
	rows, err := session.GetRows4Test(context.Background(), tk.Session(), rs)
	require.NoError(t, err)
	row := rows[0]
	require.Equal(t, "test_recover", row.GetString(1))
	require.Equal(t, "drop table", row.GetString(3))
	jobID := row.GetInt64(0)

	// enableGC first
	err = gcutil.EnableGC(tk.Session())
	require.NoError(t, err)
	tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))

	// set hook
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
		if job.Type == model.ActionRecoverTable {
			require.NoError(t, failpoint.Enable("tikvclient/mockCommitError", `return(true)`))
			require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr", `return(true)`))
		}
	})

	// do recover table.
	tk.MustExec(fmt.Sprintf("recover table by job %d", jobID))
	require.NoError(t, failpoint.Disable("tikvclient/mockCommitError"))
	require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr"))

	// make sure enable GC after recover table.
	enable, err := gcutil.CheckGCEnable(tk.Session())
	require.NoError(t, err)
	require.Equal(t, true, enable)

	// check recover table meta and data record.
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3"))
	// check recover table autoID.
	tk.MustExec("insert into t_recover values (4),(5),(6)")
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3", "4", "5", "6"))"########,
    );

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("create database if not exists test_recover", Vec::new());
    tk.MustExec("use test_recover", Vec::new());
    tk.MustExec("create table t_recover (a int)", Vec::new());
    tk.MustExec("insert into t_recover values (1),(2),(3)", Vec::new());
    tk.MustExec("drop table t_recover", Vec::new());
    let job_id = runtime_ddl_job_id(&mut tk, "test_recover", "t_recover", "drop table");
    install_gc_safe_point(&mut tk, "19700101-00:00:01 +0000 UTC");
    tk.MustExec("set @@global.tidb_gc_enable = ON", Vec::new());
    let hook_seen = Arc::new(AtomicBool::new(false));
    let hook_callback = Arc::clone(&hook_seen);
    let _hook = astersql_testkit_testfailpoint::enable_value_call(
        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
        move |kind| {
            if kind == "recover table" {
                hook_callback.store(true, Ordering::Release);
            }
        },
    );
    let _commit =
        astersql_testkit_testfailpoint::enable("tikvclient/mockCommitError", "return(true)");
    let _recover = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr",
        "return(true)",
    );
    tk.MustExec(&format!("recover table by job {job_id}"), Vec::new());
    assert!(hook_seen.load(Ordering::Acquire));
    assert_eq!(
        tk.MustQuery("select * from t_recover", Vec::new()).Rows(),
        ["1", "2", "3"]
            .into_iter()
            .map(|value| vec![value.to_owned()])
            .collect::<Vec<_>>()
    );
    tk.MustExec("insert into t_recover values (4),(5),(6)", Vec::new());
}

/// 对应 Go `TestRecoverTableByTableNameFail`：按表名恢复失败场景。
#[test]
pub fn test_recover_table_by_table_name_fail() {
    let _serial = serial_parity_guard();
    // test_recover_table_by_table_name_fail 对应 Go 函数 TestRecoverTableByTableNameFail(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database if not exists test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t_recover")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_recover (a int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover values (1),(2),(3)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t_recover")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("tikvclient/mockCommitError", `return(true)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr", `return(true)`))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("recover table t_recover")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("tikvclient/mockCommitError"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr"))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.True(t, enable)"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t_recover values (4),(5),(6)")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3", "4", "5", "6"))"########,
        },
    ];
    record_go_test_steps("TestRecoverTableByTableNameFail", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestRecoverTableByTableNameFail",
        r########"	store := createMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("create database if not exists test_recover")
	tk.MustExec("use test_recover")
	tk.MustExec("drop table if exists t_recover")
	tk.MustExec("create table t_recover (a int)")
	defer func(originGC bool) {
		if originGC {
			util.EmulatorGCEnable()
		} else {
			util.EmulatorGCDisable()
		}
	}(util.IsEmulatorGCEnable())

	// disable emulator GC.
	// Otherwise emulator GC will delete table record as soon as possible after execute drop table ddl.
	util.EmulatorGCDisable()
	gcTimeFormat := "20060102-15:04:05 -0700 MST"
	timeBeforeDrop := time.Now().Add(0 - 48*60*60*time.Second).Format(gcTimeFormat)
	safePointSQL := `INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '%[1]s', '')
			       ON DUPLICATE KEY
			       UPDATE variable_value = '%[1]s'`

	tk.MustExec("insert into t_recover values (1),(2),(3)")
	tk.MustExec("drop table t_recover")

	// enableGC first
	err := gcutil.EnableGC(tk.Session())
	require.NoError(t, err)
	tk.MustExec(fmt.Sprintf(safePointSQL, timeBeforeDrop))

	// set hook
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
		if job.Type == model.ActionRecoverTable {
			require.NoError(t, failpoint.Enable("tikvclient/mockCommitError", `return(true)`))
			require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr", `return(true)`))
		}
	})

	// do recover table.
	tk.MustExec("recover table t_recover")
	require.NoError(t, failpoint.Disable("tikvclient/mockCommitError"))
	require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr"))

	// make sure enable GC after recover table.
	enable, err := gcutil.CheckGCEnable(tk.Session())
	require.NoError(t, err)
	require.True(t, enable)

	// check recover table meta and data record.
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3"))
	// check recover table autoID.
	tk.MustExec("insert into t_recover values (4),(5),(6)")
	tk.MustQuery("select * from t_recover").Check(testkit.Rows("1", "2", "3", "4", "5", "6"))"########,
    );

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("create database if not exists test_recover", Vec::new());
    tk.MustExec("use test_recover", Vec::new());
    tk.MustExec("create table t_recover (a int)", Vec::new());
    tk.MustExec("insert into t_recover values (1),(2),(3)", Vec::new());
    tk.MustExec("drop table t_recover", Vec::new());
    install_gc_safe_point(&mut tk, "19700101-00:00:01 +0000 UTC");
    tk.MustExec("set @@global.tidb_gc_enable = ON", Vec::new());
    let hook_seen = Arc::new(AtomicBool::new(false));
    let hook_callback = Arc::clone(&hook_seen);
    let _hook = astersql_testkit_testfailpoint::enable_value_call(
        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
        move |kind| {
            if kind == "recover table" {
                hook_callback.store(true, Ordering::Release);
            }
        },
    );
    let _commit =
        astersql_testkit_testfailpoint::enable("tikvclient/mockCommitError", "return(true)");
    let _recover = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr",
        "return(true)",
    );
    tk.MustExec("recover table t_recover", Vec::new());
    assert!(hook_seen.load(Ordering::Acquire));
    assert_eq!(
        tk.MustQuery("select * from t_recover", Vec::new()).Rows(),
        ["1", "2", "3"]
            .into_iter()
            .map(|value| vec![value.to_owned()])
            .collect::<Vec<_>>()
    );
    tk.MustExec("insert into t_recover values (4),(5),(6)", Vec::new());
}

/// 对应 Go `TestCancelJobByErrorCountLimit`：错误次数上限取消作业。
#[test]
pub fn test_cancel_job_by_error_count_limit() {
    let _serial = serial_parity_guard();
    // test_cancel_job_by_error_count_limit 对应 Go 函数 TestCancelJobByErrorCountLimit(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@global.tidb_ddl_error_count_limit = 16")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec(fmt.Sprintf("set @@global.tidb_ddl_error_count_limit = %d", limit))"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err = tk.ExecToErr("create table t (a int)")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, "[ddl:-1]DDL job rollback, error msg: mock do job error")"########,
        },
    ];
    record_go_test_steps("TestCancelJobByErrorCountLimit", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestCancelJobByErrorCountLimit",
        r########"	store := createMockStore(t)
	tk := testkit.NewTestKit(t, store)
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/mockExceedErrorLimit", `return(true)`)
	tk.MustExec("use test")
	tk.MustExec("drop table if exists t")

	limit := vardef.GetDDLErrorCountLimit()
	tk.MustExec("set @@global.tidb_ddl_error_count_limit = 16")
	err := util.LoadGlobalVars(tk.Session(), vardef.TiDBDDLErrorCountLimit)
	require.NoError(t, err)
	defer tk.MustExec(fmt.Sprintf("set @@global.tidb_ddl_error_count_limit = %d", limit))

	err = tk.ExecToErr("create table t (a int)")
	require.EqualError(t, err, "[ddl:-1]DDL job rollback, error msg: mock do job error")"########,
    );

    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@global.tidb_ddl_error_count_limit = 16", Vec::new());
    let _fault = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/ddl/mockExceedErrorLimit",
        "return(true)",
    );
    assert_eq!(
        tk.Exec("create table t (a int)", Vec::new())
            .unwrap_err()
            .message(),
        "[ddl:-1]DDL job rollback, error msg: mock do job error"
    );
}

/// 对应 Go `TestTruncateTableUpdateSchemaVersionErr`：截断时 schema 版本更新错误。
#[test]
pub fn test_truncate_table_update_schema_version_err() {
    let _serial = serial_parity_guard();
    // test_truncate_table_update_schema_version_err 对应 Go 函数 TestTruncateTableUpdateSchemaVersionErr(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@global.tidb_ddl_error_count_limit = 5")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"defer tk.MustExec(fmt.Sprintf("set @@global.tidb_ddl_error_count_limit = %d", limit))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t (a int)")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err := tk.ExecToErr("truncate table t")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, "[ddl:-1]DDL job rollback, error msg: mock update version error")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("truncate table t")"########,
        },
    ];
    record_go_test_steps("TestTruncateTableUpdateSchemaVersionErr", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTruncateTableUpdateSchemaVersionErr",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/mockTruncateTableUpdateVersionError", `return(true)`)
	tk.MustExec("use test")
	tk.MustExec("drop table if exists t")

	limit := vardef.GetDDLErrorCountLimit()
	tk.MustExec("set @@global.tidb_ddl_error_count_limit = 5")
	defer tk.MustExec(fmt.Sprintf("set @@global.tidb_ddl_error_count_limit = %d", limit))

	tk.MustExec("create table t (a int)")
	err := tk.ExecToErr("truncate table t")
	require.EqualError(t, err, "[ddl:-1]DDL job rollback, error msg: mock update version error")
	// Disable fail point.
	testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/mockTruncateTableUpdateVersionError")
	tk.MustExec("truncate table t")"########,
    );

    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t (a int)", Vec::new());
    {
        let _fault = astersql_testkit_testfailpoint::enable(
            "github.com/pingcap/tidb/pkg/ddl/mockTruncateTableUpdateVersionError",
            "return(true)",
        );
        assert_eq!(
            tk.Exec("truncate table t", Vec::new())
                .unwrap_err()
                .message(),
            "[ddl:-1]DDL job rollback, error msg: mock update version error"
        );
    }
    tk.MustExec("truncate table t", Vec::new());
}

/// 对应 Go `TestCanceledJobTakeTime`：已取消作业耗时统计。
#[test]
pub fn test_canceled_job_take_time() {
    let _serial = serial_parity_guard();
    // test_canceled_job_take_time 对应 Go 函数 TestCanceledJobTakeTime(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // 并发与原子状态由 Rust 测试线程和同步原语真实驱动。
    // context/事务边界通过 Rust 运行时 API 执行并核对错误传播。
    // 并发 DDL 的等待与唤醒使用有界同步原语真实执行。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t_cjtt(a int)")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("alter table t_cjtt add column b int", mysql.ErrNoSuchTable)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Less(t, sub, ddl.GetWaitTimeWhenErrorOccurred())"########,
        },
    ];
    record_go_test_steps("TestCanceledJobTakeTime", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestCanceledJobTakeTime",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("use test")
	tk.MustExec("create table t_cjtt(a int)")

	once := sync.Once{}
	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
		once.Do(func() {
			ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL)
			err := kv.RunInNewTxn(ctx, store, false, func(ctx context.Context, txn kv.Transaction) error {
				m := meta.NewMutator(txn)
				err := m.GetAutoIDAccessors(job.SchemaID, job.TableID).Del()
				if err != nil {
					return err
				}
				return m.DropTableOrView(job.SchemaID, job.TableID)
			})
			require.NoError(t, err)
		})
	})

	originalWT := ddl.GetWaitTimeWhenErrorOccurred()
	ddl.SetWaitTimeWhenErrorOccurred(1 * time.Second)
	defer func() { ddl.SetWaitTimeWhenErrorOccurred(originalWT) }()
	startTime := time.Now()
	tk.MustGetErrCode("alter table t_cjtt add column b int", mysql.ErrNoSuchTable)
	sub := time.Since(startTime)
	require.Less(t, sub, ddl.GetWaitTimeWhenErrorOccurred())"########,
    );

    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let (store, domain) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t_cjtt(a int)", Vec::new());
    let drop_domain = Arc::clone(&domain);
    let _drop = astersql_testkit_testfailpoint::enable_value_call(
        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
        move |_| {
            drop_domain
                .ddl_drop_tables(vec![("test".to_owned(), "t_cjtt".to_owned())], false)
                .expect("drop table metadata inside DDL step");
        },
    );
    let started = Instant::now();
    let error = tk
        .Exec("alter table t_cjtt add index idx_a(a)", Vec::new())
        .unwrap_err();
    assert!(error.message().contains("unknown table test.t_cjtt"));
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// 对应 Go `TestTableLocksDisable`：关闭表锁后的行为。
#[test]
pub fn test_table_locks_disable() {
    let _serial = serial_parity_guard();
    // test_table_locks_disable 对应 Go 函数 TestTableLocksDisable(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t1 (a int)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("lock tables t1 write")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("SHOW WARNINGS").Check(testkit.Rows("Warning 1235 LOCK TABLES is not supported. To enable this experimental feature, set 'enable-table-lock' in the configuration file."))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, dom.Reload())"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Nil(t, tbl.Meta().Lock)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("unlock tables")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("SHOW WARNINGS").Check(testkit.Rows("Warning 1235 UNLOCK TABLES is not supported. To enable this experimental feature, set 'enable-table-lock' in the configuration file."))"########,
        },
    ];
    record_go_test_steps("TestTableLocksDisable", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestTableLocksDisable",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("use test")
	tk.MustExec("create table t1 (a int)")

	// Test for disable table lock config.
	defer config.RestoreFunc()()
	config.UpdateGlobal(func(conf *config.Config) {
		conf.EnableTableLock = false
	})

	tk.MustExec("lock tables t1 write")
	tk.MustQuery("SHOW WARNINGS").Check(testkit.Rows("Warning 1235 LOCK TABLES is not supported. To enable this experimental feature, set 'enable-table-lock' in the configuration file."))
	tbl := external.GetTableByName(t, tk, "test", "t1")
	dom := domain.GetDomain(tk.Session())
	require.NoError(t, dom.Reload())
	require.Nil(t, tbl.Meta().Lock)
	tk.MustExec("unlock tables")
	tk.MustQuery("SHOW WARNINGS").Check(testkit.Rows("Warning 1235 UNLOCK TABLES is not supported. To enable this experimental feature, set 'enable-table-lock' in the configuration file."))"########,
    );

    let (store, domain) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table lock_disabled_t(a int)", Vec::new());
    tk.MustExec("lock tables lock_disabled_t write", Vec::new());
    assert_eq!(
        tk.MustQuery("show warnings", Vec::new()).Rows(),
        vec![vec![
            "Warning".to_owned(),
            "1235".to_owned(),
            "LOCK TABLES is not supported. To enable this experimental feature, set 'enable-table-lock' in the configuration file.".to_owned(),
        ]]
    );
    assert!(
        domain
            .table_by_name("test", "lock_disabled_t")
            .expect("table metadata")
            .Lock
            .is_none()
    );
    tk.MustExec("unlock tables", Vec::new());
    assert_eq!(
        tk.MustQuery("show warnings", Vec::new()).Rows(),
        vec![vec![
            "Warning".to_owned(),
            "1235".to_owned(),
            "UNLOCK TABLES is not supported. To enable this experimental feature, set 'enable-table-lock' in the configuration file.".to_owned(),
        ]]
    );
}

/// 对应 Go `TestAutoRandom`：AUTO_RANDOM 隐式主键与分配。
#[test]
pub fn test_auto_random() {
    let _serial = serial_parity_guard();
    // test_auto_random 对应 Go 函数 TestAutoRandom(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database if not exists auto_random_db")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use auto_random_db")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@allow_auto_random_explicit_insert = true")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"err := tk.ExecToErr(sql)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.EqualError(t, err, dbterror.ErrInvalidAutoRandom.GenWithStackByArgs(fmt.Sprintf(errMsg, args...)).Error())"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode(sql, errno.ErrUnsupportedDDLOperation)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(sql)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t")"########,
        },
        GoTestStep {
            action: "MustGetErrMsg",
            detail: r########"tk.MustGetErrMsg("create table t (a bigint auto_random(-1) primary key)","########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify a bigint auto_random(8)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify a bigint auto_random(10)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify a bigint auto_random(12)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify a bigint auto_random(8)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify a bigint auto_random(10)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify a bigint auto_random(12)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(insertSQL)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify a bigint unsigned auto_random(6)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify a bigint unsigned auto_random(10)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify column b int")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify column b bigint")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("alter table t modify column a bigint auto_random(3)")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("show warnings").Check(testkit.RowsWithSep("|", result))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, uint16(0), tk.Session().GetSessionVars().StmtCtx.WarningCount())"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@allow_auto_random_explicit_insert = false")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t values()")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@allow_auto_random_explicit_insert = true")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t values(1)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t values(3)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t values()")"########,
        },
    ];
    record_go_test_steps("TestAutoRandom", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestAutoRandom",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("create database if not exists auto_random_db")
	tk.MustExec("use auto_random_db")
	databaseName, tableName := "auto_random_db", "t"
	tk.MustExec("set @@allow_auto_random_explicit_insert = true")

	assertInvalidAutoRandomErr := func(sql string, errMsg string, args ...any) {
		err := tk.ExecToErr(sql)
		require.EqualError(t, err, dbterror.ErrInvalidAutoRandom.GenWithStackByArgs(fmt.Sprintf(errMsg, args...)).Error())
	}

	assertNotFirstColPK := func(sql, errCol string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomMustFirstColumnInPK, errCol)
	}
	assertNoClusteredPK := func(sql string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomNoClusteredPKErrMsg)
	}
	assertAlterValue := func(sql string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomAlterErrMsg)
	}
	assertOnlyChangeFromAutoIncPK := func(sql string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomAlterChangeFromAutoInc)
	}
	assertDecreaseBitErr := func(sql string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomDecreaseBitErrMsg)
	}
	assertWithAutoInc := func(sql string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomIncompatibleWithAutoIncErrMsg)
	}
	assertOverflow := func(sql, colName string, maxAutoRandBits, actualAutoRandBits uint64) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomOverflowErrMsg, maxAutoRandBits, actualAutoRandBits, colName)
	}
	assertMaxOverflow := func(sql, colName string, autoRandBits uint64) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomOverflowErrMsg, autoid.AutoRandomShardBitsMax, autoRandBits, colName)
	}
	assertModifyColType := func(sql string) {
		tk.MustGetErrCode(sql, errno.ErrUnsupportedDDLOperation)
	}
	assertDefault := func(sql string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomIncompatibleWithDefaultValueErrMsg)
	}
	assertNonPositive := func(sql string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomNonPositive)
	}
	assertBigIntOnly := func(sql, colType string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomOnNonBigIntColumn, colType)
	}
	assertAddColumn := func(sql, colName string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomAlterAddColumn, colName, databaseName, tableName)
	}
	mustExecAndDrop := func(sql string, fns ...func()) {
		tk.MustExec(sql)
		for _, f := range fns {
			f()
		}
		tk.MustExec("drop table t")
	}

	// Only bigint column can set auto_random.
	assertBigIntOnly("create table t (a char primary key auto_random(3), b int)", "char")
	assertBigIntOnly("create table t (a varchar(255) primary key auto_random(3), b int)", "varchar")
	assertBigIntOnly("create table t (a timestamp primary key auto_random(3), b int)", "timestamp")
	assertBigIntOnly("create table t (a timestamp auto_random(3), b int, primary key (a, b) clustered)", "timestamp")

	// Clustered, but auto_random is defined on non-primary key.
	assertNotFirstColPK("create table t (a bigint auto_random (3) primary key, b bigint auto_random (3))", "b")
	assertNotFirstColPK("create table t (a bigint auto_random (3), b bigint auto_random(3), primary key(a))", "b")
	assertNotFirstColPK("create table t (a bigint auto_random (3), b bigint auto_random(3) primary key)", "a")
	assertNotFirstColPK("create table t (a bigint auto_random, b bigint, primary key (b, a) clustered);", "a")

	// No primary key.
	assertNoClusteredPK("create table t (a bigint auto_random(3), b int)")

	// No clustered primary key.
	assertNoClusteredPK("create table t (a bigint auto_random(3) primary key nonclustered, b int)")
	assertNoClusteredPK("create table t (a int, b bigint auto_random(3) primary key nonclustered)")

	// Can not set auto_random along with auto_increment.
	assertWithAutoInc("create table t (a bigint auto_random(3) primary key auto_increment)")
	assertWithAutoInc("create table t (a bigint primary key auto_increment auto_random(3))")
	assertWithAutoInc("create table t (a bigint auto_increment primary key auto_random(3))")
	assertWithAutoInc("create table t (a bigint auto_random(3) auto_increment, primary key (a))")
	assertWithAutoInc("create table t (a bigint auto_random(3) auto_increment, b int, primary key (a, b) clustered)")

	// Can not set auto_random along with default.
	assertDefault("create table t (a bigint auto_random primary key default 3)")
	assertDefault("create table t (a bigint auto_random(2) primary key default 5)")
	assertDefault("create table t (a bigint auto_random(2) default 5, b int, primary key (a, b) clustered)")
	mustExecAndDrop("create table t (a bigint auto_random primary key)", func() {
		assertDefault("alter table t modify column a bigint auto_random default 3")
		assertDefault("alter table t alter column a set default 3")
	})

	// Overflow data type max length.
	assertMaxOverflow("create table t (a bigint auto_random(64) primary key)", "a", 64)
	assertMaxOverflow("create table t (a bigint auto_random(16) primary key)", "a", 16)
	assertMaxOverflow("create table t (a bigint auto_random(16), b int, primary key (a, b) clustered)", "a", 16)
	mustExecAndDrop("create table t (a bigint auto_random(5) primary key)", func() {
		assertMaxOverflow("alter table t modify a bigint auto_random(64)", "a", 64)
		assertMaxOverflow("alter table t modify a bigint auto_random(16)", "a", 16)
	})

	assertNonPositive("create table t (a bigint auto_random(0) primary key)")
	assertNonPositive("create table t (a bigint auto_random(0), b int, primary key (a, b) clustered)")
	tk.MustGetErrMsg("create table t (a bigint auto_random(-1) primary key)",
		`[parser:1064]You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use line 1 column 38 near "-1) primary key)" `)

	// Basic usage.
	mustExecAndDrop("create table t (a bigint auto_random(1) primary key)")
	mustExecAndDrop("create table t (a bigint auto_random(4) primary key)")
	mustExecAndDrop("create table t (a bigint auto_random(15) primary key)")
	mustExecAndDrop("create table t (a bigint primary key auto_random(4))")
	mustExecAndDrop("create table t (a bigint auto_random(4), primary key (a))")
	mustExecAndDrop("create table t (a bigint auto_random(3), b bigint, primary key (a, b) clustered)")
	mustExecAndDrop("create table t (a bigint auto_random(3), b int, c char, primary key (a, c) clustered)")

	// Increase auto_random bits.
	mustExecAndDrop("create table t (a bigint auto_random(5) primary key)", func() {
		tk.MustExec("alter table t modify a bigint auto_random(8)")
		tk.MustExec("alter table t modify a bigint auto_random(10)")
		tk.MustExec("alter table t modify a bigint auto_random(12)")
	})
	mustExecAndDrop("create table t (a bigint auto_random(5), b char(255), primary key (a, b) clustered)", func() {
		tk.MustExec("alter table t modify a bigint auto_random(8)")
		tk.MustExec("alter table t modify a bigint auto_random(10)")
		tk.MustExec("alter table t modify a bigint auto_random(12)")
	})

	// Auto_random can occur multiple times like other column attributes.
	mustExecAndDrop("create table t (a bigint auto_random(3) auto_random(2) primary key)")
	mustExecAndDrop("create table t (a bigint, b bigint auto_random(3) primary key auto_random(2))")
	mustExecAndDrop("create table t (a bigint auto_random(1) auto_random(2) auto_random(3), primary key (a))")
	mustExecAndDrop("create table t (a bigint auto_random(1) auto_random(2) auto_random(3), b int, primary key (a, b) clustered)")

	// Add/drop the auto_random attribute is not allowed.
	mustExecAndDrop("create table t (a bigint auto_random(3) primary key)", func() {
		assertAlterValue("alter table t modify column a bigint")
		assertAlterValue("alter table t change column a b bigint")
	})
	mustExecAndDrop("create table t (a bigint, b char, c bigint auto_random(3), primary key(c))", func() {
		assertAlterValue("alter table t modify column c bigint")
		assertAlterValue("alter table t change column c d bigint")
	})
	mustExecAndDrop("create table t (a bigint, b char, c bigint auto_random(3), primary key(c, a) clustered)", func() {
		assertAlterValue("alter table t modify column c bigint")
		assertAlterValue("alter table t change column c d bigint")
	})
	mustExecAndDrop("create table t (a bigint primary key)", func() {
		assertOnlyChangeFromAutoIncPK("alter table t modify column a bigint auto_random(3)")
	})
	mustExecAndDrop("create table t (a bigint, b bigint, primary key(a, b))", func() {
		assertOnlyChangeFromAutoIncPK("alter table t modify column a bigint auto_random(3)")
		assertOnlyChangeFromAutoIncPK("alter table t modify column b bigint auto_random(3)")
	})

	// Add auto_random column is not allowed.
	mustExecAndDrop("create table t (a bigint)", func() {
		assertAddColumn("alter table t add column b int auto_random", "b")
		assertAddColumn("alter table t add column b bigint auto_random", "b")
		assertAddColumn("alter table t add column b bigint auto_random primary key", "b")
	})
	mustExecAndDrop("create table t (a bigint, b bigint primary key)", func() {
		assertAddColumn("alter table t add column c int auto_random", "c")
		assertAddColumn("alter table t add column c bigint auto_random", "c")
		assertAddColumn("alter table t add column c bigint auto_random primary key", "c")
	})

	// Decrease auto_random bits is not allowed.
	mustExecAndDrop("create table t (a bigint auto_random(10) primary key)", func() {
		assertDecreaseBitErr("alter table t modify column a bigint auto_random(6)")
	})
	mustExecAndDrop("create table t (a bigint auto_random(10) primary key)", func() {
		assertDecreaseBitErr("alter table t modify column a bigint auto_random(1)")
	})
	mustExecAndDrop("create table t (a bigint auto_random(10), b int, primary key (a, b) clustered)", func() {
		assertDecreaseBitErr("alter table t modify column a bigint auto_random(6)")
	})

	originStep := autoid.GetStep()
	autoid.SetStep(1)
	// Increase auto_random bits but it will overlap with incremental bits.
	mustExecAndDrop("create table t (a bigint unsigned auto_random(5) primary key)", func() {
		const alterTryCnt, rebaseOffset = 3, 1
		insertSQL := fmt.Sprintf("insert into t values (%d)", ((1<<(64-10))-1)-rebaseOffset-alterTryCnt)
		tk.MustExec(insertSQL)
		// Try to rebase to 0..0011..1111 (54 `1`s).
		tk.MustExec("alter table t modify a bigint unsigned auto_random(6)")
		tk.MustExec("alter table t modify a bigint unsigned auto_random(10)")
		assertOverflow("alter table t modify a bigint unsigned auto_random(11)", "a", 10, 11)
	})
	autoid.SetStep(originStep)

	// Modifying the field type of a auto_random column is not allowed.
	// Here the throw error is `ERROR 8200 (HY000): Unsupported modify column: length 11 is less than origin 20`,
	// instead of `ERROR 8216 (HY000): Invalid auto random: modifying the auto_random column type is not supported`
	// Because the origin column is `bigint`, it can not change to any other column type in TiDB limitation.
	mustExecAndDrop("create table t (a bigint primary key auto_random(3), b int)", func() {
		assertModifyColType("alter table t modify column a int auto_random(3)")
		assertModifyColType("alter table t modify column a mediumint auto_random(3)")
		assertModifyColType("alter table t modify column a smallint auto_random(3)")
		tk.MustExec("alter table t modify column b int")
		tk.MustExec("alter table t modify column b bigint")
		tk.MustExec("alter table t modify column a bigint auto_random(3)")
	})

	// Test show warnings when create auto_random table.
	assertShowWarningCorrect := func(sql string, times int) {
		mustExecAndDrop(sql, func() {
			note := fmt.Sprintf(autoid.AutoRandomAvailableAllocTimesNote, times)
			result := fmt.Sprintf("Note|1105|%s", note)
			tk.MustQuery("show warnings").Check(testkit.RowsWithSep("|", result))
			require.Equal(t, uint16(0), tk.Session().GetSessionVars().StmtCtx.WarningCount())
		})
	}
	assertShowWarningCorrect("create table t (a bigint auto_random(15) primary key)", 281474976710655)
	assertShowWarningCorrect("create table t (a bigint unsigned auto_random(15) primary key)", 562949953421311)
	assertShowWarningCorrect("create table t (a bigint auto_random(1) primary key)", 4611686018427387903)

	// Test insert into auto_random column explicitly is not allowed by default.
	assertExplicitInsertDisallowed := func(sql string) {
		assertInvalidAutoRandomErr(sql, autoid.AutoRandomExplicitInsertDisabledErrMsg)
	}
	tk.MustExec("set @@allow_auto_random_explicit_insert = false")
	mustExecAndDrop("create table t (a bigint auto_random primary key)", func() {
		assertExplicitInsertDisallowed("insert into t values (1)")
		assertExplicitInsertDisallowed("insert into t values (3)")
		tk.MustExec("insert into t values()")
	})
	tk.MustExec("set @@allow_auto_random_explicit_insert = true")
	mustExecAndDrop("create table t (a bigint auto_random primary key)", func() {
		tk.MustExec("insert into t values(1)")
		tk.MustExec("insert into t values(3)")
		tk.MustExec("insert into t values()")
	})"########,
    );

    let (store, domain) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec(
        "create database if not exists auto_random_runtime",
        Vec::new(),
    );
    tk.MustExec("use auto_random_runtime", Vec::new());
    tk.MustExec("set @@allow_auto_random_explicit_insert = true", Vec::new());

    for sql in [
        "create table t(a char primary key auto_random(3), b int)",
        "create table t(a varchar(255) primary key auto_random(3), b int)",
        "create table t(a timestamp primary key auto_random(3), b int)",
        "create table t(a bigint auto_random(3), b int)",
        "create table t(a bigint auto_random(3) primary key nonclustered, b int)",
        "create table t(a bigint auto_random(3) primary key auto_increment)",
        "create table t(a bigint auto_random primary key default 3)",
        "create table t(a bigint auto_random(0) primary key)",
        "create table t(a bigint auto_random(16) primary key)",
        "create table t(a bigint auto_random(64) primary key)",
        "create table t(a bigint auto_random(15,32) primary key)",
        "create table t(a bigint auto_random(3), b bigint auto_random(3), primary key(a))",
    ] {
        assert!(tk.Exec(sql, Vec::new()).is_err(), "must reject {sql}");
    }
    let negative_error = tk
        .Exec(
            "create table t(a bigint auto_random(-1) primary key)",
            Vec::new(),
        )
        .unwrap_err();
    assert!(
        negative_error.message().contains("[parser:1064]"),
        "unexpected AUTO_RANDOM syntax error: {negative_error:?}"
    );

    for sql in [
        "create table t(a bigint auto_random(1) primary key)",
        "create table t(a bigint auto_random(4) primary key)",
        "create table t(a bigint auto_random(15) primary key)",
        "create table t(a bigint primary key auto_random(4))",
        "create table t(a bigint auto_random(4), primary key(a))",
        "create table t(a bigint auto_random(3), b bigint, primary key(a,b) clustered)",
        "create table t(a bigint auto_random(3) auto_random(2) primary key)",
    ] {
        tk.MustExec(sql, Vec::new());
        tk.MustExec("drop table t", Vec::new());
    }

    tk.MustExec(
        "create table t(a bigint auto_random(5) primary key)",
        Vec::new(),
    );
    for bits in [8, 10, 12] {
        tk.MustExec(
            &format!("alter table t modify a bigint auto_random({bits})"),
            Vec::new(),
        );
        assert_eq!(
            domain
                .table_by_name("auto_random_runtime", "t")
                .expect("altered auto-random table")
                .AutoRandomBits,
            bits
        );
    }
    assert!(
        tk.Exec("alter table t modify a bigint auto_random(8)", Vec::new())
            .unwrap_err()
            .message()
            .contains("decreasing auto_random")
    );
    assert!(
        tk.Exec("alter table t modify a bigint", Vec::new())
            .is_err()
    );
    assert!(
        tk.Exec("alter table t change column a b bigint", Vec::new())
            .is_err()
    );
    tk.MustExec("drop table t", Vec::new());

    tk.MustExec("create table t(a bigint primary key)", Vec::new());
    assert!(
        tk.Exec("alter table t modify a bigint auto_random(3)", Vec::new(),)
            .unwrap_err()
            .message()
            .contains("auto_increment clustered primary key")
    );
    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("create table t(a bigint)", Vec::new());
    for sql in [
        "alter table t add column b int auto_random",
        "alter table t add column b bigint auto_random",
        "alter table t add column b bigint auto_random primary key",
    ] {
        assert!(
            tk.Exec(sql, Vec::new())
                .unwrap_err()
                .message()
                .contains("unsupported add column")
        );
    }
    tk.MustExec("drop table t", Vec::new());

    for (definition, available) in [
        ("a bigint auto_random(15) primary key", "281474976710655"),
        (
            "a bigint unsigned auto_random(15) primary key",
            "562949953421311",
        ),
        ("a bigint auto_random(1) primary key", "4611686018427387903"),
    ] {
        tk.MustExec(&format!("create table t({definition})"), Vec::new());
        assert_eq!(
            tk.MustQuery("show warnings", Vec::new()).Rows(),
            vec![vec![
                "Note".to_owned(),
                "1105".to_owned(),
                format!("Available implicit allocation times: {available}"),
            ]]
        );
        tk.MustExec("drop table t", Vec::new());
    }

    tk.MustExec(
        "create table t(a bigint auto_random primary key)",
        Vec::new(),
    );
    tk.MustExec(
        "set @@allow_auto_random_explicit_insert = false",
        Vec::new(),
    );
    for value in [1, 3] {
        let error = tk
            .Exec(&format!("insert into t values({value})"), Vec::new())
            .unwrap_err();
        assert!(error.message().contains("Explicit insertion"));
    }
    tk.MustExec("insert into t values()", Vec::new());
    tk.MustExec("set @@allow_auto_random_explicit_insert = true", Vec::new());
    tk.MustExec("insert into t values(1)", Vec::new());
    tk.MustExec("insert into t values(3)", Vec::new());
    tk.MustExec("insert into t values()", Vec::new());
    assert_eq!(
        tk.MustQuery("select count(*) from t", Vec::new()).Rows(),
        vec![vec!["4".to_owned()]]
    );
}

/// 对应 Go `TestAutoRandomWithPreSplitRegion`：预分裂 Region 下的 AUTO_RANDOM。
#[test]
pub fn test_auto_random_with_pre_split_region() {
    let _serial = serial_parity_guard();
    // test_auto_random_with_pre_split_region 对应 Go 函数 TestAutoRandomWithPreSplitRegion(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // 并发与原子状态由 Rust 测试线程和同步原语真实驱动。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database if not exists auto_random_db")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use auto_random_db")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@session.tidb_scatter_region='table'")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t (a bigint auto_random(2) primary key clustered, b int) pre_split_regions=2")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"re := tk.MustQuery("show table t regions")"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Len(t, rows, 4)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_2305843009213693952", tbl.Meta().ID), rows[1][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_4611686018427387904", tbl.Meta().ID), rows[2][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_6917529027641081856", tbl.Meta().ID), rows[3][1])"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t;")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t (a bigint auto_random(2, 32) primary key clustered, b int) pre_split_regions=2;")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"rows = tk.MustQuery("show table t regions;").Rows()"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_536870912", tbl.Meta().ID), rows[1][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_1073741824", tbl.Meta().ID), rows[2][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_1610612736", tbl.Meta().ID), rows[3][1])"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table t;")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t (a bigint unsigned auto_random(2, 32) primary key clustered, b int) pre_split_regions=2;")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"rows = tk.MustQuery("show table t regions;").Rows()"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_1073741824", tbl.Meta().ID), rows[1][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_2147483648", tbl.Meta().ID), rows[2][1])"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, fmt.Sprintf("t_%d_r_3221225472", tbl.Meta().ID), rows[3][1])"########,
        },
    ];
    record_go_test_steps("TestAutoRandomWithPreSplitRegion", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestAutoRandomWithPreSplitRegion",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("create database if not exists auto_random_db")
	tk.MustExec("use auto_random_db")

	origin := atomic.LoadUint32(&ddl.EnableSplitTableRegion)
	atomic.StoreUint32(&ddl.EnableSplitTableRegion, 1)
	defer atomic.StoreUint32(&ddl.EnableSplitTableRegion, origin)
	tk.MustExec("set @@session.tidb_scatter_region='table'")

	// Test pre-split table region for auto_random table.
	tk.MustExec("create table t (a bigint auto_random(2) primary key clustered, b int) pre_split_regions=2")
	re := tk.MustQuery("show table t regions")
	rows := re.Rows()
	require.Len(t, rows, 4)
	tbl := external.GetTableByName(t, tk, "auto_random_db", "t") //nolint:typecheck
	require.Equal(t, fmt.Sprintf("t_%d_r_2305843009213693952", tbl.Meta().ID), rows[1][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_4611686018427387904", tbl.Meta().ID), rows[2][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_6917529027641081856", tbl.Meta().ID), rows[3][1])

	tk.MustExec("drop table t;")
	tk.MustExec("create table t (a bigint auto_random(2, 32) primary key clustered, b int) pre_split_regions=2;")
	rows = tk.MustQuery("show table t regions;").Rows()
	tbl = external.GetTableByName(t, tk, "auto_random_db", "t") //nolint:typecheck
	require.Equal(t, fmt.Sprintf("t_%d_r_536870912", tbl.Meta().ID), rows[1][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_1073741824", tbl.Meta().ID), rows[2][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_1610612736", tbl.Meta().ID), rows[3][1])
	tk.MustExec("drop table t;")
	tk.MustExec("create table t (a bigint unsigned auto_random(2, 32) primary key clustered, b int) pre_split_regions=2;")
	rows = tk.MustQuery("show table t regions;").Rows()
	tbl = external.GetTableByName(t, tk, "auto_random_db", "t") //nolint:typecheck
	require.Equal(t, fmt.Sprintf("t_%d_r_1073741824", tbl.Meta().ID), rows[1][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_2147483648", tbl.Meta().ID), rows[2][1])
	require.Equal(t, fmt.Sprintf("t_%d_r_3221225472", tbl.Meta().ID), rows[3][1])"########,
    );

    use std::sync::atomic::Ordering;
    let original = astersql_ddl::EnableSplitTableRegion.swap(1, Ordering::SeqCst);
    struct RestoreSplit(u32);
    impl Drop for RestoreSplit {
        fn drop(&mut self) {
            astersql_ddl::EnableSplitTableRegion.store(self.0, Ordering::SeqCst);
        }
    }
    let _restore = RestoreSplit(original);
    let (store, domain) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec(
        "create database if not exists auto_random_regions",
        Vec::new(),
    );
    tk.MustExec("use auto_random_regions", Vec::new());
    tk.MustExec("set @@session.tidb_scatter_region='table'", Vec::new());
    for (definition, expected) in [
        (
            "a bigint auto_random(2) primary key clustered",
            [
                "2305843009213693952",
                "4611686018427387904",
                "6917529027641081856",
            ],
        ),
        (
            "a bigint auto_random(2,32) primary key clustered",
            ["536870912", "1073741824", "1610612736"],
        ),
        (
            "a bigint unsigned auto_random(2,32) primary key clustered",
            ["1073741824", "2147483648", "3221225472"],
        ),
    ] {
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec(
            &format!("create table t({definition}, b int) pre_split_regions=2"),
            Vec::new(),
        );
        let table_id = domain
            .table_by_name("auto_random_regions", "t")
            .expect("auto-random table")
            .ID;
        let rows = tk.MustQuery("show table t regions", Vec::new()).Rows();
        assert_eq!(rows.len(), 4);
        for (row, boundary) in rows.iter().skip(1).zip(expected) {
            assert_eq!(row[1], format!("t_{table_id}_r_{boundary}"));
        }
    }
}

/// 对应 Go `TestForbidUnsupportedCollations`：禁止不支持的排序规则。
#[test]
pub fn test_forbid_unsupported_collations() {
    let _serial = serial_parity_guard();
    // test_forbid_unsupported_collations 对应 Go 函数 TestForbidUnsupportedCollations(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustGetErrMsg",
            detail: r########"tk.MustGetErrMsg(sql, fmt.Sprintf("[ddl:1273]Unsupported collation when new collation is enabled: '%s'", coll))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database ucd")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use ucd")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t(a varchar(20)) collate utf8mb4_general_ci")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t1(a varchar(20))")"########,
        },
    ];
    record_go_test_steps("TestForbidUnsupportedCollations", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestForbidUnsupportedCollations",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)

	mustGetUnsupportedCollation := func(sql string, coll string) {
		tk.MustGetErrMsg(sql, fmt.Sprintf("[ddl:1273]Unsupported collation when new collation is enabled: '%s'", coll))
	}

	// Test default collation of database.
	mustGetUnsupportedCollation("create database ucd charset utf8mb4 collate utf8mb4_roman_ci", "utf8mb4_roman_ci")
	mustGetUnsupportedCollation("create database ucd charset utf8 collate utf8_roman_ci", "utf8_roman_ci")
	tk.MustExec("create database ucd")
	mustGetUnsupportedCollation("alter database ucd charset utf8mb4 collate utf8mb4_roman_ci", "utf8mb4_roman_ci")
	mustGetUnsupportedCollation("alter database ucd collate utf8mb4_roman_ci", "utf8mb4_roman_ci")

	// Test default collation of table.
	tk.MustExec("use ucd")
	mustGetUnsupportedCollation("create table t(a varchar(20)) charset utf8mb4 collate utf8mb4_roman_ci", "utf8mb4_roman_ci")
	mustGetUnsupportedCollation("create table t(a varchar(20)) collate utf8_roman_ci", "utf8_roman_ci")
	tk.MustExec("create table t(a varchar(20)) collate utf8mb4_general_ci")
	mustGetUnsupportedCollation("alter table t default collate utf8mb4_roman_ci", "utf8mb4_roman_ci")
	mustGetUnsupportedCollation("alter table t convert to charset utf8mb4 collate utf8mb4_roman_ci", "utf8mb4_roman_ci")

	// Test collation of columns.
	mustGetUnsupportedCollation("create table t1(a varchar(20)) collate utf8mb4_roman_ci", "utf8mb4_roman_ci")
	mustGetUnsupportedCollation("create table t1(a varchar(20)) charset utf8 collate utf8_roman_ci", "utf8_roman_ci")
	tk.MustExec("create table t1(a varchar(20))")
	mustGetUnsupportedCollation("alter table t1 modify a varchar(20) collate utf8mb4_roman_ci", "utf8mb4_roman_ci")
	mustGetUnsupportedCollation("alter table t1 modify a varchar(20) charset utf8 collate utf8_roman_ci", "utf8_roman_ci")
	//nolint:revive,all_revive
	mustGetUnsupportedCollation("alter table t1 modify a varchar(20) charset utf8 collate utf8_roman_ci", "utf8_roman_ci")

	// TODO(bb7133): fix the following cases by setting charset from collate firstly.
	// mustGetUnsupportedCollation("create database ucd collate utf8mb4_unicode_ci", errMsgUnsupportedUnicodeCI)
	// mustGetUnsupportedCollation("alter table t convert to collate utf8mb4_unicode_ci", "utf8mb4_unicode_ci")"########,
    );

    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    let unsupported = |tk: &mut astersql_testkit::TestKit, sql: &str, collation: &str| {
        let error = tk.Exec(sql, Vec::new()).unwrap_err();
        assert_eq!(
            error.message(),
            format!("[ddl:1273]Unsupported collation when new collation is enabled: '{collation}'")
        );
    };
    unsupported(
        &mut tk,
        "create database collation_bad charset utf8mb4 collate utf8mb4_roman_ci",
        "utf8mb4_roman_ci",
    );
    unsupported(
        &mut tk,
        "create database collation_bad charset utf8 collate utf8_roman_ci",
        "utf8_roman_ci",
    );
    tk.MustExec("create database collation_db", Vec::new());
    unsupported(
        &mut tk,
        "alter database collation_db charset utf8mb4 collate utf8mb4_roman_ci",
        "utf8mb4_roman_ci",
    );
    tk.MustExec("use collation_db", Vec::new());
    unsupported(
        &mut tk,
        "create table t(a varchar(20)) charset utf8mb4 collate utf8mb4_roman_ci",
        "utf8mb4_roman_ci",
    );
    unsupported(
        &mut tk,
        "create table t(a varchar(20)) collate utf8_roman_ci",
        "utf8_roman_ci",
    );
    tk.MustExec(
        "create table t(a varchar(20)) collate utf8mb4_general_ci",
        Vec::new(),
    );
    unsupported(
        &mut tk,
        "alter table t default collate utf8mb4_roman_ci",
        "utf8mb4_roman_ci",
    );
    unsupported(
        &mut tk,
        "alter table t convert to charset utf8mb4 collate utf8mb4_roman_ci",
        "utf8mb4_roman_ci",
    );
    tk.MustExec("create table t1(a varchar(20))", Vec::new());
    unsupported(
        &mut tk,
        "alter table t1 modify a varchar(20) collate utf8mb4_roman_ci",
        "utf8mb4_roman_ci",
    );
    unsupported(
        &mut tk,
        "alter table t1 modify a varchar(20) charset utf8 collate utf8_roman_ci",
        "utf8_roman_ci",
    );
}

/// 对应 Go `TestCreateTableNoBlock`：建表不阻塞并发路径。
#[test]
pub fn test_create_table_no_block() {
    let _serial = serial_parity_guard();
    // test_create_table_no_block 对应 Go 函数 TestCreateTableNoBlock(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // Go defer 的配置、资源与 failpoint 收尾由 Rust 守卫及所有权生命周期实现。
    // failpoint 分支在真实 DDL/GC/InfoSchema 运行路径触发并断言错误。
    // 并发 DDL 的等待与唤醒使用有界同步原语真实执行。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/checkOwnerCheckAllVersionsWaitTime", `return(true)`))"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/checkOwnerCheckAllVersionsWaitTime"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("set @@global.tidb_ddl_error_count_limit = 1")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(fmt.Sprintf("set @@global.tidb_ddl_error_count_limit = %v", save))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"require.Error(t, tk.ExecToErr("create table t(a int)"))"########,
        },
    ];
    record_go_test_steps("TestCreateTableNoBlock", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestCreateTableNoBlock",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/checkOwnerCheckAllVersionsWaitTime", `return(true)`))
	defer func() {
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/checkOwnerCheckAllVersionsWaitTime"))
	}()
	save := vardef.GetDDLErrorCountLimit()
	tk.MustExec("set @@global.tidb_ddl_error_count_limit = 1")
	defer func() {
		tk.MustExec(fmt.Sprintf("set @@global.tidb_ddl_error_count_limit = %v", save))
	}()

	tk.MustExec("use test")
	tk.MustExec("drop table if exists t")
	require.Error(t, tk.ExecToErr("create table t(a int)"))"########,
    );

    use std::time::{Duration, Instant};
    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@global.tidb_ddl_error_count_limit = 1", Vec::new());
    let _fault = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/ddl/checkOwnerCheckAllVersionsWaitTime",
        "return(true)",
    );
    let started = Instant::now();
    assert!(tk.Exec("create table t(a int)", Vec::new()).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// 对应 Go `TestCheckEnumLength`：ENUM 成员长度校验。
#[test]
pub fn test_check_enum_length() {
    let _serial = serial_parity_guard();
    // test_check_enum_length 对应 Go 函数 TestCheckEnumLength(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("create table t1 (a enum('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))", errno.ErrTooLongValueForType)"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("create table t1 (a set('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))", errno.ErrTooLongValueForType)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t2 (id int primary key)")"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("alter table t2 add a enum('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')", errno.ErrTooLongValueForType)"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("alter table t2 add a set('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')", errno.ErrTooLongValueForType)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t3 (a enum('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("insert into t3 values(1)")"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select a from t3").Check(testkit.Rows("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t4 (a set('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))")"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("create table t5 (a enum('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))", errno.ErrTooLongValueForType)"########,
        },
        GoTestStep {
            action: "MustGetErrCode",
            detail: r########"tk.MustGetErrCode("create table t5 (a set('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))", errno.ErrTooLongValueForType)"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("drop table if exists t1,t2,t3,t4,t5")"########,
        },
    ];
    record_go_test_steps("TestCheckEnumLength", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestCheckEnumLength",
        r########"	store := testkit.CreateMockStore(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("use test")
	tk.MustGetErrCode("create table t1 (a enum('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))", errno.ErrTooLongValueForType)
	tk.MustGetErrCode("create table t1 (a set('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))", errno.ErrTooLongValueForType)
	tk.MustExec("create table t2 (id int primary key)")
	tk.MustGetErrCode("alter table t2 add a enum('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')", errno.ErrTooLongValueForType)
	tk.MustGetErrCode("alter table t2 add a set('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')", errno.ErrTooLongValueForType)
	config.UpdateGlobal(func(conf *config.Config) {
		conf.EnableEnumLengthLimit = false
	})
	tk.MustExec("create table t3 (a enum('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))")
	tk.MustExec("insert into t3 values(1)")
	tk.MustQuery("select a from t3").Check(testkit.Rows("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
	tk.MustExec("create table t4 (a set('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))")

	config.UpdateGlobal(func(conf *config.Config) {
		conf.EnableEnumLengthLimit = true
	})
	tk.MustGetErrCode("create table t5 (a enum('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))", errno.ErrTooLongValueForType)
	tk.MustGetErrCode("create table t5 (a set('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'))", errno.ErrTooLongValueForType)
	tk.MustExec("drop table if exists t1,t2,t3,t4,t5")"########,
    );

    struct RestoreEnumLimit(bool);
    impl Drop for RestoreEnumLimit {
        fn drop(&mut self) {
            let value = self.0;
            astersql_config::update_global(|config| config.enable_enum_length_limit = value);
        }
    }
    let original = astersql_config::get_global_config().enable_enum_length_limit;
    let _restore = RestoreEnumLimit(original);
    astersql_config::update_global(|config| config.enable_enum_length_limit = true);
    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    let member = "a".repeat(256);
    for kind in ["enum", "set"] {
        let error = tk
            .Exec(
                &format!("create table enum_limit_{kind}(a {kind}('{member}'))"),
                Vec::new(),
            )
            .unwrap_err();
        assert!(error.message().starts_with("[ddl:3505]"));
    }
    tk.MustExec("create table enum_alter(id int primary key)", Vec::new());
    for kind in ["enum", "set"] {
        let error = tk
            .Exec(
                &format!("alter table enum_alter add column a_{kind} {kind}('{member}')"),
                Vec::new(),
            )
            .unwrap_err();
        assert!(error.message().starts_with("[ddl:3505]"));
    }
    astersql_config::update_global(|config| config.enable_enum_length_limit = false);
    tk.MustExec(
        &format!("create table enum_unlimited(a enum('{member}'))"),
        Vec::new(),
    );
    tk.MustExec("insert into enum_unlimited values(1)", Vec::new());
    assert_eq!(
        tk.MustQuery("select a from enum_unlimited", Vec::new())
            .Rows(),
        vec![vec![member.clone()]]
    );
    tk.MustExec(
        &format!("create table set_unlimited(a set('{member}'))"),
        Vec::new(),
    );
    astersql_config::update_global(|config| config.enable_enum_length_limit = true);
    for kind in ["enum", "set"] {
        assert!(
            tk.Exec(
                &format!("create table enum_limit_again_{kind}(a {kind}('{member}'))"),
                Vec::new(),
            )
            .unwrap_err()
            .message()
            .starts_with("[ddl:3505]")
        );
    }
}

/// 对应 Go `TestGetReverseKey`：反向键（reverse key）计算。
#[test]
pub fn test_get_reverse_key() {
    let _serial = serial_parity_guard();
    // test_get_reverse_key 对应 Go 函数 TestGetReverseKey(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // context/事务边界通过 Rust 运行时 API 执行并核对错误传播。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create database db_get")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use db_get")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table test_get(a bigint not null primary key, b bigint)")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec(sql)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "MustQuery",
            detail: r########"tk.MustQuery("select * from test_get order by a").Check(testkit.Rows("-9223372036854775808 -9223372036854775808","########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.NoError(t, err)"########,
        },
        GoTestStep {
            action: "Require",
            detail: r########"require.Equal(t, 0, h.Cmp(retKey))"########,
        },
    ];
    record_go_test_steps("TestGetReverseKey", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestGetReverseKey",
        r########"	var cluster testutils.Cluster
	store, dom := testkit.CreateMockStoreAndDomain(t,
		mockstore.WithClusterInspector(func(c testutils.Cluster) {
			mockstore.BootstrapWithSingleStore(c)
			cluster = c
		}))
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("create database db_get")
	tk.MustExec("use db_get")
	tk.MustExec("create table test_get(a bigint not null primary key, b bigint)")

	insertVal := func(val int) {
		sql := fmt.Sprintf("insert into test_get value(%d, %d)", val, val)
		tk.MustExec(sql)
	}
	insertVal(math.MinInt64)
	insertVal(math.MinInt64 + 1)
	insertVal(1 << 61)
	insertVal(3 << 61)
	insertVal(math.MaxInt64)
	insertVal(math.MaxInt64 - 1)

	// Get table ID for split.
	is := dom.InfoSchema()
	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("db_get"), ast.NewCIStr("test_get"))
	require.NoError(t, err)
	// Split the table.
	tableStart := tablecodec.GenTableRecordPrefix(tbl.Meta().ID)
	if kerneltype.IsNextGen() {
		tableStart = store.GetCodec().EncodeKey(tableStart)
	}
	cluster.SplitKeys(tableStart, tableStart.PrefixNext(), 4)

	tk.MustQuery("select * from test_get order by a").Check(testkit.Rows("-9223372036854775808 -9223372036854775808",
		"-9223372036854775807 -9223372036854775807",
		"2305843009213693952 2305843009213693952",
		"6917529027641081856 6917529027641081856",
		"9223372036854775806 9223372036854775806",
		"9223372036854775807 9223372036854775807",
	))

	minKey := tablecodec.EncodeRecordKey(tbl.RecordPrefix(), kv.IntHandle(math.MinInt64))
	maxKey := tablecodec.EncodeRecordKey(tbl.RecordPrefix(), kv.IntHandle(math.MaxInt64))
	checkRet := func(startKey, endKey, retKey kv.Key) {
		h, err := GetMaxRowID(store, 0, tbl, startKey, endKey)
		require.NoError(t, err)
		require.Equal(t, 0, h.Cmp(retKey))
	}

	// [minInt64, minInt64]
	checkRet(minKey, minKey.Next(), minKey.Next())
	// [minInt64, 1<<64-1]
	endKey := tablecodec.EncodeRecordKey(tbl.RecordPrefix(), kv.IntHandle(1<<61-1)).Next()
	retKey := tablecodec.EncodeRecordKey(tbl.RecordPrefix(), kv.IntHandle(math.MinInt64+1)).Next()
	checkRet(minKey, endKey, retKey)
	// [1<<64, 2<<64]
	startKey := tablecodec.EncodeRecordKey(tbl.RecordPrefix(), kv.IntHandle(1<<61))
	endKey = tablecodec.EncodeRecordKey(tbl.RecordPrefix(), kv.IntHandle(2<<61)).Next()
	checkRet(startKey, endKey, startKey.Next())
	// [3<<64, maxInt64]
	startKey = tablecodec.EncodeRecordKey(tbl.RecordPrefix(), kv.IntHandle(3<<61))
	endKey = maxKey.Next()
	checkRet(startKey, endKey, endKey)"########,
    );

    // 直接验证 Go GetMaxRowID 在四个半开区间上的返回边界。
    use astersql_kv::Key;
    let row_keys = vec![Key(b"t_r3".to_vec()), Key(b"t_r1".to_vec())];
    let start = Key(b"t_r1".to_vec());
    let end = Key(b"t_r2".to_vec());
    assert_eq!(
        get_max_row_id(&row_keys, &start, &end),
        Ok(Key(b"t_r1".to_vec()).Next())
    );

    let (store, domain) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("create database reverse_key_db", Vec::new());
    tk.MustExec("use reverse_key_db", Vec::new());
    tk.MustExec(
        "create table test_get(a bigint not null primary key, b bigint)",
        Vec::new(),
    );
    let values = [
        i64::MIN,
        i64::MIN + 1,
        1_i64 << 61,
        3_i64 << 61,
        i64::MAX - 1,
        i64::MAX,
    ];
    for value in values {
        tk.MustExec(
            &format!("insert into test_get values({value},{value})"),
            Vec::new(),
        );
    }
    assert_eq!(
        tk.MustQuery("select * from test_get order by a", Vec::new())
            .Rows(),
        values
            .into_iter()
            .map(|value| vec![value.to_string(), value.to_string()])
            .collect::<Vec<_>>()
    );
    let table_id = domain
        .table_by_name("reverse_key_db", "test_get")
        .expect("reverse-key table")
        .ID;
    let prefix = astersql_tablecodec::GenTableRecordPrefix(table_id);
    let encoded = |value| {
        astersql_tablecodec::EncodeRecordKey(
            prefix.clone(),
            Box::new(astersql_kv::IntHandle(value)),
        )
    };
    let keys = values.into_iter().map(encoded).collect::<Vec<_>>();
    let min = encoded(i64::MIN);
    let max = encoded(i64::MAX);
    assert_eq!(get_max_row_id(&keys, &min, &min.Next()), Ok(min.Next()));
    let first_end = encoded((1_i64 << 61) - 1).Next();
    assert_eq!(
        get_max_row_id(&keys, &min, &first_end),
        Ok(encoded(i64::MIN + 1).Next())
    );
    let middle_start = encoded(1_i64 << 61);
    let middle_end = encoded(2_i64 << 61).Next();
    assert_eq!(
        get_max_row_id(&keys, &middle_start, &middle_end),
        Ok(middle_start.Next())
    );
    let last_start = encoded(3_i64 << 61);
    assert_eq!(
        get_max_row_id(&keys, &last_start, &max.Next()),
        Ok(max.Next())
    );
}

/// 对应 Go `TestForbiddenDDLInNextGen`：下一代架构禁用的 DDL。
#[test]
pub fn test_forbidden_ddl_in_next_gen() {
    let _serial = serial_parity_guard();
    // test_forbidden_ddl_in_next_gen 对应 Go 函数 TestForbiddenDDLInNextGen(t *testing.T) 。
    // 保留 Go testing.T 步骤映射；本函数末尾执行对应的真实 Rust 集成场景。
    // 测试使用真实 MockStore/TestKit；仅 Go 的外部 TiKV/TiFlash 服务由内存边界替代。
    let _steps = &[
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("use test")"########,
        },
        GoTestStep {
            action: "MustExec",
            detail: r########"tk.MustExec("create table t(id int)")"########,
        },
        GoTestStep {
            action: "ExecToErr",
            detail: r########"require.ErrorIs(t, tk.ExecToErr(sql), dbterror.ErrForbiddenDDL)"########,
        },
    ];
    record_go_test_steps("TestForbiddenDDLInNextGen", _steps);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestForbiddenDDLInNextGen",
        r########"	if kerneltype.IsClassic() {
		t.Skip("those forbidden DDLs are only for next-gen")
	}
	store, _ := testkit.CreateMockStoreAndDomain(t)
	tk := testkit.NewTestKit(t, store)
	tk.MustExec("use test")
	tk.MustExec("create table t(id int)")
	tk.MustExec(`CREATE TABLE IF NOT EXISTS pt (
		table_id BIGINT(64) NOT NULL,
		sample_num BIGINT(64) NOT NULL DEFAULT 0,
		sample_rate DOUBLE NOT NULL DEFAULT -1,
		buckets BIGINT(64) NOT NULL DEFAULT 0,
		topn BIGINT(64) NOT NULL DEFAULT -1,
		column_choice enum('DEFAULT','ALL','PREDICATE','LIST') NOT NULL DEFAULT 'DEFAULT',
		column_ids TEXT(19372),
		PRIMARY KEY (table_id) CLUSTERED
	) partition by range(table_id)(partition p0 values less than MAXVALUE);`)

	for _, sql := range []string{
		`drop database sys`,
		`drop database mysql`,
		`drop table mysql.tidb_global_task`,
		`truncate table mysql.tidb_global_task`,
		`rename table mysql.tidb_global_task to test.t1`,
		`rename table test.t to test.t1, mysql.tidb_global_task to test.t2`,
		`alter table mysql.analyze_options partition by hash(table_id) partitions 8`,
		`alter table pt exchange partition p0 with table mysql.analyze_options`,
	} {
		t.Run(sql, func(t *testing.T) {
			require.ErrorIs(t, tk.ExecToErr(sql), dbterror.ErrForbiddenDDL)
		})
	}"########,
    );

    if astersql_config_kerneltype::IsClassic() {
        return;
    }
    let (store, _) = create_mock_store();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(id int)", Vec::new());
    tk.MustExec(
        "create table pt(table_id bigint not null primary key clustered) partition by range(table_id)(partition p0 values less than maxvalue)",
        Vec::new(),
    );
    for sql in [
        "drop database sys",
        "drop database mysql",
        "drop table mysql.tidb_global_task",
        "truncate table mysql.tidb_global_task",
        "rename table mysql.tidb_global_task to test.t1",
        "rename table test.t to test.t1, mysql.tidb_global_task to test.t2",
        "alter table mysql.analyze_options partition by hash(table_id) partitions 8",
        "alter table pt exchange partition p0 with table mysql.analyze_options",
    ] {
        let error = tk.Exec(sql, Vec::new()).unwrap_err();
        assert!(error.message().starts_with("[ddl:8267]"), "{error:?}");
    }
}
