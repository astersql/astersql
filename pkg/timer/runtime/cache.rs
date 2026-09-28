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

// 定时器运行时缓存（TimersCache）。
//
// 在内存中维护定时器记录、下次事件/尝试触发时间与处理状态（Idle/Triggering/
// WaitTriggerClose），按 `nextTryTriggerTime` 排序以便 runtime 轮询可触发项。
// 全量刷新会删除远端不存在的条目，对应 Go runtime 的 timers cache。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use crate::api::{SchedEventIdle, SchedEventTrigger, TimerLocation, TimerRecord, Timestamp};
use chrono::{Offset, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// 运行时处理状态枚举底层类型（对应 Go 的 int8）。
pub type RuntimeProcStatus = i8;
/// Go 风格别名，保持机械迁移命名。
pub type runtimeProcStatus = RuntimeProcStatus;

/// 空闲：可被选为下次尝试触发。
pub const procIdle: RuntimeProcStatus = 0;
/// 正在触发：worker 已接手事件。
pub const procTriggering: RuntimeProcStatus = 1;
/// 等待触发关闭：事件已发出，等待 EventID 变更后回到 Idle。
pub const procWaitTriggerClose: RuntimeProcStatus = 2;

/// 可注入的“当前时间”函数，便于测试固定时钟。
pub type NowFn = Arc<dyn Fn() -> Timestamp + Send + Sync + 'static>;

/// 系统当前 UTC 时间（FixedOffset）。
pub fn systemNow() -> Timestamp {
    Utc::now().fixed_offset()
}

/// 远未来哨兵时间，表示“暂无下次尝试触发点”。
fn farFuture() -> Timestamp {
    Utc.with_ymd_and_hms(2999, 1, 1, 0, 0, 0)
        .single()
        .expect("year 2999 is representable")
        .fixed_offset()
}

/// Go `time.Time{}` 对应的 UTC 零值。
fn goZeroTime() -> Timestamp {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0)
        .single()
        .expect("Go zero time is representable")
        .fixed_offset()
}

/// 缓存中单条定时器：记录副本、调度时间与处理状态。
#[derive(Clone, Debug)]
pub struct TimerCacheItem {
    /// 是否已经写入过记录；对应 Go 中 `timer != nil`，不能由业务 ID 推断。
    initialized: bool,
    /// 定时器记录快照。
    pub timer: TimerRecord,
    /// 计算出的下次事件时间；无则暂不可触发。
    pub nextEventTime: Option<Timestamp>,
    /// 下次尝试触发时间（排序键）。
    pub nextTryTriggerTime: Timestamp,
    /// 当前处理状态。
    pub procStatus: RuntimeProcStatus,
    /// 触发流程关联的事件 ID。
    pub triggerEventID: String,
}

/// Go 风格别名。
pub type timerCacheItem = TimerCacheItem;

impl TimerCacheItem {
    /// 构造空条目：下次尝试时间设为远未来，状态 Idle。
    fn empty() -> Self {
        Self {
            initialized: false,
            timer: TimerRecord::default(),
            nextEventTime: None,
            nextTryTriggerTime: farFuture(),
            procStatus: procIdle,
            triggerEventID: String::new(),
        }
    }

    /// 用新记录刷新条目；版本更旧或同版本且 Location 未变则跳过。
    ///
    /// 启用时重算 nextEventTime；手动请求中则立即以 now 作为下次事件时间。
    pub fn update(&mut self, timer: &TimerRecord, nowFunc: &NowFn) -> bool {
        // 已有条目时：拒绝更旧版本；同版本且时区未变则视为无变更。
        if self.initialized {
            if timer.Version < self.timer.Version {
                return false;
            }
            if timer.Version == self.timer.Version
                && !locationChanged(&timer.Location, &self.timer.Location)
            {
                return false;
            }
        }

        self.timer = timer.clone();
        self.initialized = true;
        self.nextEventTime = None;
        self.nextTryTriggerTime = farFuture();

        // 启用时计算下次事件；手动请求中则立即触发。
        if timer.Enable {
            if let Ok((next, true)) = timer.NextEventTime() {
                self.nextEventTime = next;
            }
            if timer.ManualRequest.IsManualRequesting() {
                self.nextEventTime = Some(nowFunc());
            }
        }

        // Idle 用 nextEventTime 作尝试时间；Trigger 用 EventStart。
        match timer.EventStatus.as_str() {
            SchedEventIdle => {
                if let Some(next) = self.nextEventTime {
                    self.nextTryTriggerTime = next;
                }
            }
            SchedEventTrigger => {
                // Go's EventStart is a value field, so an absent Rust Option maps
                // to time.Time{} rather than the far-future cache sentinel.
                self.nextTryTriggerTime = timer.EventStart.unwrap_or_else(goZeroTime);
            }
            _ => {}
        }
        true
    }
}

