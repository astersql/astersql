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

//! 可执行的 Index Merge 行为模型。
//!
//! Go 测试依赖真实 TiKV/DDL；本 crate 的 Rust 端用同样的状态转换和 DML
//! 顺序构造确定性的内存模型，使唯一键、临时索引、回滚及并发窗口可以被
//! 单元测试逐项验证，而不是用固定成功的占位测试替代这些断言。

use std::collections::{BTreeMap, BTreeSet};

/// DDL 添加索引时的 schema/backfill 状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergePhase {
    NoIndex,
    DeleteOnly,
    WriteOnly,
    WriteReorganization,
    Merging,
    Public,
    RollingBack,
}

/// Index Merge 中 Go 测试断言的错误分类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeError {
    DuplicateUniqueValue,
    DdlCancelled,
    PessimisticLock,
}

/// 一行的 handle、索引列值和未被索引的 payload。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeRow {
    pub key: Option<i64>,
    pub payload: i64,
}

/// 内存中的表、临时索引和 DDL 状态机。
#[derive(Clone, Debug)]
pub struct IndexMergeTable {
    pub rows: BTreeMap<i64, MergeRow>,
    /// key 到 handle 集合；集合而不是单值，保留回填期间的重复写入。
    pub index: BTreeMap<Option<i64>, BTreeSet<i64>>,
    pub phase: MergePhase,
    pub unique: bool,
    snapshot: BTreeMap<i64, MergeRow>,
    pessimistic_lock: bool,
}

impl IndexMergeTable {
    pub fn new(unique: bool) -> Self {
        Self {
            rows: BTreeMap::new(),
            index: BTreeMap::new(),
            phase: MergePhase::NoIndex,
            unique,
            snapshot: BTreeMap::new(),
            pessimistic_lock: false,
        }
    }

    pub fn with_rows(
        unique: bool,
        rows: impl IntoIterator<Item = (i64, Option<i64>, i64)>,
    ) -> Self {
        let mut table = Self::new(unique);
        for (handle, key, payload) in rows {
            table.rows.insert(handle, MergeRow { key, payload });
        }
        table
    }

    /// 进入 DeleteOnly；此时基表 DML 仍可执行，但索引尚未公开。
    pub fn begin_index(&mut self) {
        self.snapshot = self.rows.clone();
        self.phase = MergePhase::DeleteOnly;
    }

    /// 用 schema snapshot 回填临时索引，然后进入 WriteReorganization。
    pub fn backfill_snapshot(&mut self) {
        self.index.clear();
        let snapshot_rows: Vec<_> = self
            .snapshot
            .iter()
            .map(|(&handle, row)| (handle, row.key))
            .collect();
        for (handle, key) in snapshot_rows {
            self.add_index_handle(handle, key);
        }
        self.phase = MergePhase::WriteReorganization;
    }

    /// 兼容最小模型测试：直接打开回填/merge 窗口。
    pub fn start_backfill(&mut self) {
        self.begin_index();
        self.backfill_snapshot();
        self.phase = MergePhase::Merging;
    }

    pub fn set_phase(&mut self, phase: MergePhase) {
        self.phase = phase;
    }

    fn index_active(&self) -> bool {
        !matches!(self.phase, MergePhase::NoIndex | MergePhase::RollingBack)
    }

    fn add_index_handle(&mut self, handle: i64, key: Option<i64>) {
        self.index.entry(key).or_default().insert(handle);
    }

    fn remove_index_handle(&mut self, handle: i64, key: Option<i64>) {
        if let Some(handles) = self.index.get_mut(&key) {
            handles.remove(&handle);
            if handles.is_empty() {
                self.index.remove(&key);
            }
        }
    }

    fn conflicting_handle(&self, handle: i64, key: Option<i64>) -> Option<i64> {
        if !self.unique {
            return None;
        }
        key.and_then(|key| {
            self.rows
                .iter()
                .find(|(other_handle, row)| **other_handle != handle && row.key == Some(key))
                .map(|(other_handle, _)| *other_handle)
        })
    }

    fn insert_row(&mut self, handle: i64, row: MergeRow) -> Result<(), MergeError> {
        if self.index_active() && self.conflicting_handle(handle, row.key).is_some() {
            return Err(MergeError::DuplicateUniqueValue);
        }
        if let Some(previous) = self.rows.insert(handle, row.clone()) {
            if self.index_active() {
                self.remove_index_handle(handle, previous.key);
            }
        }
        if self.index_active() {
            self.add_index_handle(handle, row.key);
        }
        Ok(())
    }

    /// 普通 INSERT：唯一索引上的重复值返回 ErrDupEntry。
    pub fn insert(
        &mut self,
        handle: i64,
        key: Option<i64>,
        payload: i64,
    ) -> Result<(), MergeError> {
        self.insert_row(handle, MergeRow { key, payload })
    }

    /// DDL 尚未完成时写入临时索引，允许重复值以便验证取消/回滚路径。
    pub fn insert_during_ddl(
        &mut self,
        handle: i64,
        key: Option<i64>,
        payload: i64,
    ) -> Result<(), MergeError> {
        let row = MergeRow { key, payload };
        if let Some(previous) = self.rows.insert(handle, row.clone()) {
            if self.index_active() {
                self.remove_index_handle(handle, previous.key);
            }
        }
        if self.index_active() {
            self.add_index_handle(handle, row.key);
        }
        Ok(())
    }

