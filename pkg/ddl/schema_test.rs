// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// SchemaCatalog 单元测试。
//
// 覆盖建库、修改字符集/Placement、分阶段删库与恢复，以及字符集校验与
// 物理 ID 收集辅助函数。文件中大块注释保留了对应 Go 集成测试的迁移草稿。

/*

#![allow(dead_code, non_snake_case, non_upper_case_globals, unused_variables)]

// schema DDL 测试中创建/删除库表、DDL owner 等待、历史 job 检查，以及 rename table 与 auto id 的并发场景。
// 主要 helper 和测试函数按 Go 文件顺序保留；事务、goroutine/channel、defer 资源收尾、轮询等待和错误断言在附近补中文说明，方便人工按 Go 源回看。

// test_create_table 对应 Go 的 testCreateTable：构造 create table job，通过 ExecutorForTest 提交，并校验 history job。
pub fn test_create_table(t: &testing::T, ctx: sessionctx::Context, d: ddl::ExecutorForTest, db_info: &model::DBInfo, tbl_info: &mut model::TableInfo) -> model::Job {
    let mut job = model::Job {
        Version: model::GetJobVerInUse(),
        SchemaID: db_info.ID,
        SchemaName: db_info.Name.L.clone(),
        TableID: tbl_info.ID,
        TableName: tbl_info.Name.L.clone(),
        Type: model::ActionCreateTable,
        BinlogInfo: Some(model::HistoryInfo::default()),
        ..Default::default()
    };
    let args = model::CreateTableArgs { TableInfo: tbl_info.clone() };
    ctx.SetValue(sessionctx::QueryString, "skip");
    let err = d.DoDDLJobWrapper(ctx.clone(), ddl::NewJobWrapperWithArgs(&mut job, args, true));
    require::NoError(t, err);

    // Go 测试临时把 TableInfo 标成 public 以便与 history schema 做等值校验，随后恢复为 none。
    tbl_info.State = model::StatePublic;
    checkJobWithHistory(t, ctx, job.ID, None, Some(tbl_info));
    tbl_info.State = model::StateNone;
    job
}
*/

use crate::schema::{SchemaCatalog, SchemaError, SchemaState, SchemaTable, schema_physical_ids};

#[test]
/// 覆盖建库（含 IF NOT EXISTS）、改字符集与 Placement、分阶段删库再恢复。
fn schema_create_modify_drop_and_recover_preserve_identity() {
    let mut catalog = SchemaCatalog::default();
    let id = catalog
        .create_schema("Test", "utf8mb4", "utf8mb4_bin", None, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        None,
        catalog
            .create_schema("test", "utf8mb4", "utf8mb4_bin", None, true)
            .unwrap()
    );
    assert_eq!(
        Err(SchemaError::AlreadyExists),
        catalog.create_schema("TEST", "utf8mb4", "utf8mb4_bin", None, false)
    );

    // 修改默认字符集/排序规则与 Placement Policy。
    assert!(
        catalog
            .modify_charset_and_collation("test", "latin1", "latin1_bin")
            .unwrap()
    );
    assert!(
        catalog
            .modify_placement("test", Some("regional".into()))
            .unwrap()
    );
    assert_eq!("latin1", catalog.schema("test").unwrap().charset);

    assert_eq!(
        SchemaState::WriteOnly,
        catalog.drop_schema_step("test").unwrap()
    );
    // Public → WriteOnly → DeleteOnly → None，再按 ID 恢复。
    assert_eq!(
        SchemaState::DeleteOnly,
        catalog.drop_schema_step("test").unwrap()
    );
    assert_eq!(SchemaState::None, catalog.drop_schema_step("test").unwrap());
    assert!(catalog.schema("test").is_none());
    catalog.recover_schema(id).unwrap();
    assert_eq!(id, catalog.schema("test").unwrap().id);
    assert_eq!(SchemaState::Public, catalog.schema("test").unwrap().state);
}

