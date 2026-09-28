// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 受限优化器 hint（optimizer hint）判定。
//
// SEM 可配置 `restricted_hints`：命中则默认忽略该 hint。
// 部分 hint 通过改写系统变量生效；若对应变量仍可见且可写，则允许使用该 hint。

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::{SemImpl, globalSem};

/// hint 名（小写）到其背后系统变量名的映射；用于“变量仍可调则放行 hint”。
#[allow(non_upper_case_globals)]
pub static hintGuardVars: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("memory_quota", vardef::TiDBMemQuotaQuery),
        ("read_consistent_replica", vardef::TiDBReplicaRead),
        ("max_execution_time", vardef::MaxExecutionTime),
    ])
});

/// 对外入口：SEM 未启用时一律放行；否则委托 `SemImpl::isRestrictedHint`。
///
/// 返回 `Err` 表示该 hint 被安全策略限制（应忽略）。
#[allow(non_snake_case)]
pub fn IsRestrictedHint(hintNameLower: &str) -> Result<(), String> {
    let Some(sem) = globalSem.Load() else {
        return Ok(());
    };
    sem.isRestrictedHint(hintNameLower)
}

impl SemImpl {
    /// 若 hint 不在受限集合则放行；若有守卫变量且该变量既未隐藏也非只读，则仍放行。
    #[allow(non_snake_case)]
    pub(crate) fn isRestrictedHint(&self, hintNameLower: &str) -> Result<(), String> {
        if !self.restrictedHints.contains(hintNameLower) {
            return Ok(());
        }
        // 与系统变量联动：变量仍可被会话调整时，允许同名 hint 继续生效。
        if let Some(variable) = hintGuardVars.get(hintNameLower)
            && !self.isInvisibleSysVar(variable)
            && !self.isReadOnlyVariable(variable)
        {
            return Ok(());
        }
        Err(format!(
            "the {}() optimizer hint is restricted under the current security policy and is ignored",
            hintNameLower.to_uppercase()
        ))
    }
}
