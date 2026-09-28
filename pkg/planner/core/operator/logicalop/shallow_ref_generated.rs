// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// 逻辑算子浅引用（ShallowRef）生成代码。
// 浅拷贝算子本体字段以便规则变换时共享不可变子结构，并提供可变切片的写时复制入口。

use crate::*;

impl LogicalJoin {
    /// 浅拷贝 Join 关键字段，未列出的字段走 Default（不含子树/Schema 生产者状态）。
    pub fn LogicalJoinShallowRef(&self) -> Self {
        Self {
            JoinType: self.JoinType,
            Reordered: self.Reordered,
            StraightJoin: self.StraightJoin,
            PreferJoinType: self.PreferJoinType,
            PreferJoinOrder: self.PreferJoinOrder,
            InternalPreferJoinOrder: self.InternalPreferJoinOrder,
            LeftPreferJoinType: self.LeftPreferJoinType,
            RightPreferJoinType: self.RightPreferJoinType,
            EqualConditions: self.EqualConditions.clone(),
            NAEQConditions: self.NAEQConditions.clone(),
            LeftConditions: self.LeftConditions.clone(),
            RightConditions: self.RightConditions.clone(),
            OtherConditions: self.OtherConditions.clone(),
            LeftProperties: self.LeftProperties.clone(),
            RightProperties: self.RightProperties.clone(),
            FullSchema: self.FullSchema.as_ref().map(Schema::Clone),
            FullNames: self.FullNames.Shallow(),
            RedundantColsToOutputIdx: self.RedundantColsToOutputIdx.clone(),
            PreferCorrelate: self.PreferCorrelate,
            EqualCondOutCnt: self.EqualCondOutCnt,
            FromDecorrelatedApply: self.FromDecorrelatedApply,
            FromSemiJoinRewrite: self.FromSemiJoinRewrite,
            FromInSubqueryRewrite: self.FromInSubqueryRewrite,
            ..Default::default()
        }
    }
    /// 写时复制后返回等值条件可变引用。
    pub fn EqualConditionsShallowRef(&mut self) -> &mut Vec<Expression> {
        self.EqualConditions = self.EqualConditions.clone();
        &mut self.EqualConditions
    }
    /// 写时复制后返回空安全等值（NAEQ）条件可变引用。
    pub fn NAEQConditionsShallowRef(&mut self) -> &mut Vec<Expression> {
        self.NAEQConditions = self.NAEQConditions.clone();
        &mut self.NAEQConditions
    }
    /// 写时复制后返回左侧单表条件可变引用。
    pub fn LeftConditionsShallowRef(&mut self) -> &mut Vec<Expression> {
        self.LeftConditions = self.LeftConditions.clone();
        &mut self.LeftConditions
    }
    /// 写时复制后返回右侧单表条件可变引用。
    pub fn RightConditionsShallowRef(&mut self) -> &mut Vec<Expression> {
        self.RightConditions = self.RightConditions.clone();
        &mut self.RightConditions
    }
    /// 写时复制后返回其他连接条件可变引用。
    pub fn OtherConditionsShallowRef(&mut self) -> &mut Vec<Expression> {
        self.OtherConditions = self.OtherConditions.clone();
        &mut self.OtherConditions
    }
}

impl LogicalProjection {
    /// 浅拷贝投影表达式列表及相关标志。
    pub fn LogicalProjectionShallowRef(&self) -> Self {
        Self {
            Exprs: self.Exprs.clone(),
            CalculateNoDelay: self.CalculateNoDelay,
            Proj4Expand: self.Proj4Expand,
            ..Default::default()
        }
    }
    /// 写时复制后返回投影表达式可变引用。
    pub fn ExprsShallowRef(&mut self) -> &mut Vec<Expression> {
        self.Exprs = self.Exprs.clone();
        &mut self.Exprs
    }
}

impl LogicalAggregation {
    /// 浅拷贝聚合函数、分组项与下推偏好等字段。
    pub fn LogicalAggregationShallowRef(&self) -> Self {
        Self {
            AggFuncs: self.AggFuncs.clone(),
            GroupByItems: self.GroupByItems.clone(),
            PreferAggType: self.PreferAggType,
            PreferAggToCop: self.PreferAggToCop,
            PossibleProperties: self.PossibleProperties.clone(),
            InputCount: self.InputCount,
            NoCopPushDown: self.NoCopPushDown,
            ..Default::default()
        }
    }
    /// 写时复制后返回聚合函数描述可变引用。
    pub fn AggFuncsShallowRef(&mut self) -> &mut Vec<AggFuncDesc> {
        self.AggFuncs = self.AggFuncs.clone();
        &mut self.AggFuncs
    }
    /// 写时复制后返回 GROUP BY 项可变引用。
    pub fn GroupByItemsShallowRef(&mut self) -> &mut Vec<Expression> {
        self.GroupByItems = self.GroupByItems.clone();
        &mut self.GroupByItems
    }
    /// 写时复制后返回可能物理属性列表可变引用。
    pub fn PossiblePropertiesShallowRef(&mut self) -> &mut Vec<Vec<Column>> {
        self.PossibleProperties = self.PossibleProperties.clone();
        &mut self.PossibleProperties
    }
}

impl LogicalSort {
    /// 浅拷贝排序键；BaseLogicalPlan 重置为默认。
    pub fn LogicalSortShallowRef(&self) -> Self {
        Self {
            BaseLogicalPlan: BaseLogicalPlan::default(),
            ByItems: self.ByItems.clone(),
        }
    }
    /// 写时复制后返回排序项可变引用。
    pub fn ByItemsShallowRef(&mut self) -> &mut Vec<ByItems> {
        self.ByItems = self.ByItems.clone();
        &mut self.ByItems
    }
}
