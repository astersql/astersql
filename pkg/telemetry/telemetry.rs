// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 遥测上报核心：开关、会话上下文、上报缓冲与初始运行。
//
// 根据全局开关与会话变量 `tidb_enable_telemetry` 决定是否采集；
// [`ReportUsageData`] 生成载荷、执行后置钩子并写入内存报告列表。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;
/// 默认上报间隔：6 小时。
pub const ReportInterval: Duration = Duration::from_secs(6 * 60 * 60);
/// 遥测错误，承载可读消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryError(pub String);
impl fmt::Display for TelemetryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for TelemetryError {}
/// 遥测视角的表元数据摘要（含 TTL、放置策略等）。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    /// 表 ID。
    pub ID: i64,
    /// 是否为 public 状态。
    pub Public: bool,
    /// 是否临时表。
    pub Temporary: bool,
    /// 是否缓存表。
    pub Cached: bool,
    /// 是否绑定放置策略（Placement Policy）。
    pub PlacementPolicy: bool,
    /// 自增 ID 缓存大小。
    pub AutoIDCache: i64,
    /// 分区放置策略数量。
    pub PartitionPolicies: u64,
    /// TTL 是否启用；`None` 表示未配置 TTL。
    pub TTLEnabled: Option<bool>,
    /// TTL 清理间隔（小时）。
    pub TTLIntervalHours: i64,
}
/// Schema 级摘要：放置策略与下属表列表。
#[derive(Clone, Debug, Default)]
pub struct SchemaInfo {
    /// Schema 是否使用放置策略。
    pub PlacementPolicy: bool,
    /// 下属表摘要。
    pub Tables: Vec<TableInfo>,
}
/// 遥测采集用的会话/集群上下文桩，供单测注入变量与失败点。
#[derive(Clone, Debug)]
pub struct SessionContext {
    /// 会话/全局变量名到值的映射。
    pub GlobalVars: HashMap<String, String>,
    /// 代价模型版本。
    pub CostModelVersion: i64,
    /// 是否启用分页读。
    pub EnablePaging: bool,
    /// Schema 列表。
    pub Schemas: Vec<SchemaInfo>,
    /// 放置策略个数。
    pub PlacementPolicies: u64,
    /// 资源组个数。
    pub ResourceGroups: u64,
    /// 聚集索引表类型标签列表。
    pub ClusteredTableTypes: Vec<String>,
    /// TTL 删除行数样本（按表）。
    pub TTLDeletedRows: Vec<i64>,
    /// TTL 延迟小时数样本：`(表 ID, 小时)`。
    pub TTLDelayHours: Vec<(i64, i64)>,
    /// 注入集群查询失败。
    pub FailClusterQuery: bool,
    /// 注入 TTL 删除行查询失败。
    pub FailTTLDeletedQuery: bool,
    /// 注入 TTL 延迟查询失败。
    pub FailTTLDelayQuery: bool,
    /// 是否开启日志备份相关遥测。
    pub LogBackup: bool,
}
impl Default for SessionContext {
    fn default() -> Self {
        Self {
            GlobalVars: HashMap::new(),
            CostModelVersion: 1,
            EnablePaging: true,
            Schemas: vec![],
            PlacementPolicies: 0,
            ResourceGroups: 0,
            ClusteredTableTypes: vec![],
            TTLDeletedRows: vec![],
            TTLDelayHours: vec![],
            FailClusterQuery: false,
            FailTTLDeletedQuery: false,
            FailTTLDelayQuery: false,
            LogBackup: false,
        }
    }
}
/// 进程级遥测总开关（默认开启）。
fn global_enabled() -> &'static std::sync::atomic::AtomicBool {
    static E: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    &E
}
/// 设置全局遥测开关。
pub fn SetGlobalTelemetryEnabled(v: bool) {
    global_enabled().store(v, std::sync::atomic::Ordering::Release)
}
/// 读取会话变量 `tidb_enable_telemetry`，解析为布尔。
pub fn getTelemetryGlobalVariable(ctx: &SessionContext) -> Result<bool, TelemetryError> {
    let value = ctx
        .GlobalVars
        .get("tidb_enable_telemetry")
        .ok_or_else(|| TelemetryError("tidb_enable_telemetry not found".into()))?;
    Ok(value.eq_ignore_ascii_case("on") || value == "1")
}
/// 综合全局开关与会话变量，判断遥测是否启用。
pub fn IsTelemetryEnabled(ctx: &SessionContext) -> Result<bool, TelemetryError> {
    if !global_enabled().load(std::sync::atomic::Ordering::Acquire) {
        return Ok(false);
    }
    getTelemetryGlobalVariable(ctx)
}
/// 内存中的已上报载荷文本列表。
fn reports() -> &'static Mutex<Vec<String>> {
    static R: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(vec![]))
}
/// 返回当前已缓存的上报文本副本。
pub fn Reports() -> Vec<String> {
    reports().lock().expect("report lock poisoned").clone()
}
/// 若遥测启用则生成数据、执行后置钩子并追加到报告列表。
pub fn ReportUsageData(ctx: &SessionContext) -> Result<(), TelemetryError> {
    if !IsTelemetryEnabled(ctx)? {
        return Ok(());
    }
    let data = crate::generateTelemetryData(ctx);
    crate::postReportTelemetryData();
    reports()
        .lock()
        .expect("report lock poisoned")
        .push(data.Marshal());
    Ok(())
}
/// 首次运行：记录配置摘要并立即上报一次用量。
pub fn InitialRun(ctx: &SessionContext) -> Result<(), TelemetryError> {
    reports()
        .lock()
        .expect("report lock poisoned")
        .push(format!(
            "Telemetry configuration report_interval={:?} enabled={}",
            ReportInterval,
            IsTelemetryEnabled(ctx)?
        ));
    ReportUsageData(ctx)
}
/// 返回遥测模块日志名。
pub fn Logger() -> &'static str {
    "telemetry"
}
