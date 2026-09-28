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

// 索引修改（add/drop index）相关的 DDL 测试模块。
//
// 本文件迁移自 Go(TiDB) 的 `index_modify_test.go`，原始测试覆盖以下场景：
// - add/drop index、primary key（主键）在普通表、分区表（partition table）、
//   clustered index（聚簇索引，行数据按主键组织）表上的执行；
// - global index（全局索引，跨分区统一编码的索引）的编码与回查；
// - add index 期间与并发 DML（delete/insert/update）交错时的正确性，
//   以及回滚（rollback）路径（重复键、NULL 值等导致 DDL 失败回滚）；
// - add index 后自动 analyze（收集统计信息）与 stats 版本对齐；
// - vector index（向量索引）与 columnar index（列存倒排索引，依赖 TiFlash 副本）
//   的创建、错误路径与取消/回滚。
//

// 引入 Rust 侧 DDL 执行器的核心类型：
// - Executor：DDL 执行器，负责 create/drop schema、table、index 等操作；
// - MemoryJobBackend：内存版 DDL job（DDL 任务元数据）存储后端，用于测试；
// - SessionContext：会话上下文，携带执行 DDL 所需的会话状态；
// - Ident：库名 + 表名的限定标识符。
use crate::executor::{
    ColumnInfo, DdlAction, Executor, ExecutorError, Ident, IndexInfo, MemoryJobBackend, OnExist,
    SessionContext, TableInfo,
};
use std::time::Duration;

/// 验证索引的校验与重命名行为和 Go 版本的错误语义一致：
/// - 在不存在的列上建索引返回 `ColumnNotFound`；
/// - 索引名大小写不敏感，重复创建返回 `IndexExists`；
/// - rename 后旧名失效，drop 旧名返回 `IndexNotFound`，drop 新名成功。
#[test]
fn index_validation_and_rename_match_go_errors() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 先搭一个最小 schema/table 环境，让后续索引校验专注验证错误语义而非建表前置条件。
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
    // 在不存在的列 "missing" 上建索引，应报列不存在错误。
    assert!(matches!(
        ddl.create_index(
            &mut session,
            &ident,
            IndexInfo::new("bad", vec!["missing".into()]),
            false
        ),
        Err(ExecutorError::ColumnNotFound(_))
    ));
    ddl.create_index(
        &mut session,
        &ident,
        IndexInfo::new("idx", vec!["a".into()]),
        false,
    )
    .unwrap();
    // 索引名比较不区分大小写："IDX" 与已存在的 "idx" 视为重复。
    assert!(matches!(
        ddl.create_index(
            &mut session,
            &ident,
            IndexInfo::new("IDX", vec!["a".into()]),
            false
        ),
        Err(ExecutorError::IndexExists(_))
    ));
    ddl.rename_index(&mut session, &ident, "idx", "renamed")
        .unwrap();
    // 重命名后旧索引名不再存在，按旧名删除应失败。
    assert!(matches!(
        ddl.drop_index(&mut session, &ident, "idx", false),
        Err(ExecutorError::IndexNotFound(_))
    ));
    // 用新名字删除成功，证明 rename 已同步更新内部索引目录。
    ddl.drop_index(&mut session, &ident, "renamed", false)
        .unwrap();
}

/// Go 的 rename-index 校验只把“另一个索引”占用目标名视为重复；因此仅改变
/// 当前索引名的大小写必须成功，并且仍要提交 RenameIndex job。
#[test]
fn case_only_index_rename_matches_go() {
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
        IndexInfo::new("inDex", vec!["a".into()]),
        false,
    )
    .unwrap();

    ddl.rename_index(&mut session, &ident, "inDex", "IndEX")
        .unwrap();

    let table = &ddl.schemas["test"].tables["t"];
    assert_eq!("IndEX", table.indexes[0].name);
    assert_eq!(
        DdlAction::RenameIndex,
        ddl.backend().history().last().unwrap().action
    );
}

/// 验证不允许创建 INVISIBLE（不可见，优化器忽略）的主键索引：
/// 主键是行定位的核心索引，MySQL/TiDB 均禁止将其设为不可见。
#[test]
fn invisible_primary_key_is_rejected() {
    let mut ddl = Executor::new(MemoryJobBackend::default(), Duration::ZERO);
    let mut session = SessionContext::default();
    // 与上面的用例一样，先创建单列表，隔离“主键不可见”这一条校验规则。
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(
        &mut session,
        "test",
        TableInfo::new("t", vec![ColumnInfo::integer("a")]),
        OnExist::Error,
    )
    .unwrap();
    // 构造一个同时标记 primary 与 invisible 的索引定义，触发校验错误。
    let mut index = IndexInfo::new("PRIMARY", vec!["a".into()]);
    index.primary = true;
    index.invisible = true;
    // 该错误应在语义检查阶段直接返回，不需要真的执行建索引流程。
    assert!(matches!(
        ddl.create_index(&mut session, &Ident::new("test", "t"), index, false),
        Err(ExecutorError::InvisiblePrimaryKey)
    ));
}
