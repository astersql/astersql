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

// 表达式标志位自底向上传播，对应 Go 侧 `flag.go`。
//
// 遍历表达式树时先处理子节点，再将子节点标志按位或合并到父节点，并叠加
// 节点自身类型对应的标志（如聚合、窗口、参数占位符、列引用等）。
// 这些标志供绑定（name resolve）与优化器判断表达式性质。

// Bottom-up expression flag propagation, matching `flag.go`.

/// 纯常量表达式（无列引用、函数等副作用相关标记）。
pub const FLAG_CONSTANT: u64 = 0;
/// 含预处理语句参数占位符 `?`。
pub const FLAG_HAS_PARAM_MARKER: u64 = 1 << 1;
/// 含普通函数调用。
pub const FLAG_HAS_FUNC: u64 = 1 << 2;
/// 含列名或位置引用等外部引用。
pub const FLAG_HAS_REFERENCE: u64 = 1 << 3;
/// 含聚合函数（如 SUM/COUNT，跨多行汇总）。
pub const FLAG_HAS_AGGREGATE_FUNC: u64 = 1 << 4;
/// 含子查询。
pub const FLAG_HAS_SUBQUERY: u64 = 1 << 5;
/// 含用户变量或系统变量。
pub const FLAG_HAS_VARIABLE: u64 = 1 << 6;
/// 含 DEFAULT 表达式。
pub const FLAG_HAS_DEFAULT: u64 = 1 << 7;
/// 已在常量折叠等阶段预求值。
pub const FLAG_PRE_EVALUATED: u64 = 1 << 8;
/// 含窗口函数（OVER 子句）。
pub const FLAG_HAS_WINDOW_FUNC: u64 = 1 << 9;

/// 用于标志传播的精简表达式树；叶子可携带预置 flag，复合节点由 set_flag 计算。
#[derive(Clone, Debug, PartialEq)]
pub enum FlagExpr {
    Leaf {
        sql: String,
        flag: u64,
    },
    ParamMarker {
        flag: u64,
    },
    Aggregate {
        args: Vec<FlagExpr>,
        flag: u64,
    },
    Window {
        args: Vec<FlagExpr>,
        flag: u64,
    },
    Between {
        expr: Box<FlagExpr>,
        left: Box<FlagExpr>,
        right: Box<FlagExpr>,
        flag: u64,
    },
    Binary(Box<FlagExpr>, Box<FlagExpr>),
    Case {
        value: Option<Box<FlagExpr>>,
        when_clauses: Vec<(FlagExpr, FlagExpr)>,
        else_clause: Option<Box<FlagExpr>>,
        flag: u64,
    },
    ColumnName {
        flag: u64,
    },
    CompareSubquery {
        left: Box<FlagExpr>,
        right: Box<FlagExpr>,
        flag: u64,
    },
    Default {
        flag: u64,
    },
    ExistsSubquery {
        select: Box<FlagExpr>,
        flag: u64,
    },
    FuncCall {
        args: Vec<FlagExpr>,
        flag: u64,
    },
    FuncCast {
        expr: Box<FlagExpr>,
        flag: u64,
    },
    IsNull {
        expr: Box<FlagExpr>,
        flag: u64,
    },
    IsTruth {
        expr: Box<FlagExpr>,
        flag: u64,
    },
    Parentheses {
        expr: Box<FlagExpr>,
        flag: u64,
    },
    PatternIn {
        expr: Box<FlagExpr>,
        list: Vec<FlagExpr>,
        sel: Option<Box<FlagExpr>>,
        flag: u64,
    },
    PatternLike {
        expr: Option<Box<FlagExpr>>,
        pattern: Box<FlagExpr>,
        flag: u64,
    },
    PatternRegexp {
        expr: Option<Box<FlagExpr>>,
        pattern: Box<FlagExpr>,
        flag: u64,
    },
    Position {
        flag: u64,
    },
    Row {
        values: Vec<FlagExpr>,
        flag: u64,
    },
    Subquery {
        flag: u64,
    },
    Unary {
        value: Box<FlagExpr>,
        flag: u64,
    },
    Values {
        flag: u64,
    },
    Variable {
        value: Option<Box<FlagExpr>>,
        flag: u64,
    },
}

impl FlagExpr {
    /// 读取节点当前已计算（或预置）的标志位。
    pub fn get_flag(&self) -> u64 {
        match self {
            Self::Binary(left, right) => left.get_flag() | right.get_flag(),
            Self::Leaf { flag, .. }
            | Self::ParamMarker { flag }
            | Self::Aggregate { flag, .. }
            | Self::Window { flag, .. }
            | Self::Between { flag, .. }
            | Self::Case { flag, .. }
            | Self::ColumnName { flag }
            | Self::CompareSubquery { flag, .. }
            | Self::Default { flag }
            | Self::ExistsSubquery { flag, .. }
            | Self::FuncCall { flag, .. }
            | Self::FuncCast { flag, .. }
            | Self::IsNull { flag, .. }
            | Self::IsTruth { flag, .. }
            | Self::Parentheses { flag, .. }
            | Self::PatternIn { flag, .. }
            | Self::PatternLike { flag, .. }
            | Self::PatternRegexp { flag, .. }
            | Self::Position { flag }
            | Self::Row { flag, .. }
            | Self::Subquery { flag }
            | Self::Unary { flag, .. }
            | Self::Values { flag }
            | Self::Variable { flag, .. } => *flag,
        }
    }
}

/// 判断表达式是否带有聚合函数标志。
pub fn has_agg_flag(expr: &FlagExpr) -> bool {
    expr.get_flag() & FLAG_HAS_AGGREGATE_FUNC != 0
}

