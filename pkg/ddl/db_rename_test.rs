// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 表重命名（RENAME TABLE / ALTER TABLE RENAME）相关的 DDL 测试。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/ALTER/DROP 等改变
// 库表结构（schema，即数据库对象的元数据定义）的语句。本文件覆盖以下场景：
// - 表被表锁（LOCK TABLES）持有时的重命名许可检查；
// - 单表重命名的两种模式（RENAME TABLE 与 ALTER TABLE RENAME）在
//   目标不存在、目标已占用、同名/跨库移动等边界情况下的行为差异；
// - 多表原子重命名（RENAME TABLE t1 TO t3, t2 TO t4 ...），包括链式换名；
// - 重命名与自增 ID（auto_increment）重整（rebase）的并发交互；
// - 通过 DDL 任务队列（Job）观察正在运行的重命名任务
//   （对应 ADMIN SHOW DDL JOBS / information_schema 的展示）。

use std::collections::BTreeMap;
use std::sync::{Arc, Barrier, Mutex};

use astersql_meta_metabuild as metabuild;
use astersql_parser_ast as ast;

use crate::BuildTableInfoFromAST;
use crate::ddl::{Ddl, Job, JobState};
use crate::table::{
    RenameMode, TableCatalog, TableError, TableInfo, TableState, rebase_auto_increment,
};
use crate::table_lock::{
    LockTableInfo, LockTablesArgs, SessionInfo, TableLockError, TableLockState, TableLockTarget,
    TableLockType, check_drop_schema_lock, check_rename_table_lock, lock_table, unlock_table,
};

/// 构造一个处于 Public（对外可见）状态的最小化表元数据 `TableInfo`。
///
/// `schema_id` 为所属数据库（schema）的 ID；其余字段（自增 ID、分区、
/// 排序规则 collation 等）均取默认值，便于测试聚焦于重命名逻辑本身。
fn table(id: i64, schema_id: i64, name: &str) -> TableInfo {
    TableInfo {
        id,
        schema_id,
        name: name.to_owned(),
        state: TableState::Public,
        partition_ids: Vec::new(),
        auto_increment_id: 0,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".to_owned(),
        collation: "utf8mb4_bin".to_owned(),
        version: 0,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    }
}

/// 在 [`table`] 的基础上附加模拟的列定义与行数据（存放于 attributes），
/// 用于验证重命名后表中数据不会丢失。
fn table_with_rows(id: i64, schema_id: i64, name: &str) -> TableInfo {
    let mut table = table(id, schema_id, name);
    table.attributes.insert("columns".into(), "c1,c2".into());
    table.attributes.insert("rows".into(), "1,1|2,2".into());
    table
}

/// 按 DDL 状态机逐步删除一张表，并断言状态依次经过
/// WriteOnly（只写）→ DeleteOnly（只删）→ None（已删除）。
///
/// 这是在线 DDL（online schema change）的多阶段状态迁移：通过中间状态
/// 保证集群各节点在元数据版本不一致时数据仍然一致。
fn drop_table(catalog: &mut TableCatalog, schema_id: i64, name: &str) {
    assert_eq!(
        TableState::WriteOnly,
        catalog.drop_table_step(schema_id, name, 1).unwrap()
    );
    assert_eq!(
        TableState::DeleteOnly,
        catalog.drop_table_step(schema_id, name, 1).unwrap()
    );
    assert_eq!(
        TableState::None,
        catalog.drop_table_step(schema_id, name, 1).unwrap()
    );
}

/// 构造解锁指定表所需的 `LockTablesArgs` 参数（对应 UNLOCK TABLES 语义），
/// 解锁者为 `owner` 会话，锁类型为写锁（Write）。
fn unlock_args(owner: &SessionInfo, table_id: i64) -> LockTablesArgs {
    LockTablesArgs {
        lock_tables: Vec::new(),
        unlock_tables: vec![TableLockTarget {
            schema_id: 1,
            table_id,
            lock_type: TableLockType::Write,
        }],
        session: owner.clone(),
        index_of_lock: 0,
        index_of_unlock: 0,
        cleanup: false,
    }
}

