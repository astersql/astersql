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

// ingest 内存配额根（MemRoot）。
//
// DDL 快速加索引时，本地引擎与写入器会占用大量内存。MemRoot 统一跟踪
// 当前用量与上限（quota），并支持按标签（tag，例如某个索引引擎）分别记账，
// 以便在超限前拒绝分配或触发刷盘/导入。

use std::collections::BTreeMap;
use std::sync::Mutex;

/// 内存配额跟踪接口：设置上限、增减用量，并按标签分别统计。
pub trait MemRoot: Send + Sync {
    /// 设置最大内存配额（字节）。
    fn set_max_memory_quota(&self, quota: i64);
    /// 返回当前最大内存配额（字节）。
    fn max_memory_quota(&self) -> i64;
    /// 返回当前总内存用量（字节）。
    fn current_usage(&self) -> i64;
    /// 返回指定标签下的内存用量（字节）；无记录时为 0。
    fn current_usage_with_tag(&self, tag: &str) -> i64;
    /// 增加总用量。
    fn consume(&self, size: i64);
    /// 减少总用量。
    fn release(&self, size: i64);
    /// 增加指定标签和总用量。
    fn consume_with_tag(&self, tag: &str, size: i64);
    /// 预检：若再消耗 `size` 字节是否仍不超过配额。
    fn check_consume(&self, size: i64) -> bool;
    /// 释放某标签的全部用量，并从总用量中扣除。
    fn release_with_tag(&self, tag: &str);
    /// 刷新用量；当前实现与 Go 一致，为保留接口的空操作。
    fn refresh_consumption(&self);
}

/// 内部用量记账：总量 + 按标签分账。
#[derive(Debug, Default)]
struct Usage {
    /// 当前总占用字节数。
    current: i64,
    /// 各标签对应的占用字节数。
    by_tag: BTreeMap<String, i64>,
}

/// `MemRoot` 的默认实现，用互斥锁保护配额与用量状态。
#[derive(Debug)]
pub struct MemRootImpl {
    /// 最大内存配额（字节）。
    max_quota: Mutex<i64>,
    /// 当前用量明细。
    usage: Mutex<Usage>,
}

impl MemRootImpl {
    /// 创建指定配额的内存根。
    pub fn new(max_quota: i64) -> Self {
        Self {
            max_quota: Mutex::new(max_quota),
            usage: Mutex::new(Usage::default()),
        }
    }
}

impl MemRoot for MemRootImpl {
    fn set_max_memory_quota(&self, quota: i64) {
        *self.max_quota.lock().unwrap() = quota;
    }
    fn max_memory_quota(&self) -> i64 {
        *self.max_quota.lock().unwrap()
    }
    fn current_usage(&self) -> i64 {
        self.usage.lock().unwrap().current
    }
    fn current_usage_with_tag(&self, tag: &str) -> i64 {
        self.usage
            .lock()
            .unwrap()
            .by_tag
            .get(tag)
            .copied()
            .unwrap_or(0)
    }
    fn consume(&self, size: i64) {
        let mut usage = self.usage.lock().unwrap();
        usage.current = usage.current.wrapping_add(size);
    }
    fn release(&self, size: i64) {
        let mut usage = self.usage.lock().unwrap();
        usage.current = usage.current.wrapping_sub(size);
    }
    fn consume_with_tag(&self, tag: &str, size: i64) {
        let mut usage = self.usage.lock().unwrap();
        usage.current = usage.current.wrapping_add(size);
        let tagged = usage.by_tag.entry(tag.to_owned()).or_default();
        *tagged = tagged.wrapping_add(size);
    }
    fn check_consume(&self, size: i64) -> bool {
        self.current_usage().wrapping_add(size) <= self.max_memory_quota()
    }
    fn release_with_tag(&self, tag: &str) {
        let mut usage = self.usage.lock().unwrap();
        if let Some(size) = usage.by_tag.remove(tag) {
            usage.current = usage.current.wrapping_sub(size);
        }
    }
    fn refresh_consumption(&self) {}
}
