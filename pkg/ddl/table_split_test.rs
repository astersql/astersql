// Copyright 2025 PingCAP, Inc.
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

// 表 Region 分裂（table split / scatter region）相关测试。
//
// Region 是 TiKV 中的数据分片单元；表级/索引级 split policy 指定按主键或索引键
// 预分裂 Region，以改善热点与并行度。`tidb_scatter_region` 控制建表时是否自动打散。
//
// 可执行部分覆盖：分区表物理 ID 列表、split policy 幂等写入、
// 已有索引 policy 时新增索引的 warning。注释块保留 Go 集成测试迁移草稿。

// Copyright 2026 AsterSQL.
/*

// test_table_split 对应 Go 的 TestTableSplit：验证开启 table scatter 后，系统表和分区表 region 起始 key。
#[test]
fn test_table_split() {
    let store = mockstore::NewMockStore(mockstore::WithStoreType(mockstore::EmbedUnistore))
        .expect("Go require.NoError");
    defer(|| {
        // Go defer store.Close 并检查错误；只保留资源收尾语义。
        store.Close().expect("Go require.NoError");
    });
    vardef::SetSchemaLease(100 * time::Millisecond);
    session::DisableStats4Test();
    atomic::StoreUint32(&ddl::EnableSplitTableRegion, 1);

    let dom = session::BootstrapSession(store).expect("Go require.NoError");
    let mut tk = testkit::NewTestKit(testing::T, store);
    tk.MustExec("use test");

    // Synced split table region.
    // 先设置 session 变量创建分区表，再设置 global 变量并新建 session 创建第二张分区表。
    tk.MustExec("set @@session.tidb_scatter_region = 'table'");
    tk.MustExec(
        r#"create table t_part (a int key) partition by range(a) (
			partition p0 values less than (10),
			partition p1 values less than (20)
		)"#,
    );
    tk.MustQuery("select @@global.tidb_scatter_region;")
        .Check(testkit::Rows(""));
    tk.MustExec("set @@global.tidb_scatter_region = 'table'");
    tk = testkit::NewTestKit(testing::T, store);
    tk.MustExec("use test");
    tk.MustExec(
        r#"create table t_part_2 (a int key) partition by range(a) (
			partition p0 values less than (10),
			partition p1 values less than (20)
		)"#,
    );
    defer(|| dom.Close());
    atomic::StoreUint32(&ddl::EnableSplitTableRegion, 0);

    // Go 通过 InfoSchema 找系统表 mysql.tidb 和两张分区表，逐个校验分区 ID 对应 region 起始 key。
    let info_schema = dom.InfoSchema();
    require::NotNil(testing::T, info_schema);
    let tbl = info_schema
        .TableByName(context::Background(), ast::NewCIStr("mysql"), ast::NewCIStr("tidb"))
        .expect("Go require.NoError");
    check_region_start_with_table_id(testing::T, tbl.Meta().ID, store.as_kv_store());

    for table_name in ["t_part", "t_part_2"] {
        let tbl = info_schema
            .TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr(table_name))
            .expect("Go require.NoError");
        let pi = tbl.Meta().GetPartitionInfo();
        require::NotNil(testing::T, pi);
        for def in &pi.Definitions {
            check_region_start_with_table_id(testing::T, def.ID, store.as_kv_store());
        }
    }
}
*/

use std::collections::{BTreeMap, BTreeSet};

use crate::executor::{
    ColumnInfo, IndexInfo, TableInfo as ExecutorTableInfo, warn_missing_region_split_policy,
};
use crate::table::{TableInfo, TableState, alter_region_split_policy, table_physical_ids};

/// 构造带两个分区 ID 的测试表。
fn split_table() -> TableInfo {
    TableInfo {
        id: 10,
        schema_id: 1,
        name: "orders".into(),
        state: TableState::Public,
        partition_ids: vec![11, 12],
        auto_increment_id: 0,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        version: 1,
        foreign_keys: vec![],
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    }
}

