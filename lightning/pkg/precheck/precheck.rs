// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Precheck types matching Go package `github.com/pingcap/tidb/lightning/pkg/precheck`.
//! 本文件只保留导入前检查阶段真正被其他 Rust 模块消费的公共契约。
//! 它不实现具体检查逻辑，而是复刻 Go 侧常量、结果结构和接口形状，
//! 让调用方可以用同一套名称表达“检查项”“严重级别”和“跳过检查”等语义。

use std::collections::HashMap;

use crate::context;
use crate::errors;

/// CheckType represents the check type.
/// 这里沿用 Go 的字符串常量，而不是额外的枚举编码，
/// 这样序列化、日志和断言都能直接复用上游字面值。
pub type CheckType = &'static str;

/// CheckType constants.
/// `Warn` 对应 Go 中的 `"performance"`，
/// 调用方应按兼容名称处理，而不是按英文直觉改成 `"warning"`。
pub const Critical: CheckType = "critical";
pub const Warn: CheckType = "performance";

/// CheckItemID is the ID of a precheck item.
/// 检查项 ID 需要稳定到可直接作为 map key、日志字段和测试断言，
/// 因此仍使用静态字符串而不是数值编号。
pub type CheckItemID = &'static str;

/// CheckItemID constants.
pub const CheckLargeDataFile: CheckItemID = "CHECK_LARGE_DATA_FILES";
pub const CheckSourcePermission: CheckItemID = "CHECK_SOURCE_PERMISSION";
pub const CheckTargetTableEmpty: CheckItemID = "CHECK_TARGET_TABLE_EMPTY";
pub const CheckSourceSchemaValid: CheckItemID = "CHECK_SOURCE_SCHEMA_VALID";
pub const CheckCheckpoints: CheckItemID = "CHECK_CHECKPOINTS";
pub const CheckCSVHeader: CheckItemID = "CHECK_CSV_HEADER";
pub const CheckTargetClusterSize: CheckItemID = "CHECK_TARGET_CLUSTER_SIZE";
pub const CheckTargetClusterEmptyRegion: CheckItemID = "CHECK_TARGET_CLUSTER_EMPTY_REGION";
pub const CheckTargetClusterRegionDist: CheckItemID = "CHECK_TARGET_CLUSTER_REGION_DISTRIBUTION";
pub const CheckTargetClusterVersion: CheckItemID = "CHECK_TARGET_CLUSTER_VERSION";
pub const CheckLocalDiskPlacement: CheckItemID = "CHECK_LOCAL_DISK_PLACEMENT";
pub const CheckLocalTempKVDir: CheckItemID = "CHECK_LOCAL_TEMP_KV_DIR";
pub const CheckTargetUsingCDCPITR: CheckItemID = "CHECK_TARGET_USING_CDC_PITR";
pub const CheckPDTiDBFromSameCluster: CheckItemID = "CHECK_PD_TIDB_FROM_SAME_CLUSTER";

/// checkItemIDToDisplayName is a map from CheckItemID to its display name.
/// 这里每次构造临时 `HashMap`，优先保持 Go 侧“查表取显示名”的语义，
/// 而不是为了微小性能收益引入全局静态初始化复杂度。
fn checkItemIDToDisplayName() -> HashMap<CheckItemID, &'static str> {
    HashMap::from([
        (CheckLargeDataFile, "Large data file"),
        (CheckSourcePermission, "Source permission"),
        (CheckTargetTableEmpty, "Target table empty"),
        (CheckSourceSchemaValid, "Source schema valid"),
        (CheckCheckpoints, "Checkpoints"),
        (CheckCSVHeader, "CSV header"),
        (CheckTargetClusterSize, "Target cluster size"),
        (CheckTargetClusterEmptyRegion, "Target cluster empty region"),
        (CheckTargetClusterRegionDist, "Target cluster region dist"),
        (CheckTargetClusterVersion, "Target cluster version"),
        (CheckLocalDiskPlacement, "Local disk placement"),
        (CheckLocalTempKVDir, "Local temp KV dir"),
        (CheckTargetUsingCDCPITR, "Target using CDC/PITR"),
        (
            CheckPDTiDBFromSameCluster,
            "PD and TiDB are from the same cluster",
        ),
    ])
}

/// DisplayName returns display name for a CheckItemID.
/// Unknown IDs return the empty string (Go map miss zero value).
/// 返回空串而不是 `Option`，是为了匹配 Go map 未命中的零值行为。
pub fn DisplayName(c: CheckItemID) -> &'static str {
    checkItemIDToDisplayName().get(c).copied().unwrap_or("")
}

/// CheckResult is the result of a precheck item.
/// 字段名保持 Go 的导出风格，便于对照 parity test 和未来 JSON/日志输出。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub Item: CheckItemID,
    pub Severity: CheckType,
    pub Passed: bool,
    pub Message: String,
}

impl Default for CheckResult {
    fn default() -> Self {
        // Go zero values: empty strings, false, empty message.
        // 零值语义很重要，跳过初始化时的默认状态必须和 Go 一致。
        Self {
            Item: "",
            Severity: "",
            Passed: false,
            Message: String::new(),
        }
    }
}

impl CheckResult {
    /// Convenience constructor matching common Go usage patterns.
    /// 这里默认 `Passed = false`，
    /// 让调用方必须显式表达检查通过，避免遗漏失败信息。
    pub fn new(item: CheckItemID, severity: CheckType) -> Self {
        Self {
            Item: item,
            Severity: severity,
            Passed: false,
            Message: String::new(),
        }
    }

    /// `critical`/`warn` 是常用构造快捷方式，
    /// 让上层逻辑少写重复字段，同时固定严重级别常量来源。
    pub fn critical(item: CheckItemID, passed: bool, message: impl Into<String>) -> Self {
        Self {
            Item: item,
            Severity: Critical,
            Passed: passed,
            Message: message.into(),
        }
    }

    /// 性能类告警和阻塞性错误共用同一结构，
    /// 区别只体现在 `Severity` 字段，符合 Go 的建模方式。
    pub fn warn(item: CheckItemID, passed: bool, message: impl Into<String>) -> Self {
        Self {
            Item: item,
            Severity: Warn,
            Passed: passed,
            Message: message.into(),
        }
    }
}

/// Checker is the interface for precheck items.
///
/// If the check is skipped, `Check` returns `Ok(None)` (Go: nil `*CheckResult`).
/// 这里把“跳过”单独编码成 `None`，
/// 与“执行完成但失败”返回 `Some(CheckResult { Passed: false, .. })` 区分开。
pub trait Checker {
    /// Check checks whether prerequisites for importing are met.
    /// trait 方法接收可取消上下文，便于上层在导入任务终止时尽快退出检查。
    fn Check(
        &mut self,
        ctx: context::Context,
    ) -> std::result::Result<Option<CheckResult>, errors::Error>;

    /// 调用方依赖该 ID 将结果回填到展示名和错误分类逻辑中。
    fn GetCheckItemID(&self) -> CheckItemID;
}
