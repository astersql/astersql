// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 优化规则黑名单重载执行器。
//
// 从系统表 `mysql.opt_rule_blacklist` 读取被禁用的逻辑优化规则名，
// 并原子替换会话 / 规划器侧的黑名单集合。逻辑优化规则（logical rule）
// 是规划器在生成物理计划前对逻辑算子树做的等价变换。

#![allow(non_snake_case)]

use std::collections::HashSet;

use astersql_kv::{Context, InternalTxnPrivilege, WithInternalSourceType};

/// 以 HIGH_PRIORITY 读取黑名单规则名的受限 SQL。
const LOAD_OPT_RULE_BLACKLIST_SQL: &str = "select HIGH_PRIORITY name from mysql.opt_rule_blacklist";

/// Restricted SQL and planner-state bridge owned by the session/planner layer.
/// 受限 SQL 与规划器状态的桥接，由会话 / 规划器层实现。
pub trait OptRuleBlacklistContext {
    /// 执行或状态更新失败时的错误类型。
    type Error;

    /// 以受限权限执行 SQL，返回规则名列。
    fn exec_restricted_sql(
        &mut self,
        context: &Context,
        sql: &str,
    ) -> Result<Vec<String>, Self::Error>;
    /// 原子替换当前会话中已禁用的逻辑规则集合。
    fn replace_disabled_logical_rules(&mut self, rules: HashSet<String>);
}

/// `ADMIN RELOAD OPT_RULE_BLACKLIST` 对应的执行器壳。
pub struct ReloadOptRuleBlacklistExec<C: OptRuleBlacklistContext> {
    /// 持有会话 / 规划器桥接的上下文。
    pub context: C,
}

impl<C: OptRuleBlacklistContext> ReloadOptRuleBlacklistExec<C> {
    /// Next 即触发一次黑名单重载；无结果行输出。
    pub fn Next<T, U>(&mut self, _ctx: T, _request: &mut U) -> Result<(), C::Error> {
        let internal_context = WithInternalSourceType(Context::new(), InternalTxnPrivilege);
        LoadOptRuleBlacklist(&internal_context, &mut self.context)
    }
}

/// Loads the latest rule names and atomically replaces the planner blacklist.
/// 加载最新规则名并原子替换规划器黑名单。
pub fn LoadOptRuleBlacklist<C: OptRuleBlacklistContext>(
    internal_context: &Context,
    context: &mut C,
) -> Result<(), C::Error> {
    let rows = context.exec_restricted_sql(internal_context, LOAD_OPT_RULE_BLACKLIST_SQL)?;
    // 行集合直接作为禁用规则名 HashSet。
    let disabled_rules = rows.into_iter().collect::<HashSet<_>>();
    context.replace_disabled_logical_rules(disabled_rules);
    Ok(())
}
