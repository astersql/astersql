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
// See the License for the specific language governing permissions and
// limitations under the License.

// Go ast/flag.go applied directly to the parser's real expression nodes.
use super::flag::*;
use super::{ExprKind, ExprNode, Node, Visitor};

pub fn HasAggFlag(expr: &ExprNode) -> bool {
    expr.GetFlag() & FLAG_HAS_AGGREGATE_FUNC != 0
}
pub fn HasWindowFlag(expr: &ExprNode) -> bool {
    expr.GetFlag() & FLAG_HAS_WINDOW_FUNC != 0
}

pub fn SetFlag(node: &dyn Node) {
    struct Setter;
    impl Visitor for Setter {
        fn enter(&mut self, _: &dyn Node) -> bool {
            false
        }
        fn leave(&mut self, node: &dyn Node) -> bool {
            if let Some(expr) = node.as_any().downcast_ref::<ExprNode>() {
                expr.SetFlag(expression_flag(expr));
            }
            true
        }
    }
    node.accept(&mut Setter);
}

fn expression_flag(expr: &ExprNode) -> u64 {
    let flags = |args: &[ExprNode]| args.iter().fold(0, |f, x| f | x.GetFlag());
    match &expr.Kind {
        ExprKind::ParamMarker { .. } => FLAG_HAS_PARAM_MARKER,
        ExprKind::Column(_) => FLAG_HAS_REFERENCE,
        ExprKind::NamedDefault(_) | ExprKind::DefaultValue => FLAG_HAS_DEFAULT,
        ExprKind::Subquery { .. } => FLAG_HAS_SUBQUERY,
        ExprKind::Variable { Value, .. } => {
            FLAG_HAS_VARIABLE | Value.as_ref().map_or(0, |x| x.GetFlag())
        }
        ExprKind::Function {
            Args,
            FnName,
            Schema,
        } => {
            if Schema.O.is_empty() && FnName.L == "values" {
                FLAG_HAS_REFERENCE
            } else {
                FLAG_HAS_FUNC | flags(Args)
            }
        }
        ExprKind::AggregateFunction { Args, .. } => FLAG_HAS_AGGREGATE_FUNC | flags(Args),
        ExprKind::WindowFunction { Args, .. } => FLAG_HAS_WINDOW_FUNC | flags(Args),
        ExprKind::Cast { Expr, .. } => FLAG_HAS_FUNC | Expr.GetFlag(),
        ExprKind::Binary { L, R, .. } | ExprKind::CompareSubquery { L, R, .. } => {
            L.GetFlag() | R.GetFlag()
        }
        ExprKind::Unary { V, .. } | ExprKind::Parentheses(V) => V.GetFlag(),
        ExprKind::IsTruth { Expr, .. } | ExprKind::IsNull { Expr, .. } => Expr.GetFlag(),
        ExprKind::Between {
            Expr, Left, Right, ..
        } => Expr.GetFlag() | Left.GetFlag() | Right.GetFlag(),
        ExprKind::InList { Expr, List, .. } => Expr.GetFlag() | flags(List),
        ExprKind::InSubquery { Expr, Sel, .. } => Expr.GetFlag() | Sel.GetFlag(),
        ExprKind::Like { Expr, Pattern, .. } | ExprKind::Regexp { Expr, Pattern, .. } => {
            Expr.GetFlag() | Pattern.GetFlag()
        }
        ExprKind::ExistsSubquery { Sel, .. } => Sel.GetFlag(),
        ExprKind::Row(args) => flags(args),
        ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            Value.as_ref().map_or(0, |x| x.GetFlag())
                | WhenClauses
                    .iter()
                    .fold(0, |f, x| f | x.Expr.GetFlag() | x.Result.GetFlag())
                | ElseClause.as_ref().map_or(0, |x| x.GetFlag())
        }
        // COLLATE modifies the child expression's type in Go, not its flags.
        ExprKind::Collate { Expr, .. } => Expr.GetFlag(),
        // Go's flag setter has no assignment for these concrete nodes.
        ExprKind::Value(_)
        | ExprKind::IntroducedValue { .. }
        | ExprKind::MaxValue
        | ExprKind::MatchAgainst { .. }
        | ExprKind::TimeUnit(_)
        | ExprKind::GetFormatSelector(_)
        | ExprKind::TrimDirection(_)
        | ExprKind::TableName(_)
        | ExprKind::JSONSumCrc32 { .. } => expr.GetFlag(),
    }
}
