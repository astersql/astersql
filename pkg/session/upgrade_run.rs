// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Bootstrap 版本升级主流程。
//
// 从集群当前 bootstrap 版本逐步执行 `upgradeToVerN`，处理元数据锁（MDL）
// 相关的 v99 前后钩子，并更新版本号后提交。

#![allow(dead_code, non_snake_case)]

use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 带版本号与名称的升级步骤描述。
pub struct VersionedUpgrade {
    /// 目标 bootstrap 版本号。
    pub version: i64,
    /// 升级函数名（如 `upgradeToVer279`）。
    pub name: &'static str,
}

/// 升级流程对运行时的依赖：读版本、执行迁移、提交与休眠等。
pub trait UpgradeRuntime {
    type Error;

    fn metadata_lock(&mut self) -> Result<Option<bool>, Self::Error>;
    fn set_metadata_lock_enabled(&mut self, enabled: bool);
    fn bootstrap_version(&mut self) -> Result<i64, Self::Error>;
    fn current_bootstrap_version(&mut self) -> i64;
    fn support_upgrade_http_version(&mut self) -> i64;
    fn internal_sql_timeout(&mut self) -> Duration;
    fn check_cluster_state(&mut self, old: i64, new: i64, timeout: Duration);
    fn upgrade_functions(&mut self) -> Vec<VersionedUpgrade>;
    fn execute_upgrade(&mut self, upgrade: VersionedUpgrade, from: i64) -> Result<(), Self::Error>;
    fn upgrade_version_99_before(&mut self) -> Result<(), Self::Error>;
    fn upgrade_version_99_after(&mut self) -> Result<(), Self::Error>;
    fn update_bootstrap_version(&mut self) -> Result<(), Self::Error>;
    fn commit(&mut self) -> Result<(), Self::Error>;
    fn sleep(&mut self, duration: Duration);
}

/// Persistent bootstrap-variable operations used by version-specific upgrades.
///
/// Keeping this boundary independent from the concrete session makes the same
/// production migration sequence usable by the canonical mock SQL runtime and
/// the full server bootstrap path.
pub trait BootstrapVariableUpgradeRuntime {
    type Error;

    fn insert_global_if_missing(&mut self, name: &str, value: &str) -> Result<(), Self::Error>;
    fn delete_global_if_equal(&mut self, name: &str, value: &str) -> Result<(), Self::Error>;
    fn update_global_if_equal(
        &mut self,
        name: &str,
        old_value: &str,
        new_value: &str,
    ) -> Result<(), Self::Error>;
    fn upsert_tidb_variable(
        &mut self,
        name: &str,
        value: &str,
        comment: &str,
    ) -> Result<(), Self::Error>;
    /// Atomically reads the legacy inverse switch and replaces the new switch.
    fn migrate_legacy_txn_file_variable(&mut self) -> Result<(), Self::Error>;
}

/// Apply the variable migrations exercised by `bootstraptest/boot_test.go`.
///
/// Each condition mirrors Go's `from < targetVersion` upgrade dispatch. Missing
/// compatibility values are inserted without overwriting user choices; the two
/// historical rewrites retain their narrower value predicates.
pub fn upgrade_bootstrap_variables<R: BootstrapVariableUpgradeRuntime>(
    runtime: &mut R,
    from: i64,
) -> Result<(), R::Error> {
    use crate::upgrade_def::{
        version54, version59, version68, version80, version81, version97, version105, version135,
        version215, version255, version279, version281, version283, version284, version317,
    };

    if from < version54 && from <= crate::upgrade_def::version38 {
        runtime.upsert_tidb_variable(
            crate::bootstrap::tidbDefMemoryQuotaQuery,
            &(32_i64 << 30).to_string(),
            "memory_quota_query is 32GB by default in v3.0.x, 1GB by default in v4.0.x+",
        )?;
    }
    if from < version59 {
        runtime.upsert_tidb_variable(
            crate::bootstrap::tidbDefOOMAction,
            "log",
            "oom-action compatibility value",
        )?;
    }
    if from < version68 {
        runtime.delete_global_if_equal("tidb_enable_clustered_index", "OFF")?;
    }
    if from < version80 {
        runtime.insert_global_if_missing("tidb_analyze_version", "2")?;
    }
    if from < version81 {
        runtime.insert_global_if_missing("tidb_enable_index_merge", "OFF")?;
    }
    if from < version97 {
        runtime.insert_global_if_missing("tidb_opt_range_max_size", "0")?;
    }
    if from < version105 {
        runtime.insert_global_if_missing("tidb_cost_model_version", "1")?;
    }
    if from < version135 {
        runtime.insert_global_if_missing("tidb_opt_advanced_join_hint", "OFF")?;
    }
    if from < version215 {
        runtime.insert_global_if_missing("tidb_enable_inl_join_inner_multi_pattern", "OFF")?;
    }
    if from < version255 {
        runtime.update_global_if_equal("tidb_analyze_version", "1", "2")?;
    }
    if from < version279 {
        runtime.insert_global_if_missing("tidb_ignore_inlist_plan_digest", "OFF")?;
    }
    if from < version281 {
        // Clusters upgraded from the old implementation retain its historical
        // 0.8 behaviour even though the current built-in default has changed.
        runtime.insert_global_if_missing("tidb_default_string_match_selectivity", "0.8")?;
    }
    if from < version283 {
        use astersql_sessionctx_vardef as vardef;
        runtime.insert_global_if_missing(
            vardef::TiDBAnalyzeDefaultNumBuckets,
            &vardef::DefTiDBAnalyzeDefaultNumBuckets.to_string(),
        )?;
        runtime.insert_global_if_missing(
            vardef::TiDBAnalyzeDefaultNumTopN,
            &vardef::DefTiDBAnalyzeDefaultNumTopN.to_string(),
        )?;
    }
    if from < version284 {
        runtime.migrate_legacy_txn_file_variable()?;
    }
    if from < version317 {
        runtime.insert_global_if_missing(
            astersql_sessionctx_vardef::TiDBEnableAdaptiveLimitScan,
            astersql_sessionctx_vardef::Off,
        )?;
    }
    Ok(())
}

