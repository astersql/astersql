// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// SQL DML（数据操纵语言）AST 基础类型，自 `dml.go` 移植。
//
// 类型采用值持有数据（而非共享可变引用），对应 Go 侧指针形 AST 在 Rust 中的
// 所有权表达。`restore` 方法保持与 TiDB 相同的关键字、标识符引用与子节点顺序。

#![allow(non_snake_case)]

// SQL DML AST primitives ported from `dml.go`.
//
// The types in this file deliberately own their data. That is the Rust
// equivalent of Go's pointer-shaped AST without introducing shared mutable
// state. `Restore` methods preserve TiDB's keyword, quoting, and child order.

/// Quotes a MySQL identifier, including the escaping used by RestoreCtx.WriteName.
/// 按 MySQL 标识符规则加反引号，并将内部 `` ` `` 转义为 ```` `` ````。
pub fn quote_name(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// 还原 ORDER/GROUP BY 等处的表达式：NULL/?/数字/字面量原样输出，否则按点号分段引用标识符。
fn restore_expression(expression: &str) -> String {
    if expression.eq_ignore_ascii_case("null")
        || expression == "?"
        || expression.parse::<i128>().is_ok()
        || expression.starts_with('\'')
    {
        expression.to_owned()
    } else {
        expression
            .split('.')
            .map(quote_name)
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// 表名：可选 schema、表名、分区名列表与索引 Hint。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableName {
    pub schema: Option<String>,
    pub name: String,
    pub partition_names: Vec<String>,
    pub index_hints: Vec<IndexHint>,
}

impl TableName {
    /// 构造不含分区与 Hint 的表名。
    pub fn new(schema: Option<&str>, name: &str) -> Self {
        Self {
            schema: schema.map(str::to_owned),
            name: name.to_owned(),
            partition_names: Vec::new(),
            index_hints: Vec::new(),
        }
    }

    /// 还原为 schema.table、PARTITION(...) 与索引 Hint 文本。
    pub fn restore(&self) -> String {
        let mut output = match &self.schema {
            Some(schema) if !schema.is_empty() => {
                format!("{}.{}", quote_name(schema), quote_name(&self.name))
            }
            _ => quote_name(&self.name),
        };
        if !self.partition_names.is_empty() {
            output.push_str(" PARTITION(");
            output.push_str(
                &self
                    .partition_names
                    .iter()
                    .map(|name| quote_name(name))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            output.push(')');
        }
        for hint in &self.index_hints {
            output.push(' ');
            output.push_str(&hint.restore());
        }
        output
    }
}

/// 索引 Hint 种类：USE/IGNORE/FORCE 等。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexHintType {
    Use,
    Ignore,
    Force,
    Order,
    NoOrder,
}

/// 索引 Hint 作用域：扫描、JOIN、ORDER BY、GROUP BY。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexHintScope {
    Scan,
    Join,
    OrderBy,
    GroupBy,
}

/// 单条索引 Hint：类型、作用域与索引名列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexHint {
    pub hint_type: IndexHintType,
    pub scope: IndexHintScope,
    pub index_names: Vec<String>,
}

impl IndexHint {
    /// 还原为 `USE INDEX FOR ... (...)` 等形式。
    pub fn restore(&self) -> String {
        let kind = match self.hint_type {
            IndexHintType::Use => "USE INDEX",
            IndexHintType::Ignore => "IGNORE INDEX",
            IndexHintType::Force => "FORCE INDEX",
            IndexHintType::Order => "ORDER INDEX",
            IndexHintType::NoOrder => "NO ORDER INDEX",
        };
        let scope = match self.scope {
            IndexHintScope::Scan => "",
            IndexHintScope::Join => " FOR JOIN",
            IndexHintScope::OrderBy => " FOR ORDER BY",
            IndexHintScope::GroupBy => " FOR GROUP BY",
        };
        let names = self
            .index_names
            .iter()
            .map(|name| quote_name(name))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{kind}{scope} ({names})")
    }
}

/// LIMIT 子句：可选 offset 与 count（以字符串保存以兼容参数占位符）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limit {
    pub count: String,
    pub offset: Option<String>,
}

impl Limit {
    /// 构造 LIMIT；`offset` 为 None 时仅输出 count。
    pub fn new(offset: Option<&str>, count: &str) -> Self {
        Self {
            count: count.to_owned(),
            offset: offset.map(str::to_owned),
        }
    }

    /// 还原为 `LIMIT n` 或 `LIMIT offset,count`。
    pub fn restore(&self) -> String {
        match &self.offset {
            Some(offset) => format!("LIMIT {offset},{}", self.count),
            None => format!("LIMIT {}", self.count),
        }
    }
}

/// SELECT 列表中的通配符字段：`*`、`t.*` 或 `db.t.*`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WildCardField {
    pub schema: Option<String>,
    pub table: Option<String>,
}

impl WildCardField {
    /// 按 schema/table 可选前缀构造通配符字段。
    pub fn new(schema: Option<&str>, table: Option<&str>) -> Self {
        Self {
            schema: schema.map(str::to_owned),
            table: table.map(str::to_owned),
        }
    }

