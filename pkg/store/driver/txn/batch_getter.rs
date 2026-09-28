// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 批量读取适配器：缓冲层、语句级中间缓存与快照三层 BatchGet。
//
// 事务（transaction）写缓冲中的空值视为删除墓碑（tombstone），会抑制下层快照
// 查找且不出现在对外批量结果中。中间缓存（middle cache）补齐缓冲未命中的键。

use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    BatchBufferGetter, BatchGetOption, BatchGetToGetOptions, BatchGetter, DriverError, GetOption,
    Getter, Key, ValueEntry,
};

/// Adapter from TiDB's typed key slice to client-go's byte-slice batch getter.
///
/// 将 TiDB 侧 `BatchGetter` 适配为 client-go 风格的字节键批量获取器。
pub struct tikvBatchGetter {
    /// 底层 TiDB 批量获取实现。
    tidb_batch_getter: Arc<dyn BatchGetter>,
}

impl tikvBatchGetter {
    /// 用给定的 TiDB `BatchGetter` 构造适配器。
    pub fn new(batch_getter: Arc<dyn BatchGetter>) -> Self {
        Self {
            tidb_batch_getter: batch_getter,
        }
    }

    /// 批量获取键对应的值条目（含可选 commit_ts）。
    pub fn BatchGet(
        &self,
        keys: &[Vec<u8>],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        self.tidb_batch_getter.batch_get(keys, options)
    }
}

/// Buffer plus optional statement-level middle-cache adapter.
///
/// 内存写缓冲 + 可选语句级中间缓存的组合读取器。
pub struct tikvBatchBufferGetter {
    /// 语句级中间缓存；缓冲未命中时可回落至此。
    tidb_middle_cache: Option<Arc<dyn Getter>>,
    /// 事务内存写缓冲。
    tidb_buffer: Arc<dyn BatchBufferGetter>,
}

impl tikvBatchBufferGetter {
    /// 构造缓冲读取器；`middle_cache` 为 `None` 时 miss 直接映射为 `ClientNotExist`。
    pub fn new(buffer: Arc<dyn BatchBufferGetter>, middle_cache: Option<Arc<dyn Getter>>) -> Self {
        Self {
            tidb_middle_cache: middle_cache,
            tidb_buffer: buffer,
        }
    }

    /// 先查缓冲；not found 时再查中间缓存，并将 miss 规范化为 `ClientNotExist`。
    pub fn Get(&self, key: &[u8], options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        match self.tidb_buffer.get(key, options) {
            Ok(value) => Ok(value),
            Err(error) if !error.is_not_found() => Err(error),
            Err(_) if self.tidb_middle_cache.is_none() => Err(DriverError::ClientNotExist),
            Err(_) => match self
                .tidb_middle_cache
                .as_ref()
                .expect("middle cache was checked")
                .get(key, options)
            {
                Ok(value) => Ok(value),
                // client-go's BufferBatchGetter recognizes only its own
                // ErrNotExist sentinel, so every miss is normalized here.
                // client-go 只识别自身 ErrNotExist 哨兵，故此处统一规范化。
                Err(_) => Err(DriverError::ClientNotExist),
            },
        }
    }

    /// 批量：先从缓冲取；缺失键再逐个问中间缓存（批量选项转点查选项）。
    pub fn BatchGet(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        let mut values = self.tidb_buffer.batch_get_bytes(keys, options)?;
        let Some(middle_cache) = &self.tidb_middle_cache else {
            return Ok(values);
        };

        // 将 BatchGetOption 转为 GetOption，供点查接口使用。
        let get_options = BatchGetToGetOptions(options);
        for key in keys {
            if values.contains_key(key) {
                continue;
            }
            match middle_cache.get(key, &get_options) {
                Ok(value) => {
                    values.insert(key.clone(), value);
                }
                Err(error) if error.is_not_found() => {}
                Err(error) => return Err(error),
            }
        }
        Ok(values)
    }

    /// 返回缓冲中当前条目数。
    pub fn Len(&self) -> usize {
        self.tidb_buffer.len()
    }
}

/// Three-layer batch getter: memory buffer, statement cache, then snapshot.
///
/// 三层批量获取：内存缓冲 → 语句缓存 → 快照（snapshot）。
pub struct BufferBatchGetter {
    /// 缓冲 + 中间缓存层。
    buffer: tikvBatchBufferGetter,
    /// 快照批量获取层。
    snapshot: tikvBatchGetter,
}

/// 组装三层 `BufferBatchGetter`。
pub fn NewBufferBatchGetter(
    buffer: Arc<dyn BatchBufferGetter>,
    middle_cache: Option<Arc<dyn Getter>>,
    snapshot: Arc<dyn BatchGetter>,
) -> BufferBatchGetter {
    BufferBatchGetter {
        buffer: tikvBatchBufferGetter::new(buffer, middle_cache),
        snapshot: tikvBatchGetter::new(snapshot),
    }
}

impl BufferBatchGetter {
    /// 先缓冲/中间缓存，未解析键再问快照；最后剔除空值墓碑。
    pub fn BatchGet(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        let mut values = self.buffer.BatchGet(keys, options)?;
        // 收集缓冲层仍未命中的键，向快照补齐。
        let unresolved = keys
            .iter()
            .filter(|key| !values.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        if !unresolved.is_empty() {
            for (key, value) in self.snapshot.BatchGet(&unresolved, options)? {
                values.entry(key).or_insert(value);
            }
        }

        // A present empty buffer value is a delete tombstone. It must suppress
        // snapshot lookup but not appear in the public batch result.
        // 空缓冲值是删除墓碑：已抑制快照查找，对外结果中也要剔除。
        values.retain(|_, value| !value.is_value_empty());
        Ok(values)
    }
}

impl BatchGetter for BufferBatchGetter {
    fn batch_get(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        self.BatchGet(keys, options)
    }
}
