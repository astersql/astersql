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

// 列名表达式解析后的结果字段（ResultField）。
//
// 在名称解析阶段，将 `ColumnNameExpr` 绑定到具体的列/表元数据，
// 供后续求值与结果集元信息（别名、库表名等）使用。

use std::rc::Rc;

use crate::{ast, model};

/// Binding metadata used by a resolved `ColumnNameExpr` during evaluation.
/// 已解析列名表达式在求值时使用的绑定元数据。
#[derive(Clone, Default)]
pub struct ResultField {
    /// 绑定到的列元信息（ColumnInfo）；表达式列可能为空。
    pub column: Option<Rc<model::ColumnInfo>>,
    /// 列别名（AS name），大小写不敏感字符串 CIStr。
    pub column_as_name: ast::CIStr,
    /// True when an expression has no original column name.
    /// 为 true 表示表达式没有原始列名（如标量计算列）。
    pub empty_org_name: bool,
    /// 所属表的元信息。
    pub table: Option<Rc<model::TableInfo>>,
    /// 表别名。
    pub table_as_name: ast::CIStr,
    /// 所属数据库名。
    pub db_name: ast::CIStr,
}
