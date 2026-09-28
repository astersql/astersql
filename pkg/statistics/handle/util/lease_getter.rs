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

// 统计缓存租约（lease）的读写接口。
//
// 租约控制统计缓存多久后过期需重新加载；以纳秒存入原子变量，便于运行时动态调整。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 统计缓存租约的获取与设置。
pub trait LeaseGetter: Send + Sync {
    fn lease(&self) -> Duration;
    fn set_lease(&self, lease: Duration);
}

/// 基于 `AtomicU64`（纳秒）的租约实现，读写无锁。
pub struct AtomicLeaseGetter {
    nanos: AtomicU64,
}

impl AtomicLeaseGetter {
    /// 以给定初始租约构造。
    pub fn new(lease: Duration) -> Self {
        Self {
            nanos: AtomicU64::new(duration_nanos(lease)),
        }
    }
}

impl LeaseGetter for AtomicLeaseGetter {
    fn lease(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::Acquire))
    }

    fn set_lease(&self, lease: Duration) {
        self.nanos.store(duration_nanos(lease), Ordering::Release);
    }
}

/// 将 `Duration` 转为纳秒 `u64`，超出 `u64::MAX` 时截断。
fn duration_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

/// 构造 trait 对象形式的租约 getter。
pub fn new_lease_getter(lease: Duration) -> Arc<dyn LeaseGetter> {
    Arc::new(AtomicLeaseGetter::new(lease))
}