/// 断言表在重命名/移动后仍保留原有的列定义与行数据（模拟数据不丢失）。
fn assert_rows_preserved(table: &TableInfo) {
    assert_eq!(Some(&"c1,c2".to_owned()), table.attributes.get("columns"));
    assert_eq!(Some(&"1,1|2,2".to_owned()), table.attributes.get("rows"));
}

/// 验证表锁与重命名的交互规则：
/// - 持有写锁（Write）的会话可以重命名该表，但不能删除其所在 schema
///   （会返回 LockOrActiveTransaction 错误）；
/// - 仅持有读锁（Read）时重命名被拒绝（TableNotLockedForWrite）；
/// - 释放锁后 drop schema 可以正常执行并返回其中的表。
#[test]
fn test_rename_table_with_locked() {
    let owner = SessionInfo {
        server_id: "tidb-1".into(),
        session_id: 1,
    };
    let mut catalog = TableCatalog::default();
    assert!(catalog.create_schema(1));
    assert!(catalog.create_schema(2));
    catalog.insert(table(10, 1, "t1")).unwrap();

    let mut locked = LockTableInfo {
        schema_id: 1,
        table_id: 10,
        table_name: "t1".into(),
        lock: None,
    };
    // 对 t1 加写锁并将锁状态置为 Public（生效）；
    // 此时删除包含该表的 schema 应被拒绝。
    lock_table(&mut locked, TableLockType::Write, &owner).unwrap();
    locked.lock.as_mut().unwrap().state = TableLockState::Public;
    assert_eq!(
        Err(TableLockError::LockOrActiveTransaction),
        check_drop_schema_lock(std::slice::from_ref(&locked), &owner)
    );
    assert_eq!(Ok(()), check_rename_table_lock(&locked, &owner));
    catalog
        .rename_table_checked(RenameMode::RenameTable, 1, "t1", 1, "t2")
        .unwrap();
    assert_eq!(10, catalog.get(1, "t2").unwrap().id);

    assert!(unlock_table(&mut locked, &unlock_args(&owner, 10)));
    catalog
        .rename_table_checked(RenameMode::RenameTable, 1, "t2", 1, "t1")
        .unwrap();
    assert_eq!(10, catalog.get(1, "t1").unwrap().id);

    // 改为持有读锁：读锁不允许重命名（重命名属于写操作）。
    locked.table_name = "t1".into();
    lock_table(&mut locked, TableLockType::Read, &owner).unwrap();
    locked.lock.as_mut().unwrap().state = TableLockState::Public;
    assert_eq!(
        Err(TableLockError::TableNotLockedForWrite("t1".into())),
        check_rename_table_lock(&locked, &owner)
    );
    assert!(unlock_table(&mut locked, &unlock_args(&owner, 10)));
    assert_eq!(Ok(()), check_drop_schema_lock(&[locked], &owner));
    assert_eq!(
        vec![10],
        catalog
            .drop_schema(1)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>()
    );
}

