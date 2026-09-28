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

// DDL（数据定义语言）索引变更状态机测试。
//
// 本文件对应 Go(TiDB) 的 `index_change_test.go`，验证 add index / drop index
// 过程中 schema state（模式状态）的演进语义。TiDB 采用类 Google F1 的在线
// DDL 方案：索引创建依次经历 None -> DeleteOnly -> WriteOnly -> Public 等
// 状态，删除则反向演进；相邻状态之间必须保持数据一致，才能在不停机的情况
// 下完成 schema 变更。
//
// 下方大段 `/* ... */` 块注释保留了机械迁移自 Go 的原始测试逻辑（依赖
// testkit、failpoint 等尚未迁移完成的组件，暂不可编译），文件末尾是当前
// 可运行的 Rust 化简版测试：通过内存 job 后端验证索引增删会产生对应的
// DDL 动作记录。

/*
// record 操作或 failpoint；ddl、table、types、testkit 等依赖均保留为占位调用。
//

// TestIndexChange 对应 Go 的 add index 与 drop index 状态机测试。
// afterWaitSchemaSynced hook 在每个 schema state 首次出现时重新加载 infoschema，
// 并用 helper 检查 DeleteOnly/WriteOnly/Public 状态下索引写入和删除行为。
#[test]
fn test_index_change() {
    let (store, dom) = testkit::CreateMockStoreAndDomain(t);
    ddl::SetWaitTimeWhenErrorOccurred(1 * time::Microsecond);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("use test");
    tk.MustExec("create table t (c1 int primary key, c2 int)");
    tk.MustExec("insert t values (1, 1), (2, 2), (3, 3);");

    let mut prev_state = model::StateNone;
    let mut add_index_done = false;
    let job_id = atomic::Int64::new(0);
    let mut delete_only_table: table::Table = nil;
    let mut write_only_table: table::Table = nil;
    let mut public_table: table::Table = nil;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", |job: *mut model::Job| {
        if unsafe { (*job).Type != model::ActionAddIndex || (*job).TableName != "t" } {
            return;
        }
        if unsafe { (*job).SchemaState == prev_state } {
            return;
        }
        job_id.Store(unsafe { (*job).ID });
        let ctx1 = testkit::NewSession(t, store);
        prev_state = unsafe { (*job).SchemaState };
        require::NoError(t, dom.Reload());
        let (tbl, exist) = dom.InfoSchema().TableByID(context::Background(), unsafe { (*job).TableID });
        require::True(t, exist);
        match unsafe { (*job).SchemaState } {
            model::StateDeleteOnly => {
                delete_only_table = tbl;
            }
            model::StateWriteOnly => {
                write_only_table = tbl;
                let err = check_add_write_only_for_add_index(ctx1, delete_only_table, write_only_table);
                require::NoError(t, err);
            }
            model::StatePublic => {
                require::Equalf(t, 3_i64, unsafe { (*job).GetRowCount() }, "job's row count %d != 3", unsafe { (*job).GetRowCount() });
                public_table = tbl;
                let err = check_add_public_for_add_index(ctx1, write_only_table, public_table);
                require::NoError(t, err);
                if unsafe { (*job).State == model::JobStateSynced } {
                    add_index_done = true;
                }
            }
            _ => {}
        }
    });
    tk.MustExec("alter table t add index c2(c2)");
    // Go 等待 onJobUpdated 首次 hook 到 JobStateSynced，避免 prevState 误停在 StatePublic。
    for _ in 0..=100 {
        if add_index_done {
            break;
        }
        time::Sleep(10 * time::Millisecond);
    }
    checkJobWithHistory(t, tk.Session(), job_id.Load(), nil, public_table.Meta());

    prev_state = model::StateNone;
    let mut none_table: table::Table = nil;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", |job: *mut model::Job| {
        job_id.Store(unsafe { (*job).ID });
        if unsafe { (*job).SchemaState == prev_state } {
            return;
        }
        prev_state = unsafe { (*job).SchemaState };
        require::NoError(t, dom.Reload());
        let (tbl, exist) = dom.InfoSchema().TableByID(context::Background(), unsafe { (*job).TableID });
        require::True(t, exist);
        let ctx1 = testkit::NewSession(t, store);
        match unsafe { (*job).SchemaState } {
            model::StateWriteOnly => {
                write_only_table = tbl;
                let err = check_drop_write_only(ctx1, public_table, write_only_table);
                require::NoError(t, err);
            }
            model::StateDeleteOnly => {
                delete_only_table = tbl;
                let err = check_drop_delete_only(ctx1, write_only_table, delete_only_table);
                require::NoError(t, err);
            }
            model::StateNone => {
                none_table = tbl;
                require::Equalf(t, 0, none_table.Indices().len(), "index should have been dropped");
            }
            _ => {}
        }
    });
    tk.MustExec("alter table t drop index c2");
    checkJobWithHistory(t, tk.Session(), job_id.Load(), nil, none_table.Meta());
}

// checkIndexExists 对应 Go helper：用 table 第一个 index 的 Exist 接口检查指定 datum/handle 是否存在。
fn check_index_exists(
    ctx: sessionctx::Context,
    tbl: table::Table,
    index_value: types::Datum,
    handle: i64,
    exists: bool,
) -> Result<(), errors::Error> {
    let idx = tbl.Indices()[0];
    let (txn, err) = ctx.Txn(true);
    if err != nil {
        return Err(errors::Trace(err));
    }
    let sc = ctx.GetSessionVars().StmtCtx;
    let (does_exist, _, err) = idx.Exist(sc.ErrCtx(), sc.TimeZone(), txn, types::MakeDatums(index_value), kv::IntHandle(handle));
    if err != nil {
        return Err(errors::Trace(err));
    }
    if exists != does_exist {
        if exists {
            return Err(errors::New("index should exists"));
        }
        return Err(errors::New("index should not exists"));
    }
    Ok(())
}

// checkAddWriteOnlyForAddIndex 对应 add-index 从 DeleteOnly 到 WriteOnly 的写入可见性检查。
fn check_add_write_only_for_add_index(
    ctx: sessionctx::Context,
    del_only_tbl: table::Table,
    write_only_tbl: table::Table,
) -> Result<(), errors::Error> {
    // DeleteOnlyTable 插入 (4,4) 不应写入新索引。
    let (txn, err) = newTxn(ctx);
    if err != nil {
        return Err(errors::Trace(err));
    }
    let (_, err) = del_only_tbl.AddRecord(ctx.GetTableCtx(), txn, types::MakeDatums(4, 4));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_only_tbl, 4.into(), 4, false).map_err(errors::Trace)?;

    // WriteOnlyTable 插入 (5,5) 应写入新索引。
    let (_, err) = write_only_tbl.AddRecord(ctx.GetTableCtx(), txn, types::MakeDatums(5, 5));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_only_tbl, 5.into(), 5, true).map_err(errors::Trace)?;

    // WriteOnlyTable 更新 c2=1 时应写入新值索引。
    let err = write_only_tbl.UpdateRecord(ctx.GetTableCtx(), txn, kv::IntHandle(4), types::MakeDatums(4, 4), types::MakeDatums(4, 1), touchedSlice(write_only_tbl));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_only_tbl, 1.into(), 4, true).map_err(errors::Trace)?;

    // DeleteOnlyTable 更新 c2=3 不应写入旧值或新值索引。
    let err = del_only_tbl.UpdateRecord(ctx.GetTableCtx(), txn, kv::IntHandle(4), types::MakeDatums(4, 1), types::MakeDatums(4, 3), touchedSlice(write_only_tbl));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_only_tbl, 1.into(), 4, false).map_err(errors::Trace)?;
    check_index_exists(ctx, write_only_tbl, 3.into(), 4, false).map_err(errors::Trace)?;

    // 删除路径分别覆盖 WriteOnlyTable 和 DeleteOnlyTable 的 index 清理语义。
    let err = write_only_tbl.RemoveRecord(ctx.GetTableCtx(), txn, kv::IntHandle(4), types::MakeDatums(4, 3));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_only_tbl, 3.into(), 4, false).map_err(errors::Trace)?;
    let err = del_only_tbl.RemoveRecord(ctx.GetTableCtx(), txn, kv::IntHandle(5), types::MakeDatums(5, 5));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_only_tbl, 5.into(), 5, false).map_err(errors::Trace)?;
    Ok(())
}

// checkAddPublicForAddIndex 对应 add-index 进入 Public 后的写入、更新、删除和全表校验。
fn check_add_public_for_add_index(
    ctx: sessionctx::Context,
    write_tbl: table::Table,
    public_tbl: table::Table,
) -> Result<(), errors::Error> {
    let mut err1: Option<errors::Error> = None;
    let (txn, err) = newTxn(ctx);
    if err != nil {
        return Err(errors::Trace(err));
    }
    let (_, err) = write_tbl.AddRecord(ctx.GetTableCtx(), txn, types::MakeDatums(6, 6));
    if err != nil {
        return Err(errors::Trace(err));
    }
    let mut err = check_index_exists(ctx, public_tbl, 6.into(), 6, true).err();
    if vardef::EnableFastReorg.Load() {
        // fast reorg 下 Go 额外检查临时索引。
        err1 = check_index_exists(ctx, write_tbl, 6.into(), 6, true).err();
    }
    if err.is_some() && err1.is_some() {
        return Err(errors::Trace(err.unwrap()));
    }

    let (_, err2) = public_tbl.AddRecord(ctx.GetTableCtx(), txn, types::MakeDatums(7, 7));
    if err2 != nil {
        return Err(errors::Trace(err2));
    }
    check_index_exists(ctx, public_tbl, 7.into(), 7, true).map_err(errors::Trace)?;

    let err3 = write_tbl.UpdateRecord(ctx.GetTableCtx(), txn, kv::IntHandle(7), types::MakeDatums(7, 7), types::MakeDatums(7, 5), touchedSlice(write_tbl));
    if err3 != nil {
        return Err(errors::Trace(err3));
    }
    err = check_index_exists(ctx, public_tbl, 5.into(), 7, true).err();
    if vardef::EnableFastReorg.Load() {
        err1 = check_index_exists(ctx, write_tbl, 5.into(), 7, true).err();
    }
    if err.is_some() && err1.is_some() {
        return Err(errors::Trace(err.unwrap()));
    }
    if vardef::EnableFastReorg.Load() {
        check_index_exists(ctx, write_tbl, 7.into(), 7, false).map_err(errors::Trace)?;
    } else {
        check_index_exists(ctx, public_tbl, 7.into(), 7, false).map_err(errors::Trace)?;
    }

    let err4 = write_tbl.RemoveRecord(ctx.GetTableCtx(), txn, kv::IntHandle(6), types::MakeDatums(6, 6));
    if err4 != nil {
        return Err(errors::Trace(err4));
    }
    check_index_exists(ctx, public_tbl, 6.into(), 6, false).map_err(errors::Trace)?;

    let mut rows: Vec<Vec<types::Datum>> = Vec::new();
    let err5 = tables::IterRecords(public_tbl, ctx, public_tbl.Cols(), |_: kv::Handle, data: Vec<types::Datum>, cols: Vec<*mut table::Column>| {
        rows.push(data);
        (true, nil)
    });
    if err5 != nil {
        return Err(errors::Trace(err5));
    }
    if rows.is_empty() {
        return Err(errors::New("table is empty"));
    }
    for row in rows {
        let idx_val = row[1].GetInt64();
        let handle = row[0].GetInt64();
        err = check_index_exists(ctx, public_tbl, idx_val.into(), handle, true).err();
        if vardef::EnableFastReorg.Load() {
            err1 = check_index_exists(ctx, write_tbl, idx_val.into(), handle, true).err();
        }
        if err.is_some() && err1.is_some() {
            return Err(errors::Trace(err.unwrap()));
        }
    }
    txn.Commit(context::Background())
}

// checkDropWriteOnly 对应 drop-index 进入 WriteOnly 后，public 索引仍需维护到记录删除为止。
fn check_drop_write_only(ctx: sessionctx::Context, public_tbl: table::Table, write_tbl: table::Table) -> Result<(), errors::Error> {
    let (txn, err) = newTxn(ctx);
    if err != nil {
        return Err(errors::Trace(err));
    }
    let (_, err) = write_tbl.AddRecord(ctx.GetTableCtx(), txn, types::MakeDatums(8, 8));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, public_tbl, 8.into(), 8, true).map_err(errors::Trace)?;

    let err = write_tbl.UpdateRecord(ctx.GetTableCtx(), txn, kv::IntHandle(8), types::MakeDatums(8, 8), types::MakeDatums(8, 7), touchedSlice(write_tbl));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, public_tbl, 7.into(), 8, true).map_err(errors::Trace)?;

    let err = write_tbl.RemoveRecord(ctx.GetTableCtx(), txn, kv::IntHandle(8), types::MakeDatums(8, 7));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, public_tbl, 7.into(), 8, false).map_err(errors::Trace)?;
    txn.Commit(context::Background())
}

// checkDropDeleteOnly 对应 drop-index 进入 DeleteOnly 后，新增/更新记录不应再写入被删除索引。
fn check_drop_delete_only(ctx: sessionctx::Context, write_tbl: table::Table, del_tbl: table::Table) -> Result<(), errors::Error> {
    let (txn, err) = newTxn(ctx);
    if err != nil {
        return Err(errors::Trace(err));
    }
    let (_, err) = write_tbl.AddRecord(ctx.GetTableCtx(), txn, types::MakeDatums(9, 9));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_tbl, 9.into(), 9, true).map_err(errors::Trace)?;

    let (_, err) = del_tbl.AddRecord(ctx.GetTableCtx(), txn, types::MakeDatums(10, 10));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_tbl, 10.into(), 10, false).map_err(errors::Trace)?;

    let err = del_tbl.UpdateRecord(ctx.GetTableCtx(), txn, kv::IntHandle(9), types::MakeDatums(9, 9), types::MakeDatums(9, 10), touchedSlice(del_tbl));
    if err != nil {
        return Err(errors::Trace(err));
    }
    check_index_exists(ctx, write_tbl, 9.into(), 9, false).map_err(errors::Trace)?;
    check_index_exists(ctx, write_tbl, 10.into(), 9, false).map_err(errors::Trace)?;
    txn.Commit(context::Background())
}

// TestAddIndexRowCountUpdate 对应 Go 的 backfill 进度行数更新测试。
// failpoint 暂停 afterHandleBackfillTask，另一个 session 轮询 admin show ddl jobs 的 row count。
#[test]
fn test_add_index_row_count_update() {
    if kerneltype::IsNextGen() {
        t.Skip("add-index always runs on DXF with ingest mode in nextgen");
    }
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("use test");
    tk.MustExec("create table t (c1 int primary key, c2 int)");
    tk.MustExec("insert t values (1, 1), (2, 2), (3, 3);");
    tk.MustExec("set @@tidb_ddl_reorg_worker_cnt = 1;");
    tk.MustExec("set global tidb_ddl_enable_fast_reorg = 0;");
    tk.MustExec("set global tidb_enable_dist_task = 0;");

    let mut job_id = 0_i64;
    let row_cnt_updated = make_chan::<()>();
    let backfill_done = make_chan::<()>();
    testfailpoint::Enable(t, "github.com/pingcap/tidb/pkg/ddl/updateProgressIntervalInMs", "return(50)");
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterHandleBackfillTask", |id: i64| {
        job_id = id;
        backfill_done.send(());
        // Go 在这里阻塞 backfill，直到轮询 goroutine 观察到 row count 更新。
        row_cnt_updated.recv();
    });
    go(|| {
        defer(|| row_cnt_updated.send(()));
        backfill_done.recv();
        let tk2 = testkit::NewTestKit(t, store);
        tk2.MustExec("use test");
        require::Eventually(t, || {
            let rs = tk2.MustQuery("admin show ddl jobs 1;").Rows();
            let id_str = rs[0][0].clone().into_string();
            let (id, err) = strconv::Atoi(id_str);
            require::NoError(t, err);
            require::Equal(t, id as i64, job_id);
            let rc_str = rs[0][7].clone().into_string();
            let (rc, err) = strconv::Atoi(rc_str);
            require::NoError(t, err);
            rc > 0
        }, 2 * time::Minute, 60 * time::Millisecond);
    });
    tk.MustExec("alter table t add index idx(c2);");
}

// TestFastReOrgAlwaysEnabledOnNextGen 对应 next-gen 模式下 fast reorg 全局变量只读检查。
#[test]
fn test_fast_reorg_always_enabled_on_next_gen() {
    if kerneltype::IsClassic() {
        t.Skip("This test is only for next-gen TiDB");
    }
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    tk.MustQuery("select @@global.tidb_ddl_enable_fast_reorg").Equal(testkit::Rows("1"));
    require::ErrorContains(
        t,
        tk.ExecToErr("set global tidb_ddl_enable_fast_reorg=0"),
        "setting tidb_ddl_enable_fast_reorg is not supported in the next generation of TiDB",
    );
}

// TestReadOnlyVarsInNextGen 对应 next-gen 模式下多个 DDL 变量不可设置的错误检查。
#[test]
fn test_read_only_vars_in_next_gen() {
    if kerneltype::IsClassic() {
        t.Skip("This test is only for next-gen TiDB");
    }
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    require::ErrorContains(t, tk.ExecToErr("set global tidb_max_dist_task_nodes=5"), "setting tidb_max_dist_task_nodes is not supported in the next generation of TiDB");
    require::ErrorContains(t, tk.ExecToErr("set global tidb_ddl_reorg_max_write_speed=5"), "setting tidb_ddl_reorg_max_write_speed is not supported in the next generation of TiDB");
    require::ErrorContains(t, tk.ExecToErr("set global tidb_ddl_disk_quota=5"), "setting tidb_ddl_disk_quota is not supported in the next generation of TiDB");
}
*/

