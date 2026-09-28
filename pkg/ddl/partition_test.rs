// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 分区 DDL 相关测试。
//
// 文件前半为迁移自 Go 的 block 注释用例（drop/truncate/reorganize/exchange 等，
// 当前整段注释保留对照）；可执行部分使用内存 `Executor` 验证加/删分区与
// exchange partition（交换分区：用普通表与某分区互换物理数据）后物理 ID 语义。

/*
// 这段逻辑覆盖分区 drop/truncate 辅助流程、reorganize 回滚、加列期间更新以及多列 exchange partition。

// test_drop_and_truncate_partition 对应 Go 的 TestDropAndTruncatePartition。
// 它构造 5 个 range 分区，先 drop p0/p1，再 truncate p3/p4 并检查 history job。
#[test]
fn test_drop_and_truncate_partition() {
    let (store, domain) = testkit::CreateMockStoreAndDomainWithSchemaLease(testLease);

    let db_info = testSchemaInfo(store.clone(), "test_partition").expect("Go require.NoError: testSchemaInfo");
    let de = domain.DDLExecutor().cast::<ddl::ExecutorForTest>();
    testCreateSchema(testkit::NewTestKit(store.clone()).Session(), de.clone(), db_info.clone());

    // generate 5 partition in tableInfo.
    // Go 原注释说明这里一次性生成 5 个分区定义，供后续 drop/truncate 复用。
    let (tbl_info, part_ids) = build_table_info_with_partition(store.clone());
    let ctx = testkit::NewTestKit(store.clone()).Session();
    testCreateTable(ctx.clone(), de.clone(), db_info.clone(), tbl_info.clone());
    test_drop_partition(ctx.clone(), de.clone(), db_info.clone(), tbl_info.clone(), vec!["p0".to_string(), "p1".to_string()]);

    let new_ids = genGlobalIDs(store, 2).expect("Go require.NoError: genGlobalIDs");
    test_truncate_partition(ctx, de, db_info, tbl_info, vec![part_ids[3], part_ids[4]], new_ids);
}

// build_table_info_with_partition 对应 Go 的 buildTableInfoWithPartition。
// 它手工组装 model.TableInfo、ColumnInfo 和 5 个 PartitionDefinition，并返回分区 ID 列表。
fn build_table_info_with_partition(store: kv::Storage) -> (model::TableInfo, Vec<i64>) {
    let mut tbl = model::TableInfo {
        Name: ast::NewCIStr("t"),
        ..Default::default()
    };
    tbl.MaxColumnID += 1;
    let col = model::ColumnInfo {
        Name: ast::NewCIStr("c"),
        Offset: 0,
        State: model::StatePublic,
        FieldType: *types::NewFieldType(mysql::TypeLong),
        ID: tbl.MaxColumnID,
        ..Default::default()
    };

    let gen_ids = genGlobalIDs(store.clone(), 1).expect("Go require.NoError: genGlobalIDs table");
    tbl.ID = gen_ids[0];
    tbl.Columns = vec![col];
    tbl.Charset = "utf8".to_string();
    tbl.Collate = "utf8_bin".to_string();

    let part_ids = genGlobalIDs(store, 5).expect("Go require.NoError: genGlobalIDs partitions");
    let part_info = model::PartitionInfo {
        Type: ast::PartitionTypeRange,
        Expr: tbl.Columns[0].Name.L.clone(),
        Enable: true,
        Definitions: vec![
            model::PartitionDefinition { ID: part_ids[0], Name: ast::NewCIStr("p0"), LessThan: vec!["100".to_string()], ..Default::default() },
            model::PartitionDefinition { ID: part_ids[1], Name: ast::NewCIStr("p1"), LessThan: vec!["200".to_string()], ..Default::default() },
            model::PartitionDefinition { ID: part_ids[2], Name: ast::NewCIStr("p2"), LessThan: vec!["300".to_string()], ..Default::default() },
            model::PartitionDefinition { ID: part_ids[3], Name: ast::NewCIStr("p3"), LessThan: vec!["400".to_string()], ..Default::default() },
            model::PartitionDefinition { ID: part_ids[4], Name: ast::NewCIStr("p4"), LessThan: vec!["500".to_string()], ..Default::default() },
        ],
        ..Default::default()
    };
    tbl.Partition = Some(part_info);
    (tbl, part_ids)
}

// build_drop_partition_job 对应 Go 的 buildDropPartitionJob。
// 返回值同时包含 DDL job 与 TablePartitionArgs，保持 Go 调用 DoDDLJobWrapper 的参数形状。
fn build_drop_partition_job(
    db_info: &model::DBInfo,
    tbl_info: &model::TableInfo,
    part_names: Vec<String>,
) -> (model::Job, model::TablePartitionArgs) {
    (
        model::Job {
            Version: model::GetJobVerInUse(),
            SchemaID: db_info.ID,
            SchemaName: db_info.Name.L.clone(),
            TableID: tbl_info.ID,
            TableName: tbl_info.Name.L.clone(),
            SchemaState: model::StatePublic,
            Type: model::ActionDropTablePartition,
            BinlogInfo: Some(model::HistoryInfo::default()),
            ..Default::default()
        },
        model::TablePartitionArgs { PartNames: part_names, ..Default::default() },
    )
}

// test_drop_partition 对应 Go 的 testDropPartition。
// 它设置 QueryString 为 skip，提交 drop partition job，再读取 history 校验。
fn test_drop_partition(
    ctx: sessionctx::Context,
    d: ddl::ExecutorForTest,
    db_info: model::DBInfo,
    tbl_info: model::TableInfo,
    part_names: Vec<String>,
) -> model::Job {
    let (mut job, args) = build_drop_partition_job(&db_info, &tbl_info, part_names);
    ctx.SetValue(sessionctx::QueryString, "skip");
    d.DoDDLJobWrapper(ctx.clone(), ddl::NewJobWrapperWithArgs(job.clone(), args, true))
        .expect("Go require.NoError: DoDDLJobWrapper drop partition");
    checkJobWithHistory(ctx, job.ID, None, tbl_info);
    job
}

// build_truncate_partition_job 对应 Go 的 buildTruncatePartitionJob。
// OldPartitionIDs 和 NewPartitionIDs 的对应关系由调用方保持，不重新推导。
fn build_truncate_partition_job(
    db_info: &model::DBInfo,
    tbl_info: &model::TableInfo,
    pids: Vec<i64>,
    new_ids: Vec<i64>,
) -> (model::Job, model::TruncateTableArgs) {
    (
        model::Job {
            Version: model::GetJobVerInUse(),
            SchemaID: db_info.ID,
            SchemaName: db_info.Name.L.clone(),
            TableID: tbl_info.ID,
            TableName: tbl_info.Name.L.clone(),
            Type: model::ActionTruncateTablePartition,
            SchemaState: model::StatePublic,
            BinlogInfo: Some(model::HistoryInfo::default()),
            ..Default::default()
        },
        model::TruncateTableArgs { OldPartitionIDs: pids, NewPartitionIDs: new_ids, ..Default::default() },
    )
}

// test_truncate_partition 对应 Go 的 testTruncatePartition。
// 与 drop 分支相同，它通过 DoDDLJobWrapper 执行 job 并检查 history。
fn test_truncate_partition(
    ctx: sessionctx::Context,
    d: ddl::ExecutorForTest,
    db_info: model::DBInfo,
    tbl_info: model::TableInfo,
    pids: Vec<i64>,
    new_ids: Vec<i64>,
) -> model::Job {
    let (mut job, args) = build_truncate_partition_job(&db_info, &tbl_info, pids, new_ids);
    ctx.SetValue(sessionctx::QueryString, "skip");
    d.DoDDLJobWrapper(ctx.clone(), ddl::NewJobWrapperWithArgs(job.clone(), args, true))
        .expect("Go require.NoError: DoDDLJobWrapper truncate partition");
    checkJobWithHistory(ctx, job.ID, None, tbl_info);
    job
}

// test_reorganize_partition_rollback 对应 Go 的 TestReorganizePartitionRollback。
// 它复现 issue 42448：在 WriteReorganization failpoint 处暂停 reorganize partition，
// 取消 DDL 后继续执行，最终确认 rollback done 且表元信息恢复。
#[test]
fn test_reorganize_partition_rollback() {
    let (store, domain) = testkit::CreateMockStoreAndDomain();
    let tk = testkit::NewTestKit(store.clone());
    tk.MustExec("use test");
    tk.MustExec("CREATE TABLE `t1` (\n    `id` bigint(20) NOT NULL AUTO_INCREMENT,\n    `k` int(11) NOT NULL DEFAULT '0',\n    `c` char(120) NOT NULL DEFAULT '',\n    `pad` char(60) NOT NULL DEFAULT '',\n    PRIMARY KEY (`id`) /*T![clustered_index] CLUSTERED */
