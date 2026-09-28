// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 慢日志规则数据结构。
//
// 定义条件、规则、会话/全局规则集合；规则间为逻辑或（OR），
// 单条规则内条件为逻辑与（AND）。用于判定语句是否写入慢查询日志。

#![allow(dead_code, non_snake_case)]

use std::any::Any;
use std::collections::{HashMap, HashSet};

/// SlowLogCondition defines a single condition within a slow log rule.
/// 慢日志规则中的单条条件（字段名 + 阈值）。
pub struct SlowLogCondition {
    /// Name of the slow log field to check (e.g. `Conn_ID` or `Query_time`).
    /// 待检查的慢日志字段名（如连接 ID、查询耗时）。
    pub Field: String,
    /// Threshold value for triggering the condition.
    ///
    /// Go stores this as `any`; retaining `Any` here preserves the supported
    /// integer, floating-point, boolean, and string threshold types.
    /// 触发条件的阈值；对应 Go 的 `any`，可承载整型、浮点、布尔与字符串。
    pub Threshold: Box<dyn Any>,
}

/// SlowLogRule represents a single slow log rule.
/// A rule is triggered only if all of its conditions are satisfied (logical AND).
/// 单条慢日志规则；全部条件满足（逻辑与）时才触发。
#[derive(Default)]
pub struct SlowLogRule {
    /// List of conditions combined with logical AND.
    /// 以逻辑与组合的条件列表。
    pub Conditions: Vec<SlowLogCondition>,
}

/// SlowLogRules represents all slow log rules defined for the current scope
/// (for example, session or global scope).
///
/// The rules are evaluated using logical OR between them: if any rule matches,
/// it triggers the slow log.
/// 当前作用域（会话或全局）下的慢日志规则集合；规则之间为逻辑或。
#[derive(Default)]
pub struct SlowLogRules {
    /// Raw rule string before parsing.
    /// 解析前的原始规则字符串。
    pub RawRules: String,
    /// All unique fields used in this rule set.
    /// 本规则集用到的全部字段名（去重）。
    pub Fields: HashSet<String>,
    /// List of rules combined with logical OR.
    /// 以逻辑或组合的规则列表。
    pub Rules: Vec<Box<SlowLogRule>>,
}

/// SessionSlowLogRules represents the slow log rules effective for a specific
/// session and tracks the additional session-level state.
/// 会话生效的慢日志规则，并附带会话级缓存状态。
pub struct SessionSlowLogRules {
    /// 本会话持有的规则集合。
    pub SlowLogRules: Box<SlowLogRules>,
    /// All unique fields visible to this session (session and global rules).
    /// 会话可见的全部字段（合并会话与全局规则）。
    pub EffectiveFields: HashSet<String>,
    /// 最近一次观察到的全局原始规则哈希，用于检测全局规则变更。
    pub GlobalRawRulesHash: u64,
    /// Whether `EffectiveFields` needs to be updated before evaluation.
    /// 评估前是否需要刷新 `EffectiveFields`。
    pub NeedUpdateEffectiveFields: bool,
}

/// Creates a new session rule set from the given slow log rules.
/// 由给定规则集构造会话规则，初始标记需更新有效字段。
pub fn NewSessionSlowLogRules(slRules: Box<SlowLogRules>) -> Box<SessionSlowLogRules> {
    Box::new(SessionSlowLogRules {
        SlowLogRules: slRules,
        EffectiveFields: HashSet::new(),
        GlobalRawRulesHash: 0,
        NeedUpdateEffectiveFields: true,
    })
}

/// GlobalSlowLogRules represents all slow log rules defined at global scope.
/// It maps a connection ID to its rule set; key `-1` is the rule set applying
/// globally to every session.
///
/// Rule evaluation is logical OR across all matched rule sets.
/// 全局慢日志规则；按连接 ID 映射规则集，`-1` 表示作用于所有会话的全局规则。
#[derive(Default)]
pub struct GlobalSlowLogRules {
    /// Raw rule string before parsing.
    /// 解析前的原始规则字符串。
    pub RawRules: String,
    /// Hash of `RawRules` for fast comparison.
    /// `RawRules` 的哈希，便于快速比较是否变更。
    pub RawRulesHash: u64,
    /// Mapping from connection ID to slow log rules.
    /// 连接 ID → 慢日志规则集。
    pub RulesMap: HashMap<i64, Box<SlowLogRules>>,
}