#[test]
/// 校验非法字符集/空 Placement，以及 schema_physical_ids 收集表与分区 ID。
fn schema_validation_and_physical_ids_match_go_helpers() {
    let mut catalog = SchemaCatalog::default();
    assert_eq!(
        Err(SchemaError::InvalidCharsetCollation),
        catalog.create_schema("bad", "utf8", "latin1_bin", None, false)
    );
    assert_eq!(
        Err(SchemaError::InvalidPlacementPolicy),
        catalog.create_schema("bad", "utf8", "utf8_bin", Some(" ".into()), false)
    );
    assert_eq!(
        // 表 10 带分区 11/12，表 20 无分区：期望 [10,11,12,20]。
        vec![10, 11, 12, 20],
        schema_physical_ids(&[
            SchemaTable {
                id: 10,
                partition_ids: vec![11, 12]
            },
            SchemaTable {
                id: 20,
                partition_ids: vec![]
            },
        ])
    );
}

#[test]
fn schema_modify_reports_missing_schema_before_invalid_new_values_like_go() {
    let mut catalog = SchemaCatalog::default();

    assert_eq!(
        Err(SchemaError::NotFound),
        catalog.modify_charset_and_collation("missing", "unknown", "also_unknown")
    );
    assert_eq!(
        Err(SchemaError::NotFound),
        catalog.modify_placement("missing", Some(" ".into()))
    );
}

