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

// 定时器调度事件的 Hook 接口。
//
// 在调度前后注入自定义逻辑（如延迟触发、附加 EventData），供 Timer 调度器回调。

use crate::client::TimerClient;
use crate::error::TimerResult;
use crate::store::Context;
use crate::timer::TimerRecord;
use std::sync::Arc;
use std::time::Duration;

/// 一次调度事件的只读视图：事件 ID 与关联的定时器记录。
pub trait TimerShedEvent: Send + Sync {
    /// 返回本次调度事件的唯一标识。
    fn EventID(&self) -> String;
    /// 返回触发该事件的定时器记录（可能尚未加载则为 `None`）。
    fn Timer(&self) -> Option<TimerRecord>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// `OnPreSchedEvent` 的返回值：可请求延迟，并附带事件载荷。
pub struct PreSchedEventResult {
    /// 建议推迟触发的时长；为零表示立即调度。
    pub Delay: Duration,
    /// 写入定时器事件的附加二进制数据。
    pub EventData: Vec<u8>,
}

/// 定时器调度生命周期钩子。
///
/// `Start`/`Stop` 控制钩子启停；`OnPreSchedEvent` 在真正调度前调用，
/// `OnSchedEvent` 在事件进入触发状态后调用。
pub trait Hook: Send {
    /// 启动钩子（可注册资源或后台任务）。
    fn Start(&mut self);
    /// 停止钩子并释放资源。
    fn Stop(&mut self);
    /// 调度前钩子：可返回延迟与 EventData。
    fn OnPreSchedEvent(
        &mut self,
        ctx: &Context,
        event: &dyn TimerShedEvent,
    ) -> TimerResult<PreSchedEventResult>;
    /// 调度后钩子：事件已进入触发流程时调用。
    fn OnSchedEvent(&mut self, ctx: &Context, event: &dyn TimerShedEvent) -> TimerResult<()>;
}

/// 按钩子类名与 TimerClient 构造 Hook 实例的工厂函数类型。
pub type HookFactory =
    Arc<dyn Fn(String, Box<dyn TimerClient>) -> Box<dyn Hook> + Send + Sync + 'static>;
