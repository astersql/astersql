// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// Region（TiKV 数据分片）复制状态与 PD 调度器配置相关 API。
//
// 通过全局 InfoSyncer 的 PD HTTP 客户端查询 key range 复制进度、
// Region 分布，以及创建/取消调度器任务。

use crate::{ConfigValue, Error, KeyRange, RegionDistributions, Result, getGlobalInfoSyncer};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 放置调度（placement schedule）完成状态。
pub enum PlacementScheduleState {
    #[default]
    /// 尚未开始或未知，视为挂起。
    PlacementScheduleStatePending,
    /// PD 正在复制/调度中。
    PlacementScheduleStateInProgress,
    /// 副本已按规则复制完成（REPLICATED）。
    PlacementScheduleStateScheduled,
}
impl PlacementScheduleState {
    /// 返回与 PD 字符串状态对应的展示名。
    pub fn String(self) -> &'static str {
        match self {
            Self::PlacementScheduleStateScheduled => "SCHEDULED",
            Self::PlacementScheduleStateInProgress => "INPROGRESS",
            Self::PlacementScheduleStatePending => "PENDING",
        }
    }
}

/// 查询 `[startKey, endKey)` 范围内 Region 的复制状态。
///
/// PD 客户端缺失时返回 Pending，避免阻塞调用方。
pub fn GetReplicationState(startKey: Vec<u8>, endKey: Vec<u8>) -> Result<PlacementScheduleState> {
    let is = getGlobalInfoSyncer()?;
    let client = is.pdHTTPCli.read().unwrap().clone();
    let Some(client) = client else {
        return Ok(PlacementScheduleState::PlacementScheduleStatePending);
    };
    Ok(
        match client
            .get_regions_replicated_state(&KeyRange {
                start_key: startKey,
                end_key: endKey,
            })?
            .as_str()
        {
            "REPLICATED" => PlacementScheduleState::PlacementScheduleStateScheduled,
            "INPROGRESS" => PlacementScheduleState::PlacementScheduleStateInProgress,
            _ => PlacementScheduleState::PlacementScheduleStatePending,
        },
    )
}
/// 按 key range 与引擎类型查询 Region 分布统计。
pub fn GetRegionDistributionByKeyRange(
    startKey: Vec<u8>,
    endKey: Vec<u8>,
    engine: &str,
) -> Result<RegionDistributions> {
    let is = getGlobalInfoSyncer()?;
    let client = is
        .pdHTTPCli
        .read()
        .unwrap()
        .clone()
        .ok_or(Error::PdHttpClientMissing)?;
    client.get_region_distribution(
        &KeyRange {
            start_key: startKey,
            end_key: endKey,
        },
        engine,
    )
}
/// 读取指定名称调度器的配置。
pub fn GetSchedulerConfig(schedulerName: &str) -> Result<ConfigValue> {
    let is = getGlobalInfoSyncer()?;
    let client = is
        .pdHTTPCli
        .read()
        .unwrap()
        .clone()
        .ok_or(Error::PdHttpClientMissing)?;
    client.get_scheduler_config(schedulerName)
}
/// 使用输入参数创建（或更新）调度器配置。
pub fn CreateSchedulerConfigWithInput(
    schedulerName: &str,
    input: &HashMap<String, ConfigValue>,
) -> Result<()> {
    let is = getGlobalInfoSyncer()?;
    let client = is
        .pdHTTPCli
        .read()
        .unwrap()
        .clone()
        .ok_or(Error::PdHttpClientMissing)?;
    client.create_scheduler(schedulerName, input)
}
/// 取消指定调度器上的异步任务。
pub fn CancelSchedulerJob(schedulerName: &str, jobID: u64) -> Result<()> {
    let is = getGlobalInfoSyncer()?;
    let client = is
        .pdHTTPCli
        .read()
        .unwrap()
        .clone()
        .ok_or(Error::PdHttpClientMissing)?;
    client.cancel_scheduler_job(schedulerName, jobID)
}
