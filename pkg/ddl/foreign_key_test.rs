// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 外键（Foreign Key）DDL 测试模块。
//
// 外键是关系数据库中的引用完整性约束：子表（child）的某些列必须引用
// 父表（parent）中已存在的键值。本文件测试 DDL 执行器对外键的
// 增加（add foreign key）与删除（drop foreign key）生命周期管理。
//
// 文件分为两部分：
// - 顶部大段块注释：从 Go(TiDB) 机械迁移过来的原始测试占位代码，
//   覆盖并发 DDL、failpoint 注入、跨库引用等场景，待后续人工接线；
// - 底部活动代码：基于内存 `Executor` 的最小化外键生命周期测试。

/*
// 这里保留的是从 Go 版本机械迁移来的原始测试草稿。
// 当前 crate 还没有把相关依赖全部接上，所以用块注释整体封存，
// 既保留场景信息，也避免在本轮“只加注释”的任务里误改测试语义。
// ddl、testkit、sessionctx、table、failpoint 等依赖均保留为占位调用，方便后续人工逐段接线。
//

// testCreateForeignKey 对应 Go helper：组装 FKInfo 和 AddForeignKey job，
// 开启新事务后通过 DoDDLJobWrapper 执行，返回原始 job 供后续 history 检查。
fn test_create_foreign_key(
    t: testing::T,
    d: ddl::ExecutorForTest,
    ctx: sessionctx::Context,
    db_info: *mut model::DBInfo,
    tbl_info: *mut model::TableInfo,
    fk_name: &str,
    keys: Vec<&str>,
    ref_table: &str,
    ref_keys: Vec<&str>,
    on_delete: ast::ReferOptionType,
    on_update: ast::ReferOptionType,
) -> *mut model::Job {
    let fk_name_ci = ast::NewCIStr(fk_name);
    let mut key_cis: Vec<ast::CIStr> = Vec::with_capacity(keys.len());
    for key in keys {
        key_cis.push(ast::NewCIStr(key));
    }

    let ref_table_ci = ast::NewCIStr(ref_table);
    let mut ref_key_cis: Vec<ast::CIStr> = Vec::with_capacity(ref_keys.len());
    for key in ref_keys {
        ref_key_cis.push(ast::NewCIStr(key));
    }

    let fk_info = model::FKInfo {
        Name: fk_name_ci,
        RefTable: ref_table_ci,
        RefCols: ref_key_cis,
        Cols: key_cis,
        OnDelete: on_delete as i32,
        OnUpdate: on_update as i32,
        State: model::StateNone,
        ..Default::default()
    };

    let job = Box::into_raw(Box::new(model::Job {
        Version: model::GetJobVerInUse(),
        SchemaID: unsafe { (*db_info).ID },
        SchemaName: unsafe { (*db_info).Name.L.clone() },
        TableID: unsafe { (*tbl_info).ID },
        TableName: unsafe { (*tbl_info).Name.L.clone() },
        Type: model::ActionAddForeignKey,
        BinlogInfo: Box::into_raw(Box::new(model::HistoryInfo::default())),
        ..Default::default()
    }));
    let err = sessiontxn::NewTxn(context::Background(), ctx);
    require::NoError(t, err);
    ctx.SetValue(sessionctx::QueryString, "skip");

    let args = model::AddForeignKeyArgs { FkInfo: fk_info };
    let err = d.DoDDLJobWrapper(ctx, ddl::NewJobWrapperWithArgs(job, &args, true));
    require::NoError(t, err);
    job
}

// testDropForeignKey 对应 Go helper：构造 DropForeignKey job 并检查 DDL history。
fn test_drop_foreign_key(
    t: testing::T,
    ctx: sessionctx::Context,
    d: ddl::ExecutorForTest,
    db_info: *mut model::DBInfo,
    tbl_info: *mut model::TableInfo,
    foreign_key_name: &str,
) -> *mut model::Job {
    let job = Box::into_raw(Box::new(model::Job {
        Version: model::GetJobVerInUse(),
        SchemaID: unsafe { (*db_info).ID },
        SchemaName: unsafe { (*db_info).Name.L.clone() },
        TableID: unsafe { (*tbl_info).ID },
        TableName: unsafe { (*tbl_info).Name.L.clone() },
        Type: model::ActionDropForeignKey,
        BinlogInfo: Box::into_raw(Box::new(model::HistoryInfo::default())),
        ..Default::default()
    }));
    ctx.SetValue(sessionctx::QueryString, "skip");
    let args = model::DropForeignKeyArgs { FkName: ast::NewCIStr(foreign_key_name) };
    let err = d.DoDDLJobWrapper(ctx, ddl::NewJobWrapperWithArgs(job, &args, true));
    require::NoError(t, err);
    checkJobWithHistory(t, ctx, unsafe { (*job).ID }, nil, tbl_info);
    job
}

// getForeignKey 对应 Go helper：只返回 Public 状态的外键，名称比较使用小写语义。
fn get_foreign_key(t: table::Table, name: &str) -> Option<*mut model::FKInfo> {
    for fk in t.Meta().ForeignKeys {
        // Go 原测试只允许读到 public foreign key，其他状态在 schema 中应被过滤。
        if unsafe { (*fk).State != model::StatePublic } {
            continue;
        }
        if unsafe { (*fk).Name.L.clone() } == strings::ToLower(name) {
            return Some(fk);
        }
    }
    None
}

// TestForeignKey 对应 Go 的基础 add/drop foreign key 测试。
// 两段 failpoint 分别在 add 完成后确认外键可见、drop 完成后确认外键被移除。
#[test]
fn test_foreign_key() {
    let (store, dom) = testkit::CreateMockStoreAndDomainWithSchemaLease(t, testLease);

    let (db_info, err) = testSchemaInfo(store, "test_foreign");
    require::NoError(t, err);
    let de = dom.DDLExecutor().as_executor_for_test();
    testCreateSchema(t, testkit::NewTestKit(t, store).Session(), de, db_info);
    let (tbl_info, err) = testTableInfo(store, "t", 3);
    require::NoError(t, err);
    unsafe {
        (*tbl_info).Indices.push(model::IndexInfo {
            ID: 1,
            Name: ast::NewCIStr("idx_fk"),
            Table: ast::NewCIStr("t"),
            Columns: vec![model::IndexColumn {
                Name: ast::NewCIStr("c1"),
                Offset: 0,
                Length: types::UnspecifiedLength,
            }],
            State: model::StatePublic,
            ..Default::default()
        });
    }
    testCreateTable(t, testkit::NewTestKit(t, store).Session(), de, db_info, tbl_info);

    let mut mu = sync::Mutex::new(());
    let mut check_ok = false;
    let mut hook_err: Option<errors::Error> = None;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", |job: *mut model::Job| {
        if unsafe { (*job).State != model::JobStateDone } {
            return;
        }
        let _guard = mu.Lock();
        let (tbl, err) = testGetTableWithError(dom, unsafe { (*db_info).ID }, unsafe { (*tbl_info).ID });
        if err != nil {
            hook_err = Some(errors::Trace(err));
            return;
        }
        let fk = get_foreign_key(tbl, "c1_fk");
        if fk.is_none() {
            hook_err = Some(errors::New("foreign key not exists"));
            return;
        }
        check_ok = true;
    });

    let ctx = testkit::NewTestKit(t, store).Session();
    let mut job = test_create_foreign_key(
        t,
        de,
        ctx,
        db_info,
        tbl_info,
        "c1_fk",
        vec!["c1"],
        "t2",
        vec!["c1"],
        ast::ReferOptionCascade,
        ast::ReferOptionSetNull,
    );
    testCheckJobDone(t, store, unsafe { (*job).ID }, true);
    require::NoError(t, err);
    let _guard = mu.Lock();
    require::NoError(t, hook_err);
    require::True(t, check_ok);
    checkJobWithHistory(t, ctx, unsafe { (*job).ID }, nil, tbl_info);

    check_ok = false;
    // 第二次 hook 覆盖 drop FK：job done 后重新读 table，确认同名外键已经不可见。
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", |job: *mut model::Job| {
        if unsafe { (*job).State != model::JobStateDone } {
            return;
        }
        let _guard = mu.Lock();
        let (tbl, err) = testGetTableWithError(dom, unsafe { (*db_info).ID }, unsafe { (*tbl_info).ID });
        if err != nil {
            hook_err = Some(errors::Trace(err));
            return;
        }
        if get_foreign_key(tbl, "c1_fk").is_some() {
            hook_err = Some(errors::New("foreign key has not been dropped"));
            return;
        }
        check_ok = true;
    });

    job = test_drop_foreign_key(t, ctx, de, db_info, tbl_info, "c1_fk");
    testCheckJobDone(t, store, unsafe { (*job).ID }, false);
    require::NoError(t, hook_err);
    require::True(t, check_ok);
    testfailpoint::Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced");

    let tk = testkit::NewTestKit(t, store);
    let job_id = testDropTable(tk, t, unsafe { (*db_info).Name.L.clone() }, unsafe { (*tbl_info).Name.L.clone() }, dom);
    testCheckJobDone(t, store, job_id, false);
    require::NoError(t, err);
}

// TestTruncateOrDropTableWithForeignKeyReferred2 保留 Go 的并发窗口：
// create child table 进入 StateNone 时，另一个 session 尝试 truncate/drop 被引用的 parent。
#[test]
fn test_truncate_or_drop_table_with_foreign_key_referred2() {
    let store = testkit::CreateMockStoreWithSchemaLease(t, testLease);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("set @@global.tidb_enable_foreign_key=1");
    tk.MustExec("set @@foreign_key_checks=1;");
    tk.MustExec("use test");
    let tk2 = testkit::NewTestKit(t, store);
    tk2.MustExec("set @@global.tidb_enable_foreign_key=1");
    tk2.MustExec("set @@foreign_key_checks=1;");
    tk2.MustExec("use test");

    tk.MustExec("create table t1 (id int key, a int);");

    let mut wg = sync::WaitGroup::new();
    let mut truncate_err: Option<errors::Error> = None;
    let mut drop_err: Option<errors::Error> = None;
    let mut test_truncate = true;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", |job: *mut model::Job| {
        if unsafe { (*job).SchemaState != model::StateNone || (*job).Type != model::ActionCreateTable } {
            return;
        }
        wg.Add(1);
        if test_truncate {
            go(|| {
                defer(|| wg.Done());
                truncate_err = Some(tk2.ExecToErr("truncate table t1"));
            });
        } else {
            go(|| {
                defer(|| wg.Done());
                drop_err = Some(tk2.ExecToErr("drop table t1"));
            });
        }
        // Go 通过短暂 sleep 保证 tk2 的 DDL job 已进入队列。
        time::Sleep(time::Millisecond * 100);
    });

    tk.MustExec("create table t2 (a int, b int, foreign key fk(b) references t1(id));");
    wg.Wait();
    require::Error(t, truncate_err);
    require::Equal(t, "[ddl:1701]Cannot truncate a table referenced in a foreign key constraint (`test`.`t2` CONSTRAINT `fk`)", truncate_err.unwrap().Error());

    tk.MustExec("drop table t2");
    test_truncate = false;
    tk.MustExec("create table t2 (a int, b int, foreign key fk(b) references t1(id));");
    wg.Wait();
    require::Error(t, drop_err);
    require::Equal(t, "[ddl:1701]Cannot truncate a table referenced in a foreign key constraint (`test`.`t2` CONSTRAINT `fk`)", drop_err.unwrap().Error());
}

// TestDropIndexNeededInForeignKey2 对应 Go 的并发 drop index 场景。
// 第一个 drop index 进入 public 阶段时，第二个 session 删除另一个被外键依赖的索引并应失败。
#[test]
fn test_drop_index_needed_in_foreign_key2() {
    let store = testkit::CreateMockStoreWithSchemaLease(t, testLease);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("set @@global.tidb_enable_foreign_key=1");
    tk.MustExec("set @@foreign_key_checks=1;");
    tk.MustExec("use test");
    let tk2 = testkit::NewTestKit(t, store);
    tk2.MustExec("set @@global.tidb_enable_foreign_key=1");
    tk2.MustExec("set @@foreign_key_checks=1;");
    tk2.MustExec("use test");
    tk.MustExec("create table t1 (id int key, b int)");
    tk.MustExec("create table t2 (a int, b int, index idx1 (b),index idx2 (b), foreign key (b) references t1(id));");

    let mut wg = sync::WaitGroup::new();
    let mut drop_err: Option<errors::Error> = None;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", |job: *mut model::Job| {
        if unsafe { (*job).SchemaState != model::StatePublic || (*job).Type != model::ActionDropIndex } {
            return;
        }
        wg.Add(1);
        go(|| {
            defer(|| wg.Done());
            drop_err = Some(tk2.ExecToErr("alter table t2 drop index idx2"));
        });
        time::Sleep(time::Millisecond * 100);
    });

    tk.MustExec("alter table t2 drop index idx1");
    wg.Wait();
    require::Error(t, drop_err);
    require::Equal(t, "[ddl:1553]Cannot drop index 'idx2': needed in a foreign key constraint", drop_err.unwrap().Error());
}

// TestDropDatabaseWithForeignKeyReferred2 对应跨库外键引用下 drop database 的错误检查。
#[test]
fn test_drop_database_with_foreign_key_referred2() {
    let store = testkit::CreateMockStoreWithSchemaLease(t, testLease);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("set @@global.tidb_enable_foreign_key=1");
    tk.MustExec("set @@foreign_key_checks=1;");
    tk.MustExec("use test");
    let tk2 = testkit::NewTestKit(t, store);
    tk2.MustExec("set @@global.tidb_enable_foreign_key=1");
    tk2.MustExec("set @@foreign_key_checks=1;");
    tk2.MustExec("use test");
    tk.MustExec("create table t1 (id int key, b int, index(b));");
    tk.MustExec("create table t2 (id int key, b int, foreign key fk_b(b) references t1(id));");
    tk.MustExec("create database test2");
    let mut wg = sync::WaitGroup::new();
    let mut drop_err: Option<errors::Error> = None;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", |job: *mut model::Job| {
        if unsafe { (*job).SchemaState != model::StateNone || (*job).Type != model::ActionCreateTable } {
            return;
        }
        wg.Add(1);
        go(|| {
            defer(|| wg.Done());
            drop_err = Some(tk2.ExecToErr("drop database test"));
        });
        time::Sleep(time::Millisecond * 100);
    });

    tk.MustExec("create table test2.t3 (id int key, b int, foreign key fk_b(b) references test.t2(id));");
    wg.Wait();
    require::Error(t, drop_err);
    require::Equal(t, "[ddl:3730]Cannot drop table 't2' referenced by a foreign key constraint 'fk_b' on table 't3'.", drop_err.unwrap().Error());
    tk.MustExec("drop table test2.t3");
    tk.MustExec("drop database test");
}

// TestAddForeignKey2 对应 Go 的“添加外键时索引被并发删除”场景。
#[test]
fn test_add_foreign_key2() {
    let store = testkit::CreateMockStoreWithSchemaLease(t, testLease);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("set @@global.tidb_enable_foreign_key=1");
    tk.MustExec("set @@foreign_key_checks=1;");
    tk.MustExec("use test");
    let tk2 = testkit::NewTestKit(t, store);
    tk2.MustExec("use test");
    tk.MustExec("create table t1 (id int key, b int, index(b));");
    tk.MustExec("create table t2 (id int key, b int, index(b));");
    let mut wg = sync::WaitGroup::new();
    let mut add_err: Option<errors::Error> = None;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", |job: *mut model::Job| {
        if unsafe { (*job).SchemaState != model::StatePublic || (*job).Type != model::ActionDropIndex } {
            return;
        }
        wg.Add(1);
        go(|| {
            defer(|| wg.Done());
            add_err = Some(tk2.ExecToErr("alter table t2 add foreign key (b) references t1(id);"));
        });
        time::Sleep(time::Millisecond * 100);
    });

    tk.MustExec("alter table t2 drop index b");
    wg.Wait();
    require::Error(t, add_err);
    require::Equal(t, "[ddl:-1]Failed to add the foreign key constraint. Missing index for 'fk_1' foreign key columns in the table 't2'", add_err.unwrap().Error());
}

// TestAddForeignKey3 对应 Go 的 add FK 过程中 DML 被约束检查拦截。
#[test]
fn test_add_foreign_key3() {
    let store = testkit::CreateMockStoreWithSchemaLease(t, testLease);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("set @@global.tidb_enable_foreign_key=1");
    tk.MustExec("set @@foreign_key_checks=1;");
    tk.MustExec("use test");
    let tk2 = testkit::NewTestKit(t, store);
    tk2.MustExec("use test");
    tk2.MustExec("set @@foreign_key_checks=1;");
    tk.MustExec("create table t1 (id int key, b int, index(b));");
    tk.MustExec("create table t2 (id int, b int, index(id), index(b));");
    tk.MustExec("insert into t1 values (1, 1), (2, 2), (3, 3)");
    tk.MustExec("insert into t2 values (1, 1), (2, 2), (3, 3)");

    let mut insert_errs: Vec<errors::Error> = Vec::new();
    let mut delete_errs: Vec<errors::Error> = Vec::new();
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", |job: *mut model::Job| {
        if unsafe { (*job).Type != model::ActionAddForeignKey } {
            return;
        }
        if unsafe { (*job).SchemaState == model::StateWriteOnly || (*job).SchemaState == model::StateWriteReorganization } {
            insert_errs.push(tk2.ExecToErr("insert into t2 values (10, 10)"));
            delete_errs.push(tk2.ExecToErr("delete from t1 where id = 1"));
        }
    });

    tk.MustExec("alter table t2 add foreign key (id) references t1(id) on delete cascade");
    require::Equal(t, 2, insert_errs.len());
    for err in insert_errs {
        require::Error(t, err);
        require::Equal(t, "[planner:1452]Cannot add or update a child row: a foreign key constraint fails (`test`.`t2`, CONSTRAINT `fk_1` FOREIGN KEY (`id`) REFERENCES `t1` (`id`) ON DELETE CASCADE)", err.Error());
    }
    for err in delete_errs {
        require::Error(t, err);
        require::Equal(t, "[planner:1451]Cannot delete or update a parent row: a foreign key constraint fails (`test`.`t2`, CONSTRAINT `fk_1` FOREIGN KEY (`id`) REFERENCES `t1` (`id`) ON DELETE CASCADE)", err.Error());
    }
    tk.MustQuery("select * from t1 order by id").Check(testkit::Rows("1 1", "2 2", "3 3"));
    tk.MustQuery("select * from t2 order by id").Check(testkit::Rows("1 1", "2 2", "3 3"));
}

// TestForeignKeyInWriteOnlyMode 对应 Go 的建表 DeleteOnly 阶段 DML 不应看到 child 表。
#[test]
fn test_foreign_key_in_write_only_mode() {
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("use test");

    let tk_ddl = testkit::NewTestKit(t, store);
    tk_ddl.MustExec("use test");
    tk_ddl.MustExec("create table parent (id int key)");
    tk_ddl.MustExec("insert into parent values(1)");

    let mut not_exist_errs: Vec<errors::Error> = Vec::new();
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", |job: *mut model::Job| {
        if unsafe { (*job).Type == model::ActionCreateTable && (*job).TableName == "child" && (*job).SchemaState == model::StateDeleteOnly } {
            // tk 持有最新 schema，但 DeleteOnly 阶段 child 对普通 DML 仍应不可见。
            not_exist_errs.push(tk.Exec("insert into child values (1, 1)").err);
            not_exist_errs.push(tk.Exec("update child set id = 2 where id = 1").err);
            not_exist_errs.push(tk.Exec("delete from child where id = 1").err);
            not_exist_errs.push(tk.Exec("delete child from child inner join parent where child.pid = parent.id").err);
            not_exist_errs.push(tk.Exec("delete parent from child inner join parent where child.pid = parent.id").err);
        }
    });
    tk_ddl.MustExec("create table child (id int, pid int, index idx_pid(pid), foreign key (pid) references parent(id) on delete cascade);");

    testfailpoint::Disable(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep");

    for err in not_exist_errs {
        require::Error(t, err);
        require::Contains(t, err.Error(), "Table 'test.child' doesn't exist");
    }
}

// TestFix59705 对应 Go 的回归测试：foreign_key_checks=off 时，父表不存在或列类型不兼容的
// change column 错误信息，以及最终 show create table 中外键元数据的修复结果。
#[test]
fn test_fix59705() {
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("use test;");
    tk.MustExec("set foreign_key_checks=off;");
    tk.MustExec("create table child (id int,pid_test int,foreign key (pid_test) references parent(pid));");
    tk.MustGetErrMsg("alter table child change column pid_test pid varchar(10);", "[schema:1146]Table 'test.parent' doesn't exist");
    tk.MustExec("create table parent(pid int primary key);");
    tk.MustGetErrMsg("alter table child change column pid_test pid varchar(10);", "[ddl:3780]Referencing column 'pid' and referenced column 'pid' in foreign key constraint 'fk_1' are incompatible.");
    tk.MustQuery("select * from information_schema.key_column_usage;");
    tk.MustExec("alter table child change column pid_test pid int");
    tk.MustQuery("select * from information_schema.key_column_usage;");
    tk.MustQuery("show create table child").Check(testkit::Rows("child CREATE TABLE `child` (\n  `id` int(11) DEFAULT NULL,\n  `pid` int(11) DEFAULT NULL,\n  KEY `fk_1` (`pid`),\n  CONSTRAINT `fk_1` FOREIGN KEY (`pid`) REFERENCES `parent` (`pid`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"));
}
*/