,\n    KEY `k_1` (`k`)\n  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n  PARTITION BY RANGE (`id`)\n  (PARTITION `p0` VALUES LESS THAN (2000000),\n   PARTITION `p1` VALUES LESS THAN (4000000),\n   PARTITION `p2` VALUES LESS THAN (6000000),\n   PARTITION `p3` VALUES LESS THAN (8000000),\n   PARTITION `p4` VALUES LESS THAN (10000000),\n   PARTITION `p5` VALUES LESS THAN (MAXVALUE))");
    tk.MustExec("insert into t1(k, c, pad) values (1, 'a', 'beijing'), (2, 'b', 'chengdu')");

    let (wait_tx, wait_rx) = channel::<()>();
    let (ddl_done_tx, ddl_done_rx) = channel::<Result<(), errors::Error>>();
    // Go defer close(wait) / close(ddlDone) 表示测试结束时关闭 channel；不承担真实并发资源释放。
    testfailpoint::EnableCall("github.com/pingcap/tidb/pkg/ddl/afterRunOneJobStep", move |job: model::Job| {
        if job.Type == model::ActionReorganizePartition && job.SchemaState == model::StateWriteReorganization {
            // Go failpoint 连续等待两次，用于先让主线程取消 job，再继续 DDL。
            wait_rx.recv().unwrap();
            wait_rx.recv().unwrap();
        }
    });

    go(move || {
        let tk2 = testkit::NewTestKit(store.clone());
        tk2.MustExec("use test");
        let err = tk2.ExecToErr("alter table t1 reorganize partition p0, p1, p2, p3, p4 into( partition pnew values less than (10000000))");
        ddl_done_tx.send(err).unwrap();
    });

    let mut job_id = String::new();

    // wait DDL job reaches hook and then cancel
    // 第一次 select 等 failpoint 到达后读取 JOB_ID 并取消 DDL。
    select! {
        _ = wait_tx.send(()) => {
            let rows = tk.MustQuery("admin show ddl jobs where JOB_TYPE='alter table reorganize partition'").Rows();
            assert_eq!(1, rows.len());
            job_id = rows[0][0].to_string();
            tk.MustExec(format!("admin cancel ddl jobs {}", job_id));
        }
        _ = time::After(time::Minute) => panic!("timeout"),
    }

    // continue to run DDL
    // 第二次 select 释放 failpoint，让后台 DDL 进入取消/回滚流程。
    select! {
        _ = wait_tx.send(()) => {}
        _ = time::After(time::Minute) => panic!("timeout"),
    }

    // wait ddl done
    select! {
        err = ddl_done_rx.recv() => assert!(err.unwrap().is_err()),
        _ = time::After(time::Minute) => panic!("wait ddl cancelled timeout"),
    }

    // check job rollback finished
    let rows = tk.MustQuery(format!("admin show ddl jobs where JOB_ID={}", job_id)).Rows();
    assert_eq!(1, rows.len());
    assert_eq!("rollback done", rows[0][rows[0].len() - 2]);

    // check table meta after rollback
