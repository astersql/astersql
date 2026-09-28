// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 会话级序列（SEQUENCE）最近一次 `NEXTVAL` 缓存。
//
// 序列是按命名对象递增/递减产生整型值的数据库对象；
// 本模块在会话内记录各 sequenceID 最近返回值，供 `LASTVAL` 等读取。

use std::collections::HashMap;
use std::sync::Mutex;

/// Session-scoped cache for the most recent nextval of each sequence.
/// Putting the map inside the mutex preserves Go's concurrency guarantee for
/// shared references instead of requiring exclusive `&mut self` access.
///
/// 会话作用域的序列最近值缓存：将 `HashMap` 放进 `Mutex`，
/// 以共享引用即可更新，对齐 Go 侧并发语义。
#[derive(Default)]
pub struct SequenceState {
    /// sequenceID → 最近一次 nextval 的映射，受互斥锁保护。
    latestValueMap: Mutex<HashMap<i64, i64>>,
}

/// 构造空的序列状态缓存。
pub fn NewSequenceState() -> SequenceState {
    SequenceState::default()
}

impl SequenceState {
    /// 更新指定序列的最近一次取值。
    pub fn UpdateState(&self, sequenceID: i64, value: i64) {
        self.latestValueMap
            .lock()
            .expect("sequence state mutex poisoned")
            .insert(sequenceID, value);
    }

    /// 读取指定序列最近值；未命中时返回 `(0, true, None)`，第二个布尔表示缺失。
    pub fn GetLastValue(&self, sequenceID: i64) -> (i64, bool, Option<String>) {
        match self
            .latestValueMap
            .lock()
            .expect("sequence state mutex poisoned")
            .get(&sequenceID)
            .copied()
        {
            Some(value) => (value, false, None),
            None => (0, true, None),
        }
    }

    /// 克隆当前全部序列状态快照。
    pub fn GetAllStates(&self) -> HashMap<i64, i64> {
        self.latestValueMap
            .lock()
            .expect("sequence state mutex poisoned")
            .clone()
    }

    /// Go's maps.Copy merges the supplied states and does not clear entries
    /// that are absent from the input.
    ///
    /// 合并写入：仅覆盖/追加输入中的条目，不删除输入未包含的既有键。
    pub fn SetAllStates(&self, states: &HashMap<i64, i64>) {
        self.latestValueMap
            .lock()
            .expect("sequence state mutex poisoned")
            .extend(states.iter().map(|(&id, &value)| (id, value)));
    }
}
