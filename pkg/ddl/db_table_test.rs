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

// DDL（数据定义语言）表相关操作的测试模块。
//
// 覆盖场景：建表（含 ID 分配与重名处理）、列定义校验、truncate（截断表）后
// 表锁迁移、drop view/table 的对象类型区分、以及 DDL 作业（job）状态流转。
//
// 术语说明：DDL 指修改库表结构的语句（如 CREATE/ALTER/DROP）；表锁用于
// 阻止其他会话并发读写；DDL 作业指后台异步执行 DDL 的任务单元。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

use std::collections::BTreeSet;
use std::time::Duration;

use crate::executor::{
    ColumnInfo, ColumnKind, Executor, ExecutorError, Ident, MemoryJobBackend, OnExist,
    SessionContext, TableInfo, TableLockType,
};

/// 构造一个整数类型的测试列定义。
///
/// id 置 0 表示由执行器在建表时统一分配；charset/collation 使用 binary
/// 表示按原始字节比较，不做字符集转换。
fn column(name: &str) -> ColumnInfo {
    ColumnInfo {
        id: 0,
        name: name.into(),
        kind: ColumnKind::Integer,
        charset: "binary".into(),
        collation: "binary".into(),
        nullable: true,
        hidden: false,
        generated_dependencies: BTreeSet::new(),
        masking_policy: None,
    }
}

/// 构造只含一个 `id` 列的最小测试表定义。
///
/// 各种可选特性（分区、TTL、TiFlash 副本、放置策略等）均取空值/默认值，
/// id 与 schema_id 置 0 表示由执行器分配全局唯一 ID。
fn table(name: &str) -> TableInfo {
    TableInfo {
        id: 0,
        schema_id: 0,
        name: name.into(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        columns: vec![column("id")],
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        partitions: Vec::new(),
        auto_increment: 0,
        auto_random_bits: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        temporary: false,
        view: false,
        sequence: false,
        cached: false,
        tiflash_replica_count: 0,
        tiflash_available_ids: BTreeSet::new(),
        placement_policy: None,
        affinity: None,
        ttl_column: None,
        table_lock: None,
    }
}

/// 创建基于内存作业后端的 DDL 执行器与默认会话上下文。
///
/// `Duration::ZERO` 表示 DDL 作业无人工延迟，测试同步完成；
/// `SessionContext` 记录会话级状态（如当前持有的表锁）。
fn executor() -> (Executor<MemoryJobBackend>, SessionContext) {
    (
        Executor::new(MemoryJobBackend::default(), Duration::ZERO),
        SessionContext::default(),
    )
}

/// 验证建表会分配正整数表 ID，并遵循 OnExist（同名表存在时）策略：
/// Error 模式返回 TableExists 错误，Ignore 模式返回已有表的 ID。
#[test]
fn create_table_allocates_ids_and_honours_on_exist() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let id = ddl
        .create_table(&mut session, "test", table("t"), OnExist::Error)
        .unwrap();
    assert!(id > 0);
    assert!(ddl.table_exists(&Ident::new("test", "t")));
    // 重复建同名表：Error 策略必须报 TableExists。
    assert!(matches!(
        ddl.create_table(&mut session, "test", table("t"), OnExist::Error),
        Err(ExecutorError::TableExists(_))
    ));
    // Ignore 策略应静默返回原表 ID，不新建。
    assert_eq!(
        id,
        ddl.create_table(&mut session, "test", table("t"), OnExist::Ignore)
            .unwrap()
    );
}

/// 对应 Go `TestCreateTableWithInfo` 的 ID 契约：调用方已分配的全局表 ID
/// 必须原样写入目录；未指定 ID 的表仍由执行器分配非零 ID。
#[test]
fn create_table_preserves_preallocated_id() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();

    let mut preallocated = table("preallocated");
    preallocated.id = 42_042;
    assert_eq!(
        42_042,
        ddl.create_table(&mut session, "test", preallocated, OnExist::Error)
            .unwrap()
    );

    let generated = ddl
        .create_table(&mut session, "test", table("generated"), OnExist::Error)
        .unwrap();
    assert_ne!(0, generated);
    assert_ne!(42_042, generated);
}

