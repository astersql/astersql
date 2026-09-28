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

// SEM 兼容层：在 SEM v1 与 SEM v2 之间统一对外查询接口。
//
// SEM（Security Enhanced Mode，安全增强模式）用于隐藏敏感 schema/表/变量并限制权限。
// 本模块对应 Go `pkg/util/sem/compat`：调用方只依赖兼容层 API，内部按启用版本转发到
// v1（硬编码规则）或 v2（配置驱动规则），并断言两版本不会同时启用。

#![allow(non_snake_case)]

use astersql_util_sem as sem;
use astersql_util_sem_v2 as semv2;

// IsEnabled checks if either SEM v1 or SEM v2 is enabled.
// IsEnabled 对应 Go 的同名函数：先断言 v1 与 v2 不会同时开启，再返回任一版本是否启用。
pub fn IsEnabled() -> bool {
    intest::Assert(
        !(sem::IsEnabled() && semv2::IsEnabled()),
        &["SEM v1 and v2 cannot be enabled at the same time".into()],
    );

    // 保持 Go 的短路布尔语义：只要任一版本启用，兼容层就视为 SEM 已启用。
    sem::IsEnabled() || semv2::IsEnabled()
}

// IsInvisibleSchema is a compatibility wrapper for SEM v1 and v2.
// IsInvisibleSchema 兼容包装数据库隐藏规则：优先查询已启用的 SEM v1，再查询已启用的 SEM v2。
pub fn IsInvisibleSchema(dbName: &str) -> bool {
    intest::Assert(
        !(sem::IsEnabled() && semv2::IsEnabled()),
        &["SEM v1 and v2 cannot be enabled at the same time".into()],
    );

    // 关键分支保持 Go 的顺序：只有对应版本启用且该版本判断为隐藏时才返回 true。
    if sem::IsEnabled() && sem::IsInvisibleSchema(dbName) {
        return true;
    } else if semv2::IsEnabled() && semv2::IsInvisibleSchema(dbName) {
        return true;
    }

    false
}

// IsInvisibleTable is a compatibility wrapper for SEM v1 and v2.
// IsInvisibleTable 兼容包装表隐藏规则，参数沿用 Go 语义：库名和表名都已经是小写形式。
pub fn IsInvisibleTable(dbLowerName: &str, tblLowerName: &str) -> bool {
    // 互斥断言说明兼容层只允许一个 SEM 版本实际生效，避免两套规则叠加改变 SQL 可见性。
    intest::Assert(
        !(sem::IsEnabled() && semv2::IsEnabled()),
        &["SEM v1 and v2 cannot be enabled at the same time".into()],
    );

    // 先按 v1 规则判断，再按 v2 规则判断；这是对 Go if/else if 控制流的直接迁移。
    if sem::IsEnabled() && sem::IsInvisibleTable(dbLowerName, tblLowerName) {
        return true;
    } else if semv2::IsEnabled() && semv2::IsInvisibleTable(dbLowerName, tblLowerName) {
        return true;
    }

    false
}

// IsInvisibleStatusVar is a compatibility wrapper for SEM v1 and v2.
// IsInvisibleStatusVar 兼容包装 status variable 隐藏规则，参数保持 Go 中的变量名字符串语义。
pub fn IsInvisibleStatusVar(varName: &str) -> bool {
    // 继续保留 Go 层的版本互斥检查，避免 v1/v2 同时启用时出现冲突的可见性结果。
    intest::Assert(
        !(sem::IsEnabled() && semv2::IsEnabled()),
        &["SEM v1 and v2 cannot be enabled at the same time".into()],
    );

    // 只有启用中的版本会参与判断；未启用版本的规则不会被调用。
    if sem::IsEnabled() && sem::IsInvisibleStatusVar(varName) {
        return true;
    } else if semv2::IsEnabled() && semv2::IsInvisibleStatusVar(varName) {
        return true;
    }

    false
}

// IsInvisibleSysVar is a compatibility wrapper for SEM v1 and v2.
// IsInvisibleSysVar 兼容包装 system variable 隐藏规则，保留 Go 中逐版本查询的行为。
pub fn IsInvisibleSysVar(varName: &str) -> bool {
    // 这个断言不是业务分支，而是迁移 Go 中的开发期一致性检查。
    intest::Assert(
        !(sem::IsEnabled() && semv2::IsEnabled()),
        &["SEM v1 and v2 cannot be enabled at the same time".into()],
    );

    // 如果 v1 已启用并判定变量隐藏，立即返回；否则只在 v2 启用时检查 v2。
    if sem::IsEnabled() && sem::IsInvisibleSysVar(varName) {
        return true;
    } else if semv2::IsEnabled() && semv2::IsInvisibleSysVar(varName) {
        return true;
    }

    false
}

// IsRestrictedPrivilege is a compatibility wrapper for SEM v1 and v2.
// IsRestrictedPrivilege 兼容包装受限权限判断；Go 约定传入的 privilege 必须已经是大写。
pub fn IsRestrictedPrivilege(privilege: &str) -> bool {
    // 先保留 SEM 版本互斥断言；若两个版本同时启用，Go 代码会在测试/开发构建中暴露问题。
    intest::Assert(
        !(sem::IsEnabled() && semv2::IsEnabled()),
        &["SEM v1 and v2 cannot be enabled at the same time".into()],
    );
    // Go 使用 strings.ToUpper(privilege) == privilege 校验调用方传入大写权限名；
    // Rust 用标准库 to_uppercase 表达同一校验，不引入新的外部依赖。
    intest::Assert(
        privilege.to_uppercase() == privilege,
        &["privilege name must be uppercase".into()],
    );

    // 受限权限同样按 v1 -> v2 的顺序查询，只返回启用版本的规则结果。
    if sem::IsEnabled() && sem::IsRestrictedPrivilege(privilege) {
        return true;
    } else if semv2::IsEnabled() && semv2::IsRestrictedPrivilege(privilege) {
        return true;
    }

    false
}
