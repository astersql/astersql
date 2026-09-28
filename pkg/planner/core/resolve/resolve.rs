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

// AST 名称解析上下文与节点包装。
//
// 在遍历 SQL 抽象语法树（AST）时，将 `TableName` 等节点绑定到
// InfoSchema 中的库表元数据（DBInfo/TableInfo）。键控方式与 Go 一致：
// 以 AST 对象指针身份（而非值相等）作为 map 键。

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;

use crate::{ast, model};

/// A resolved AST table name and its database and table metadata.
/// 已解析的 AST 表名，及其对应的库（DBInfo）与表（TableInfo）元数据。
#[derive(Clone)]
pub struct TableNameW {
    /// 原始 AST 表名节点（大小写不敏感字符串 CIStr）。
    pub table_name: Rc<ast::TableName>,
    /// 解析得到的数据库元信息。
    pub db_info: Option<Rc<model::DBInfo>>,
    /// 解析得到的表元信息。
    pub table_info: Option<Rc<model::TableInfo>>,
}

/// An AST node and the resolve context associated with its traversal.
/// 携带 AST 节点及其遍历过程中共享的解析上下文。
#[derive(Clone)]
pub struct NodeW {
    /// 被包装的 AST 节点引用。
    pub node: ast::NodeRef,
    /// 解析上下文；克隆节点时可共享同一份。
    resolve_ctx: Context,
}

impl NodeW {
    /// 用新的独立解析上下文包装 AST 节点。
    pub fn new(node: ast::NodeRef) -> Self {
        Self {
            node,
            resolve_ctx: Context::new(),
        }
    }

    /// 用指定解析上下文包装 AST 节点（可与其他 NodeW 共享）。
    pub fn new_with_ctx(node: ast::NodeRef, resolve_ctx: Context) -> Self {
        Self { node, resolve_ctx }
    }

    /// Replaces the AST node while retaining the exact same resolve context.
    /// 替换 AST 节点但保留完全相同的解析上下文（浅克隆 Context）。
    pub fn clone_with_new_node(&self, new_node: ast::NodeRef) -> Self {
        Self {
            node: new_node,
            resolve_ctx: self.resolve_ctx.clone(),
        }
    }

    /// 返回解析上下文的克隆（内部为 Rc，共享底层 map）。
    pub fn get_resolve_context(&self) -> Context {
        self.resolve_ctx.clone()
    }

    /// Go 风格命名：CloneWithNewNode。
    #[allow(non_snake_case)]
    pub fn CloneWithNewNode(&self, new_node: ast::NodeRef) -> Self {
        self.clone_with_new_node(new_node)
    }

    /// Go 风格命名：GetResolveContext。
    #[allow(non_snake_case)]
    pub fn GetResolveContext(&self) -> Context {
        self.get_resolve_context()
    }
}

/// 以 AST `TableName` 原始指针为键的解析结果表，对齐 Go map[*ast.TableName]。
type TableNameMap = HashMap<*const ast::TableName, Rc<TableNameW>>;

/// Resolve results keyed by AST object identity, matching Go pointer-map keys.
/// 按 AST 对象身份键控的解析结果集合，对齐 Go 指针 map。
#[derive(Clone, Default)]
pub struct Context {
    table_names: Rc<RefCell<TableNameMap>>,
}

impl Context {
    /// 创建空的解析上下文。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记已解析表名；同一 AST 指针再次插入会覆盖旧元数据。
    pub fn add_table_name(&self, table_name_w: Rc<TableNameW>) {
        let key = Rc::as_ptr(&table_name_w.table_name);
        self.table_names.borrow_mut().insert(key, table_name_w);
    }

    /// 按 AST 指针查找已解析的 `TableNameW`。
    pub fn get_table_name(&self, table_name: &Rc<ast::TableName>) -> Option<Rc<TableNameW>> {
        self.table_names
            .borrow()
            .get(&Rc::as_ptr(table_name))
            .cloned()
    }

    /// 借出整个表名 map；与 Go 返回内部 map 一致，调用方可直接修改条目。
    pub fn get_table_names(&self) -> RefMut<'_, TableNameMap> {
        self.table_names.borrow_mut()
    }

    /// Go 风格命名：AddTableName。
    #[allow(non_snake_case)]
    pub fn AddTableName(&self, table_name_w: Rc<TableNameW>) {
        self.add_table_name(table_name_w);
    }

    /// Go 风格命名：GetTableName。
    #[allow(non_snake_case)]
    pub fn GetTableName(&self, table_name: &Rc<ast::TableName>) -> Option<Rc<TableNameW>> {
        self.get_table_name(table_name)
    }

    /// Go 风格命名：GetTableNames。
    #[allow(non_snake_case)]
    pub fn GetTableNames(&self) -> RefMut<'_, TableNameMap> {
        self.get_table_names()
    }
}

/// Go 风格工厂：NewNodeW。
#[allow(non_snake_case)]
pub fn NewNodeW(node: ast::NodeRef) -> NodeW {
    NodeW::new(node)
}

/// Go 风格工厂：NewNodeWWithCtx。
#[allow(non_snake_case)]
pub fn NewNodeWWithCtx(node: ast::NodeRef, resolve_ctx: Context) -> NodeW {
    NodeW::new_with_ctx(node, resolve_ctx)
}

/// Go 风格工厂：NewContext。
#[allow(non_snake_case)]
pub fn NewContext() -> Context {
    Context::new()
}