// 下面的活动测试使用内存版 DDL 执行器，只覆盖当前已经接通的最小外键行为。
use std::time::Duration;

// 引入 DDL 执行器相关类型：
// - Executor：DDL 语句的执行入口；
// - MemoryJobBackend：内存版 DDL job（任务）存储后端，测试专用；
// - SessionContext：会话上下文，保存会话级变量与状态；
// - TableInfo / ColumnInfo / ForeignKeyInfo：表、列、外键的元数据描述；
// - Ident：带 schema 限定的对象标识符（schema.table）。
use crate::executor::{
    ColumnInfo, Executor, ExecutorError, ForeignKeyInfo, Ident, MemoryJobBackend, OnExist,
    SessionContext, TableInfo,
};

/// 验证外键的完整生命周期：
/// 1. 添加外键成功（父表存在、列匹配）；
/// 2. 重复添加同名外键报 `ForeignKeyExists`；
/// 3. 删除外键时名称大小写不敏感（用 "FK_PARENT" 删除 "fk_parent"）；
/// 4. 重复删除已不存在的外键报 `ForeignKeyNotFound`。
#[test]
fn foreign_key_lifecycle_checks_referenced_table_and_duplicate_names() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 准备环境：创建 test 库以及 parent/child 两张表。
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("parent", vec![ColumnInfo::integer("id")]),
        OnExist::Error,
    )
    .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("child", vec![ColumnInfo::integer("parent_id")]),
        OnExist::Error,
    )
    .unwrap();
    // 先缓存 child 表标识，后续 add/drop foreign key 都围绕这张子表执行。
    let child = Ident::new("test", "child");
    // 外键定义：child.parent_id 引用 parent.id。
    let fk = ForeignKeyInfo::new(
        "fk_parent",
        vec!["parent_id".into()],
        Ident::new("test", "parent"),
        vec!["id".into()],
    );
    // 第一次添加应成功。
    ddl.add_foreign_key(&mut session, &child, fk.clone())
        .unwrap();
    // 再次提交同名外键定义，验证执行器会在元数据层拒绝重复名字。
    // 同名外键重复添加应报 ForeignKeyExists。
    assert!(matches!(
        ddl.add_foreign_key(&mut session, &child, fk),
        Err(ExecutorError::ForeignKeyExists(_))
    ));
    // 外键名比较大小写不敏感：大写名称也能删除小写定义的外键。
    ddl.drop_foreign_key(&mut session, &child, "FK_PARENT")
        .unwrap();
    // 已删除的外键再次删除应报 ForeignKeyNotFound。
    assert!(matches!(
        ddl.drop_foreign_key(&mut session, &child, "fk_parent"),
        Err(ExecutorError::ForeignKeyNotFound(_))
    ));
}

