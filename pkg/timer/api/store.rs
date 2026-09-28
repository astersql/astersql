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

// 定时器存储抽象层。
//
// 定义请求上下文、可选字段、查询条件、更新补丁、Watch 事件，
// 以及 `TimerStoreCore`/`TimerStore` 门面与事件通知接口。

use crate::error::{ErrEventIDNotMatch, ErrTimerNotExist, ErrVersionNotMatch, TimerResult};
use crate::timer::{
    EventExtra, ManualRequest, TimerLocation, TimerRecord, Timestamp, parse_location,
};
use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

#[derive(Clone, Default)]
/// 可取消的请求上下文（对应 Go `context.Context` 的简化版）。
pub struct Context {
    cancelled: Arc<AtomicBool>,
}

impl Context {
    /// 不可取消的后台上下文。
    pub fn background() -> Self {
        Self::default()
    }

    /// 占位上下文（语义同 background）。
    pub fn todo() -> Self {
        Self::default()
    }

    /// 创建可取消上下文及其取消句柄。
    pub fn with_cancel() -> (Self, CancelContext) {
        let ctx = Self::default();
        let cancel = CancelContext {
            cancelled: Arc::downgrade(&ctx.cancelled),
        };
        (ctx, cancel)
    }

    /// 是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// 用于取消关联 `Context` 的弱引用句柄。
pub struct CancelContext {
    cancelled: Weak<AtomicBool>,
}

impl CancelContext {
    /// 标记上下文已取消。
    pub fn cancel(&self) {
        if let Some(cancelled) = self.cancelled.upgrade() {
            cancelled.store(true, Ordering::Release);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 可选字段容器：区分“未设置”与“设置为某值”（含默认值）。
pub struct OptionalVal<T> {
    v: Option<T>,
}

impl<T> Default for OptionalVal<T> {
    fn default() -> Self {
        Self { v: None }
    }
}

/// 构造已设置值的 `OptionalVal`。
pub fn NewOptionalVal<T>(value: T) -> OptionalVal<T> {
    OptionalVal { v: Some(value) }
}

impl<T> OptionalVal<T> {
    /// 字段是否已被显式设置。
    pub fn Present(&self) -> bool {
        self.v.is_some()
    }

    /// 若已设置则返回引用。
    pub fn Get(&self) -> Option<&T> {
        self.v.as_ref()
    }

    /// 设置为给定值。
    pub fn Set(&mut self, value: T) {
        self.v = Some(value);
    }

    /// 清除设置状态。
    pub fn Clear(&mut self) {
        self.v = None;
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 定时器查询条件：按 ID/命名空间/Key/Tags 过滤。
pub struct TimerCond {
    /// 按定时器 ID 精确匹配。
    pub ID: OptionalVal<String>,
    /// 按命名空间匹配。
    pub Namespace: OptionalVal<String>,
    /// 按 Key 匹配（可配合 KeyPrefix）。
    pub Key: OptionalVal<String>,
    /// 为 true 时 Key 按前缀匹配。
    pub KeyPrefix: bool,
    /// 要求记录包含全部指定标签。
    pub Tags: OptionalVal<Vec<String>>,
}

impl TimerCond {
    /// 判断记录是否满足本条件。
    pub fn Match(&self, timer: &TimerRecord) -> bool {
        if self.ID.Get().is_some_and(|id| timer.ID != *id) {
            return false;
        }
        if self
            .Namespace
            .Get()
            .is_some_and(|namespace| timer.Namespace != *namespace)
        {
            return false;
        }
        if let Some(key) = self.Key.Get() {
            if self.KeyPrefix && !timer.Key.starts_with(key) {
                return false;
            }
            if !self.KeyPrefix && timer.Key != *key {
                return false;
            }
        }
        if let Some(tags) = self.Tags.Get()
            && tags.iter().any(|tag| !timer.Tags.contains(tag))
        {
            return false;
        }
        true
    }

    /// 返回已设置字段名列表（可排除若干名）。
    pub fn FieldsSet(&self, excludes: &[&str]) -> Vec<String> {
        let fields = [
            ("ID", self.ID.Present()),
            ("Namespace", self.Namespace.Present()),
            ("Key", self.Key.Present()),
            ("Tags", self.Tags.Present()),
        ];
        fields
            .into_iter()
            .filter(|(name, present)| *present && !excludes.contains(name))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// 清空所有条件字段。
    pub fn Clear(&mut self) {
        self.ID.Clear();
        self.Namespace.Clear();
        self.Key.Clear();
        self.Tags.Clear();
        self.KeyPrefix = false;
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 定时器更新补丁：仅 Present 的字段会被应用到记录。
///
/// `CheckVersion`/`CheckEventID` 用于乐观并发校验。
pub struct TimerUpdate {
    pub Tags: OptionalVal<Vec<String>>,
    pub Enable: OptionalVal<bool>,
    pub TimeZone: OptionalVal<String>,
    pub SchedPolicyType: OptionalVal<String>,
    pub SchedPolicyExpr: OptionalVal<String>,
    pub ManualRequest: OptionalVal<ManualRequest>,
    pub EventStatus: OptionalVal<String>,
    pub EventID: OptionalVal<String>,
    pub EventData: OptionalVal<Vec<u8>>,
    pub EventStart: OptionalVal<Option<Timestamp>>,
    pub EventExtra: OptionalVal<EventExtra>,
    pub Watermark: OptionalVal<Option<Timestamp>>,
    pub SummaryData: OptionalVal<Vec<u8>>,
    pub CheckVersion: OptionalVal<u64>,
    pub CheckEventID: OptionalVal<String>,
}

impl TimerUpdate {
    /// 将补丁应用到记录副本；校验失败返回版本/事件 ID 错误。
    pub fn apply(&self, record: &TimerRecord) -> TimerResult<TimerRecord> {
        if self
            .CheckVersion
            .Get()
            .is_some_and(|version| record.Version != *version)
        {
            return Err(ErrVersionNotMatch);
        }
        if self
            .CheckEventID
            .Get()
            .is_some_and(|event_id| record.EventID != *event_id)
        {
            return Err(ErrEventIDNotMatch);
        }

        let mut result = record.clone();
        if let Some(value) = self.Tags.Get() {
            result.Tags = value.clone();
        }
        if let Some(value) = self.Enable.Get() {
            result.Enable = *value;
        }
        if let Some(value) = self.TimeZone.Get() {
            result.TimeZone = value.clone();
            // 时区解析失败时回退到系统/本地固定偏移。
            result.Location = parse_location(value).ok().or_else(|| {
                parse_location("")
                    .ok()
                    .or(Some(TimerLocation::Fixed(*chrono::Local::now().offset())))
            });
        }
        if let Some(value) = self.SchedPolicyType.Get() {
            result.SchedPolicyType = value.clone();
        }
        if let Some(value) = self.SchedPolicyExpr.Get() {
            result.SchedPolicyExpr = value.clone();
        }
        if let Some(value) = self.ManualRequest.Get() {
            result.ManualRequest = value.clone();
        }
        if let Some(value) = self.EventStatus.Get() {
            result.EventStatus = value.clone();
        }
        if let Some(value) = self.EventID.Get() {
            result.EventID = value.clone();
        }
        if let Some(value) = self.EventData.Get() {
            result.EventData = value.clone();
        }
        if let Some(value) = self.EventStart.Get() {
            result.EventStart = *value;
        }
        if let Some(value) = self.EventExtra.Get() {
            result.EventExtra = value.clone();
        }
        if let Some(value) = self.Watermark.Get() {
            result.Watermark = *value;
        }
        if let Some(value) = self.SummaryData.Get() {
            result.SummaryData = value.clone();
        }
        Ok(result)
    }

    /// 返回更新补丁中已设置的字段名。
    pub fn FieldsSet(&self, excludes: &[&str]) -> Vec<String> {
        let fields = [
            ("Tags", self.Tags.Present()),
            ("Enable", self.Enable.Present()),
            ("TimeZone", self.TimeZone.Present()),
            ("SchedPolicyType", self.SchedPolicyType.Present()),
            ("SchedPolicyExpr", self.SchedPolicyExpr.Present()),
            ("ManualRequest", self.ManualRequest.Present()),
            ("EventStatus", self.EventStatus.Present()),
            ("EventID", self.EventID.Present()),
            ("EventData", self.EventData.Present()),
            ("EventStart", self.EventStart.Present()),
            ("EventExtra", self.EventExtra.Present()),
            ("Watermark", self.Watermark.Present()),
            ("SummaryData", self.SummaryData.Present()),
            ("CheckVersion", self.CheckVersion.Present()),
            ("CheckEventID", self.CheckEventID.Present()),
        ];
        fields
            .into_iter()
            .filter(|(name, present)| *present && !excludes.contains(name))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// 重置为默认空补丁。
    pub fn Clear(&mut self) {
        *self = Self::default();
    }
}

/// 可组合的匹配条件抽象（支持 And/Or/Not）。
pub trait Cond: Any + Send + Sync {
    fn Match(&self, timer: &TimerRecord) -> bool;
    fn as_any(&self) -> &dyn Any;
}

// 将 TimerCond 接入 Cond trait。
impl Cond for TimerCond {
    fn Match(&self, timer: &TimerRecord) -> bool {
        TimerCond::Match(self, timer)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 逻辑组合运算符类型。
pub enum OperatorTp {
    /// 全部子条件为真。
    OperatorAnd,
    /// 任一子条件为真。
    OperatorOr,
}

#[derive(Clone)]
/// 条件组合节点：And/Or，并可取反。
pub struct Operator {
    pub Op: OperatorTp,
    pub Not: bool,
    pub Children: Vec<Arc<dyn Cond>>,
}

/// 构造 AND 组合条件。
pub fn And(children: Vec<Arc<dyn Cond>>) -> Operator {
    Operator {
        Op: OperatorTp::OperatorAnd,
        Not: false,
        Children: children,
    }
}

/// 构造 OR 组合条件。
pub fn Or(children: Vec<Arc<dyn Cond>>) -> Operator {
    Operator {
        Op: OperatorTp::OperatorOr,
        Not: false,
        Children: children,
    }
}

/// 对条件取反；若已是 Operator 则翻转其 Not 标志。
pub fn Not(cond: Arc<dyn Cond>) -> Operator {
    if let Some(operator) = cond.as_any().downcast_ref::<Operator>() {
        let mut result = operator.clone();
        result.Not = !result.Not;
        return result;
    }
    Operator {
        Op: OperatorTp::OperatorAnd,
        Not: true,
        Children: vec![cond],
    }
}

// And/Or 求值后按 Not 翻转结果。
impl Cond for Operator {
    fn Match(&self, timer: &TimerRecord) -> bool {
        let matched = match self.Op {
            OperatorTp::OperatorAnd => self.Children.iter().all(|child| child.Match(timer)),
            OperatorTp::OperatorOr => self.Children.iter().any(|child| child.Match(timer)),
        };
        matched != self.Not
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Watch 事件类型位标志。
pub type WatchTimerEventType = i8;
/// 创建事件。
pub const WatchTimerEventCreate: WatchTimerEventType = 1 << 0;
/// 更新事件。
pub const WatchTimerEventUpdate: WatchTimerEventType = 1 << 1;
/// 删除事件。
pub const WatchTimerEventDelete: WatchTimerEventType = 1 << 2;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条 Watch 通知：类型与定时器 ID。
pub struct WatchTimerEvent {
    pub Tp: WatchTimerEventType,
    pub TimerID: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一批 Watch 事件的响应。
pub struct WatchTimerResponse {
    pub Events: Vec<WatchTimerEvent>,
}

/// 接收 Watch 响应的通道类型。
pub type WatchTimerChan = crossbeam_channel::Receiver<WatchTimerResponse>;

/// 定时器持久化核心接口（CRUD + Watch）。
pub trait TimerStoreCore: Send + Sync {
    fn Create(&self, ctx: &Context, record: Option<TimerRecord>) -> TimerResult<String>;
    fn List(&self, ctx: &Context, cond: Option<&dyn Cond>) -> TimerResult<Vec<TimerRecord>>;
    fn Update(&self, ctx: &Context, timerID: &str, update: Option<TimerUpdate>) -> TimerResult<()>;
    fn Delete(&self, ctx: &Context, timerID: &str) -> TimerResult<bool>;
    fn WatchSupported(&self) -> bool;
    fn Watch(&self, ctx: &Context) -> WatchTimerChan;
    fn Close(&self);
}

#[derive(Clone)]
/// 对 `TimerStoreCore` 的引用包装门面。
pub struct TimerStore {
    pub TimerStoreCore: Arc<dyn TimerStoreCore>,
}

impl TimerStore {
    /// 从具体实现构造门面。
    pub fn from_core(core: impl TimerStoreCore + 'static) -> Self {
        Self {
            TimerStoreCore: Arc::new(core),
        }
    }

    pub fn Create(&self, ctx: &Context, record: Option<TimerRecord>) -> TimerResult<String> {
        self.TimerStoreCore.Create(ctx, record)
    }

    pub fn List(&self, ctx: &Context, cond: Option<&dyn Cond>) -> TimerResult<Vec<TimerRecord>> {
        self.TimerStoreCore.List(ctx, cond)
    }

    pub fn Update(
        &self,
        ctx: &Context,
        timerID: &str,
        update: Option<TimerUpdate>,
    ) -> TimerResult<()> {
        self.TimerStoreCore.Update(ctx, timerID, update)
    }

    pub fn Delete(&self, ctx: &Context, timerID: &str) -> TimerResult<bool> {
        self.TimerStoreCore.Delete(ctx, timerID)
    }

    pub fn WatchSupported(&self) -> bool {
        self.TimerStoreCore.WatchSupported()
    }

    pub fn Watch(&self, ctx: &Context) -> WatchTimerChan {
        self.TimerStoreCore.Watch(ctx)
    }

    pub fn Close(&self) {
        self.TimerStoreCore.Close();
    }

    /// 按 ID 取唯一记录，不存在则 `ErrTimerNotExist`。
    pub fn GetByID(&self, ctx: &Context, timerID: &str) -> TimerResult<TimerRecord> {
        let cond = TimerCond {
            ID: NewOptionalVal(timerID.to_string()),
            ..TimerCond::default()
        };
        self.getOneRecord(ctx, &cond)
    }

    /// 按命名空间与 Key 取唯一记录。
    pub fn GetByKey(&self, ctx: &Context, namespace: &str, key: &str) -> TimerResult<TimerRecord> {
        let cond = TimerCond {
            Namespace: NewOptionalVal(namespace.to_string()),
            Key: NewOptionalVal(key.to_string()),
            ..TimerCond::default()
        };
        self.getOneRecord(ctx, &cond)
    }

    /// List 后取首条；空结果视为不存在。
    fn getOneRecord(&self, ctx: &Context, cond: &dyn Cond) -> TimerResult<TimerRecord> {
        self.List(ctx, Some(cond))?
            .into_iter()
            .next()
            .ok_or(ErrTimerNotExist)
    }
}

/// Watch 订阅与变更通知接口。
pub trait TimerWatchEventNotifier: Send + Sync {
    fn Watch(&self, ctx: &Context) -> WatchTimerChan;
    fn Notify(&self, tp: WatchTimerEventType, timerID: &str);
    fn Close(&self);
}