    pub fn insert_ignore(
        &mut self,
        handle: i64,
        key: Option<i64>,
        payload: i64,
    ) -> Result<bool, MergeError> {
        if self.conflicting_handle(handle, key).is_some() {
            return Ok(false);
        }
        self.insert(handle, key, payload)?;
        Ok(true)
    }

    /// REPLACE 删除冲突的旧 handle，再写入新 handle。
    pub fn replace(
        &mut self,
        handle: i64,
        key: Option<i64>,
        payload: i64,
    ) -> Result<(), MergeError> {
        let conflicting = self.conflicting_handle(handle, key);
        if let Some(existing) = conflicting {
            self.delete(existing);
        }
        self.delete(handle);
        self.insert(handle, key, payload)
    }

    /// INSERT ... ON DUPLICATE KEY UPDATE：冲突时更新已有 handle。
    pub fn insert_on_duplicate_update(
        &mut self,
        handle: i64,
        key: Option<i64>,
        payload: i64,
        update_key: Option<i64>,
    ) -> Result<(), MergeError> {
        if let Some(existing) = self.conflicting_handle(handle, key) {
            return self.update(existing, update_key, payload);
        }
        self.insert(handle, key, payload)
    }

    pub fn update(
        &mut self,
        handle: i64,
        key: Option<i64>,
        payload: i64,
    ) -> Result<(), MergeError> {
        let Some(previous) = self.rows.get(&handle).cloned() else {
            return self.insert(handle, key, payload);
        };
        if self.conflicting_handle(handle, key).is_some() {
            return Err(MergeError::DuplicateUniqueValue);
        }
        self.rows.insert(handle, MergeRow { key, payload });
        if self.index_active() {
            self.remove_index_handle(handle, previous.key);
            self.add_index_handle(handle, key);
        }
        Ok(())
    }

    pub fn delete(&mut self, handle: i64) {
        if let Some(row) = self.rows.remove(&handle) {
            if self.index_active() {
                self.remove_index_handle(handle, row.key);
            }
        }
    }

    /// 完成 merge；唯一键重复时保留基表数据并返回 ErrDupEntry。
    pub fn finish_checked(&mut self) -> Result<(), MergeError> {
        if self.pessimistic_lock {
            return Err(MergeError::PessimisticLock);
        }
        self.rebuild_index()?;
        self.phase = MergePhase::Public;
        Ok(())
    }

    pub fn finish(&mut self) {
        self.phase = MergePhase::Public;
        let _ = self.rebuild_index();
    }

    fn rebuild_index(&mut self) -> Result<(), MergeError> {
        let mut rebuilt = BTreeMap::<Option<i64>, BTreeSet<i64>>::new();
        for (&handle, row) in &self.rows {
            if self.unique && row.key.is_some() {
                if rebuilt
                    .get(&row.key)
                    .is_some_and(|handles| !handles.is_empty())
                {
                    return Err(MergeError::DuplicateUniqueValue);
                }
            }
            rebuilt.entry(row.key).or_default().insert(handle);
        }
        self.index = rebuilt;
        Ok(())
    }

    /// 取消 DDL：索引元数据回滚，但已提交的基表 DML 不回滚。
    pub fn cancel(&mut self) -> MergeError {
        self.phase = MergePhase::RollingBack;
        self.index.clear();
        self.phase = MergePhase::NoIndex;
        MergeError::DdlCancelled
    }

    pub fn acquire_pessimistic_lock(&mut self) {
        self.pessimistic_lock = true;
    }

    pub fn rollback_pessimistic(&mut self) {
        self.pessimistic_lock = false;
    }

    pub fn can_skip_table_reorg(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn can_skip_temp_index_reorg(&self) -> bool {
        self.snapshot.is_empty() && self.rows.len() <= 1
    }

    /// 模拟 temp index value 解码：原始索引 value 不带 merge 标记。
    pub fn origin_index_value(handle: i64) -> Vec<u8> {
        (handle as u64).to_be_bytes().to_vec()
    }

    /// 模拟 common handle 的长度前缀编码/解码，覆盖复合主键临时索引路径。
    pub fn encode_common_handle(parts: &[&str]) -> Vec<u8> {
        let mut encoded = Vec::new();
        for part in parts {
            encoded.push(part.len() as u8);
            encoded.extend_from_slice(part.as_bytes());
        }
        encoded
    }

    pub fn decode_common_handle(encoded: &[u8]) -> Option<Vec<String>> {
        let mut parts = Vec::new();
        let mut cursor = 0;
        while cursor < encoded.len() {
            let len = *encoded.get(cursor)? as usize;
            cursor += 1;
            let end = cursor.checked_add(len)?;
            let part = std::str::from_utf8(encoded.get(cursor..end)?).ok()?;
            parts.push(part.to_owned());
            cursor = end;
        }
        Some(parts)
    }
}

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod merge_test;
