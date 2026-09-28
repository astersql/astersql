// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 逻辑属性（LogicalProperty）：Cascades Group 内共享的输出特征。
//
// 逻辑属性与具体物理实现无关，描述 Schema、统计、函数依赖（FD）、
// 至多一行等约束，供逻辑优化与物理择优共同使用。

use crate::{StatsInfo, expression, funcdep};

/// Logical properties shared by all group expressions in one cascades group.
/// Cascades 同一 Group 内所有等价表达式共享的逻辑属性。
#[derive(Default)]
pub struct LogicalProperty {
    /// 该 Group 输出的统计信息（行数、NDV 等）。
    pub Stats: Option<Box<StatsInfo>>,
    /// 输出列集合（Schema）。
    pub Schema: Option<Box<expression::Schema>>,
    /// 函数依赖集合（Functional Dependency，刻画列间决定关系）。
    pub FD: Option<Box<funcdep::FDSet>>,
    /// 是否保证输出至多一行（可用于消除多余聚合/Limit 等）。
    pub MaxOneRow: bool,
    /// 候选物理属性集合（列组合），供物理枚举参考。
    pub PossibleProps: Vec<Vec<expression::Column>>,
    /// 是否可能走 TiFlash（列存加速引擎）路径。
    pub HasTiFlash: bool,
}

/// 构造空的逻辑属性盒子。
pub fn NewLogicalProp() -> Box<LogicalProperty> {
    Box::new(LogicalProperty::default())
}