/// 针对单表重命名的公共测试逻辑，`mode` 区分两种语法：
/// - `RenameMode::RenameTable`：RENAME TABLE 语句；
/// - `RenameMode::AlterTable`：ALTER TABLE ... RENAME 语句。
///
/// 两者在错误报告上有细微差异：源表不存在且目标已占用时，
/// ALTER TABLE 报 NotFound，而 RENAME TABLE 报 AlreadyExists；
/// 同名重命名与仅大小写不同的重命名也只有 ALTER TABLE 允许。
fn exercise_single_rename(mode: RenameMode) {
    let mut catalog = TableCatalog::default();
    assert!(catalog.create_schema(1));
    assert!(catalog.create_schema(2));

    assert_eq!(
        Err(TableError::NotFound),
        catalog.rename_table_checked(mode, 1, "tb1", 1, "tb2")
    );
    // 跨库移动：schema 1 的 t 移动到 schema 2 并更名为 t1，数据应保留。
    catalog.insert(table_with_rows(10, 1, "t")).unwrap();
    catalog.rename_table_checked(mode, 1, "t", 2, "t1").unwrap();
    assert_eq!(Err(TableError::NotFound), catalog.get(1, "t"));
    let moved = catalog.get(2, "t1").unwrap();
    assert_eq!(10, moved.id);
    assert_eq!(2, moved.schema_id);
    assert_rows_preserved(moved);

    catalog.insert(table(11, 1, "t")).unwrap();
    drop_table(&mut catalog, 1, "t");
    catalog
        .rename_table_checked(mode, 2, "t1", 2, "t2")
        .unwrap();
    let moved = catalog.get(2, "t2").unwrap();
    assert_eq!(10, moved.id);
    assert_eq!(2, moved.schema_id);
    assert_rows_preserved(moved);
    assert_eq!(Err(TableError::NotFound), catalog.get(2, "t1"));
    assert_eq!(vec!["t2"], catalog.table_names(2).unwrap());

    // 各类失败场景：源 schema 不存在、源表不存在、目标 schema 不存在。
    assert_eq!(
        Err(TableError::NotFound),
        catalog.rename_table_checked(mode, 99, "t", 99, "t")
    );
    assert_eq!(
        Err(TableError::NotFound),
        catalog.rename_table_checked(mode, 1, "missing", 1, "missing")
    );
    assert_eq!(
        Err(TableError::NotFound),
        catalog.rename_table_checked(mode, 1, "missing", 99, "t")
    );
    assert_eq!(
        Err(TableError::SchemaNotFound),
        catalog.rename_table_checked(mode, 2, "t2", 99, "t")
    );

    // 目标名已被占用：两种模式对"源缺失 + 目标占用"的报错优先级不同。
    catalog.insert(table(12, 2, "occupied")).unwrap();
    assert_eq!(
        Err(TableError::AlreadyExists),
        catalog.rename_table_checked(mode, 2, "t2", 2, "occupied")
    );
    let missing_to_occupied = catalog.rename_table_checked(mode, 1, "missing", 2, "occupied");
    if mode == RenameMode::AlterTable {
        assert_eq!(Err(TableError::NotFound), missing_to_occupied);
    } else {
        assert_eq!(Err(TableError::AlreadyExists), missing_to_occupied);
    }
    let missing_schema_to_occupied =
        catalog.rename_table_checked(mode, 99, "missing", 2, "occupied");
    if mode == RenameMode::AlterTable {
        assert_eq!(Err(TableError::NotFound), missing_schema_to_occupied);
    } else {
        assert_eq!(Err(TableError::AlreadyExists), missing_schema_to_occupied);
    }

    // 同名重命名与仅大小写变化的重命名：只有 ALTER TABLE 模式允许。
    catalog.insert(table(13, 2, "same")).unwrap();
    catalog.insert(table(14, 2, "case_name")).unwrap();
    if mode == RenameMode::AlterTable {
        catalog
            .rename_table_checked(mode, 2, "same", 2, "same")
            .unwrap();
        catalog
            .rename_table_checked(mode, 2, "case_name", 2, "CASE_NAME")
            .unwrap();
        assert_eq!(14, catalog.get(2, "case_name").unwrap().id);
    } else {
        assert_eq!(
            Err(TableError::AlreadyExists),
            catalog.rename_table_checked(mode, 2, "same", 2, "same")
        );
        assert_eq!(
            Err(TableError::AlreadyExists),
            catalog.rename_table_checked(mode, 2, "case_name", 2, "CASE_NAME")
        );
    }
    // 表名超过 64 字符（MySQL 的标识符长度上限）应报 NameTooLong。
    assert_eq!(
        Err(TableError::NameTooLong),
        catalog.rename_table_checked(mode, 2, "case_name", 2, &"x".repeat(65))
    );
}

/// RENAME TABLE 语句模式下的单表重命名测试。
#[test]
fn test_rename_table_2() {
    exercise_single_rename(RenameMode::RenameTable);
}

/// ALTER TABLE ... RENAME 语句模式下的单表重命名测试。
#[test]
fn test_alter_table_rename_table() {
    exercise_single_rename(RenameMode::AlterTable);
}

