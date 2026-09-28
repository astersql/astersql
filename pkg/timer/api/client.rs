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

// 定时器客户端 API。
//
// 提供按条件查询/更新的 Option 构建器，以及基于 `TimerStore` 的默认客户端实现，
// 涵盖创建、查询、更新、手动触发、关闭事件与删除。

use crate::error::{ErrVersionNotMatch, TimerError, TimerResult};
use crate::store::{Context, TimerCond, TimerStore, TimerUpdate};
use crate::timer::{
    EventExtra, ManualRequest, SchedEventIdle, TimerRecord, TimerSpec, Timestamp, now_timestamp,
};
use std::thread;
use std::time::Duration;

/// 手动触发等写操作在版本冲突时的最大重试次数。
pub const clientMaxRetry: usize = 5;
/// 版本冲突重试的退避等待毫秒数。
pub const clientRetryBackoff: u64 = 1000;

/// 查询条件 Option：闭包形式修改 `TimerCond`。
pub type GetTimerOption = Box<dyn Fn(&mut TimerCond) + Send + Sync>;

/// 按精确 Key 匹配（关闭 KeyPrefix）。
pub fn WithKey(key: String) -> GetTimerOption {
    Box::new(move |cond| {
        cond.Key.Set(key.clone());
        cond.KeyPrefix = false;
    })
}

/// 按 Key 前缀匹配。
pub fn WithKeyPrefix(keyPrefix: String) -> GetTimerOption {
    Box::new(move |cond| {
        cond.Key.Set(keyPrefix.clone());
        cond.KeyPrefix = true;
    })
}

/// 按定时器 ID 匹配。
pub fn WithID(id: String) -> GetTimerOption {
    Box::new(move |cond| cond.ID.Set(id.clone()))
}

/// 要求记录包含给定全部 Tags。
pub fn WithTag(tags: Vec<String>) -> GetTimerOption {
    Box::new(move |cond| cond.Tags.Set(tags.clone()))
}

/// 更新字段 Option：闭包形式修改 `TimerUpdate`。
pub type UpdateTimerOption = Box<dyn Fn(&mut TimerUpdate) + Send + Sync>;

/// 设置是否启用定时器。
pub fn WithSetEnable(enable: bool) -> UpdateTimerOption {
    Box::new(move |update| update.Enable.Set(enable))
}

/// 设置时区名称。
pub fn WithSetTimeZone(name: String) -> UpdateTimerOption {
    Box::new(move |update| update.TimeZone.Set(name.clone()))
}

/// 设置调度策略类型与表达式（如 interval/cron）。
pub fn WithSetSchedExpr(policy_type: String, expr: String) -> UpdateTimerOption {
    Box::new(move |update| {
        update.SchedPolicyType.Set(policy_type.clone());
        update.SchedPolicyExpr.Set(expr.clone());
    })
}

/// 设置水位线（Watermark：已处理到的时间点，用于计算下次触发）。
pub fn WithSetWatermark(watermark: Timestamp) -> UpdateTimerOption {
    Box::new(move |update| update.Watermark.Set(Some(watermark)))
}

/// 设置摘要数据。
pub fn WithSetSummaryData(summary: Vec<u8>) -> UpdateTimerOption {
    Box::new(move |update| update.SummaryData.Set(summary.clone()))
}

/// 设置标签列表。
pub fn WithSetTags(tags: Vec<String>) -> UpdateTimerOption {
    Box::new(move |update| update.Tags.Set(tags.clone()))
}

/// 定时器客户端接口：对上层暴露命名空间内的 CRUD 与事件操作。
pub trait TimerClient: Send + Sync {
    /// 返回客户端默认命名空间。
    fn GetDefaultNamespace(&self) -> String;
    /// 创建定时器并返回完整记录。
    fn CreateTimer(&self, ctx: &Context, spec: TimerSpec) -> TimerResult<TimerRecord>;
    /// 按 ID 获取定时器。
    fn GetTimerByID(&self, ctx: &Context, timerID: &str) -> TimerResult<TimerRecord>;
    /// 按默认命名空间下的 Key 获取定时器。
    fn GetTimerByKey(&self, ctx: &Context, key: &str) -> TimerResult<TimerRecord>;
    /// 按 Option 组合条件列出定时器。
    fn GetTimers(&self, ctx: &Context, opts: Vec<GetTimerOption>) -> TimerResult<Vec<TimerRecord>>;
    /// 按 ID 更新定时器字段。
    fn UpdateTimer(
        &self,
        ctx: &Context,
        timerID: &str,
        opts: Vec<UpdateTimerOption>,
    ) -> TimerResult<()>;
    /// 请求手动触发：写入 ManualRequest，返回请求 ID。
    fn ManualTriggerEvent(&self, ctx: &Context, timerID: &str) -> TimerResult<String>;
    /// 关闭进行中的调度事件，可将 Watermark/Summary 一并更新。
    fn CloseTimerEvent(
        &self,
        ctx: &Context,
        timerID: &str,
        eventID: &str,
        opts: Vec<UpdateTimerOption>,
    ) -> TimerResult<()>;
    /// 删除定时器；返回是否实际删除。
    fn DeleteTimer(&self, ctx: &Context, timerID: &str) -> TimerResult<bool>;
}

/// 默认存储命名空间名称。
pub const DefaultStoreNamespace: &str = "default";

