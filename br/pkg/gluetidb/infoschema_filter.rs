// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.
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

//! InfoSchema filter matching `br/pkg/gluetidb/infoschema_filter.go`.
//!
//! BR 专用 infoschema 过滤器：按库名谓词决定是否跳过 SchemaDiff / Schema 加载。
//! 某些 DDL 类型（建库、放置策略、资源组）永不跳过，以保持全局元数据完整。
//! 谓词为 None 时不安装过滤器，保持 TiDB 默认全量加载行为。

use astersql_br_pkg_glue::CIStr;

/// ActionType values used by SchemaDiff (from meta/model.Job ActionType).
/// 与 Go meta/model ActionType 数值对齐的子集；Other=0 表示其余类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ActionType {
    ActionCreateSchema = 1,
    ActionCreatePlacementPolicy = 51,
    ActionAlterPlacementPolicy = 52,
    ActionDropPlacementPolicy = 53,
    ActionCreateResourceGroup = 68,
    ActionAlterResourceGroup = 69,
    ActionDropResourceGroup = 70,
    Other = 0,
}

/// Schema 变更摘要：过滤器只关心 Type 与 SchemaID。
#[derive(Clone, Debug, Default)]
pub struct SchemaDiff {
    pub Version: i64,
    pub Type: ActionType,
    pub SchemaID: i64,
    pub TableID: i64,
    pub OldSchemaID: i64,
}

impl Default for ActionType {
    fn default() -> Self {
        ActionType::Other
    }
}

/// 最小库信息：SkipLoadSchema 用 Name 做 allow 判定。
#[derive(Clone, Debug, Default)]
pub struct DBInfo {
    pub Name: CIStr,
}

/// Minimal InfoSchema for SkipLoadDiff SchemaByID lookup.
/// 仅需 SchemaByID：用 SchemaID 反查库名再套 allow。
pub trait InfoSchema: Send + Sync {
    fn SchemaByID(&self, schema_id: i64) -> Option<DBInfo>;
}

/// issyncer.Filter stand-in.
/// 返回 true 表示跳过加载该 diff/schema。
pub trait Filter: Send + Sync {
    fn SkipLoadDiff(&self, diff: &SchemaDiff, latestIS: Option<&dyn InfoSchema>) -> bool;
    fn SkipLoadSchema(&self, dbInfo: Option<&DBInfo>) -> bool;
}

// 库名允许谓词；由 NewInfoSchemaFilter 注入。
type AllowFn = Box<dyn Fn(&CIStr) -> bool + Send + Sync>;

struct brInfoSchemaFilter {
    allow: AllowFn,
}

/// NewInfoSchemaFilter builds a BR-specific filter from a DB-name predicate.
/// Returns None if the predicate is None so default loading behavior is used.
/// allow 为 None 时返回 None，表示走默认全量加载，不安装过滤器。
pub fn NewInfoSchemaFilter(
    allow: Option<Box<dyn Fn(&CIStr) -> bool + Send + Sync>>,
) -> Option<Box<dyn Filter>> {
    let Some(allow) = allow else {
        return None;
    };
    Some(Box::new(brInfoSchemaFilter { allow }))
}

impl Filter for brInfoSchemaFilter {
    fn SkipLoadDiff(&self, diff: &SchemaDiff, latestIS: Option<&dyn InfoSchema>) -> bool {
        let skip = self.skip_load_diff_inner(diff, latestIS);
        // Go 在 skip 时打日志；此处保留调用点但不输出，避免污染测试。
        let _ = skip; // Go logs when skip; keep side-effect site silent in tests
        skip
    }

    fn SkipLoadSchema(&self, dbInfo: Option<&DBInfo>) -> bool {
        // 无库信息则不跳过（保守加载）。
        let Some(dbInfo) = dbInfo else {
            return false;
        };
        // allow 为 false → 跳过该库 schema。
        !(self.allow)(&dbInfo.Name)
    }
}

impl brInfoSchemaFilter {
    // 核心决策：永不跳过的 Action → SchemaID==0 不跳 → 无 IS 则跳 → 按 allow 反选。
    fn skip_load_diff_inner(&self, diff: &SchemaDiff, latestIS: Option<&dyn InfoSchema>) -> bool {
        match diff.Type {
            // 全局性 DDL：必须加载，否则放置策略/资源组状态会漂。
            ActionType::ActionCreateSchema
            | ActionType::ActionCreatePlacementPolicy
            | ActionType::ActionAlterPlacementPolicy
            | ActionType::ActionDropPlacementPolicy
            | ActionType::ActionCreateResourceGroup
            | ActionType::ActionDropResourceGroup
            | ActionType::ActionAlterResourceGroup => {
                return false;
            }
            _ => {}
        }
        // SchemaID 为 0 的 diff 无法按库过滤，保守不跳过。
        if diff.SchemaID == 0 {
            return false;
        }
        // 无 latestIS 时无法解析库名，与 Go 一样选择跳过。
        let Some(latestIS) = latestIS else {
            return true;
        };
        // 查到库且 allow → selected=true → 不跳过；查不到或拒绝 → 跳过。
        let selected = latestIS
            .SchemaByID(diff.SchemaID)
            .map(|schema| (self.allow)(&schema.Name))
            .unwrap_or(false);
        !selected
    }
}