/// Persisted bind-info fields needed by the v282 digest refresh. Callers must
/// provide rows newest first, matching Go's update/create/row-id ordering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingDigestRefreshRow {
    pub identity: String,
    pub bind_sql: String,
    pub default_db: String,
    pub source: String,
    pub plan_digest: Option<String>,
}

/// One database mutation produced by the v282 refresh planner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BindingDigestRefreshAction {
    ClearInvalidPlanDigest {
        identity: String,
    },
    DeleteDuplicate {
        identity: String,
    },
    UpdateDigest {
        identity: String,
        original_sql: String,
        sql_digest: String,
    },
}

/// Plan Go-equivalent v282 binding mutations without coupling the algorithm to
/// a particular SQL result-set implementation.
pub fn plan_binding_digest_refresh(
    rows: impl IntoIterator<Item = BindingDigestRefreshRow>,
) -> Vec<BindingDigestRefreshAction> {
    let mut seen = HashSet::<(String, String)>::new();
    let mut invalid = Vec::new();
    let mut duplicates = Vec::new();
    let mut updates = Vec::new();

    for row in rows {
        if row.source == "builtin" {
            continue;
        }
        let statement = astersql_bindinfo::Statement {
            SQL: row.bind_sql,
            ..Default::default()
        };
        let (original_sql, sql_digest) =
            astersql_bindinfo::NormalizeStmtForBinding(&statement, &row.default_db, false);
        if original_sql.is_empty() || sql_digest.is_empty() {
            if row.plan_digest.is_some() {
                invalid.push(BindingDigestRefreshAction::ClearInvalidPlanDigest {
                    identity: row.identity,
                });
            }
            continue;
        }
        if let Some(plan_digest) = row.plan_digest {
            if !seen.insert((sql_digest.clone(), plan_digest)) {
                duplicates.push(BindingDigestRefreshAction::DeleteDuplicate {
                    identity: row.identity,
                });
                continue;
            }
        }
        updates.push(BindingDigestRefreshAction::UpdateDigest {
            identity: row.identity,
            original_sql,
            sql_digest,
        });
    }
    invalid.extend(duplicates);
    invalid.extend(updates);
    invalid
}

/// 执行升级主循环：已达目标则直接返回。
pub fn upgrade<R: UpgradeRuntime>(runtime: &mut R) -> Result<(), R::Error> {
    // 元数据锁变量若原为 NULL，需在升级链前后跑 v99 钩子。
    let metadata_lock_was_null = InitMDLVariableForUpgrade(runtime)?;
    let from = runtime.bootstrap_version()?;
    let target = runtime.current_bootstrap_version();
    // 已是最新 bootstrap 版本，无需升级。
    if from >= target {
        return Ok(());
    }
    printClusterState(runtime, from);
    if metadata_lock_was_null {
        runtime.upgrade_version_99_before()?;
    }
    // 仅执行版本号大于当前 from 的迁移步骤。
    for item in runtime.upgrade_functions() {
        if from < item.version {
            runtime.execute_upgrade(item, from)?;
        }
    }
    if metadata_lock_was_null {
        runtime.upgrade_version_99_after()?;
    }
    runtime.update_bootstrap_version()?;
    // 提交失败时短暂休眠再读版本：若他节点已升到目标则视为成功。
    if let Err(commit_error) = runtime.commit() {
        runtime.sleep(Duration::from_secs(1));
        if runtime.bootstrap_version()? >= target {
            return Ok(());
        }
        return Err(commit_error);
    }
    Ok(())
}

/// 初始化升级用元数据锁变量；返回原先是否为 NULL。
pub fn InitMDLVariableForUpgrade<R: UpgradeRuntime>(runtime: &mut R) -> Result<bool, R::Error> {
    let setting = match runtime.metadata_lock() {
        Ok(setting) => setting,
        Err(error) => {
            // Go initializes `enable` to false and applies it even when the
            // metadata transaction returns an error.
            runtime.set_metadata_lock_enabled(false);
            return Err(error);
        }
    };
    runtime.set_metadata_lock_enabled(setting.unwrap_or(false));
    Ok(setting.is_none())
}

/// 当版本达到支持升级 HTTP 检查的阈值时，打印/检查集群状态。
pub fn printClusterState<R: UpgradeRuntime>(runtime: &mut R, version: i64) {
    if version >= runtime.support_upgrade_http_version() {
        let target = runtime.current_bootstrap_version();
        let timeout = runtime.internal_sql_timeout();
        runtime.check_cluster_state(version, target, timeout);
    }
}
