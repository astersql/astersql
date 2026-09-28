// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 名称解析（resolve）模块的 AsterSQL 单元测试。
//
// 验证解析上下文按 AST 指针身份键控、节点包装共享/独立上下文，
// 以及 `ResultField` 绑定元数据的保留行为，对齐 Go 侧指针 map 语义。

use std::rc::Rc;

use super::{Context, NodeW, ResultField, TableNameW, ast, model, resolve};

/// 构造带 schema/table 的 AST `TableName`，用于解析键测试。
fn table_name(schema: &str, table: &str) -> Rc<ast::TableName> {
    Rc::new(ast::TableName {
        Schema: ast::NewCIStr(schema),
        Name: ast::NewCIStr(table),
        ..Default::default()
    })
}

/// 将 AST 表名包装为带库表元数据（DBInfo/TableInfo）的 `TableNameW`。
fn wrapped_table(table_name: Rc<ast::TableName>, id: i64) -> Rc<TableNameW> {
    Rc::new(TableNameW {
        table_name,
        db_info: Some(Rc::new(model::DBInfo {
            ID: id,
            ..Default::default()
        })),
        table_info: Some(Rc::new(model::TableInfo {
            ID: id,
            ..Default::default()
        })),
    })
}

/// 同名但不同 Rc 指针的表名应各自独立登记，不能按值相等合并。
#[test]
fn context_uses_ast_pointer_identity_like_go() {
    let context = Context::new();
    let first_name = table_name("app", "orders");
    let equal_but_distinct_name = table_name("app", "orders");
    let first = wrapped_table(first_name.clone(), 11);
    let second = wrapped_table(equal_but_distinct_name.clone(), 22);

    context.add_table_name(first.clone());
    context.add_table_name(second.clone());

    assert_eq!(context.get_table_names().len(), 2);
    assert!(Rc::ptr_eq(
        &context.get_table_name(&first_name).expect("first table"),
        &first
    ));
    assert!(Rc::ptr_eq(
        &context
            .get_table_name(&equal_but_distinct_name)
            .expect("second table"),
        &second
    ));
    // 新建同名 TableName 指针不同，查找应失败。
    assert!(
        context
            .get_table_name(&table_name("app", "orders"))
            .is_none()
    );
}

/// 同一 AST 指针再次 add 时应覆盖已解析的元数据，且 map 长度仍为 1。
#[test]
fn adding_the_same_ast_pointer_replaces_its_resolved_metadata() {
    let context = Context::new();
    let name = table_name("app", "orders");
    context.add_table_name(wrapped_table(name.clone(), 1));
    context.add_table_name(wrapped_table(name.clone(), 2));

    assert_eq!(context.get_table_names().len(), 1);
    assert_eq!(
        context
            .get_table_name(&name)
            .and_then(|entry| entry.table_info.clone())
            .map(|table| table.ID),
        Some(2)
    );
}

/// Go 返回内部 map 本身，调用方可以删除预处理阶段写入的条目。
#[test]
fn get_table_names_exposes_the_mutable_inner_map_like_go() {
    let context = Context::new();
    let name = table_name("app", "orders");
    context.add_table_name(wrapped_table(name.clone(), 1));

    context.GetTableNames().clear();

    assert!(context.get_table_name(&name).is_none());
}

/// clone_with_new_node / new_with_ctx 共享同一解析上下文，写入可互相可见。
#[test]
fn cloned_node_and_explicit_context_share_resolution_results() {
    let original_node = ast::NodeRef::new(Box::new(ast::DoStmt::default()));
    let replacement_node = ast::NodeRef::new(Box::new(ast::ShowStmt::default()));
    let wrapped = NodeW::new(original_node.clone());
    let cloned = wrapped.clone_with_new_node(replacement_node.clone());
    let shared = NodeW::new_with_ctx(replacement_node.clone(), wrapped.get_resolve_context());
    let name = table_name("app", "orders");

    cloned
        .get_resolve_context()
        .add_table_name(wrapped_table(name.clone(), 42));

    assert_eq!(wrapped.node, original_node);
    assert_eq!(cloned.node, replacement_node);
    assert_eq!(shared.node, replacement_node);
    assert_eq!(
        wrapped
            .get_resolve_context()
            .get_table_name(&name)
            .and_then(|entry| entry.table_info.clone())
            .map(|table| table.ID),
        Some(42)
    );
    assert!(shared.get_resolve_context().get_table_name(&name).is_some());
}

/// 每次 `NodeW::new` 应创建独立上下文，互不污染。
#[test]
fn new_nodes_receive_independent_contexts() {
    let first = NodeW::new(ast::NodeRef::new(Box::new(ast::DoStmt::default())));
    let second = NodeW::new(ast::NodeRef::new(Box::new(ast::DoStmt::default())));
    let name = table_name("app", "orders");

    first
        .get_resolve_context()
        .add_table_name(wrapped_table(name.clone(), 7));

    assert!(first.get_resolve_context().get_table_name(&name).is_some());
    assert!(second.get_resolve_context().get_table_name(&name).is_none());
}

/// Go 风格命名入口（NewContext/AddTableName 等）应委托到同一套共享上下文逻辑。
#[test]
fn go_named_entry_points_delegate_to_the_same_shared_context_logic() {
    let context = resolve::NewContext();
    let name = table_name("app", "orders");
    context.AddTableName(wrapped_table(name.clone(), 88));
    let wrapped =
        resolve::NewNodeWWithCtx(ast::NodeRef::new(Box::new(ast::DoStmt::default())), context);
    let cloned = wrapped.CloneWithNewNode(ast::NodeRef::new(Box::new(ast::ShowStmt::default())));
    let fresh = resolve::NewNodeW(ast::NodeRef::new(Box::new(ast::DoStmt::default())));

    assert_eq!(cloned.GetResolveContext().GetTableNames().len(), 1);
    assert!(cloned.GetResolveContext().GetTableName(&name).is_some());
    assert!(fresh.GetResolveContext().GetTableName(&name).is_none());
}

/// `ResultField` 应保留列别名、空原始名标记、表元数据与库名等绑定信息。
#[test]
fn result_field_preserves_binding_metadata_and_empty_org_name() {
    let result = ResultField {
        column: Some(Rc::new(model::ColumnInfo::default())),
        column_as_name: ast::NewCIStr("total"),
        empty_org_name: true,
        table: Some(Rc::new(model::TableInfo {
            ID: 9,
            ..Default::default()
        })),
        table_as_name: ast::NewCIStr("o"),
        db_name: ast::NewCIStr("app"),
    };

    assert_eq!(result.column_as_name.O, "total");
    assert!(result.empty_org_name);
    assert_eq!(result.table.as_ref().map(|table| table.ID), Some(9));
    assert_eq!(result.table_as_name.L, "o");
    assert_eq!(result.db_name.L, "app");
}