/// 验证多表批量重命名（RENAME TABLE 一次改多张表）：
/// - 同库批量改名、跨库批量移动均保留数据；
/// - 支持链式换名（t3→t1、t4→t2、t5→t3 在同一批中完成）；
/// - 批中任何一项失败（表/库不存在、目标冲突）整体报错，
///   且原有表保持不变（原子性）。
#[test]
fn test_rename_multi_tables() {
    let mut catalog = TableCatalog::default();
    assert!(catalog.create_schema(1));
    assert!(catalog.create_schema(2));
    catalog.insert(table(1, 1, "t1")).unwrap();
    catalog.insert(table(2, 1, "t2")).unwrap();
    catalog
        .rename_tables_checked(&[
            (1, "t1".into(), 1, "t3".into()),
            (1, "t2".into(), 1, "t4".into()),
        ])
        .unwrap();
    assert_eq!(1, catalog.get(1, "t3").unwrap().id);
    assert_eq!(2, catalog.get(1, "t4").unwrap().id);
    drop_table(&mut catalog, 1, "t3");
    drop_table(&mut catalog, 1, "t4");

    catalog.insert(table_with_rows(10, 1, "t1")).unwrap();
    catalog.insert(table_with_rows(20, 1, "t2")).unwrap();
    catalog
        .rename_tables_checked(&[
            (1, "t1".into(), 2, "t1".into()),
            (1, "t2".into(), 2, "t2".into()),
        ])
        .unwrap();
    assert_eq!(Err(TableError::NotFound), catalog.get(1, "t1"));
    assert_eq!(Err(TableError::NotFound), catalog.get(1, "t2"));
    for (name, id) in [("t1", 10), ("t2", 20)] {
        let moved = catalog.get(2, name).unwrap();
        assert_eq!(id, moved.id);
        assert_rows_preserved(moved);
    }

    catalog
        .rename_tables_checked(&[
            (2, "t1".into(), 2, "t3".into()),
            (2, "t2".into(), 2, "t4".into()),
        ])
        .unwrap();
    assert_eq!(vec!["t3", "t4"], catalog.table_names(2).unwrap());
    catalog.insert(table_with_rows(30, 2, "t5")).unwrap();
    // 链式换名：新名字与批内其他表的旧名字重叠，也应按顺序正确处理。
    catalog
        .rename_tables_checked(&[
            (2, "t3".into(), 2, "t1".into()),
            (2, "t4".into(), 2, "t2".into()),
            (2, "t5".into(), 2, "t3".into()),
        ])
        .unwrap();
    assert_eq!(vec!["t1", "t2", "t3"], catalog.table_names(2).unwrap());
    assert_eq!(10, catalog.get(2, "t1").unwrap().id);
    assert_eq!(20, catalog.get(2, "t2").unwrap().id);
    assert_eq!(30, catalog.get(2, "t3").unwrap().id);

    catalog
        .rename_tables_checked(&[
            (2, "t1".into(), 1, "t2".into()),
            (2, "t2".into(), 1, "t3".into()),
            (2, "t3".into(), 1, "t4".into()),
        ])
        .unwrap();
    assert_eq!(vec!["t2", "t3", "t4"], catalog.table_names(1).unwrap());
    assert_eq!(10, catalog.get(1, "t2").unwrap().id);
    assert_eq!(20, catalog.get(1, "t3").unwrap().id);
    assert_eq!(30, catalog.get(1, "t4").unwrap().id);

    // 一组必然失败的批量重命名：schema 不存在、表不存在、目标 schema 缺失、
    // 同一源表重复出现等，都应整体返回 NotFound 且不产生任何副作用。
    for renames in [
        vec![
            (99, "t".into(), 99, "t".into()),
            (99, "t".into(), 99, "t".into()),
        ],
        vec![
            (1, "missing".into(), 1, "missing".into()),
            (1, "missing2".into(), 1, "missing2".into()),
        ],
        vec![
            (1, "missing".into(), 99, "t".into()),
            (1, "missing2".into(), 99, "t2".into()),
        ],
        vec![
            (2, "t2".into(), 99, "t".into()),
            (2, "t2".into(), 99, "t2".into()),
        ],
    ] {
        assert_eq!(
            Err(TableError::NotFound),
            catalog.rename_tables_checked(&renames)
        );
    }
    assert_eq!(vec!["t2", "t3", "t4"], catalog.table_names(1).unwrap());
}