tk.MustQuery("show create table t1").Check(testkit::Rows("t1 CREATE TABLE `t1` (\n  `id` bigint(20) NOT NULL AUTO_INCREMENT,\n  `k` int(11) NOT NULL DEFAULT '0',\n  `c` char(120) NOT NULL DEFAULT '',\n  `pad` char(60) NOT NULL DEFAULT '',\n  PRIMARY KEY (`id`) /*T![clustered_index] CLUSTERED */
,\n  KEY `k_1` (`k`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin AUTO_INCREMENT=5001\nPARTITION BY RANGE (`id`)\n(PARTITION `p0` VALUES LESS THAN (2000000),\n PARTITION `p1` VALUES LESS THAN (4000000),\n PARTITION `p2` VALUES LESS THAN (6000000),\n PARTITION `p3` VALUES LESS THAN (8000000),\n PARTITION `p4` VALUES LESS THAN (10000000),\n PARTITION `p5` VALUES LESS THAN (MAXVALUE))"));
    let tbl = domain.InfoSchema().TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr("t1"))
        .expect("Go require.NoError: TableByName");
    assert!(tbl.Meta().Partition.is_some());
    assert!(tbl.Meta().Partition.as_ref().unwrap().AddingDefinitions.is_none());
    assert!(tbl.Meta().Partition.as_ref().unwrap().DroppingDefinitions.is_none());

    // test then add index should success
    tk.MustExec("alter table t1 add index idx_kc (k, c)");
}

// test_update_during_add_column 对应 Go 的 TestUpdateDuringAddColumn。
// 它在 schema synced failpoint 的 WriteOnly 阶段执行多表 update，确认新增列默认值仍正确补齐。
#[test]
fn test_update_during_add_column() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(store.clone());
    tk.MustExec("use test");
    tk.MustExec("create table t1 (c1 int, c2 int) partition by hash (c1) partitions 16");
    tk.MustExec("insert t1 values (1, 1), (2, 2)");
    tk.MustExec("create table t2 (c1 int, c2 int) partition by hash (c1) partitions 16");
    tk.MustExec("insert t2 values (1, 3), (2, 5)");
    let tk2 = testkit::NewTestKit(store);
    tk2.MustExec("use test");

    testfailpoint::EnableCall("github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", move |job: model::Job| {
        if job.SchemaState == model::StateWriteOnly {
            tk2.MustExec("update t1, t2 set t1.c1 = 8, t2.c2 = 10 where t1.c2 = t2.c1");
            tk2.MustQuery("select * from t1").Sort().Check(testkit::Rows("8 1", "8 2"));
            tk2.MustQuery("select * from t2").Sort().Check(testkit::Rows("1 10", "2 10"));
        }
    });

    tk.MustExec("alter table t1 add column c3 bigint default 9");
    tk.MustQuery("select * from t1").Sort().Check(testkit::Rows("8 1 9", "8 2 9"));
}

