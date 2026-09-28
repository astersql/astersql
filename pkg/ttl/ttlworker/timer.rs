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

// TTL 定时器钩子：在调度窗口内提交作业，并轮询直至作业完成。
//
// 对接外部定时器框架的事件驱动模型：`on_event` 尝试提交 TTL Job，
// `poll` 在作业结束后汇总 `TtlTimerSummary`。`TtlTimerRuntime` 仅跟踪启停标志。

use crate::job_manager::TtlSummary;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 定时器侧记录的最近一次作业摘要，供下次调度参考。
pub struct TtlTimerSummary {
    /// 最近一次作业请求 ID。
    pub last_job_request_id: String,
    /// 作业完成时的汇总统计；未完成时为 `None`。
    pub last_job_summary: Option<TtlSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 作业适配层返回的作业追踪快照。
pub struct TtlJobTrace {
    /// 请求/作业 ID。
    /// 本次触发关联的作业请求 ID。
    pub request_id: String,
    /// 是否已结束。
    pub finished: bool,
    /// 结束后的汇总；未结束时通常为 `None`。
    pub summary: Option<TtlSummary>,
}

/// 将定时器事件桥接到 TTL Job 管理器的适配接口。
pub trait TtlJobAdapter {
    /// 当前是否允许对该物理表提交新作业（容量、冲突等）。
    fn can_submit_job(&self, table_id: i64, physical_id: i64) -> bool;
    /// 提交 TTL 作业并返回追踪信息。
    fn submit_job(
        &mut self,
        table_id: i64,
        physical_id: i64,
        request_id: &str,
        now: u64,
    ) -> Result<TtlJobTrace, String>;
    /// 按请求 ID 查询作业状态。
    fn get_job(
        &self,
        table_id: i64,
        physical_id: i64,
        request_id: &str,
    ) -> Result<TtlJobTrace, String>;
    /// 适配器提供的当前时间（Unix 秒）。
    fn now(&self) -> u64;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 定时器触发的一次事件载荷。
pub struct TimerEvent {
    /// 定时器框架内的事件 ID。
    pub event_id: String,
    /// 逻辑表 ID。
    pub table_id: i64,
    /// 物理表 ID（分区场景下与 table_id 可能不同）。
    pub physical_id: i64,
    /// 旧调用方保留的请求 ID；Go 定时器协议以 `event_id` 作为 TTL job request ID。
    pub request_id: String,
    /// 事件开始时间（Unix 秒），提交作业时作为 watermark。
    pub created_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 钩子对定时器框架的响应：重试、已提交或已关闭并带回摘要。
pub enum TimerResponse {
    /// 不在窗口或暂不可提交，请稍后重试。
    Retry,
    /// 作业已提交或仍在运行中。
    Submitted(TtlJobTrace),
    /// 作业已完成，可关闭本次定时器事件。
    Closed(TtlTimerSummary),
}

/// TTL 定时器钩子：持有作业适配器与每日可调度的分钟窗口。
pub struct TtlTimerHook<A: TtlJobAdapter> {
    /// 作业提交/查询适配器。
    pub adapter: A,
    /// 允许调度的起始分钟（一天内 0..1440）。
    pub schedule_start_minute: u16,
    /// 允许调度的结束分钟；若小于 start 则表示跨午夜窗口。
    pub schedule_end_minute: u16,
}

impl<A: TtlJobAdapter> TtlTimerHook<A> {
    /// 处理定时器触发：窗口外或不可提交则 `Retry`，否则提交作业。
    pub fn on_event(&mut self, event: &TimerEvent) -> Result<TimerResponse, String> {
        let now = self.adapter.now();
        // 非调度窗口或适配器拒绝提交时，请求框架稍后重试。
        let minute = ((now / 60) % (24 * 60)) as u16;
        if !within_window(minute, self.schedule_start_minute, self.schedule_end_minute)
            || !self
                .adapter
                .can_submit_job(event.table_id, event.physical_id)
        {
            return Ok(TimerResponse::Retry);
        }
        self.adapter
            .submit_job(
                event.table_id,
                event.physical_id,
                &event.event_id,
                event.created_at,
            )
            .map(TimerResponse::Submitted)
    }
    /// 轮询作业是否完成；完成后组装 `TtlTimerSummary` 并关闭事件。
    pub fn poll(&self, event: &TimerEvent) -> Result<TimerResponse, String> {
        let trace = self
            .adapter
            .get_job(event.table_id, event.physical_id, &event.event_id)?;
        if !trace.finished {
            return Ok(TimerResponse::Submitted(trace));
        }
        Ok(TimerResponse::Closed(TtlTimerSummary {
            last_job_request_id: event.event_id.clone(),
            last_job_summary: trace.summary,
        }))
    }
}

/// 判断当天分钟数是否落在 Go `WithinDayTimePeriod` 的闭区间 `[start, end]`；
/// `start > end` 时为跨午夜区间。
fn within_window(minute: u16, start: u16, end: u16) -> bool {
    if start <= end {
        (start..=end).contains(&minute)
    } else {
        minute >= start || minute <= end
    }
}

#[derive(Default)]
/// TTL 定时器运行时的轻量启停状态。
pub struct TtlTimerRuntime {
    /// 是否处于运行（未 pause）状态。
    pub running: bool,
}
impl TtlTimerRuntime {
    /// 恢复运行；可重复调用。
    pub fn resume(&mut self) {
        self.running = true;
    }
    /// 暂停运行；可重复调用。
    pub fn pause(&mut self) {
        self.running = false;
    }
}