    /// 还原通配符字段文本。
    pub fn restore(&self) -> String {
        match (&self.schema, &self.table) {
            (Some(schema), Some(table)) => {
                format!("{}.{}.*", quote_name(schema), quote_name(table))
            }
            (_, Some(table)) => format!("{}.*", quote_name(table)),
            _ => "*".to_owned(),
        }
    }
}

/// ORDER/GROUP/PARTITION BY 中的单项：表达式、是否 DESC、NULLS 顺序。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByItem {
    pub expression: String,
    pub desc: bool,
    pub null_order: bool,
}

impl ByItem {
    /// 构造 ByItem；默认不启用 NULLS FIRST 特殊输出。
    pub fn new(expression: &str, desc: bool) -> Self {
        Self {
            expression: expression.to_owned(),
            desc,
            null_order: false,
        }
    }

    /// 还原表达式并附加 DESC / NULLS FIRST。
    pub fn restore(&self) -> String {
        let mut output = restore_expression(&self.expression);
        if self.desc {
            output.push_str(" DESC");
        }
        output
    }
}

macro_rules! item_clause {
    ($name:ident, $keyword:literal, $separator:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name {
            pub items: Vec<ByItem>,
        }

        impl $name {
            /// 构造 Frame 子句。
            pub fn new(items: Vec<ByItem>) -> Self {
                Self { items }
            }

            /// 还原 Frame 边界关键字。
            pub fn restore(&self) -> String {
                format!(
                    concat!($keyword, " {}"),
                    self.items
                        .iter()
                        .map(ByItem::restore)
                        .collect::<Vec<_>>()
                        .join($separator)
                )
            }
        }
    };
}

item_clause!(GroupByClause, "GROUP BY", ",");
item_clause!(OrderByClause, "ORDER BY", ",");
item_clause!(PartitionByClause, "PARTITION BY", ", ");

/// 窗口 Frame 类型：ROWS / RANGE / GROUPS。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameType {
    Rows,
    Range,
    Groups,
}

/// Frame 边界方向：PRECEDING / FOLLOWING。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundDirection {
    Preceding,
    Following,
}

/// 窗口 Frame 单侧边界：当前行、无界或表达式+方向。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameBound {
    CurrentRow,
    Unbounded(BoundDirection),
    Expr {
        expression: String,
        direction: BoundDirection,
    },
}

impl FrameBound {
    /// 构造 CURRENT ROW 边界。
    pub fn current_row() -> Self {
        Self::CurrentRow
    }

    /// 构造 `expr PRECEDING` 边界。
    pub fn preceding(expression: &str) -> Self {
        Self::Expr {
            expression: expression.to_owned(),
            direction: BoundDirection::Preceding,
        }
    }

    /// 构造 `expr FOLLOWING` 边界。
    pub fn following(expression: &str) -> Self {
        Self::Expr {
            expression: expression.to_owned(),
            direction: BoundDirection::Following,
        }
    }

    /// 还原 PRECEDING/FOLLOWING 关键字。
    pub fn restore(&self) -> String {
        match self {
            Self::CurrentRow => "CURRENT ROW".to_owned(),
            Self::Unbounded(direction) => {
                format!("UNBOUNDED {}", direction.restore())
            }
            Self::Expr {
                expression,
                direction,
            } => format!("{expression} {}", direction.restore()),
        }
    }
}

impl BoundDirection {
    /// 还原完整 Frame 子句。
    fn restore(self) -> &'static str {
        match self {
            Self::Preceding => "PRECEDING",
            Self::Following => "FOLLOWING",
        }
    }
}

/// 完整窗口 Frame：`ROWS/RANGE/GROUPS BETWEEN ... AND ...`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameClause {
    pub frame_type: FrameType,
    pub start: FrameBound,
    pub end: FrameBound,
}

impl FrameClause {
    /// 构造左右表的指定类型 Join。
    pub fn new(frame_type: FrameType, start: FrameBound, end: FrameBound) -> Self {
        Self {
            frame_type,
            start,
            end,
        }
    }

    /// 按 Go `FrameClause.Restore` 的错误契约还原窗口 Frame。
    pub fn try_restore(&self) -> Result<String, &'static str> {
        let frame_type = match self.frame_type {
            FrameType::Rows => "ROWS",
            FrameType::Range => "RANGE",
            FrameType::Groups => return Err("Unsupported window function frame type"),
        };
        Ok(format!(
            "{frame_type} BETWEEN {} AND {}",
            self.start.restore(),
            self.end.restore()
        ))
    }

    /// 还原有效的 ROWS/RANGE Frame；无错误处理需求的调用方可使用此便捷接口。
    pub fn restore(&self) -> String {
        self.try_restore()
            .expect("unsupported window function frame type")
    }
}

/// 连接类型：交叉连接、左连接、右连接。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JoinType {
    #[default]
    CrossJoin,
    LeftJoin,
    RightJoin,
}