/// Go TestTableSplit 的可执行元数据契约：分区表按各分区物理 ID 分裂，
/// 非分区表则使用表 ID。
#[test]
fn table_split_uses_every_physical_partition_id() {
    let mut table = split_table();
    assert_eq!(vec![11, 12], table_physical_ids(&table));

    table.partition_ids.clear();
    assert_eq!(vec![10], table_physical_ids(&table));
}

/// Go TestTableSplitPolicy 的可执行元数据契约：完整策略文本会被持久化，
/// 重复写入幂等，替换与清除均被视为真实变更。
#[test]
fn table_split_policy_persists_replaces_and_clears() {
    let mut table = split_table();
    let first = "BETWEEN (0) AND (1000000) REGIONS 4";
    assert!(alter_region_split_policy(&mut table, Some(first.into())));
    assert_eq!(Some(first), table.split_policy.as_deref());
    assert!(!alter_region_split_policy(&mut table, Some(first.into())));

    let replacement = "BETWEEN (100) AND (100000) REGIONS 3";
    assert!(alter_region_split_policy(
        &mut table,
        Some(replacement.into())
    ));
    assert_eq!(Some(replacement), table.split_policy.as_deref());
    assert!(alter_region_split_policy(&mut table, None));
    assert_eq!(None, table.split_policy);
    assert!(!alter_region_split_policy(&mut table, None));
}

/// Go TestTableSplitPolicyMultipleIndexes 的可执行元数据契约：多个索引的策略
/// 彼此独立，未配置策略的索引保持为空。
#[test]
fn multiple_index_split_policies_remain_independent() {
    let mut idx_user = IndexInfo::new("idx_user", vec!["user_id".into()]);
    idx_user.split_policy = Some("BETWEEN (100) AND (100000) REGIONS 3".into());
    let mut idx_status = IndexInfo::new("idx_status", vec!["status".into()]);
    idx_status.split_policy = Some("BETWEEN ('a') AND ('z') REGIONS 2".into());
    let idx_created = IndexInfo::new("idx_created", vec!["created_at".into()]);

    let policies = [idx_user, idx_status, idx_created]
        .into_iter()
        .map(|index| (index.name, index.split_policy))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        Some("BETWEEN (100) AND (100000) REGIONS 3"),
        policies["idx_user"].as_deref()
    );
    assert_eq!(
        Some("BETWEEN ('a') AND ('z') REGIONS 2"),
        policies["idx_status"].as_deref()
    );
    assert_eq!(None, policies["idx_created"]);
}

/// Go TestTableSplitPolicyWarning 的可执行契约：表上任一既有索引带策略时，
/// 新增无策略索引会追加包含新索引名与热点原因的 warning。
#[test]
fn adding_index_without_split_policy_emits_go_equivalent_warning() {
    let mut existing = IndexInfo::new("idx_a", vec!["a".into()]);
    existing.split_policy = Some("BETWEEN (0) AND (100) REGIONS 4".into());
    let table = ExecutorTableInfo {
        indexes: vec![existing],
        ..ExecutorTableInfo::new("orders", vec![ColumnInfo::integer("a")])
    };
    let mut warnings = vec!["existing warning".to_owned()];
    warn_missing_region_split_policy(&table, "idx_b", &mut warnings);
    assert_eq!(2, warnings.len());
    assert_eq!("existing warning", warnings[0]);
    assert!(warnings[1].contains("region split strategy"));
    assert!(warnings[1].contains("idx_b"));
    assert!(warnings[1].contains("write hotspots"));

    // 已有索引均无 policy 时，新增索引不产生 warning。
    let no_policy = ExecutorTableInfo {
        indexes: vec![IndexInfo::new("idx_a", vec!["a".into()])],
        ..ExecutorTableInfo::new("orders", vec![ColumnInfo::integer("a")])
    };
    warnings.clear();
    warn_missing_region_split_policy(&no_policy, "idx_b", &mut warnings);
    assert!(warnings.is_empty());
}

