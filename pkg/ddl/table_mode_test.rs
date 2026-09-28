// Copyright 2026 AsterSQL.
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

// 表模式（Table Mode）相关测试。
//
// 可执行部分覆盖模式切换、版本递增与 Import/Restore 下的操作权限矩阵。
// 大段注释块保留 Go 集成测试（受保护语句、并发切换、RefreshMeta 等）的迁移草稿，
// 便于后续接通 testkit 后对照。

/*

// get_cloned_table_info_from_domain 对应 Go helper：从 InfoSchema 读取表并 Clone 元信息。
fn get_cloned_table_info_from_domain(
    t: testing::T,
    db_name: &str,
    table_name: &str,
    dom: &domain::Domain,
) -> *mut model::TableInfo {
    let tbl = dom
        .InfoSchema()
        .TableByName(context::Background(), ast::NewCIStr(db_name), ast::NewCIStr(table_name))
        .expect("Go require.NoError");
    tbl.Meta().Clone()
}
*/

use crate::table_mode::{
    TableInfo, TableMode, TableOperation, alter_table_mode, on_alter_table_mode, table_mode_allows,
};

/// 校验模式切换时 ID 匹配、幂等不升版本，以及 Import↔Restore 非法互转。
#[test]
fn table_mode_transitions_validate_ids_and_advance_version_once() {
    let mut table = TableInfo {
        schema_id: 7,
        table_id: 11,
        mode: TableMode::Normal,
        version: 5,
    };
    // Normal -> Import：版本 5→6。
    assert_eq!(
        6,
        on_alter_table_mode(&mut table, 7, 11, TableMode::Import).unwrap()
    );
    // 幂等：Go 的 onAlterTableMode 不更新 schema，返回零值版本，表版本保持 6。
    assert_eq!(
        0,
        on_alter_table_mode(&mut table, 7, 11, TableMode::Import).unwrap()
    );
    assert_eq!(6, table.version);
    // table_id 不匹配应失败。
    assert!(on_alter_table_mode(&mut table, 7, 12, TableMode::Normal).is_err());
    // Import 不能直接切到 Restore。
    assert!(alter_table_mode(&mut table, TableMode::Restore).is_err());
    // 先回 Normal，再进 Restore；随后 Restore→Import 仍非法。
    assert!(alter_table_mode(&mut table, TableMode::Normal).unwrap());
    assert!(alter_table_mode(&mut table, TableMode::Restore).unwrap());
    assert!(alter_table_mode(&mut table, TableMode::Import).is_err());
}

/// 校验 Import/Restore 仅允许 Metadata/Checksum，Normal 允许全部操作。
#[test]
fn protected_table_modes_allow_metadata_and_checksum_only() {
    for mode in [TableMode::Import, TableMode::Restore] {
        assert!(table_mode_allows(mode, TableOperation::Metadata));
        assert!(table_mode_allows(mode, TableOperation::Checksum));
        for operation in [
            TableOperation::Read,
            TableOperation::Write,
            TableOperation::Alter,
            TableOperation::Drop,
        ] {
            assert!(!table_mode_allows(mode, operation));
        }
    }
    for operation in [
        TableOperation::Metadata,
        TableOperation::Checksum,
        TableOperation::Read,
        TableOperation::Write,
        TableOperation::Alter,
        TableOperation::Drop,
    ] {
        assert!(table_mode_allows(TableMode::Normal, operation));
    }
}

