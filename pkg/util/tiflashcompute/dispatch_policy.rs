// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TiFlash Compute 任务分发策略（dispatch policy）。
//
// 对应 Go `pkg/util/tiflashcompute` 中的分发策略：轮询（round robin）与
// 一致性哈希（consistent hash）。提供合法策略列表、字符串解析与反向映射。

#![allow(non_snake_case, non_upper_case_globals)]

use crate::vardef;

/// 将任务分发到 TiFlash Compute 节点的策略类型（`isize` 别名）。
/// DispatchPolicy means different policy to dispatch tasks to TiFlash Compute nodes.
pub type DispatchPolicy = isize;

/// 轮询分发策略常量。
/// DispatchPolicyRR means dispatching by round robin.
pub const DispatchPolicyRR: DispatchPolicy = 0;
/// 一致性哈希分发策略常量。
/// DispatchPolicyConsistentHash means dispatching by consistent hash.
pub const DispatchPolicyConsistentHash: DispatchPolicy = 1;
/// 非法/未知策略哨兵值。
/// DispatchPolicyInvalid is an invalid policy.
pub const DispatchPolicyInvalid: DispatchPolicy = 2;

/// 分发策略解析错误，携带可读说明字符串。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct DispatchPolicyError(String);

/// 返回全部合法策略字符串，顺序与 Go 一致（先 consistent_hash，后 round_robin）。
/// GetValidDispatchPolicy returns all valid policy strings in Go order.
pub fn GetValidDispatchPolicy() -> Vec<&'static str> {
    vec![
        vardef::DispatchPolicyConsistentHashStr,
        vardef::DispatchPolicyRRStr,
    ]
}

/// 按字符串解析策略；非法值返回列出期望集合的错误。
/// GetDispatchPolicyByStr returns the corresponding policy.
pub fn GetDispatchPolicyByStr(value: &str) -> Result<DispatchPolicy, DispatchPolicyError> {
    match value {
        vardef::DispatchPolicyConsistentHashStr => Ok(DispatchPolicyConsistentHash),
        vardef::DispatchPolicyRRStr => Ok(DispatchPolicyRR),
        // 未知策略：错误信息中拼接合法列表，便于排查会话/配置拼写问题。
        _ => Err(DispatchPolicyError(format!(
            "unexpected tiflash_compute dispatch policy, expect [{}], got {}",
            GetValidDispatchPolicy().join(" "),
            value
        ))),
    }
}

/// 将策略整型转为字符串；未知整型映射为 invalid。
/// GetDispatchPolicy returns the corresponding policy string.
pub fn GetDispatchPolicy(policy: DispatchPolicy) -> &'static str {
    match policy {
        DispatchPolicyConsistentHash => vardef::DispatchPolicyConsistentHashStr,
        DispatchPolicyRR => vardef::DispatchPolicyRRStr,
        _ => vardef::DispatchPolicyInvalidStr,
    }
}