/// 定时器内存缓存：按下次尝试触发时间排序，并跟踪等待关闭的 ID 集合。
pub struct TimersCache {
    /// timerID → 缓存条目。
    items: HashMap<String, TimerCacheItem>,
    // Go uses container/list. Keeping the IDs in a stable sorted vector gives the
    // same ordering and avoids self-referential list nodes in safe Rust.
    /// 按 nextTryTriggerTime 升序排列的 timerID 列表。
    sorted: Vec<String>,
    /// 处于 WaitTriggerClose 状态的 timerID 集合。
    waitCloseTimerIDs: HashSet<String>,
    /// 当前时间注入点。
    nowFunc: NowFn,
}

/// Go 风格别名。
pub type timersCache = TimersCache;

/// 使用系统时钟创建空缓存。
pub fn newTimersCache() -> TimersCache {
    TimersCache::with_now(Arc::new(systemNow))
}

impl TimersCache {
    /// 使用自定义 NowFn 构造缓存（测试常用固定时钟）。
    pub fn with_now(nowFunc: NowFn) -> Self {
        Self {
            items: HashMap::new(),
            sorted: Vec::new(),
            waitCloseTimerIDs: HashSet::new(),
            nowFunc,
        }
    }

    /// 替换当前时间函数。
    pub fn setNowFunc(&mut self, nowFunc: NowFn) {
        self.nowFunc = nowFunc;
    }

    /// 按 ID 查询缓存条目。
    pub fn item(&self, timerID: &str) -> Option<&TimerCacheItem> {
        self.items.get(timerID)
    }

    /// 返回等待触发关闭的 timerID 集合。
    pub fn waitCloseTimerIDs(&self) -> &HashSet<String> {
        &self.waitCloseTimerIDs
    }

    /// 返回按下次尝试触发时间排序的 ID 列表副本。
    pub fn sortedTimerIDs(&self) -> Vec<String> {
        self.sorted.clone()
    }

    /// 返回当前处于 Idle、可尝试触发的 timerID（保持排序）。
    pub fn tryTriggerTimerIDs(&self) -> Vec<String> {
        self.sorted
            .iter()
            .filter(|id| {
                self.items
                    .get(*id)
                    .is_some_and(|item| item.procStatus == procIdle)
            })
            .cloned()
            .collect()
    }

    /// 插入或更新定时器；若变更则重排。
    ///
    /// 若处于 WaitTriggerClose 且 EventID 已变，则复位为 Idle。
    pub fn updateTimer(&mut self, timer: &TimerRecord) -> bool {
        let timerID = timer.ID.clone();
        let changed = self
            .items
            .entry(timerID.clone())
            .or_insert_with(TimerCacheItem::empty)
            .update(timer, &self.nowFunc);
        if changed {
            self.resort(&timerID);
        }

        // EventID 变化意味着触发流程已结束，可从 WaitTriggerClose 回到 Idle。
        if self.items.get(&timerID).is_some_and(|item| {
            item.procStatus == procWaitTriggerClose && item.triggerEventID != timer.EventID
        }) {
            self.setTimerProcStatus(&timerID, procIdle, String::new());
        }
        changed
    }

    /// 删除定时器及其排序项与 waitClose 跟踪；不存在返回 false。
    pub fn removeTimer(&mut self, timerID: &str) -> bool {
        if self.items.remove(timerID).is_none() {
            return false;
        }
        self.sorted.retain(|id| id != timerID);
        self.waitCloseTimerIDs.remove(timerID);
        true
    }

