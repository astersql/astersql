// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 会话游标跟踪器。
//
// 负责为服务端游标分配整数 ID、按 ID 查询句柄，以及在回调中遍历全部游标；
// 关闭时从映射表移除，对齐 Go `cursorTracker` 的并发语义。

use super::state::State;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};

// Tracker 对应 Go 的同名 interface：负责创建、按 ID 查询以及遍历 cursor。
/// 游标跟踪器接口：创建、按 ID 查询与遍历。
#[allow(non_snake_case)]
pub trait Tracker {
    /// 以给定状态新建游标并返回共享句柄。
    fn NewCursor(self: &Arc<Self>, state: State) -> Handle;
    /// 按整数 ID 查询游标；不存在时返回 `None`。
    fn GetCursor(&self, id: i64) -> Option<Handle>;
    /// 遍历当前全部游标；回调返回 `false` 时中断。
    fn RangeCursor<F>(&self, f: F)
    where
        F: FnMut(Handle) -> bool;
}

// Go 的 `var _ Tracker = &cursorTracker{}` 是编译期接口断言；通过 impl Tracker for CursorTracker 表达。

// cursorTracker 对应 Go 的私有实现体；cursors 保存 ID 到 Handle 的映射，idAlloc 保留原子递增语义。
/// 游标跟踪器的具体实现：互斥映射表 + 原子 ID 分配器。
pub struct CursorTracker {
    /// ID → 游标句柄映射；加锁保护并发读写。
    cursors: Mutex<HashMap<i64, Handle>>,
    /// 原子递增的游标 ID 分配器，保证跨线程唯一。
    id_alloc: AtomicI64,
}

// NewTracker creates a new cursor tracker.
// NewTracker 对应 Go 构造函数，返回 trait 可用的共享实现；初始 map 为空。
/// 构造空的游标跟踪器并以 `Arc` 共享。
#[allow(non_snake_case)]
pub fn NewTracker() -> Arc<CursorTracker> {
    Arc::new(CursorTracker {
        cursors: Mutex::new(HashMap::new()),
        id_alloc: AtomicI64::new(0),
    })
}

impl Tracker for CursorTracker {
    fn NewCursor(self: &Arc<Self>, state: State) -> Handle {
        // Go 使用 Add(1) 后转 int；这里 fetch_add 返回旧值，所以手动 +1 保持 ID 从 1 开始。
        let id = self.id_alloc.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        let cursor = Arc::new(CursorHandle {
            id,
            state,
            tracker: Arc::downgrade(self),
        });
        self.cursors
            .lock()
            .expect("cursor map poisoned")
            .insert(id, cursor.clone());
        cursor
    }

    fn GetCursor(&self, id: i64) -> Option<Handle> {
        // Go 的 sync.Map.Load 失败返回 nil；用 Option 保留这个分支。
        self.cursors
            .lock()
            .expect("cursor map poisoned")
            .get(&id)
            .cloned()
    }

    fn RangeCursor<F>(&self, mut f: F)
    where
        F: FnMut(Handle) -> bool,
    {
        // Go Range 允许回调返回 false 中断。这里先复制句柄，避免遍历时长期持有锁。
        let values: Vec<Handle> = self
            .cursors
            .lock()
            .expect("cursor map poisoned")
            .values()
            .cloned()
            .collect();
        for cursor in values {
            if !f(cursor) {
                break;
            }
        }
    }
}

impl CursorTracker {
    // remove 对应 Go 的私有方法，由 cursorHandle.Close 调用并从 map 中删除。
    /// 从映射表中移除指定 ID 的游标。
    fn remove(&self, id: i64) {
        self.cursors
            .lock()
            .expect("cursor map poisoned")
            .remove(&id);
    }

    #[cfg(test)]
    pub(crate) fn set_id_alloc_for_test(&self, id: i64) {
        self.id_alloc.store(id, Ordering::SeqCst);
    }
}

// Handle is used to update/close the cursor.
// Handle 对应 Go 的 interface；用 Arc 包装具体 cursorHandle。
/// 游标句柄类型别名：以 `Arc` 共享的 `CursorHandle`。
pub type Handle = Arc<CursorHandle>;

// Go 的 `var _ Handle = &cursorHandle{}` 同样由下面的方法集合表达。

// cursorHandle 保存 cursor ID、状态和所属 tracker；Close 需要反向访问 tracker 删除自身。
/// 单个游标句柄：持有 ID、状态及对跟踪器的弱引用。
pub struct CursorHandle {
    /// 创建时分配的整数 ID。
    id: i64,
    /// 打开时绑定的状态快照。
    state: State,
    /// 所属跟踪器的弱引用，用于 `Close` 时回删映射项。
    tracker: Weak<CursorTracker>,
}

impl CursorHandle {
    // ID 对应 Go 方法，返回创建时分配的整数 ID。
    /// 返回创建时分配的游标 ID。
    #[allow(non_snake_case)]
    pub fn ID(&self) -> i64 {
        self.id
    }

    // Close 对应 Go 方法：只移除追踪表项，不修改状态对象。
    /// 关闭游标：从跟踪器映射中移除自身，可重复调用。
    #[allow(non_snake_case)]
    pub fn Close(&self) {
        if let Some(tracker) = self.tracker.upgrade() {
            tracker.remove(self.id);
        }
    }

    // GetState 对应 Go 方法，返回创建 cursor 时传入的 State。
    /// 返回创建游标时传入的状态快照。
    #[allow(non_snake_case)]
    pub fn GetState(&self) -> State {
        self.state
    }
}