/// 解析一条 CREATE TABLE 语句并复制出其 AST（抽象语法树）节点。
///
/// 通过 SQL 解析器得到 `CreateTableStmt`，再手工克隆各字段以获得
/// 独立所有权的 `Box`，供后续构建表元数据使用。
fn parse_create(sql: &str) -> Box<ast::CreateTableStmt> {
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(sql, "", "")
        .expect("parse CREATE TABLE");
    let create = statement
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("CREATE TABLE AST");
    Box::new(ast::CreateTableStmt {
        node_text: Default::default(),
        IfNotExists: create.IfNotExists,
        TemporaryKeyword: create.TemporaryKeyword,
        OnCommitDelete: create.OnCommitDelete,
        Table: create.Table.clone(),
        ReferTable: create.ReferTable.clone(),
        Cols: create.Cols.clone(),
        Constraints: create.Constraints.clone(),
        Options: create.Options.clone(),
        Partition: create.Partition.clone(),
        SplitIndex: create.SplitIndex.clone(),
        OnDuplicate: create.OnDuplicate,
        Select: None,
    })
}

/// 对应 TiDB issue #47064 的回归测试：批量跨库重命名后，
/// 表的列信息（columns 属性）与 Public 状态必须保持完整。
/// 同时验证从 CREATE TABLE 的 AST 构建表元数据时列名解析正确。
#[test]
fn test_rename_multi_tables_issue_47064() {
    let statement = parse_create("create table t1(a int)");
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let formal = BuildTableInfoFromAST(&context, &statement).expect("build table metadata");
    assert_eq!(1, formal.Columns.len());
    assert_eq!("a", formal.Columns[0].Name.L);

    let mut catalog = TableCatalog::default();
    assert!(catalog.create_schema(1));
    assert!(catalog.create_schema(2));
    let mut t1 = table(1, 1, "t1");
    t1.attributes.insert("columns".into(), "a".into());
    let mut t2 = table(2, 1, "t2");
    t2.attributes.insert("columns".into(), "a".into());
    catalog.insert(t1).unwrap();
    catalog.insert(t2).unwrap();
    catalog
        .rename_tables_checked(&[
            (1, "t1".into(), 2, "t1".into()),
            (1, "t2".into(), 2, "t2".into()),
        ])
        .unwrap();
    assert_eq!(
        Some(&"a".to_owned()),
        catalog.get(2, "t1").unwrap().attributes.get("columns")
    );
    assert_eq!(TableState::Public, catalog.get(2, "t1").unwrap().state);
}