// test_exchange_partition_multi_column 对应 Go 的 TestExchangePartitionMultiColumn。
// 它验证 range columns 多列分区在 exchange partition 时按多列边界校验数据。
#[test]
fn test_exchange_partition_multi_column() {
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("CREATE TABLE t (a1 int(11) not null,a2 int(11) not null,a3 date default null, primary key (`a1`,`a2`)) partition by range columns(`a1`,`a2`)(partition `p10` values less than (10,10),partition `p20` values less than (20,20),partition `pmax` values less than (maxvalue,maxvalue))");
    tk.MustExec("insert into t values(5,10,null),(10,4,null)");
    tk.MustExec("CREATE TABLE t_np (a1 int(11) not null,a2 int(11) not null,a3 date default null, primary key (`a1`,`a2`))");
    tk.MustExec("insert into t_np values(10,4,null),(4,10,null)");
    tk.MustExec("alter table t exchange partition p10 with table t_np");
}
*/

use crate::executor::{
    ColumnInfo, Executor, ExecutorError, Ident, MemoryJobBackend, OnExist, PartitionDefinition,
    SessionContext, TableInfo,
};
use std::time::Duration;

use crate::partition::{
    PartitionDefinition as ModelPartitionDefinition, PartitionInfo, PartitionType, PartitionValue,
    build_check_condition_for_list, build_check_condition_for_range,
};