    /// 是否包含指定 timerID。
    pub fn hasTimer(&self, timerID: &str) -> bool {
        self.items.contains_key(timerID)
    }

    /// 批量部分更新：任一条目变更则返回 true。
    pub fn partialBatchUpdateTimers(&mut self, timers: Vec<TimerRecord>) -> bool {
        timers
            .iter()
            .fold(false, |changed, timer| self.updateTimer(timer) || changed)
    }

    /// 全量同步：删除不在远端列表中的本地条目，再批量更新。
    pub fn fullUpdateTimers(&mut self, timers: Vec<TimerRecord>) {
        // 先收集远端 ID，再删除本地多余项。
        let remoteIDs: HashSet<&str> = timers.iter().map(|timer| timer.ID.as_str()).collect();
        let removed: Vec<String> = self
            .items
            .keys()
            .filter(|id| !remoteIDs.contains(id.as_str()))
            .cloned()
            .collect();
        for timerID in removed {
            self.removeTimer(&timerID);
        }
        self.partialBatchUpdateTimers(timers);
    }

    /// 设置处理状态与关联 eventID；WaitTriggerClose 时加入 waitClose 集合。
    pub fn setTimerProcStatus(
        &mut self,
        timerID: &str,
        status: RuntimeProcStatus,
        triggerEventID: String,
    ) {
        let Some(item) = self.items.get_mut(timerID) else {
            return;
        };
        item.procStatus = status;
        item.triggerEventID = triggerEventID;
        if status == procWaitTriggerClose {
            self.waitCloseTimerIDs.insert(timerID.to_string());
        } else {
            self.waitCloseTimerIDs.remove(timerID);
        }
    }

    /// 更新下次尝试触发时间；Idle 且早于 nextEventTime 的下调会被忽略（重试下限）。
    pub fn updateNextTryTriggerTime(&mut self, timerID: &str, time: Timestamp) {
        let Some(item) = self.items.get_mut(timerID) else {
            return;
        };
        // Idle 状态下不允许把尝试时间压到 nextEventTime 之前。
        if item.timer.EventStatus == SchedEventIdle
            && item
                .nextEventTime
                .is_none_or(|nextEventTime| time < nextEventTime)
        {
            return;
        }
        item.nextTryTriggerTime = time;
        self.resort(timerID);
    }

    /// 按排序遍历 Idle 定时器；回调返回 false 时提前停止。
    pub fn iterTryTriggerTimers<F>(&self, mut fn_: F)
    where
        F: FnMut(&TimerRecord, Timestamp, Option<Timestamp>) -> bool,
    {
        for timerID in &self.sorted {
            let Some(item) = self.items.get(timerID) else {
                continue;
            };
            if item.procStatus == procIdle
                && !fn_(&item.timer, item.nextTryTriggerTime, item.nextEventTime)
            {
                break;
            }
        }
    }

    /// 确保 timerID 在 sorted 中，并按 nextTryTriggerTime 重排。
    fn resort(&mut self, timerID: &str) {
        if !self.sorted.iter().any(|id| id == timerID) {
            self.sorted.push(timerID.to_string());
        }
        let items = &self.items;
        self.sorted.sort_by(|left, right| {
            items[left]
                .nextTryTriggerTime
                .cmp(&items[right].nextTryTriggerTime)
        });
    }
}

/// 计算 Location 相对 UTC 的秒偏移，用于比较时区是否实质变化。
fn locationOffset(location: &TimerLocation) -> i32 {
    match location {
        TimerLocation::Named(tz) => Utc::now()
            .with_timezone(tz)
            .offset()
            .fix()
            .local_minus_utc(),
        TimerLocation::Fixed(offset) => offset.local_minus_utc(),
    }
}

/// 判断两个可选 Location 的 UTC 偏移是否不同。
pub fn locationChanged(a: &Option<TimerLocation>, b: &Option<TimerLocation>) -> bool {
    match (a, b) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(left), Some(right)) => locationOffset(left) != locationOffset(right),
    }
}