/// 验证重命名与自增 ID 分配的并发交互：
/// - 使用两个线程（借助 Barrier 同步）先重命名再执行
///   `rebase_auto_increment`（把自增基值抬高到指定值）；
/// - `auto_id_schema_id` 记录自增 ID 归属的原始 schema，跨库移动后
///   仍指向最初分配自增序列的库，移回原库时被清零；
/// - 多次重命名/移库后自增基值与 `auto_id_cache`（自增缓存步长）不丢失。
#[test]
fn test_rename_concurrent_auto_id() {
    let catalog = Arc::new(Mutex::new(TableCatalog::default()));
    {
        let mut guard = catalog.lock().unwrap();
        assert!(guard.create_schema(1));
        assert!(guard.create_schema(2));
        let mut original = table(1, 1, "t1");
        original.auto_id_cache = 5;
        original.auto_increment_id = 5;
        guard.insert(original).unwrap();
    }
    // Barrier 保证 rebase 线程在 rename 线程完成后才开始执行。
    let public = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let rename_catalog = Arc::clone(&catalog);
        let rename_public = Arc::clone(&public);
        scope.spawn(move || {
            rename_catalog
                .lock()
                .unwrap()
                .rename_table_checked(RenameMode::RenameTable, 1, "t1", 2, "t2")
                .unwrap();
            rename_public.wait();
        });
        let rebase_catalog = Arc::clone(&catalog);
        let rebase_public = Arc::clone(&public);
        scope.spawn(move || {
            rebase_public.wait();
            let mut guard = rebase_catalog.lock().unwrap();
            let renamed = guard.get_mut(2, "t2").unwrap();
            assert_eq!(1, renamed.auto_id_schema_id);
            assert!(rebase_auto_increment(renamed, 15, true).unwrap());
        });
    });

    // 并发阶段结束后继续在主线程验证：改名不影响自增归属与基值。
    let mut guard = catalog.lock().unwrap();
    guard
        .rename_table_checked(RenameMode::RenameTable, 2, "t2", 2, "t1")
        .unwrap();
    assert_eq!(1, guard.get(2, "t1").unwrap().auto_id_schema_id);
    assert_eq!(15, guard.get(2, "t1").unwrap().auto_increment_id);
    for base in [17, 19, 22, 24] {
        assert!(rebase_auto_increment(guard.get_mut(2, "t1").unwrap(), base, true).unwrap());
    }
    // 移回原始 schema 1 时 auto_id_schema_id 清零（不再需要跨库指针）。
    guard
        .rename_table_checked(RenameMode::RenameTable, 2, "t1", 1, "t1")
        .unwrap();
    assert_eq!(0, guard.get(1, "t1").unwrap().auto_id_schema_id);
    guard
        .rename_table_checked(RenameMode::RenameTable, 1, "t1", 2, "t2")
        .unwrap();
    assert_eq!(1, guard.get(2, "t2").unwrap().auto_id_schema_id);
    assert!(guard.drop_schema(1).unwrap().is_empty());
    for base in [30, 32] {
        assert!(rebase_auto_increment(guard.get_mut(2, "t2").unwrap(), base, true).unwrap());
    }
    guard
        .rename_table_checked(RenameMode::RenameTable, 2, "t2", 2, "t1")
        .unwrap();
    for base in [35, 37] {
        assert!(rebase_auto_increment(guard.get_mut(2, "t1").unwrap(), base, true).unwrap());
    }
    assert!(guard.create_schema(3));
    guard
        .rename_table_checked(RenameMode::RenameTable, 2, "t1", 3, "t1")
        .unwrap();
    for base in [39, 40] {
        assert!(rebase_auto_increment(guard.get_mut(3, "t1").unwrap(), base, true).unwrap());
    }
    let final_table = guard.get(3, "t1").unwrap();
    assert_eq!(1, final_table.id);
    assert_eq!(1, final_table.auto_id_schema_id);
    assert_eq!(40, final_table.auto_increment_id);
    assert_eq!(5, final_table.auto_id_cache);
}

/// 验证运行中的重命名任务可通过 DDL 任务队列查询：
/// 提交一个 rename job 后，模拟 ADMIN SHOW DDL JOBS（按非 Synced 状态过滤）
/// 与 information_schema（按 query 前缀过滤）两种查询视角，
/// 均能看到该任务及其目标 schema/table ID；任务完成后状态转为
/// Synced（元数据已在集群内同步）并从队列移除。
#[test]
fn test_show_running_rename_table() {
    let mut catalog = TableCatalog::default();
    assert!(catalog.create_schema(1));
    assert!(catalog.create_schema(2));
    catalog.insert(table(10, 1, "t1")).unwrap();
    catalog
        .rename_table_checked(RenameMode::RenameTable, 1, "t1", 2, "t2")
        .unwrap();

    let mut ddl = Ddl::new("rename-running", Vec::new());
    ddl.started = true;
    ddl.submit_job(Job {
        id: 1,
        query: "rename table test.t1 to test2.t2".into(),
        state: JobState::None,
        version: 0,
        start_ts: 100,
        real_start_ts: 0,
        action_type: crate::ddl::ActionType::Other,
        table_id: 10,
        schema_id: 2,
        paused_by: None,
    })
    .unwrap();

    let admin_rows: Vec<_> = ddl
        .all_jobs()
        .into_iter()
        .filter(|job| job.state != JobState::Synced)
        .collect();
    assert_eq!(1, admin_rows.len());
    assert_eq!(2, admin_rows[0].schema_id);
    assert_eq!(10, admin_rows[0].table_id);
    assert_eq!("t2", catalog.get(2, "t2").unwrap().name);
    let information_schema_rows: Vec<_> = ddl
        .all_jobs()
        .into_iter()
        .filter(|job| job.query.starts_with("rename table"))
        .collect();
    assert_eq!(1, information_schema_rows.len());
    assert_eq!(2, information_schema_rows[0].schema_id);
    assert_eq!("t2", catalog.get(2, "t2").unwrap().name);

    ddl.finish_job(1).unwrap();
    assert_eq!(JobState::Synced, ddl.all_jobs()[0].state);
    assert!(ddl.jobs.is_empty());
}