fn model_partition(
    name: &str,
    less_than: Vec<PartitionValue>,
    in_values: Vec<Vec<PartitionValue>>,
) -> ModelPartitionDefinition {
    ModelPartitionDefinition {
        name: name.to_owned(),
        less_than,
        in_values,
        ..ModelPartitionDefinition::default()
    }
}

#[test]
fn exchange_range_validation_selects_rows_outside_the_partition_like_go() {
    let info = PartitionInfo {
        partition_type: PartitionType::Range,
        expression: "a".to_owned(),
        definitions: vec![
            model_partition("p0", vec![PartitionValue::Int(10)], vec![]),
            model_partition("p1", vec![PartitionValue::Int(20)], vec![]),
            model_partition("pmax", vec![PartitionValue::MaxValue], vec![]),
        ],
        ..PartitionInfo::default()
    };

    assert_eq!(
        ("a >= ?".to_owned(), vec![PartitionValue::Int(10)]),
        build_check_condition_for_range(&info, 0).unwrap()
    );
    assert_eq!(
        (
            "a < ? OR a >= ? OR a IS NULL".to_owned(),
            vec![PartitionValue::Int(10), PartitionValue::Int(20)],
        ),
        build_check_condition_for_range(&info, 1).unwrap()
    );
    assert_eq!(
        (
            "a < ? OR a IS NULL".to_owned(),
            vec![PartitionValue::Int(20)],
        ),
        build_check_condition_for_range(&info, 2).unwrap()
    );
}

#[test]
fn exchange_list_validation_uses_null_safe_non_membership_like_go() {
    let info = PartitionInfo {
        partition_type: PartitionType::List,
        expression: "a".to_owned(),
        definitions: vec![model_partition(
            "p0",
            vec![],
            vec![vec![PartitionValue::Int(1)], vec![PartitionValue::Null]],
        )],
        ..PartitionInfo::default()
    };

    assert_eq!(
        "NOT ((a) <=> 1 OR (a) <=> NULL)",
        build_check_condition_for_list(&info, 0).unwrap()
    );
}

/// 加分区后禁止一次删光全部；删掉尾分区后再与普通表 exchange，应成功。
#[test]
fn partition_add_drop_and_exchange_preserve_physical_ids() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("p", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("normal", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    let partitioned = Ident::new("test", "p");
    ddl.add_partitions(
        &mut session,
        &partitioned,
        vec![
            PartitionDefinition::new("p0", vec!["10".into()]),
            PartitionDefinition::new("p1", vec!["MAXVALUE".into()]),
        ],
    )
    .unwrap();
    assert!(matches!(
        ddl.drop_partitions(&mut session, &partitioned, &["p0".into(), "p1".into()]),
        Err(ExecutorError::InvalidPartition(_))
    ));
    let removed = ddl
        .drop_partitions(&mut session, &partitioned, &["p1".into()])
        .unwrap();
    assert_eq!(1, removed.len());
    ddl.exchange_partition(
        &mut session,
        &partitioned,
        "p0",
        &Ident::new("test", "normal"),
    )
    .unwrap();
}