/*
// test_scatter_region 对应 Go 的 TestScatterRegion：验证 tidb_scatter_region session/global 变量取值规则。
#[test]
fn test_scatter_region() {
    let store = testkit::CreateMockStore(testing::T);
    let tk = testkit::NewTestKit(testing::T, store);
    let mut tk2 = testkit::NewTestKit(testing::T, store);

    for (sql, row) in [
        ("select @@tidb_scatter_region;", ""),
        ("set @@tidb_scatter_region = 'table'; select @@tidb_scatter_region;", "table"),
        ("set @@tidb_scatter_region = 'global'; select @@tidb_scatter_region;", "global"),
        ("set @@tidb_scatter_region = 'TABLE'; select @@tidb_scatter_region;", "table"),
        ("set @@tidb_scatter_region = 'GLOBAL'; select @@tidb_scatter_region;", "global"),
        ("set @@tidb_scatter_region = ''; select @@tidb_scatter_region;", ""),
    ] {
        // Go 源文件逐条 MustExec/MustQuery；这里把同类 session 设置压缩成表驱动步骤并保留 SQL 字面量。
        run_scatter_region_step(&tk, sql, row);
    }

    tk.MustExec("set global tidb_scatter_region = 'table';");
    tk.MustQuery("select @@global.tidb_scatter_region;")
        .Check(testkit::Rows("table"));
    tk.MustQuery("select @@tidb_scatter_region;")
        .Check(testkit::Rows(""));
    tk2.MustQuery("select @@tidb_scatter_region;")
        .Check(testkit::Rows(""));
    tk2 = testkit::NewTestKit(testing::T, store);
    tk2.MustQuery("select @@tidb_scatter_region;")
        .Check(testkit::Rows("table"));

    // global 变量只影响新 session；大小写值会标准化为小写。
    for (set_sql, global_row, new_session_row) in [
        ("set global tidb_scatter_region = 'global';", "global", ""),
        ("set global tidb_scatter_region = 'TABLE';", "table", "table"),
        ("set global tidb_scatter_region = 'GLOBAL';", "global", ""),
    ] {
        tk.MustExec(set_sql);
        tk.MustQuery("select @@global.tidb_scatter_region;")
            .Check(testkit::Rows(global_row));
        tk.MustExec("set global tidb_scatter_region = '';");
        tk.MustQuery("select @@global.tidb_scatter_region;")
            .Check(testkit::Rows(""));
        tk2 = testkit::NewTestKit(testing::T, store);
        tk2.MustQuery("select @@tidb_scatter_region;")
            .Check(testkit::Rows(new_session_row));
    }

    // 非法值需要回显原始输入并提示只允许 '', 'table' 或 'global'。
    for value in ["test", "te st", "1", "0"] {
        let sql = if value == "0" {
            "set @@tidb_scatter_region = 0;".to_string()
        } else {
            format!("set @@tidb_scatter_region = '{}';", value)
        };
        let err = tk.ExecToErr(sql);
        require::ErrorContains(
            testing::T,
            err,
            format!("invalid value for '{}', it should be either '', 'table' or 'global'", value),
        );
    }

    tk.MustQuery("select @@tidb_scatter_region;")
        .Check(testkit::Rows(""));
    tk.MustExec("set @@tidb_scatter_region = 'TaBlE';");
    tk.MustQuery("select @@tidb_scatter_region;")
        .Check(testkit::Rows("table"));
    tk.MustExec("set @@tidb_scatter_region = 'gLoBaL';");
    tk.MustQuery("select @@tidb_scatter_region;")
        .Check(testkit::Rows("global"));
}

fn run_scatter_region_step(tk: &testkit::TestKit, sql_pair: &str, expected_row: &str) {
    let parts = strings::Split(sql_pair, ";").collect::<Vec<_>>();
    if parts.len() > 1 && parts[0].starts_with("set ") {
        tk.MustExec(format!("{};", parts[0]));
    }
    tk.MustQuery(parts.last().unwrap())
        .Check(testkit::Rows(expected_row));
}

// kv_store 对应 Go 的 kvStore interface：只暴露 RegionCache。
trait kv_store {
    fn GetRegionCache(&self) -> *mut tikv::RegionCache;
}

// check_region_start_with_table_id 对应 Go helper：定位 table prefix 对应 region 并检查 StartKey。
fn check_region_start_with_table_id(t: testing::T, id: i64, store: &dyn kv_store) {
    let region_start_key = tablecodec::EncodeTablePrefix(id);
    let cache = store.GetRegionCache();
    let loc = cache
        .LocateKey(tikv::NewBackoffer(context::Background(), 5000), region_start_key)
        .expect("Go require.NoError");
    // Region cache may be out of date, so we need to drop this expired region and load it again.
    cache.InvalidateCachedRegion(loc.Region);
    require::Equal(t, region_start_key.as_bytes(), loc.StartKey);
}

// test_table_split_policy 对应 Go 的 TestTableSplitPolicy：验证表级和索引级 split policy 元数据。
#[test]
fn test_table_split_policy() {
    let store = testkit::CreateMockStore(testing::T);
    let tk = testkit::NewTestKit(testing::T, store);
    tk.MustExec("use test");
    tk.MustExec("set @@session.tidb_scatter_region = 'table'");
    tk.MustExec("drop table if exists t1");
    tk.MustExec("create table t1 (id bigint primary key, name varchar(100))");
    tk.MustExec("alter table t1 split between (0) and (1000000) regions 4");

    let tbl = external::GetTableByName(testing::T, tk, "test", "t1");
    assert_table_split_policy(testing::T, tbl.Meta().TableSplitPolicy, 4, &["0"], &["1000000"]);

    tk.MustExec("alter table t1 add index idx_name (name)");
    tk.MustExec("alter table t1 split index idx_name between ('a') and ('z') regions 3");
    let tbl = external::GetTableByName(testing::T, tk, "test", "t1");
    assert_index_policy_regions(testing::T, tbl.Meta(), "idx_name", 3);
    tk.MustExec("admin check table t1");

    tk.MustExec("drop table if exists t2");
    tk.MustExec(
        r#"create table t2 (
			id bigint primary key,
			user_id bigint,
			index idx_user (user_id)
		) split between (0) and (1000000) regions 4
		  split index idx_user between (100) and (100000) regions 3"#,
    );
    let tbl = external::GetTableByName(testing::T, tk, "test", "t2");
    require::NotNil(testing::T, tbl.Meta().TableSplitPolicy);
    require::Equal(testing::T, 4_i64, tbl.Meta().TableSplitPolicy.Regions);
    assert_index_policy_regions(testing::T, tbl.Meta(), "idx_user", 3);
    tk.MustExec("admin check table t2");
}

// test_table_split_policy_for_partitioned_table 对应 Go 同名测试：分区表也保留表级和索引级 split policy。
#[test]
fn test_table_split_policy_for_partitioned_table() {
    let store = testkit::CreateMockStore(testing::T);
    let tk = testkit::NewTestKit(testing::T, store);
    tk.MustExec("use test");
    tk.MustExec("set @@session.tidb_scatter_region = 'table'");

    tk.MustExec("drop table if exists t_part");
    tk.MustExec(
        r#"create table t_part (
			id bigint primary key,
			val bigint,
			index idx_val (val)
		) partition by range (id) (
			partition p0 values less than (1000),
			partition p1 values less than (2000),
			partition p2 values less than (maxvalue)
		) split between (0) and (10000) regions 5
		  split index idx_val between (0) and (10000) regions 3"#,
    );
    let tbl = external::GetTableByName(testing::T, tk, "test", "t_part");
    require::NotNil(testing::T, tbl.Meta().TableSplitPolicy);
    require::Equal(testing::T, 5_i64, tbl.Meta().TableSplitPolicy.Regions);
    assert_index_policy_regions(testing::T, tbl.Meta(), "idx_val", 3);
    tk.MustExec("admin check table t_part");

    tk.MustExec("drop table if exists t_part2");
    tk.MustExec(
        r#"create table t_part2 (
			id bigint primary key,
			val bigint
		) partition by range (id) (
			partition p0 values less than (1000),
			partition p1 values less than (2000)
		)"#,
    );
    tk.MustExec("alter table t_part2 split between (0) and (10000) regions 5");
    let tbl = external::GetTableByName(testing::T, tk, "test", "t_part2");
    require::NotNil(testing::T, tbl.Meta().TableSplitPolicy);
    require::Equal(testing::T, 5_i64, tbl.Meta().TableSplitPolicy.Regions);
    tk.MustExec("admin check table t_part2");
}

// test_table_split_policy_warning 对应 Go 同名测试：已有索引 split policy 时，新索引产生提示。
#[test]
fn test_table_split_policy_warning() {
    let store = testkit::CreateMockStore(testing::T);
    let tk = testkit::NewTestKit(testing::T, store);
    tk.MustExec("use test");

    tk.MustExec("drop table if exists t_warn");
    tk.MustExec("create table t_warn (id bigint primary key, user_id bigint, status varchar(10))");
    tk.MustExec("alter table t_warn add index idx_user_id (user_id)");
    tk.MustExec("alter table t_warn split index idx_user_id between (0) and (10000) regions 5");

    let tbl = external::GetTableByName(testing::T, tk, "test", "t_warn");
    require::True(testing::T, has_index_policy(tbl.Meta(), "idx_user_id"));

    // Go 等待一秒后新增索引，确保 warning 路径与上一次 split policy 元数据分离。
    time::Sleep(time::Second);
    tk.MustExec("alter table t_warn add index idx_status (status)");
    let warnings = tk.Session().GetSessionVars().StmtCtx.GetWarnings();
    let mut found_warning = false;
    for warn in warnings {
        if warn.Level == "Warning" {
            found_warning = true;
            require::Contains(testing::T, warn.Err.Error(), "region split strategy");
            require::Contains(testing::T, warn.Err.Error(), "idx_status");
            break;
        }
    }
    require::True(testing::T, found_warning);
    tk.MustExec("admin check table t_warn");
}

// test_table_split_policy_multiple_indexes 对应 Go 同名测试：同一张表可有多个索引策略，未指定索引为空。
#[test]
fn test_table_split_policy_multiple_indexes() {
    let store = testkit::CreateMockStore(testing::T);
    let tk = testkit::NewTestKit(testing::T, store);
    tk.MustExec("use test");
    tk.MustExec("set @@session.tidb_scatter_region = 'table'");

    tk.MustExec("drop table if exists t_multi");
    tk.MustExec(
        r#"create table t_multi (
			id bigint primary key,
			user_id bigint,
			status varchar(10),
			created_at bigint,
			index idx_user (user_id),
			index idx_status (status),
			index idx_created (created_at)
		) split between (0) and (1000000) regions 4
		  split index idx_user between (100) and (100000) regions 3
		  split index idx_status between ('a') and ('z') regions 2"#,
    );

    let tbl = external::GetTableByName(testing::T, tk, "test", "t_multi");
    require::NotNil(testing::T, tbl.Meta().TableSplitPolicy);
    require::Equal(testing::T, 4_i64, tbl.Meta().TableSplitPolicy.Regions);
    let index_policies = collect_index_policies(tbl.Meta());
    require::NotNil(testing::T, index_policies["idx_user"]);
    require::Equal(testing::T, 3_i64, index_policies["idx_user"].Regions);
    require::NotNil(testing::T, index_policies["idx_status"]);
    require::Equal(testing::T, 2_i64, index_policies["idx_status"].Regions);
    require::Nil(testing::T, index_policies.get("idx_created"));

    tk.MustExec("alter table t_multi split index idx_created between (0) and (1000000000) regions 5");
    let tbl = external::GetTableByName(testing::T, tk, "test", "t_multi");
    assert_index_policy_regions(testing::T, tbl.Meta(), "idx_created", 5);
    tk.MustExec("admin check table t_multi");
}

// test_table_split_policy_show_create_round_trip 对应 Go 同名测试：SHOW CREATE 中的 special comment 可回放。
#[test]
fn test_table_split_policy_show_create_round_trip() {
    let store = testkit::CreateMockStore(testing::T);
    let tk = testkit::NewTestKit(testing::T, store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t_src, t_dst");
    tk.MustExec(
        r#"create table t_src (
			id bigint primary key,
			user_id bigint,
			index idx_user_id (user_id)
		)
		split between (0) and (1000000) regions 4
		split index idx_user_id between (1000) and (100000) regions 3"#,
    );

    let create_sql = tk.MustQuery("show create table t_src").Rows()[0][1].as_string();
    require::Contains(testing::T, &create_sql, "/ *T![region_split]");
    let round_trip_sql = strings::Replace(&create_sql, "CREATE TABLE `t_src`", "CREATE TABLE `t_dst`", 1);
    tk.MustExec(round_trip_sql);

    let tbl = external::GetTableByName(testing::T, tk, "test", "t_dst");
    require::NotNil(testing::T, tbl.Meta().TableSplitPolicy);
    require::Equal(testing::T, 4_i64, tbl.Meta().TableSplitPolicy.Regions);
    assert_index_policy_regions(testing::T, tbl.Meta(), "idx_user_id", 3);
}

// test_table_split_policy_reject_split_index_primary_on_clustered 对应 Go 同名测试。
// clustered primary key 上的 PRIMARY split index 在 alter/create 两条路径都应返回 ErrForbiddenDDL。
#[test]
fn test_table_split_policy_reject_split_index_primary_on_clustered() {
    let store = testkit::CreateMockStore(testing::T);
    let tk = testkit::NewTestKit(testing::T, store);
    tk.MustExec("use test");
    tk.MustExec("set @@session.tidb_enable_clustered_index = ON");

    tk.MustExec("drop table if exists t");
    tk.MustExec(
        r#"create table t (
			id bigint,
			user_id bigint,
			primary key (id) clustered,
			index idx_user_id (user_id)
		)"#,
    );
    tk.MustGetErrCode(
        "alter table t split index `PRIMARY` between (0) and (1000000) regions 4",
        errno::ErrForbiddenDDL,
    );
    tk.MustGetErrCode(
        r#"create table t_create_fail (
			id bigint,
			primary key (id) clustered
		) split index `PRIMARY` between (0) and (1000000) regions 4"#,
        errno::ErrForbiddenDDL,
    );
}

fn assert_table_split_policy(
    t: testing::T,
    policy: *mut model::RegionSplitPolicy,
    regions: i64,
    lower: &[&str],
    upper: &[&str],
) {
    require::NotNil(t, policy);
    require::Equal(t, regions, unsafe { (*policy).Regions });
    require::Equal(t, lower, unsafe { (*policy).Lower.clone() });
    require::Equal(t, upper, unsafe { (*policy).Upper.clone() });
}

fn assert_index_policy_regions(t: testing::T, meta: *mut model::TableInfo, name: &str, regions: i64) {
    for idx in unsafe { &(*meta).Indices } {
        if idx.Name.L == name {
            require::NotNil(t, idx.RegionSplitPolicy);
            require::Equal(t, regions, idx.RegionSplitPolicy.Regions);
            return;
        }
    }
    require::Fail(t, format!("index {} not found", name));
}

fn has_index_policy(meta: *mut model::TableInfo, name: &str) -> bool {
    for idx in unsafe { &(*meta).Indices } {
        if idx.Name.L == name && idx.RegionSplitPolicy.is_some() {
            return true;
        }
    }
    false
}

fn collect_index_policies(meta: *mut model::TableInfo) -> HashMap<String, *mut model::RegionSplitPolicy> {
    let mut index_policies = HashMap::new();
    for idx in unsafe { &(*meta).Indices } {
        if idx.RegionSplitPolicy.is_some() {
            index_policies.insert(idx.Name.L.clone(), idx.RegionSplitPolicy);
        }
    }
    index_policies
}
*/
