// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// Cascades 变换规则类型枚举。
//
// `Type` 用 `usize` 判别值标识各类 XF（transformation）规则，例如
// Join→Apply、解相关 Apply，以及从各类算子上拉相关谓词（correlated predicate）。
// `XFMaximumRuleLength` 标记枚举上界，供规则表/掩码定长使用。

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 变换规则类型；判别值与 Go 侧规则 ID 对齐。
pub enum Type {
    /// 占位/未指定规则。
    DefaultNone = 0,
    /// 将 Join 改写为 Apply。
    XFJoinToApply = 1,
    /// 解相关简单 Apply（去掉相关子查询依赖）。
    XFDeCorrelateSimpleApply = 2,
    /// 从 Projection 上拉相关谓词。
    XFPullCorrPredFromProj = 3,
    /// 从 Selection（过滤）上拉相关谓词。
    XFPullCorrPredFromSel = 4,
    /// 从 DataSource 上拉相关谓词。
    XFPullCorrPredFromDS = 5,
    /// 从 Sort 上拉相关谓词。
    XFPullCorrPredFromSort = 6,
    /// 从 Limit 上拉相关谓词。
    XFPullCorrPredFromLimit = 7,
    /// 从 Max1Row 上拉相关谓词。
    XFPullCorrPredFromMax1Row = 8,
    /// 从聚合上拉相关谓词（变体 1）。
    XFPullCorrPredFromAgg1 = 9,
    /// 从聚合上拉相关谓词（变体 2）。
    XFPullCorrPredFromAgg2 = 10,
    /// 规则类型数量上界（不含有效规则本体）。
    XFMaximumRuleLength = 11,
}

/// 与 `Type::DefaultNone` 同名常量，便于 Go 风格调用。
pub const DefaultNone: Type = Type::DefaultNone;
/// 与 `Type::XFJoinToApply` 同名常量。
pub const XFJoinToApply: Type = Type::XFJoinToApply;
/// 与 `Type::XFDeCorrelateSimpleApply` 同名常量。
pub const XFDeCorrelateSimpleApply: Type = Type::XFDeCorrelateSimpleApply;
/// 与 `Type::XFPullCorrPredFromProj` 同名常量。
pub const XFPullCorrPredFromProj: Type = Type::XFPullCorrPredFromProj;
/// 与 `Type::XFPullCorrPredFromSel` 同名常量。
pub const XFPullCorrPredFromSel: Type = Type::XFPullCorrPredFromSel;
/// 与 `Type::XFPullCorrPredFromDS` 同名常量。
pub const XFPullCorrPredFromDS: Type = Type::XFPullCorrPredFromDS;
/// 与 `Type::XFPullCorrPredFromSort` 同名常量。
pub const XFPullCorrPredFromSort: Type = Type::XFPullCorrPredFromSort;
/// 与 `Type::XFPullCorrPredFromLimit` 同名常量。
pub const XFPullCorrPredFromLimit: Type = Type::XFPullCorrPredFromLimit;
/// 与 `Type::XFPullCorrPredFromMax1Row` 同名常量。
pub const XFPullCorrPredFromMax1Row: Type = Type::XFPullCorrPredFromMax1Row;
/// 与 `Type::XFPullCorrPredFromAgg1` 同名常量。
pub const XFPullCorrPredFromAgg1: Type = Type::XFPullCorrPredFromAgg1;
/// 与 `Type::XFPullCorrPredFromAgg2` 同名常量。
pub const XFPullCorrPredFromAgg2: Type = Type::XFPullCorrPredFromAgg2;
/// 与 `Type::XFMaximumRuleLength` 同名常量。
pub const XFMaximumRuleLength: Type = Type::XFMaximumRuleLength;

impl Type {
    /// 返回规则的稳定字符串名；多数类型暂映射为 `default_none`。
    pub fn String(&self) -> &'static str {
        match self {
            Type::XFJoinToApply => "join_to_apply",
            _ => "default_none",
        }
    }
}
