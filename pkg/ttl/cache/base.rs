// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TTL（Time To Live，按过期时间清理过期行）缓存的基础刷新间隔控制。
//
// `baseCache` 记录「多久刷新一次」与「上次刷新时刻」；上层 InfoSchema / 任务缓存
// 据此判断是否需要重新扫描元数据，避免在 schema 未变化时重复遍历全部 TTL 表。

// TTL cache 的基础更新时间间隔逻辑。

use std::time::{Duration, Instant};

/// TTL 缓存的刷新节拍：保存间隔与最近一次成功更新时间。
///
/// `update_time` 为 `None` 表示从未更新过，此时 `ShouldUpdate` 恒为 true。
#[derive(Clone, Debug)]
pub struct baseCache {
    /// 两次刷新之间的最短间隔。
    interval: Duration,
    /// 最近一次 `MarkUpdated` 的时刻；`None` 表示尚未刷新。
    update_time: Option<Instant>,
}
/// 构造仅带刷新间隔的基础缓存；`update_time` 置为未刷新。
pub fn newBaseCache(interval: Duration) -> baseCache {
    baseCache {
        interval,
        update_time: None,
    }
}
impl baseCache {
    /// 距离上次更新是否已超过 `interval`（或从未更新），需要重新拉取数据。
    pub fn ShouldUpdate(&self) -> bool {
        self.update_time
            .is_none_or(|updated| updated.elapsed() > self.interval)
    }
    /// 替换刷新间隔（例如按系统变量动态调整）。
    pub fn SetInterval(&mut self, interval: Duration) {
        self.interval = interval;
    }
    /// 返回当前刷新间隔。
    pub fn GetInterval(&self) -> Duration {
        self.interval
    }
    /// 将最近更新时间标为当前时刻，表示缓存内容已与源同步。
    pub fn MarkUpdated(&mut self) {
        self.update_time = Some(Instant::now());
    }
}
