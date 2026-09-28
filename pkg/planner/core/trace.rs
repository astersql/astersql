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

// 优化过程轨迹（trace）：记录逻辑优化规则、物理优化步骤与最终计划摘要。
//
// 逻辑计划（logical plan）是与物理实现无关的算子树；物理计划（physical plan）
// 选定具体执行算法（如 HashJoin、IndexScan）。Trace 供调试与可观测性使用。

/// 一次优化过程中的规则/步骤轨迹与最终计划字符串。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Trace {
    /// 已应用的逻辑优化规则名称列表。
    pub logical: Vec<String>,
    /// 已记录的物理优化步骤描述列表。
    pub physical: Vec<String>,
    /// 最终选定的执行计划摘要。
    pub final_plan: String,
}
impl Trace {
    /// 追加一条逻辑优化规则记录。
    pub fn AppendLogical(&mut self, rule: impl Into<String>) {
        self.logical.push(rule.into());
    }
    /// 追加一条物理优化步骤记录。
    pub fn AppendPhysical(&mut self, step: impl Into<String>) {
        self.physical.push(step.into());
    }
}