/// 对应 Go `TestBatchCreateTable`：成功批量创建全部表；批内名称按
/// 大小写不敏感规则查重，并在提交任何表之前拒绝整批请求。
#[test]
fn batch_create_tables_creates_all_and_rejects_duplicate_names_atomically() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();

    let ids = ddl
        .batch_create_tables(
            &mut session,
            "test",
            vec![table("tables_1"), table("tables_2"), table("tables_3")],
            OnExist::Error,
        )
        .unwrap();
    assert_eq!(3, ids.len());
    assert!(ids.iter().all(|id| *id > 0));
    assert!(ddl.table_exists(&Ident::new("test", "tables_1")));
    assert!(ddl.table_exists(&Ident::new("test", "tables_2")));
    assert!(ddl.table_exists(&Ident::new("test", "tables_3")));

    assert!(matches!(
        ddl.batch_create_tables(
            &mut session,
            "test",
            vec![table("new_table"), table("NEW_TABLE")],
            OnExist::Error,
        ),
        Err(ExecutorError::TableExists(_))
    ));
    assert!(!ddl.table_exists(&Ident::new("test", "new_table")));
}

/// 验证列名查重不区分大小写：`id` 与 `ID` 视为重复列，建表应报 ColumnExists。
/// 这与 MySQL 列名大小写不敏感的语义一致。
#[test]
fn table_definition_rejects_case_insensitive_duplicate_columns() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    // table("...") 已含小写 "id" 列，再追加大写 "ID" 构成大小写冲突。
    let mut invalid = table("duplicate_columns");
    invalid.columns.push(column("ID"));
    assert!(matches!(
        ddl.create_table(&mut session, "test", invalid, OnExist::Error),
        Err(ExecutorError::ColumnExists(_))
    ));
}

/// 验证 truncate（截断表，等价于删表重建）后会话持有的表锁迁移：
/// 截断会分配新的表 ID，旧 ID 上的锁应转移到新 ID 上，锁类型保持不变。
#[test]
fn truncate_table_moves_session_lock_to_new_table_id() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let old_id = ddl
        .create_table(&mut session, "test", table("t"), OnExist::Error)
        .unwrap();
    // 模拟当前会话在旧表 ID 上持有读锁。
    session.locked_tables.insert(old_id, TableLockType::Read);
    let new_id = ddl
        .truncate_table(&mut session, &Ident::new("test", "t"))
        .unwrap();
    // 截断必须生成新表 ID，且锁记录随之迁移。
    assert_ne!(old_id, new_id);
    assert!(!session.locked_tables.contains_key(&old_id));
    assert_eq!(
        Some(&TableLockType::Read),
        session.locked_tables.get(&new_id)
    );
}

/// 验证删除对象前的类型检查：对普通表执行 DROP VIEW（is_view=true）
/// 应报 Unsupported 且不删除元数据；按表删除（is_view=false）才能成功。
#[test]
fn drop_view_checks_object_kind_before_removing_metadata() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(&mut session, "test", table("t"), OnExist::Error)
        .unwrap();
    let ident = Ident::new("test", "t");
    // 以视图身份删除普通表：应被拒绝，表元数据保持不变。
    assert!(matches!(
        ddl.drop_table(&mut session, &ident, false, true),
        Err(ExecutorError::Unsupported(_))
    ));
    assert!(ddl.table_exists(&ident));
    // 以表身份删除：成功并移除元数据。
    ddl.drop_table(&mut session, &ident, false, false).unwrap();
    assert!(!ddl.table_exists(&ident));
}

/// 验证 DDL 作业生命周期：建库 + 建表共产生两条历史作业记录，
/// 且全部到达 Synced（已同步）终态，表示各节点元数据版本已一致。
#[test]
fn created_tables_are_public_and_jobs_reach_synced_history() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let mut info = table("t");
    info.columns[0].hidden = false;
    ddl.create_table(&mut session, "test", info, OnExist::Error)
        .unwrap();
    // 两条作业：create schema 与 create table，均应进入历史队列。
    assert_eq!(2, ddl.backend().history().len());
    assert!(
        ddl.backend()
            .history()
            .iter()
            .all(|job| job.state == crate::executor::JobState::Synced)
    );
}
