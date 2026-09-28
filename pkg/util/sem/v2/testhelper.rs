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

// SEM v2 测试辅助函数。
//
// 提供按配置文件启用 SEM、以及在单测中动态增删受限权限的工具；
// 清理回调会 Disable SEM 并恢复被覆盖的系统变量默认值。

use std::collections::HashMap;

use crate::{Disable, Enable, globalSem, parseSEMConfigFromFile};

/// 按配置文件路径启用 SEM v2，并返回清理闭包。
///
/// EnableFromPathForTest enables SEM v2 using a configuration file and returns cleanup.
/// 启用前先备份带 Value 的受限系统变量原值，清理时 Disable 后写回。
#[allow(non_snake_case)]
pub fn EnableFromPathForTest(configPath: &str) -> Result<Box<dyn Fn()>, String> {
    let semConfig = parseSEMConfigFromFile(configPath)?;
    let mut variableDefValue = HashMap::new();
    // 仅备份会在 Enable 时被 override 的变量，避免恢复不存在的系统变量。
    for restriction in &semConfig.RestrictedVariables {
        if !restriction.Value.is_empty()
            && let Some(sys_var) = variable::GetSysVar(&restriction.Name)
        {
            variableDefValue.insert(restriction.Name.clone(), sys_var.Value.clone());
        }
    }

    Enable(configPath)?;
    Ok(Box::new(move || {
        Disable();
        for (name, value) in &variableDefValue {
            let _ = variable::SetSysVar(name, value);
        }
    }))
}

/// 向当前 SEM 实例的受限权限集合追加一项（大写）。不得与其他 SEM 变更并发。
///
/// Adds a restricted privilege for tests. It must not race with other SEM mutations.
#[allow(non_snake_case)]
pub fn AddRestrictedPrivilegesForTest(privilege: &str) {
    let Some(sem) = globalSem.Load() else {
        return;
    };
    sem.restrictedPrivileges
        .write()
        .expect("SEM privilege lock poisoned")
        .insert(privilege.to_uppercase());
}

/// 从当前 SEM 实例的受限权限集合移除一项（大写）。不得与其他 SEM 变更并发。
///
/// Removes a restricted privilege for tests. It must not race with other SEM mutations.
#[allow(non_snake_case)]
pub fn RemoveRestrictedPrivilegesForTest(privilege: &str) {
    let Some(sem) = globalSem.Load() else {
        return;
    };
    sem.restrictedPrivileges
        .write()
        .expect("SEM privilege lock poisoned")
        .remove(&privilege.to_uppercase());
}