/// 验证被引用的父表不存在时，添加外键应报 `TableNotFound`。
#[test]
fn foreign_key_rejects_missing_parent() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 只创建 child 表，故意不创建被引用的 parent 表。
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("child", vec![ColumnInfo::integer("id")]),
        OnExist::Error,
    )
    .unwrap();
    // 构造一条指向缺失父表的外键定义，用来验证引用完整性前置检查。
    let fk = ForeignKeyInfo::new(
        "fk",
        vec!["id".into()],
        Ident::new("test", "parent"),
        vec!["id".into()],
    );
    // 父表 test.parent 不存在，添加外键应失败并返回 TableNotFound。
    assert!(matches!(
        ddl.add_foreign_key(&mut session, &Ident::new("test", "child"), fk),
        Err(ExecutorError::TableNotFound(_))
    ));
}

#[test]
fn foreign_key_definition_compares_charset_and_collation_for_every_type() {
    use crate::column::{
        ColumnInfo as DdlColumnInfo, FieldType, IndexColumn, IndexInfo, SchemaState,
        TableInfo as DdlTableInfo,
    };
    use crate::foreign_key::{
        ForeignKeyError, ForeignKeyInfo as DdlForeignKeyInfo, ForeignKeyTable, ReferentialAction,
        check_foreign_key_definition,
    };

    let make_table = |id, name: &str, charset: &str| {
        let mut field_type = FieldType::integer();
        field_type.charset = charset.to_owned();
        field_type.collation = charset.to_owned();
        let mut column = DdlColumnInfo::new("id", field_type);
        column.id = 1;
        column.state = SchemaState::Public;
        let mut table = DdlTableInfo::new(id, name);
        table.columns.push(column);
        table.indices.push(IndexInfo {
            id: 1,
            name: "idx_id".to_owned(),
            state: SchemaState::Public,
            columns: vec![IndexColumn {
                name: "id".to_owned(),
                offset: 0,
                length: None,
                use_changing_type: false,
            }],
            primary: false,
            columnar: false,
        });
        ForeignKeyTable {
            schema_name: "test".to_owned(),
            temporary: false,
            partitioned: false,
            ttl_enabled: false,
            primary_key_is_handle: false,
            table,
            max_foreign_key_id: 0,
            foreign_keys: Vec::new(),
        }
    };
    let parent = make_table(1, "parent", "binary");
    let child = make_table(2, "child", "utf8mb4");
    let foreign_key = DdlForeignKeyInfo {
        id: 0,
        name: "fk".to_owned(),
        columns: vec!["id".to_owned()],
        referenced_schema: "test".to_owned(),
        referenced_table: "parent".to_owned(),
        referenced_columns: vec!["id".to_owned()],
        on_delete: ReferentialAction::Restrict,
        on_update: ReferentialAction::Restrict,
        version: 1,
        state: SchemaState::None,
    };

    assert_eq!(
        check_foreign_key_definition(&parent, &child, &foreign_key),
        Err(ForeignKeyError::IncompatibleColumns(
            "id".to_owned(),
            "id".to_owned()
        ))
    );
}