#[derive(Clone)]
/// 基于 `TimerStore` 的默认客户端实现。
pub struct DefaultTimerClient {
    /// 客户端绑定的命名空间。
    pub namespace: String,
    /// 底层定时器存储。
    pub store: TimerStore,
    /// 版本冲突重试退避（毫秒）。
    pub retryBackoff: u64,
}

/// 使用默认命名空间与重试参数构造客户端。
pub fn NewDefaultTimerClient(store: TimerStore) -> DefaultTimerClient {
    DefaultTimerClient {
        namespace: DefaultStoreNamespace.to_string(),
        store,
        retryBackoff: clientRetryBackoff,
    }
}

// 将 Option 折叠为 Cond/Update 后委托给 store；手动触发带版本冲突重试。
impl TimerClient for DefaultTimerClient {
    fn GetDefaultNamespace(&self) -> String {
        self.namespace.clone()
    }

    // 未指定 Namespace 时填入客户端默认值。
    fn CreateTimer(&self, ctx: &Context, mut spec: TimerSpec) -> TimerResult<TimerRecord> {
        if spec.Namespace.is_empty() {
            spec.Namespace = self.namespace.clone();
        }
        let timer_id = self.store.Create(
            ctx,
            Some(TimerRecord {
                TimerSpec: spec,
                ..TimerRecord::default()
            }),
        )?;
        self.store.GetByID(ctx, &timer_id)
    }

    fn GetTimerByID(&self, ctx: &Context, timerID: &str) -> TimerResult<TimerRecord> {
        self.store.GetByID(ctx, timerID)
    }

    fn GetTimerByKey(&self, ctx: &Context, key: &str) -> TimerResult<TimerRecord> {
        self.store.GetByKey(ctx, &self.namespace, key)
    }

    fn GetTimers(&self, ctx: &Context, opts: Vec<GetTimerOption>) -> TimerResult<Vec<TimerRecord>> {
        let mut cond = TimerCond::default();
        for option in opts {
            option(&mut cond);
        }
        self.store.List(ctx, Some(&cond))
    }

    fn UpdateTimer(
        &self,
        ctx: &Context,
        timerID: &str,
        opts: Vec<UpdateTimerOption>,
    ) -> TimerResult<()> {
        let mut update = TimerUpdate::default();
        for option in opts {
            option(&mut update);
        }
        self.store.Update(ctx, timerID, Some(update))
    }

    // 生成请求 ID，在事件未关闭且已启用时写入 ManualRequest；
    // 若版本不匹配则退避重试，耗尽后返回 ErrVersionNotMatch。
    fn ManualTriggerEvent(&self, ctx: &Context, timerID: &str) -> TimerResult<String> {
        let request_id = uuid::Uuid::new_v4().simple().to_string();
        for attempt in 0..clientMaxRetry {
            let timer = self.store.GetByID(ctx, timerID)?;
            if !timer.EventID.is_empty() {
                return Err(TimerError::message(
                    "manual trigger is not allowed when event is not closed",
                ));
            }
            if !timer.Enable {
                return Err(TimerError::message(
                    "manual trigger is not allowed when timer is disabled",
                ));
            }

            let mut update = TimerUpdate::default();
            update.ManualRequest.Set(ManualRequest {
                ManualRequestID: request_id.clone(),
                ManualRequestTime: Some(now_timestamp()),
                ManualTimeout: Duration::from_secs(2 * 60),
                ..ManualRequest::default()
            });
            update.CheckVersion.Set(timer.Version);

            // 乐观并发：版本冲突时重试，其他错误直接返回。
            match self.store.Update(ctx, timerID, Some(update)) {
                Ok(()) => return Ok(request_id),
                Err(error) if error == ErrVersionNotMatch => {
                    if attempt + 1 == clientMaxRetry {
                        return Err(error);
                    }
                    if self.retryBackoff > 0 {
                        thread::sleep(Duration::from_millis(self.retryBackoff));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(ErrVersionNotMatch)
    }

    // 关闭事件时禁止修改调度相关字段；未显式设 Watermark 则用 EventStart。
    fn CloseTimerEvent(
        &self,
        ctx: &Context,
        timerID: &str,
        eventID: &str,
        opts: Vec<UpdateTimerOption>,
    ) -> TimerResult<()> {
        let mut update = TimerUpdate::default();
        for option in opts {
            option(&mut update);
        }
        // 除 Watermark/SummaryData 外若还有待写字段则拒绝。
        let fields = update.FieldsSet(&["Watermark", "SummaryData"]);
        if !fields.is_empty() {
            return Err(TimerError::message(format!(
                "The field(s) [{}] are not allowed to update when close event",
                fields.join(", ")
            )));
        }

        let timer = self.GetTimerByID(ctx, timerID)?;
        update.CheckEventID.Set(eventID.to_string());
        update.EventStatus.Set(SchedEventIdle.to_string());
        update.EventID.Set(String::new());
        update.EventData.Set(Vec::new());
        update.EventStart.Set(None);
        // 默认把水位推进到本次事件开始时间。
        if !update.Watermark.Present() {
            update.Watermark.Set(timer.EventStart);
        }
        update.EventExtra.Set(EventExtra::default());
        self.store.Update(ctx, timerID, Some(update))
    }

    fn DeleteTimer(&self, ctx: &Context, timerID: &str) -> TimerResult<bool> {
        self.store.Delete(ctx, timerID)
    }
}
