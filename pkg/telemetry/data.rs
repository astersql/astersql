// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 遥测上报顶层数据结构与生命周期钩子。
//
// 组装一次遥测报告：时间戳、功能使用统计（featureUsage）、
// 滑动窗口统计（windowData）；上报成功后重置各计数器。

use crate::*;
use std::time::{SystemTime, UNIX_EPOCH};

/// 一次遥测上报的完整载荷。
#[derive(Clone, Debug, Default)]
pub struct telemetryData {
    /// 报告生成的 Unix 秒时间戳。
    pub ReportTimestamp: i64,
    /// 功能使用情况；采集失败时为 None，序列化为 JSON null。
    pub FeatureUsage: Option<featureUsage>,
    /// 按时间窗口聚合的执行/下推/缓存等统计。
    pub WindowedStats: Vec<windowData>,
}
impl telemetryData {
    /// 序列化为与 Go 侧字段名一致的 JSON 字符串。
    pub fn Marshal(&self) -> String {
        let feature = self
            .FeatureUsage
            .as_ref()
            .map(featureUsage::Marshal)
            .unwrap_or_else(|| "null".into());
        let windows = self
            .WindowedStats
            .iter()
            .map(windowData::Marshal)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"reportTimestamp\":{},\"featureUsage\":{},\"windowedStats\":[{}]}}",
            self.ReportTimestamp, feature, windows
        )
    }
}
/// 基于当前会话上下文采集并组装一份遥测数据。
pub fn generateTelemetryData(ctx: &SessionContext) -> telemetryData {
    telemetryData {
        ReportTimestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
        // 功能使用采集失败时降级为 None，不阻断整份报告。
        FeatureUsage: getFeatureUsage(ctx).ok(),
        WindowedStats: getWindowData(),
    }
}

/// 上报成功后的清理：将各类型使用计数快照回基准，避免重复累计。
pub fn postReportTelemetryData() {
    postReportTxnUsage();
    postReportCTEUsage();
    postReportAccountLockUsage();
    postReportMultiSchemaChangeUsage();
    postReportExchangePartitionUsage();
    postReportTablePartitionUsage();
    postReportNonTransactionalCounter();
    PostSavepointCount();
    postReportLazyPessimisticUniqueCheckSetCount();
    postReportDDLUsage();
    postReportIndexMergeUsage();
    postStoreBatchUsage();
    postReportFairLockingUsageCounter();
}

/// 测试用精简清理入口，仅重置表分区相关计数。
pub fn PostReportTelemetryDataForTest() {
    postReportTablePartitionUsage()
}