/// FROM 子句中的结果集：表、嵌套 Join 或括号查询。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResultSet {
    Table(TableName),
    Join(Box<Join>),
    Query(String),
}

impl ResultSet {
    /// 以裸表名构造结果集。
    pub fn table(name: &str) -> Self {
        Self::Table(TableName::new(None, name))
    }

    /// 按 TiDB 规则还原 Join 文本（含嵌套括号）。
    pub fn restore(&self) -> String {
        match self {
            Self::Table(table) => table.restore(),
            Self::Join(join) => join.restore(),
            Self::Query(query) => format!("({query})"),
        }
    }
}

/// Join 节点：左右结果集、连接类型、ON/USING、NATURAL/STRAIGHT 与显式括号。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Join {
    pub left: ResultSet,
    pub right: Option<ResultSet>,
    pub join_type: JoinType,
    pub on: Option<String>,
    pub using: Vec<String>,
    pub natural_join: bool,
    pub straight_join: bool,
    pub explicit_parens: bool,
}

impl Join {
    pub fn new(left: ResultSet, right: ResultSet, join_type: JoinType) -> Self {
        Self {
            left,
            right: Some(right),
            join_type,
            on: None,
            using: Vec::new(),
            natural_join: false,
            straight_join: false,
            explicit_parens: false,
        }
    }

    /// 标记为 NATURAL JOIN。
    pub fn natural(mut self) -> Self {
        self.natural_join = true;
        self
    }

    /// 设置 USING 列列表。
    pub fn using(mut self, columns: Vec<&str>) -> Self {
        self.using = columns.into_iter().map(str::to_owned).collect();
        self
    }

    /// 设置 ON 条件表达式文本。
    pub fn on(mut self, expression: &str) -> Self {
        self.on = Some(expression.to_owned());
        self
    }

    /// 标记为 STRAIGHT_JOIN（强制左表驱动）。
    pub fn straight(mut self) -> Self {
        self.straight_join = true;
        self
    }

    /// 标记右子树带显式括号，影响 NewCrossJoin 改写。
    pub fn explicit_parens(mut self) -> Self {
        self.explicit_parens = true;
        self
    }

    pub fn restore(&self) -> String {
        let left_is_join = matches!(self.left, ResultSet::Join(_));
        let mut output = if left_is_join {
            format!("({})", self.left.restore())
        } else {
            self.left.restore()
        };
        let Some(right) = &self.right else {
            return output;
        };
        if self.natural_join {
            output.push_str(" NATURAL");
        }
        match self.join_type {
            JoinType::LeftJoin => output.push_str(" LEFT"),
            JoinType::RightJoin => output.push_str(" RIGHT"),
            JoinType::CrossJoin => {}
        }
        output.push_str(if self.straight_join {
            " STRAIGHT_JOIN "
        } else {
            " JOIN "
        });
        if matches!(right, ResultSet::Join(_)) {
            output.push('(');
            output.push_str(&right.restore());
            output.push(')');
        } else {
            output.push_str(&right.restore());
        }
        if let Some(on) = &self.on {
            output.push_str(" ON ");
            output.push_str(on);
        }
        if !self.using.is_empty() {
            output.push_str(" USING (");
            output.push_str(
                &self
                    .using
                    .iter()
                    .map(|column| quote_name(column))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            output.push(')');
        }
        output
    }
}

/// Builds a cross join while preserving MySQL join precedence.
///
/// This mirrors `NewCrossJoin` in Go: an explicit right subtree stays scoped;
/// otherwise the new join is inserted at the subtree's left-most leaf.
/// 构建交叉连接并保持 MySQL Join 优先级：无显式括号时插入到右子树最左叶。
pub fn NewCrossJoin(left: ResultSet, mut right: ResultSet) -> Join {
    let ResultSet::Join(right_join) = &mut right else {
        return Join::new(left, right, JoinType::CrossJoin);
    };
    if right_join.right.is_none() || right_join.explicit_parens {
        return Join::new(left, right, JoinType::CrossJoin);
    }

    // 沿左子树下降到可插入的最左 Join 叶
    let mut current = right_join.as_mut();
    loop {
        if current.join_type == JoinType::RightJoin && current.explicit_parens {
            // 右连接无括号时转为左连接以匹配 MySQL 结合性
            std::mem::swap(&mut current.left, current.right.as_mut().unwrap());
            current.join_type = JoinType::LeftJoin;
        }
        let descend = matches!(
            current.left,
            ResultSet::Join(ref child) if child.right.is_some()
        );
        if !descend {
            break;
        }
        let ResultSet::Join(child) = &mut current.left else {
            unreachable!();
        };
        current = child.as_mut();
    }
    let old_left = std::mem::replace(&mut current.left, ResultSet::Query(String::new()));
    current.left = ResultSet::Join(Box::new(Join::new(left, old_left, JoinType::CrossJoin)));
    match right {
        ResultSet::Join(join) => *join,
        _ => unreachable!(),
    }
}
