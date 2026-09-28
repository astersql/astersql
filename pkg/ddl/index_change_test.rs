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