/// 判断表达式是否带有窗口函数标志。
pub fn has_window_flag(expr: &FlagExpr) -> bool {
    expr.get_flag() & FLAG_HAS_WINDOW_FUNC != 0
}

/// Visits children before their parent, exactly like Go's `Accept`/`Leave` flow.
/// 自底向上访问子节点再写回父节点标志，对应 Go 的 Accept/Leave 流程。
pub fn set_flag(expr: &mut FlagExpr) {
    match expr {
        FlagExpr::Leaf { .. }
        | FlagExpr::ColumnName { .. }
        | FlagExpr::Default { .. }
        | FlagExpr::Position { .. }
        | FlagExpr::Subquery { .. }
        | FlagExpr::Values { .. } => {}
        FlagExpr::ParamMarker { flag } => *flag = FLAG_HAS_PARAM_MARKER,
        FlagExpr::Aggregate { args, flag } => {
            // 先递归子参数，再并上聚合函数标志位
            args.iter_mut().for_each(set_flag);
            *flag = FLAG_HAS_AGGREGATE_FUNC | args.iter().fold(0, |v, arg| v | arg.get_flag());
        }
        FlagExpr::Window { args, flag } => {
            args.iter_mut().for_each(set_flag);
            *flag = FLAG_HAS_WINDOW_FUNC | args.iter().fold(0, |v, arg| v | arg.get_flag());
        }
        FlagExpr::Between {
            expr,
            left,
            right,
            flag,
        } => {
            set_flag(expr);
            set_flag(left);
            set_flag(right);
            *flag = expr.get_flag() | left.get_flag() | right.get_flag();
        }
        FlagExpr::Binary(left, right) => {
            set_flag(left);
            set_flag(right);
        }
        FlagExpr::Case {
            value,
            when_clauses,
            else_clause,
            flag,
        } => {
            // 合并 VALUE/WHEN/ELSE 各子表达式的标志
            let mut combined = 0;
            if let Some(value) = value {
                set_flag(value);
                combined |= value.get_flag();
            }
            for (condition, result) in when_clauses {
                set_flag(condition);
                set_flag(result);
                combined |= condition.get_flag() | result.get_flag();
            }
            if let Some(otherwise) = else_clause {
                set_flag(otherwise);
                combined |= otherwise.get_flag();
            }
            *flag = combined;
        }
        FlagExpr::CompareSubquery { left, right, flag } => {
            set_flag(left);
            set_flag(right);
            *flag = left.get_flag() | right.get_flag();
        }
        FlagExpr::ExistsSubquery { select, flag } => {
            set_flag(select);
            *flag = select.get_flag();
        }
        FlagExpr::FuncCall { args, flag } => {
            args.iter_mut().for_each(set_flag);
            *flag = FLAG_HAS_FUNC | args.iter().fold(0, |v, arg| v | arg.get_flag());
        }
        FlagExpr::FuncCast { expr, flag } => {
            set_flag(expr);
            *flag = FLAG_HAS_FUNC | expr.get_flag();
        }
        FlagExpr::IsNull { expr, flag }
        | FlagExpr::IsTruth { expr, flag }
        | FlagExpr::Parentheses { expr, flag }
        | FlagExpr::Unary { value: expr, flag } => {
            set_flag(expr);
            *flag = expr.get_flag();
        }
        FlagExpr::PatternIn {
            expr,
            list,
            sel,
            flag,
        } => {
            set_flag(expr);
            let mut combined = expr.get_flag();
            for item in list {
                set_flag(item);
                combined |= item.get_flag();
            }
            if let Some(select) = sel {
                set_flag(select);
                combined |= select.get_flag();
            }
            *flag = combined;
        }
        FlagExpr::PatternLike {
            expr,
            pattern,
            flag,
        }
        | FlagExpr::PatternRegexp {
            expr,
            pattern,
            flag,
        } => {
            set_flag(pattern);
            let mut combined = pattern.get_flag();
            if let Some(value) = expr {
                set_flag(value);
                combined |= value.get_flag();
            }
            *flag = combined;
        }
        FlagExpr::Row { values, flag } => {
            values.iter_mut().for_each(set_flag);
            *flag = values.iter().fold(0, |v, item| v | item.get_flag());
        }
        FlagExpr::Variable { value, flag } => {
            let child = if let Some(value) = value {
                set_flag(value);
                value.get_flag()
            } else {
                0
            };
            *flag = FLAG_HAS_VARIABLE | child;
        }
    }

    // 叶子型节点在子树遍历后写入固有标志（列引用/DEFAULT/子查询）
    match expr {
        FlagExpr::ColumnName { flag } | FlagExpr::Position { flag } | FlagExpr::Values { flag } => {
            *flag = FLAG_HAS_REFERENCE
        }
        FlagExpr::Default { flag } => *flag = FLAG_HAS_DEFAULT,
        FlagExpr::Subquery { flag } => *flag = FLAG_HAS_SUBQUERY,
        _ => {}
    }
}

/// Go 风格命名别名：[`has_agg_flag`]。
#[allow(non_snake_case)]
pub fn HasAggFlag(expr: &FlagExpr) -> bool {
    has_agg_flag(expr)
}
/// Go 风格命名别名：[`has_window_flag`]。
#[allow(non_snake_case)]
pub fn HasWindowFlag(expr: &FlagExpr) -> bool {
    has_window_flag(expr)
}
/// Go 风格命名别名：[`set_flag`]。
#[allow(non_snake_case)]
pub fn SetFlag(expr: &mut FlagExpr) {
    set_flag(expr)
}