/*
// check_error_code 对应 Go helper：把 TiDB terror 转成 SQL error 后检查错误码。
fn check_error_code(t: testing::T, err: errors::Error, expected: i32) {
    let origin_err = errors::Cause(err);
    let t_err = origin_err.downcast_ref::<terror::Error>();
    require::True(t, t_err.is_some());
    let sql_err = terror::ToSQLError(t_err.unwrap());
    require::Equal(t, expected, sql_err.Code as i32);
}

// test_table_mode_basic 对应 Go 的 TestTableModeBasic。
// 它覆盖 ModeImport/ModeRestore 表的创建、受保护语句、模式切换合法性和批量建表模式持久化。
#[test]
fn test_table_mode_basic() {
    let (store, domain) = testkit::CreateMockStoreAndDomain(testing::T);
    let de = domain.DDLExecutor();
    let tk = testkit::NewTestKit(testing::T, store);
    let ctx = testkit::NewTestKit(testing::T, store).Session();

    // init test：三张普通/外键表作为后续 Clone TableInfo 的来源。
    tk.MustExec("use test");
    tk.MustExec("create table t1(id int, c1 int, c2 int, index idx1(c1))");
    tk.MustExec("create table t2(id int, c1 int, c2 int, index idx1(c1))");
    tk.MustExec(
        "create table t3(id int, pid INT, INDEX idx_pid (pid),FOREIGN KEY fk_1 (pid) REFERENCES t1(c1) ON UPDATE SET NULL)",
    );

    // For testing create foreign key table as ModeImport
    let mut tbl_info = get_cloned_table_info_from_domain(testing::T, "test", "t3", &domain);
    unsafe {
        (*tbl_info).Name = ast::NewCIStr("t1_foreign_key");
        (*tbl_info).Mode = model::TableModeImport;
    }
    de.CreateTableWithInfo(
        tk.Session(),
        ast::NewCIStr("test"),
        tbl_info,
        nil,
        ddl::WithOnExist(ddl::OnExistIgnore),
    )
    .expect("Go require.NoError");
    let (db_info, ok) = domain.InfoSchema().SchemaByName(ast::NewCIStr("test"));
    require::True(testing::T, ok);
    testutil::CheckTableMode(testing::T, store, db_info, tbl_info, model::TableModeImport);
    // 外键约束删除会改变受保护表结构，Go 期望返回 ErrProtectedTableMode。
    tk.MustGetErrCode(
        "ALTER TABLE t1_foreign_key DROP FOREIGN KEY fk_1",
        errno::ErrProtectedTableMode,
    );

    // For testing create table as ModeRestore
    tbl_info = get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain);
    unsafe {
        (*tbl_info).Name = ast::NewCIStr("t1_restore_import");
        (*tbl_info).Mode = model::TableModeRestore;
    }
    de.CreateTableWithInfo(
        tk.Session(),
        ast::NewCIStr("test"),
        tbl_info,
        nil,
        ddl::WithOnExist(ddl::OnExistIgnore),
    )
    .expect("Go require.NoError");
    let (db_info, ok) = domain.InfoSchema().SchemaByName(ast::NewCIStr("test"));
    require::True(testing::T, ok);
    testutil::CheckTableMode(testing::T, store, db_info, tbl_info, model::TableModeRestore);

    // ModeRestore 允许访问元数据类语句，Go 逐条执行以覆盖 show/describe/create like/view/foreign key/checksum。
    for sql in [
        "show create table t1_restore_import",
        "show table status where Name = 't1_restore_import'",
        "show columns from t1_restore_import",
        "show create table t1_restore_import",
        "show table status where Name = 't1_restore_import'",
        "show index from t1_restore_import",
        "describe t1_restore_import",
        "create table t1_restore_import_2 like t1_restore_import",
        "create view t1_restore_import_view as select * from t1_restore_import",
        "create table foreign_key_child(id int, pid INT, INDEX idx_pid (pid),FOREIGN KEY (pid) REFERENCES t1_restore_import(c1) ON DELETE CASCADE)",
        "drop table foreign_key_child",
        "admin checksum table t1_restore_import;",
    ] {
        tk.MustExec(sql);
    }

    // ModeImport/ModeRestore 下 DML 和结构变更会被拒绝，错误码保持 ErrProtectedTableMode。
    for sql in [
        "select * from t1_restore_import",
        "explain select * from t1_restore_import",
        "desc select * from t1_restore_import",
        "insert into t1_restore_import values(1, 1, 1)",
        "replace into t1_restore_import values(1,1,1)",
        "update t1_restore_import set id = 2 where id = 1",
        "delete from t1_restore_import where id = 2",
        "truncate table t1_restore_import",
        "drop table t1_restore_import",
        "alter table t1_restore_import rename to t1_new",
        "rename table t1_restore_import to t1_new",
        "rename table t1_restore_import to t1_new, t2 to t2_new",
        "alter table t1_restore_import modify column c2 bigint",
        "alter table t1_restore_import add column c3 int",
        "alter table t1_restore_import drop column c2",
        "alter table t1_restore_import drop index idx1",
        "alter table t1_restore_import add index idx2(c2)",
        "alter table t1_restore_import partition by range(id) (partition p0 values less than (100))",
        "alter table t1_restore_import comment='new comment'",
        "alter table t1_restore_import convert to character set utf8mb4",
        "alter table t1_restore_import rename column c1 to c1_new",
        "alter table t1_restore_import alter column c1 set default 100",
        "alter table t1_restore_import add foreign key fk_1 (c2) REFERENCES t1(c1) ON UPDATE SET NULL ",
    ] {
        tk.MustGetErrCode(sql, errno::ErrProtectedTableMode);
    }

    // Transaction related operations：事务中写受保护表同样报错，随后 rollback 收尾。
    tk.MustExec("begin");
    tk.MustGetErrCode(
        "insert into t1_restore_import values(1,1,1)",
        errno::ErrProtectedTableMode,
    );
    tk.MustExec("rollback");

    // Go 校验 TableMode 状态转换矩阵：Restore -> Import 禁止，Restore/Normal 往返允许。
    let err = testutil::SetTableMode(ctx, testing::T, store, de, db_info, tbl_info, model::TableModeImport);
    require::ErrorContains(
        testing::T,
        err,
        "Invalid mode set from (or by default) Restore to Import for table t1_restore_import",
    );
    testutil::SetTableMode(ctx, testing::T, store, de, db_info, tbl_info, model::TableModeNormal)
        .expect("Go require.NoError");
    testutil::SetTableMode(ctx, testing::T, store, de, db_info, tbl_info, model::TableModeRestore)
        .expect("Go require.NoError");
    testutil::SetTableMode(ctx, testing::T, store, de, db_info, tbl_info, model::TableModeRestore)
        .expect("Go require.NoError");

    // 已存在 Import 表不能被 BR 以 Restore 模式重复创建。
    testutil::SetTableMode(ctx, testing::T, store, de, db_info, tbl_info, model::TableModeNormal)
        .expect("Go require.NoError");
    testutil::SetTableMode(ctx, testing::T, store, de, db_info, tbl_info, model::TableModeImport)
        .expect("Go require.NoError");
    unsafe {
        (*tbl_info).Mode = model::TableModeRestore;
    }
    let err = de.CreateTableWithInfo(
        tk.Session(),
        ast::NewCIStr("test"),
        tbl_info,
        nil,
        ddl::WithOnExist(ddl::OnExistIgnore),
    );
    require::ErrorContains(
        testing::T,
        err,
        "Invalid mode set from (or by default) Import to Restore for table t1_restore_import",
    );

    // BatchCreateTableWithInfo 同时创建 Normal/Import/Restore 三种模式并逐个检查持久化结果。
    let tbl_info1 = get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain);
    unsafe {
        (*tbl_info1).Name = ast::NewCIStr("t1_1");
        (*tbl_info1).Mode = model::TableModeNormal;
    }
    let tbl_info2 = get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain);
    unsafe {
        (*tbl_info2).Name = ast::NewCIStr("t1_2");
        (*tbl_info2).Mode = model::TableModeImport;
    }
    let tbl_info3 = get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain);
    unsafe {
        (*tbl_info3).Name = ast::NewCIStr("t1_3");
        (*tbl_info3).Mode = model::TableModeRestore;
    }
    de.BatchCreateTableWithInfo(
        ctx,
        ast::NewCIStr("test"),
        vec![tbl_info1, tbl_info2, tbl_info3],
        ddl::WithOnExist(ddl::OnExistIgnore),
    )
    .expect("Go require.NoError");
    testutil::CheckTableMode(testing::T, store, db_info, tbl_info1, model::TableModeNormal);
    testutil::CheckTableMode(testing::T, store, db_info, tbl_info2, model::TableModeImport);
    testutil::CheckTableMode(testing::T, store, db_info, tbl_info3, model::TableModeRestore);
}

// test_table_mode_concurrent 对应 Go 的 TestTableModeConcurrent。
// 这里保留 WaitGroup、channel 收集错误和四组并发转换预期；不会真的调度 goroutine。
#[test]
fn test_table_mode_concurrent() {
    let (store, domain) = testkit::CreateMockStoreAndDomain(testing::T);
    let de = domain.DDLExecutor();
    let tk = testkit::NewTestKit(testing::T, store);
    let ctx = testkit::NewTestKit(testing::T, store).Session();

    tk.MustExec("use test");
    tk.MustExec("create table t1(id int)");
    let (db_info, ok) = domain.InfoSchema().SchemaByName(ast::NewCIStr("test"));
    require::True(testing::T, ok);

    // Concurrency test1: concurrently alter t1 to ModeImport, expecting both success in current Go test.
    let t1_infos = vec![
        get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain),
        get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain),
    ];
    let errs =
        run_table_mode_changes_concurrently(ctx, testing::T, store, de, db_info, &t1_infos, model::TableModeImport);
    require::Equal(testing::T, 2, count_success(&errs));
    require::Nil(testing::T, first_error(&errs));
    testutil::CheckTableMode(testing::T, store, db_info, t1_infos[0], model::TableModeImport);

    // Concurrency test2: concurrently alter t1 to ModeNormal, expecting both success.
    let t1_normal_infos = vec![
        get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain),
        get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain),
    ];
    let errs2 = run_table_mode_changes_concurrently(
        ctx,
        testing::T,
        store,
        de,
        db_info,
        &t1_normal_infos,
        model::TableModeNormal,
    );
    for err in errs2 {
        require::NoError(testing::T, err);
    }
    testutil::CheckTableMode(testing::T, store, db_info, t1_normal_infos[0], model::TableModeNormal);

    // Concurrency test3: concurrently alter t1 to ModeRestore, expecting both success.
    let t1_infos = vec![
        get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain),
        get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain),
    ];
    let errs =
        run_table_mode_changes_concurrently(ctx, testing::T, store, de, db_info, &t1_infos, model::TableModeRestore);
    require::Equal(testing::T, 2, count_success(&errs));
    require::Nil(testing::T, first_error(&errs));
    testutil::CheckTableMode(testing::T, store, db_info, t1_infos[0], model::TableModeRestore);

    // Concurrency test4: concurrently alter t1 to ModeRestore and ModeImport, expecting one success, one failure.
    let modes = vec![model::TableModeRestore, model::TableModeImport];
    let clones = modes
        .iter()
        .map(|_| get_cloned_table_info_from_domain(testing::T, "test", "t1", &domain))
        .collect::<Vec<_>>();
    let errs = run_table_mode_mixed_changes_concurrently(ctx, testing::T, store, de, db_info, &clones, &modes);
    require::Equal(testing::T, 1, count_success(&errs));
    let failed_err = first_error(&errs);
    require::NotNil(testing::T, failed_err);
    check_error_code(testing::T, failed_err, errno::ErrInvalidTableModeSet);
}

// run_table_mode_changes_concurrently 对应 Go 中 WaitGroup + buffered channel 收集并发 SetTableMode 结果。
fn run_table_mode_changes_concurrently(
    ctx: sessionctx::Context,
    t: testing::T,
    store: kv::Storage,
    de: ddl::Executor,
    db_info: *mut model::DBInfo,
    infos: &[*mut model::TableInfo],
    mode: model::TableMode,
) -> Vec<errors::Error> {
    let mut wg = sync::WaitGroup::new();
    let errs = channel::bounded(infos.len());
    for info in infos {
        wg.Add(1);
        go(|| {
            defer(|| wg.Done());
            errs.send(testutil::SetTableMode(ctx, t, store, de, db_info, *info, mode));
        });
    }
    wg.Wait();
    errs.close();
    errs.collect()
}

// run_table_mode_mixed_changes_concurrently 保留 Go 第四组并发：每个 TableInfo 使用不同目标模式。
fn run_table_mode_mixed_changes_concurrently(
    ctx: sessionctx::Context,
    t: testing::T,
    store: kv::Storage,
    de: ddl::Executor,
    db_info: *mut model::DBInfo,
    infos: &[*mut model::TableInfo],
    modes: &[model::TableMode],
) -> Vec<errors::Error> {
    let mut wg = sync::WaitGroup::new();
    let errs = channel::bounded(modes.len());
    for (info, mode) in infos.iter().zip(modes.iter()) {
        wg.Add(1);
        go(|| {
            defer(|| wg.Done());
            errs.send(testutil::SetTableMode(ctx, t, store, de, db_info, *info, *mode));
        });
    }
    wg.Wait();
    errs.close();
    errs.collect()
}

fn count_success(errs: &[errors::Error]) -> i32 {
    errs.iter().filter(|err| err.is_nil()).count() as i32
}

fn first_error(errs: &[errors::Error]) -> errors::Error {
    errs.iter().find(|err| !err.is_nil()).cloned().unwrap_or(nil)
}

// test_table_mode_with_refresh_meta 对应 Go 的 TestTableModeWithRefreshMeta。
// 它通过把普通表 ID 替换为分区 ID，验证 RefreshMeta 前后 SetTableMode 的行为差异。
#[test]
fn test_table_mode_with_refresh_meta() {
    let (store, domain) = testkit::CreateMockStoreAndDomain(testing::T);
    let de = domain.DDLExecutor();
    let tk = testkit::NewTestKit(testing::T, store);
    let sctx = testkit::NewTestKit(testing::T, store).Session();

    tk.MustExec("use test");
    tk.MustExec("create table nt(id int, c1 int)");
    tk.MustExec("create table pt(id int, c1 int) partition by range (c1) (partition p10 values less than (10))");
    tk.MustExec("insert into nt values(3, 3), (4, 4), (5, 5)");
    tk.MustExec("insert into pt values(1, 1), (2, 2)");

    let (db_info, ok) = domain.InfoSchema().SchemaByName(ast::NewCIStr("test"));
    require::True(testing::T, ok);
    require::NotNil(testing::T, db_info);
    let mut nt_info = get_cloned_table_info_from_domain(testing::T, "test", "nt", &domain);
    let pt_info = get_cloned_table_info_from_domain(testing::T, "test", "pt", &domain);

    // change non-partition table ID to partition ID
    let part_id = unsafe { (*pt_info).Partition.Definitions[0].ID };
    recreate_table_with_partition_id(testing::T, &store, unsafe { (*db_info).ID }, nt_info, pt_info, "p10");
    nt_info = testutil::GetTableInfoByTxn(testing::T, store, unsafe { (*db_info).ID }, unsafe { (*nt_info).ID });
    require::Equal(testing::T, part_id, unsafe { (*nt_info).ID });

    // RefreshMeta 前 DDL executor 仍按旧 meta 查找，Go 期望报 doesn't exist。
    let err = testutil::SetTableMode(sctx, testing::T, store, de, db_info, nt_info, model::TableModeImport);
    require::ErrorContains(testing::T, err, "doesn't exist");
    testutil::RefreshMeta(
        sctx,
        testing::T,
        de,
        unsafe { (*db_info).ID },
        unsafe { (*nt_info).ID },
        unsafe { (*db_info).Name.O.clone() },
        unsafe { (*nt_info).Name.O.clone() },
    );

    // RefreshMeta 后同一 TableInfo 可成功切到 Import，随后受保护查询返回 ErrProtectedTableMode。
    testutil::SetTableMode(sctx, testing::T, store, de, db_info, nt_info, model::TableModeImport)
        .expect("Go require.NoError");
    tk.MustGetErrCode("select * from nt", errno::ErrProtectedTableMode);
    testutil::SetTableMode(sctx, testing::T, store, de, db_info, nt_info, model::TableModeNormal)
        .expect("Go require.NoError");
    tk.MustExec("select * from nt");
}

// recreate_table_with_partition_id 对应 Go helper：在新事务里删除普通表，再用分区定义 ID 重建。
fn recreate_table_with_partition_id(
    t: testing::T,
    store: &kv::Storage,
    db_id: i64,
    nt_info: *mut model::TableInfo,
    pt_info: *mut model::TableInfo,
    part_name: &str,
) {
    let (_, part_def, err) = get_partition_def(pt_info, part_name);
    require::NoError(t, err);
    let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnDDL);
    let err = kv::RunInNewTxn(ctx, *store, true, |_, txn| {
        let mut m = meta::NewMutator(txn);
        m.DropTableOrView(db_id, unsafe { (*nt_info).ID })
            .expect("Go require.NoError");
        unsafe {
            (*nt_info).ID = (*part_def).ID;
        }
        m.CreateTableOrView(db_id, nt_info)
            .expect("Go require.NoError");
        Ok(())
    });
    require::NoError(t, err);
}

// get_partition_def 对应 Go helper：按名称大小写不敏感查找分区定义，不存在时返回 table.ErrUnknownPartition。
fn get_partition_def(
    tbl_info: *mut model::TableInfo,
    part_name: &str,
) -> (i32, *mut model::PartitionDefinition, errors::Error) {
    let defs = unsafe { &mut (*tbl_info).Partition.Definitions };
    for i in 0..defs.len() {
        if strings::EqualFold(&defs[i].Name.L, &strings::ToLower(part_name)) {
            return (i as i32, &mut defs[i], nil);
        }
    }
    (
        0,
        nil,
        table::ErrUnknownPartition.GenWithStackByArgs(part_name, unsafe { (*tbl_info).Name.O.clone() }),
    )
}
*/