/*
// test_check_table_state 对应 Go 的 testCheckTableState：在新的内部 DDL 事务中读取 meta table 并校验状态。
pub fn test_check_table_state(t: &testing::T, store: kv::Storage, db_info: &model::DBInfo, tbl_info: &model::TableInfo, state: model::SchemaState) {
    let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnDDL);
    let err = kv::RunInNewTxn(ctx, store, false, |txn: kv::Transaction| {
        let mut m = meta::NewMutator(txn);
        let (info, err) = m.GetTable(db_info.ID, tbl_info.ID);
        require::NoError(t, err);
        if state == model::StateNone {
            // Go 源这里仅确认读取不报错后返回，保留 StateNone 分支的短路语义。
            return Ok(());
        }
        require::Equal(t, info.Name, tbl_info.Name.clone());
        require::Equal(t, info.State, state);
        Ok(())
    });
    require::NoError(t, err);
}

// test_table_info 对应 Go 的 testTableInfo：生成一个包含 num 个 int 列且无索引的 TableInfo fixture。
pub fn test_table_info(store: kv::Storage, name: &str, num: i32) -> Result<model::TableInfo, errors::Error> {
    let mut tbl_info = model::TableInfo { Name: ast::NewCIStr(name), ..Default::default() };
    let gen_ids = gen_global_ids(store, 1)?;
    tbl_info.ID = gen_ids[0];

    let mut cols = Vec::with_capacity(num as usize);
    for i in 0..num {
        let mut col = model::ColumnInfo {
            Name: ast::NewCIStr(format!("c{}", i + 1)),
            Offset: i,
            DefaultValue: AnyValue::from(i + 1),
            State: model::StatePublic,
            ..Default::default()
        };
        col.FieldType = *types::NewFieldType(mysql::TypeLong);
        tbl_info.MaxColumnID += 1;
        col.ID = tbl_info.MaxColumnID;
        cols.push(col);
    }
    tbl_info.Columns = cols;
    tbl_info.Charset = "utf8".to_string();
    tbl_info.Collate = "utf8_bin".to_string();
    Ok(tbl_info)
}

// gen_global_ids 对应 Go 的 genGlobalIDs：在内部 DDL 事务中向 meta 申请全局 ID。
pub fn gen_global_ids(store: kv::Storage, count: i32) -> Result<Vec<i64>, errors::Error> {
    let mut ret = Vec::new();
    let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnDDL);
    let err = kv::RunInNewTxn(ctx, store, false, |txn: kv::Transaction| {
        let mut m = meta::NewMutator(txn);
        ret = m.GenGlobalIDs(count)?;
        Ok(())
    });
    err.map(|_| ret)
}

// test_schema_info 对应 Go 的 testSchemaInfo：为测试数据库生成 DBInfo 与全局 schema ID。
pub fn test_schema_info(store: kv::Storage, name: &str) -> Result<model::DBInfo, errors::Error> {
    let mut db_info = model::DBInfo { Name: ast::NewCIStr(name), ..Default::default() };
    let gen_ids = gen_global_ids(store, 1)?;
    db_info.ID = gen_ids[0];
    Ok(db_info)
}

// test_create_schema 对应 Go 的 testCreateSchema：提交 create schema job 并校验 history 中的 public 状态。
pub fn test_create_schema(t: &testing::T, ctx: sessionctx::Context, d: ddl::ExecutorForTest, db_info: &mut model::DBInfo) -> model::Job {
    let mut job = model::Job {
        Version: model::GetJobVerInUse(),
        SchemaID: db_info.ID,
        Type: model::ActionCreateSchema,
        BinlogInfo: Some(model::HistoryInfo::default()),
        InvolvingSchemaInfo: vec![model::InvolvingSchemaInfo { Database: db_info.Name.L.clone(), Table: model::InvolvingAll }],
        ..Default::default()
    };
    ctx.SetValue(sessionctx::QueryString, "skip");
    let err = d.DoDDLJobWrapper(ctx.clone(), ddl::NewJobWrapperWithArgs(&mut job, model::CreateSchemaArgs { DBInfo: db_info.clone() }, true));
    require::NoError(t, err);

    db_info.State = model::StatePublic;
    checkJobWithHistory(t, ctx, job.ID, Some(db_info), None);
    db_info.State = model::StateNone;
    job
}

// build_drop_schema_job 对应 Go 的 buildDropSchemaJob：只组装 drop schema job，不提交。
pub fn build_drop_schema_job(db_info: &model::DBInfo) -> model::Job {
    model::Job {
        Version: model::GetJobVerInUse(),
        SchemaID: db_info.ID,
        Type: model::ActionDropSchema,
        BinlogInfo: Some(model::HistoryInfo::default()),
        InvolvingSchemaInfo: vec![model::InvolvingSchemaInfo { Database: db_info.Name.L.clone(), Table: model::InvolvingAll }],
        ..Default::default()
    }
}

// test_drop_schema 对应 Go 的 testDropSchema：提交 drop schema job，并打开外键检查参数。
pub fn test_drop_schema(t: &testing::T, ctx: sessionctx::Context, d: ddl::ExecutorForTest, db_info: &model::DBInfo) -> model::Job {
    let mut job = build_drop_schema_job(db_info);
    ctx.SetValue(sessionctx::QueryString, "skip");
    let err = d.DoDDLJobWrapper(ctx, ddl::NewJobWrapperWithArgs(&mut job, model::DropSchemaArgs { FKCheck: true }, true));
    require::NoError(t, err);
    job
}

// is_ddl_job_done 对应 Go 的 isDDLJobDone：查询 mysql.tidb_ddl_job；有残留 job 时按 testLease 睡眠后继续轮询。
pub fn is_ddl_job_done(test: &testing::T, _m: &meta::Mutator, store: kv::Storage) -> bool {
    let tk = testkit::NewTestKit(test, store);
    let rows = tk.MustQuery("select * from mysql.tidb_ddl_job").Rows();
    if rows.is_empty() {
        return true;
    }
    time::Sleep(testLease);
    false
}

// test_check_schema_state 对应 Go 的 testCheckSchemaState：循环读取 DBInfo，直到 drop schema job 完成或状态匹配。
pub fn test_check_schema_state(test: &testing::T, store: kv::Storage, db_info: &model::DBInfo, state: model::SchemaState) {
    let mut is_dropped = true;
    let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnDDL);
    loop {
        let err = kv::RunInNewTxn(ctx.clone(), store.clone(), false, |txn: kv::Transaction| {
            let mut m = meta::NewMutator(txn);
            let (info, err) = m.GetDatabase(db_info.ID);
            require::NoError(test, err);
            if state == model::StateNone {
                // Drop schema 需要等待 DDL job 表清空；未完成时本轮事务直接返回，外层继续轮询。
                is_dropped = is_ddl_job_done(test, &m, store.clone());
                if !is_dropped { return Ok(()); }
                require::Nil(test, info);
                return Ok(());
            }
            require::Equal(test, info.Name, db_info.Name.clone());
            require::Equal(test, info.State, state);
            Ok(())
        });
        require::NoError(test, err);
        if is_dropped { break; }
    }
}

// test_schema 对应 Go 的 TestSchema：覆盖创建库、创建两张表并写入数据、drop schema、drop 不存在库和 drop 空库。
#[test]
fn test_schema() {
    let t = testing::T::current();
    let (store, domain) = testkit::CreateMockStoreAndDomainWithSchemaLease(&t, testLease);
    let mut db_info = test_schema_info(store.clone(), "test_schema").unwrap();

    // 创建 database 后用 meta 状态和 DDL job 表双重确认 job 已完成。
    let tk = testkit::NewTestKit(&t, store.clone());
    let de = domain.DDLExecutor().as_executor_for_test();
    let mut job = test_create_schema(&t, tk.Session(), de.clone(), &mut db_info);
    test_check_schema_state(&t, store.clone(), &db_info, model::StatePublic);
    testCheckJobDone(&t, store.clone(), job.ID, true);

    // 创建第一张表并通过 table.AddRecord 写入 100 行；事务对象来自测试 session。
    let mut tbl_info1 = test_table_info(store.clone(), "t", 3).unwrap();
    let t_job1 = test_create_table(&t, tk.Session(), de.clone(), &db_info, &mut tbl_info1);
    test_check_table_state(&t, store.clone(), &db_info, &tbl_info1, model::StatePublic);
    testCheckJobDone(&t, store.clone(), t_job1.ID, true);
    let tbl1 = testGetTable(&t, domain.clone(), tbl_info1.ID);
    let mut txn = newTxn(tk.Session()).unwrap();
    for i in 1..=100 {
        let err = tbl1.AddRecord(tk.Session().GetTableCtx(), &mut txn, types::MakeDatums(vec![i, i, i]));
        require::NoError(&t, err);
    }

    // 第二张表使用另一个 TestKit session，原 Go 写入 1034 行来覆盖 drop schema 多表清理。
    let mut tbl_info2 = test_table_info(store.clone(), "t1", 3).unwrap();
    let tk2 = testkit::NewTestKit(&t, store.clone());
    let t_job2 = test_create_table(&t, tk2.Session(), de.clone(), &db_info, &mut tbl_info2);
    test_check_table_state(&t, store.clone(), &db_info, &tbl_info2, model::StatePublic);
    testCheckJobDone(&t, store.clone(), t_job2.ID, true);
    let tbl2 = testGetTable(&t, domain.clone(), tbl_info2.ID);
    txn = newTxn(tk.Session()).unwrap();
    for i in 1..=1034 {
        let err = tbl2.AddRecord(tk2.Session().GetTableCtx(), &mut txn, types::MakeDatums(vec![i, i, i]));
        require::NoError(&t, err);
    }

    let tk3 = testkit::NewTestKit(&t, store.clone());
    job = test_drop_schema(&t, tk3.Session(), de.clone(), &db_info);
    test_check_schema_state(&t, store.clone(), &db_info, model::StateNone);
    let mut ids = std::collections::HashSet::new();
    ids.insert(tbl_info1.ID);
    ids.insert(tbl_info2.ID);
    checkJobWithHistory(&t, tk3.Session(), job.ID, Some(&db_info), None);

    // Drop 不存在的 database 应返回 ErrDatabaseDropExists，而不是成功清理。
    let mut missing_job = model::Job { Version: model::JobVersion1, SchemaID: db_info.ID, SchemaName: "test_schema".to_string(), Type: model::ActionDropSchema, BinlogInfo: Some(model::HistoryInfo::default()), ..Default::default() };
    let ctx = testkit::NewTestKit(&t, store.clone()).Session();
    ctx.SetValue(sessionctx::QueryString, "skip");
    let err = de.DoDDLJobWrapper(ctx.clone(), ddl::NewJobWrapperWithArgs(&mut missing_job, model::DropSchemaArgs::default(), true));
    require::True(&t, terror::ErrorEqual(err, infoschema::ErrDatabaseDropExists));

    // Drop 空库覆盖无 table ID 清理路径，Go 源期望 testCheckJobDone 的第三个参数为 false。
    let mut db_info1 = test_schema_info(store.clone(), "test1").unwrap();
    job = test_create_schema(&t, ctx.clone(), de.clone(), &mut db_info1);
    test_check_schema_state(&t, store.clone(), &db_info1, model::StatePublic);
    testCheckJobDone(&t, store.clone(), job.ID, true);
    job = test_drop_schema(&t, ctx, de, &db_info1);
    test_check_schema_state(&t, store.clone(), &db_info1, model::StateNone);
    testCheckJobDone(&t, store, job.ID, false);
}

// test_schema_wait_job 对应 Go 的 TestSchemaWaitJob：启动第二个 DDL，退休 owner 后验证非 owner 提交 job 会取消。
#[test]
fn test_schema_wait_job() {
    let t = testing::T::current();
    let (store, domain) = testkit::CreateMockStoreAndDomainWithSchemaLease(&t, testLease);
    require::True(&t, domain.DDL().OwnerManager().IsOwner());

    let (mut d2, de2) = ddl::NewDDL(
        context::Background(),
        ddl::WithEtcdClient(domain.GetEtcdClient()),
        ddl::WithStore(store.clone()),
        ddl::WithInfoCache(domain.InfoCache()),
        ddl::WithLease(testLease),
        ddl::WithSchemaLoader(domain.clone()),
    );
    let det2 = de2.as_executor_for_test();
    let err = d2.Start(ddl::Normal, pools::NewResourcePool(|| {
        let session = testkit::NewTestKit(&t, store.clone()).Session();
        session.GetSessionVars().CommonGlobalLoaded = true;
        Ok(session)
    }, 20, 20, 5));
    require::NoError(&t, err);
    // Go defer 确保 d2.Stop 被调用；保留 DDL worker 资源收尾点。
    defer!({ require::NoError(&t, d2.Stop()); });

    d2.OwnerManager().RetireOwner();
    time::Sleep(time::Second);
    let mut db_info = test_schema_info(store.clone(), "test_schema").unwrap();
    let se = testkit::NewTestKit(&t, store.clone()).Session();
    test_create_schema(&t, se, det2.clone(), &mut db_info);
    test_check_schema_state(&t, store.clone(), &db_info, model::StatePublic);
    require::False(&t, d2.OwnerManager().IsOwner());

    let schema_id = gen_global_ids(store.clone(), 1).unwrap()[0];
    do_ddl_job_err(&t, schema_id, 0, "test_schema", "", model::ActionCreateSchema,
        testkit::NewTestKit(&t, store.clone()).Session(), det2, store,
        |job| model::CreateSchemaArgs { DBInfo: db_info.clone() });
}

// do_ddl_job_err 对应 Go 的 doDDLJobErr：提交预期失败的 DDL job，并检查 history job 已取消或回滚完成。
pub fn do_ddl_job_err<F>(t: &testing::T, schema_id: i64, table_id: i64, schema_name: &str, table_name: &str, tp: model::ActionType, ctx: sessionctx::Context, d: ddl::ExecutorForTest, store: kv::Storage, handler: F) -> model::Job
where F: Fn(&mut model::Job) -> model::JobArgs {
    let mut job = model::Job { Version: model::GetJobVerInUse(), SchemaID: schema_id, SchemaName: schema_name.to_string(), TableID: table_id, TableName: table_name.to_string(), Type: tp, BinlogInfo: Some(model::HistoryInfo::default()), ..Default::default() };
    let args = handler(&mut job);
    // Go 源 TODO 标出错误细节未校验；这里同样只保留“必须报错并取消 job”的断言。
    ctx.SetValue(sessionctx::QueryString, "skip");
    require::Error(t, d.DoDDLJobWrapper(ctx, ddl::NewJobWrapperWithArgs(&mut job, args, true)));
    test_check_job_cancelled(t, store, &job, None);
    job
}

// test_check_job_cancelled 对应 Go 的 testCheckJobCancelled：从 history job 读取取消/回滚状态，可选校验 SchemaState。
pub fn test_check_job_cancelled(t: &testing::T, store: kv::Storage, job: &model::Job, state: Option<model::SchemaState>) {
    let se = testkit::NewTestKit(t, store).Session();
    let (history_job, err) = ddl::GetHistoryJobByID(se, job.ID);
    require::NoError(t, err);
    require::True(t, history_job.IsCancelled() || history_job.IsRollbackDone());
    if let Some(expected) = state {
        require::Equal(t, history_job.SchemaState, expected);
    }
}

// test_rename_table_auto_ids 对应 Go 的 TestRenameTableAutoIDs：用多个 session 与一个 rename goroutine 覆盖 auto id 分配兼容性。
#[test]
fn test_rename_table_auto_ids() {
    let t = testing::T::current();
    let (store, dom) = testkit::CreateMockStoreAndDomain(&t);
    let tk1 = testkit::NewTestKit(&t, store.clone());
    let tk2 = testkit::NewTestKit(&t, store.clone());
    let tk3 = testkit::NewTestKit(&t, store.clone());
    let tk4 = testkit::NewTestKit(&t, store.clone());
    let db_name = "RenameTableAutoIDs";
    tk1.MustExec(format!("create schema {}", db_name));
    tk1.MustExec(format!("create schema {}2", db_name));
    tk1.MustExec(format!("use {}", db_name));
    tk2.MustExec(format!("use {}", db_name));
    tk3.MustExec(format!("use {}", db_name));
    tk1.MustExec("CREATE TABLE t (a int auto_increment primary key nonclustered, b varchar(255), key (b)) AUTO_ID_CACHE 100");
    tk1.MustExec("insert into t values (11,11),(2,2),(null,12)");
    tk1.MustExec("insert into t values (null,18)");
    tk1.MustQuery("select _tidb_rowid, a, b from t").Sort().Check(testkit::Rows(vec!["13 11 11", "14 2 2", "15 12 12", "17 16 18"]));

    // wait_for 保留 Go 中轮询 admin show ddl jobs 的逻辑；失败时打印 job 列表并短暂 sleep。
    let wait_for = |col: usize, table_name: &str, s: &str| {
        loop {
            let sql = format!("admin show ddl jobs where db_name like '{}%' and table_name like '{}%' and job_type = 'rename table'", strings::ToLower(db_name), table_name);
            let res = tk4.MustQuery(sql).Rows();
            if res.len() == 1 && res[0][col] == s { break; }
            logutil::DDLLogger().Info("Could not find match", zap::String("tableName", table_name), zap::String("s", s), zap::Int("colNum", col as i32));
            for row in res { logutil::DDLLogger().Info("ddl jobs", zap::Strings("jobs", row.to_strings())); }
            time::Sleep(10 * time::Millisecond);
        }
    };

    let alter_chan = channel::make::<errors::Error>();
    tk2.MustExec("set @@session.innodb_lock_wait_timeout = 0");
    tk2.MustExec("BEGIN");
    tk2.MustExec("insert into t values (null, 4)");
    let v1 = dom.InfoSchema().SchemaMetaVersion();

    // Go goroutine 异步执行 rename table；channel 用来在最后接收错误，避免提前吞掉 DDL 失败。
    go!({ alter_chan.send(tk1.ExecToErr(format!("rename table t to {}2.t2", db_name))); });
    wait_for(11, "t", "running");
    wait_for(4, "t", "public");
    require::Eventually(&t, || dom.InfoSchema().SchemaMetaVersion() > v1, time::Minute, 2 * time::Millisecond);

    tk3.MustExec("BEGIN");
    tk3.MustExec(format!("insert into {}2.t2 values (50, 5)", db_name));
    // 保留 Go 注释：infoschema v1->v2 切换期间旧/新 auto id allocator 并存，曾触发 index key 冲突。
    tk2.MustExec("insert into t values (null, 6)");
    tk3.MustExec(format!("insert into {}2.t2 values (20, 5)", db_name));
    tk2.MustExec("insert into t values (null, 6)");
    tk3.MustExec(format!("insert into {}2.t2 values (null, 7)", db_name));
    tk2.MustExec("COMMIT");

    wait_for(11, "t", "done");
    tk2.MustExec("BEGIN");
    tk2.MustExec(format!("insert into {}2.t2 values (null, 8)", db_name));
    tk3.MustExec(format!("insert into {}2.t2 values (null, 9)", db_name));
    tk2.MustExec(format!("insert into {}2.t2 values (null, 10)", db_name));
    tk3.MustExec("COMMIT");
    wait_for(11, "t", "synced");
    tk2.MustExec("COMMIT");

    let expected_rows = testkit::Rows(vec![
        "13 11 11", "14 2 2", "15 12 12", "17 16 18", "19 18 4", "51 50 5", "53 52 6",
        "54 20 5", "56 55 6", "58 57 7", "60 59 8", "62 61 9", "64 63 10",
    ]);
    tk3.MustQuery(format!("select _tidb_rowid, a, b from {}2.t2", db_name)).Sort().Check(expected_rows.clone());
    require::NoError(&t, alter_chan.recv());
    tk2.MustQuery(format!("select _tidb_rowid, a, b from {}2.t2", db_name)).Sort().Check(expected_rows);
}
*/