#[test]
fn dropping_redundant_index_accepts_primary_key_handle_like_go() {
    use crate::column::{
        ColumnInfo as DdlColumnInfo, FieldType, IndexColumn, IndexInfo, SchemaState,
        TableInfo as DdlTableInfo,
    };
    use crate::foreign_key::{
        ForeignKeyCatalog, ForeignKeyInfo as DdlForeignKeyInfo, ForeignKeyTable, ReferentialAction,
        check_index_needed_in_foreign_key,
    };

    let mut column = DdlColumnInfo::new("id", FieldType::integer());
    column.id = 1;
    column.state = SchemaState::Public;
    let mut metadata = DdlTableInfo::new(1, "parent");
    metadata.columns.push(column);
    metadata.indices.push(IndexInfo {
        id: 10,
        name: "idx_id".to_owned(),
        state: SchemaState::Public,
        columns: vec![IndexColumn {
            name: "id".to_owned(),
            offset: 0,
            length: None,
            use_changing_type: false,
        }],
        primary: false,
        columnar: false,
    });
    let mut parent = ForeignKeyTable {
        schema_name: "test".to_owned(),
        temporary: false,
        partitioned: false,
        ttl_enabled: false,
        primary_key_is_handle: true,
        table: metadata,
        max_foreign_key_id: 0,
        foreign_keys: Vec::new(),
    };
    parent.foreign_keys.push(DdlForeignKeyInfo {
        id: 1,
        name: "fk".to_owned(),
        columns: vec!["id".to_owned()],
        referenced_schema: "test".to_owned(),
        referenced_table: "parent".to_owned(),
        referenced_columns: vec!["id".to_owned()],
        on_delete: ReferentialAction::Restrict,
        on_update: ReferentialAction::Restrict,
        version: 1,
        state: SchemaState::Public,
    });
    let mut catalog = ForeignKeyCatalog {
        enabled: true,
        ..Default::default()
    };
    catalog.add_table(parent);

    check_index_needed_in_foreign_key(&catalog, "test", "parent", 10).unwrap();
}