// 引入 DDL 执行器及其配套类型：
// - Executor：DDL 语句执行入口，负责把建表/建索引等请求转成 DDL job（任务）；
// - MemoryJobBackend：把 DDL job 记录在内存中的后端实现，便于测试断言历史动作；
// - Ident：库名 + 表名的限定标识符；OnExist：对象已存在时的处理策略。
use crate::executor::{
    ColumnInfo, DdlAction, Executor, Ident, IndexInfo, MemoryJobBackend, ObjectState, OnExist,
    SessionContext, TableInfo,
};
use std::time::Duration;

/// 验证添加/删除索引会向 job 历史写入与 Go 状态机对应的 DDL 动作。
///
/// 流程：先建库建表，再对列 `a` 创建索引 `idx`，断言最近一条 job 的动作为
/// `AddIndex`；随后删除该索引，断言动作变为 `DropIndex`。这是对上方 Go
/// 版状态机测试（DeleteOnly/WriteOnly/Public 各阶段可见性检查）的简化替代。
#[test]
fn add_and_drop_index_emit_go_state_actions() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("t", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    let ident = Ident::new("test", "t");
    ddl.create_index(
        &mut session,
        &ident,
        IndexInfo::new("idx", vec!["a".into()]),
        false,
    )
    .unwrap();
    // Go 的 add-index 只有在经历 DeleteOnly/WriteOnly/WriteReorganization 后进入
    // Public 才返回；返回时 infoschema 中的索引必须已经对外可见，而不能仍停在 None。
    let table = &ddl.schemas["test"].tables["t"];
    assert_eq!(1, table.indexes.len());
    assert_eq!(ObjectState::Public, table.indexes[0].state);
    // 建索引后读取内存 backend 中最新一条历史 job，确认记录的是 AddIndex 动作。
    let history = ddl.backend().history();
    let add_job = history.last().unwrap();
    assert_eq!(DdlAction::AddIndex, add_job.action);
    assert_eq!(ObjectState::Public, add_job.schema_state);
    ddl.drop_index(&mut session, &ident, "idx", false).unwrap();
    assert!(ddl.schemas["test"].tables["t"].indexes.is_empty());
    // 删除索引后再次检查最新历史 job，确认动作切换为 DropIndex。
    let history = ddl.backend().history();
    let drop_job = history.last().unwrap();
    assert_eq!(DdlAction::DropIndex, drop_job.action);
    assert_eq!(ObjectState::None, drop_job.schema_state);
}
