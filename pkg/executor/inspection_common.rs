// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 巡检（inspection）规则检索公共逻辑。
//
// 将包级规则注册表快照化为行，供 INFORMATION_SCHEMA / 巡检相关表物化；
// 通过快照避免在排序与物化行时长时间持有注册表锁。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::BTreeSet;

use astersql_types::datum::{Datum, NewStringDatum};

/// 巡检规则类型名：明细 inspection。
pub const inspectionRuleTypeInspection: &str = "inspection";
/// 巡检规则类型名：汇总 summary。
pub const inspectionRuleTypeSummary: &str = "summary";

/// Inputs are snapshots of the two package-level rule registries. This avoids
/// holding their locks while rows are materialized and sorted.
///
/// 巡检规则检索器：持有两类规则名快照与请求过滤条件。
pub struct inspectionRuleRetriever {
    /// 是否已检索过（保证只物化一次）。
    pub retrieved: bool,
    /// 为 true 时跳过本次请求，直接返回空行。
    pub skip_request: bool,
    /// 请求的规则类型集合；空表示不过滤。
    pub requested_types: BTreeSet<String>,
    /// inspection 类型规则名快照。
    pub inspection_rule_names: Vec<String>,
    /// summary 类型规则名快照。
    pub summary_rule_names: Vec<String>,
}

impl inspectionRuleRetriever {
    /// 判断给定规则类型是否在请求过滤集合中（空集合表示全部启用）。
    fn type_enabled(&self, rule_type: &str) -> bool {
        self.requested_types.is_empty() || self.requested_types.contains(rule_type)
    }

    /// 物化规则行为 Datum 行；已检索或 skip 时返回空。
    pub fn retrieve<C>(&mut self, _ctx: C) -> Vec<Vec<Datum>> {
        if self.retrieved || self.skip_request {
            return Vec::new();
        }
        self.retrieved = true;

        let mut rows = Vec::new();
        // 按类型过滤后展开为 (name, type, remark) 三列
        if self.type_enabled(inspectionRuleTypeInspection) {
            rows.extend(self.inspection_rule_names.iter().map(|name| {
                vec![
                    NewStringDatum(name.clone()),
                    NewStringDatum(inspectionRuleTypeInspection.to_owned()),
                    NewStringDatum(String::new()),
                ]
            }));
        }
        if self.type_enabled(inspectionRuleTypeSummary) {
            // summary 规则名排序后再物化，保证输出稳定
            let mut summary_rules = self.summary_rule_names.clone();
            summary_rules.sort();
            rows.extend(summary_rules.into_iter().map(|name| {
                vec![
                    NewStringDatum(name),
                    NewStringDatum(inspectionRuleTypeSummary.to_owned()),
                    NewStringDatum(String::new()),
                ]
            }));
        }
        rows
    }
}
