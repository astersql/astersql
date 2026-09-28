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

// 事务本地 UnionStore / memBuffer 驱动。
//
// `memBuffer` 是事务未提交写入的有序内存缓冲，支持 Staging（暂存/回滚）、
// KeyFlags（如 PresumeKeyNotExists、断言）以及流水线 DML 下的 Flush 语义。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use crate::{
    BatchBufferGetter, BatchGetOption, DriverError, GetOption, Getter, Key, KvIterator, ValueEntry,
    tikvScanner,
};

/// 键级标志位集合：悲观锁、唯一性假定、断言等提交期元数据。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyFlags(u16);

/// 假定键原先不存在（唯一约束插入优化相关）。
const PRESUME_NOT_EXISTS: u16 = 1 << 0;
/// 需要加锁。
const NEED_LOCKED: u16 = 1 << 1;
/// 断言键存在。
const ASSERT_EXISTS: u16 = 1 << 2;
/// 断言键不存在。
const ASSERT_NOT_EXISTS: u16 = 1 << 3;
/// Prewrite 阶段需要约束检查。
const NEED_CONSTRAINT_CHECK: u16 = 1 << 4;
/// 保留先前的 PresumeKeyNotExists 状态。
const PREVIOUS_PRESUME_NOT_EXISTS: u16 = 1 << 5;

impl KeyFlags {
    /// 是否设置 PresumeKeyNotExists。
    pub fn has_presume_key_not_exists(self) -> bool {
        self.0 & PRESUME_NOT_EXISTS != 0
    }

    /// 是否需要加锁。
    pub fn has_need_locked(self) -> bool {
        self.0 & NEED_LOCKED != 0
    }

    /// 是否仅为 AssertExist（存在断言）。
    pub fn has_assert_exist(self) -> bool {
        self.0 & ASSERT_EXISTS != 0 && self.0 & ASSERT_NOT_EXISTS == 0
    }

    /// 是否仅为 AssertNotExist。
    pub fn has_assert_not_exist(self) -> bool {
        self.0 & ASSERT_NOT_EXISTS != 0 && self.0 & ASSERT_EXISTS == 0
    }

    /// 是否 AssertUnknown（两断言位同时置位）。
    pub fn has_assert_unknown(self) -> bool {
        self.0 & (ASSERT_EXISTS | ASSERT_NOT_EXISTS) == ASSERT_EXISTS | ASSERT_NOT_EXISTS
    }

    /// Prewrite 是否需要约束检查。
    pub fn has_need_constraint_check_in_prewrite(self) -> bool {
        self.0 & NEED_CONSTRAINT_CHECK != 0
    }

    /// 原始标志位。
    pub fn bits(self) -> u16 {
        self.0
    }
}

/// TiDB 侧对 KeyFlags 的设置操作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlagsOp {
    SetPresumeKeyNotExists,
    SetNeedLocked,
    SetNeedConstraintCheckInPrewrite,
    SetPreviousPresumeKeyNotExists,
}

/// 断言操作：存在 / 不存在 / 未知 / 无。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssertionOp {
    AssertExist,
    AssertNotExist,
    AssertUnknown,
    AssertNone,
}

/// 映射到 TiKV client 的标志/断言操作枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TiKVFlagsOp {
    SetPresumeKeyNotExists,
    SetNeedLocked,
    SetNeedConstraintCheckInPrewrite,
    SetPreviousPresumeKNE,
    SetAssertExist,
    SetAssertNotExist,
    SetAssertUnknown,
    SetAssertNone,
}

/// 将 FlagsOp 列表按位或到现有标志。
fn apply_flag_ops(mut flags: KeyFlags, operations: &[FlagsOp]) -> KeyFlags {
    for operation in operations {
        flags.0 |= match operation {
            FlagsOp::SetPresumeKeyNotExists => PRESUME_NOT_EXISTS,
            FlagsOp::SetNeedLocked => NEED_LOCKED,
            FlagsOp::SetNeedConstraintCheckInPrewrite => NEED_CONSTRAINT_CHECK,
            FlagsOp::SetPreviousPresumeKeyNotExists => PREVIOUS_PRESUME_NOT_EXISTS,
        };
    }
    flags
}

/// 应用断言操作，互斥更新 AssertExist / AssertNotExist 位。
fn apply_assertion(mut flags: KeyFlags, assertion: AssertionOp) -> KeyFlags {
    match assertion {
        AssertionOp::AssertExist => {
            flags.0 |= ASSERT_EXISTS;
            flags.0 &= !ASSERT_NOT_EXISTS;
        }
        AssertionOp::AssertNotExist => {
            flags.0 |= ASSERT_NOT_EXISTS;
            flags.0 &= !ASSERT_EXISTS;
        }
        AssertionOp::AssertUnknown => flags.0 |= ASSERT_EXISTS | ASSERT_NOT_EXISTS,
        AssertionOp::AssertNone => flags.0 &= !(ASSERT_EXISTS | ASSERT_NOT_EXISTS),
    }
    flags
}

/// 缓冲中单键的值与标志。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct BufferValue {
    value: Vec<u8>,
    flags: KeyFlags,
}

/// Staging 快照：保存进入暂存前的条目副本与句柄。
#[derive(Clone)]
struct Stage {
    handle: i32,
    before: BTreeMap<Key, BufferValue>,
}

/// memBuffer 可变状态：条目、暂存栈、Flush 错误注入。
#[derive(Default)]
struct MemBufferState {
    entries: BTreeMap<Key, BufferValue>,
    stages: Vec<Stage>,
    flush_error: Option<DriverError>,
}

/// MemDB 检查点：用于语句失败时回滚缓冲内容。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemDBCheckpoint {
    entries: BTreeMap<Key, (Vec<u8>, KeyFlags)>,
}

/// Transaction-local ordered buffer with staging and pipelined-DML behavior.
/// 事务本地有序写缓冲，支持 Staging 与流水线 DML。
pub struct memBuffer {
    state: RwLock<MemBufferState>,
    is_pipelined_dml: bool,
}

/// 可选地从条目映射构造 memBuffer；`None` 表示无缓冲。
pub fn newMemBuffer(
    entries: Option<BTreeMap<Key, ValueEntry>>,
    is_pipelined_dml: bool,
) -> Option<Arc<memBuffer>> {
    entries.map(|entries| {
        Arc::new(memBuffer::from_entries(
            entries.into_iter().map(|(key, value)| (key, value.value)),
            is_pipelined_dml,
        ))
    })
}

impl memBuffer {
    /// 空缓冲。
    pub fn empty(is_pipelined_dml: bool) -> Self {
        Self::from_entries(std::iter::empty::<(Key, Vec<u8>)>(), is_pipelined_dml)
    }

    /// 由键值迭代器构造，标志默认为空。
    pub fn from_entries(
        entries: impl IntoIterator<Item = (Key, Vec<u8>)>,
        is_pipelined_dml: bool,
    ) -> Self {
        Self {
            state: RwLock::new(MemBufferState {
                entries: entries
                    .into_iter()
                    .map(|(key, value)| {
                        (
                            key,
                            BufferValue {
                                value,
                                flags: KeyFlags::default(),
                            },
                        )
                    })
                    .collect(),
                stages: Vec::new(),
                flush_error: None,
            }),
            is_pipelined_dml,
        }
    }

    /// 键+值字节总大小。
    pub fn Size(&self) -> usize {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .iter()
            .map(|(key, value)| key.len() + value.value.len())
            .sum()
    }

    /// 条目个数。
    pub fn Len(&self) -> usize {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .len()
    }

    /// 写入空值表示删除。
    pub fn Delete(&self, key: Key) -> Result<(), DriverError> {
        self.set_internal(key, Vec::new(), &[])
    }

    /// 从缓冲物理移除键（不同于 Delete tombstone）。
    pub fn RemoveFromBuffer(&self, key: &[u8]) {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .remove(key);
    }

    /// 带标志的删除。
    pub fn DeleteWithFlags(&self, key: Key, operations: &[FlagsOp]) -> Result<(), DriverError> {
        self.set_internal(key, Vec::new(), operations)
    }

    /// 更新已有（或默认）条目的标志位。
    pub fn UpdateFlags(&self, key: Key, operations: &[FlagsOp]) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = state.entries.entry(key).or_default();
        entry.flags = apply_flag_ops(entry.flags, operations);
    }

    /// 更新断言相关标志。
    pub fn UpdateAssertionFlags(&self, key: Key, operation: AssertionOp) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = state.entries.entry(key).or_default();
        entry.flags = apply_assertion(entry.flags, operation);
    }

    /// 读取缓冲中的值。
    pub fn Get(&self, key: &[u8], options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        self.get(key, options)
    }

    /// 读取键标志；缺失返回 NotFound。
    pub fn GetFlags(&self, key: &[u8]) -> Result<KeyFlags, DriverError> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(key)
            .map(|entry| entry.flags)
            .ok_or(DriverError::NotFound)
    }

    /// 进入 Staging：保存当前条目快照并返回句柄。
    pub fn Staging(&self) -> i32 {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let handle =
            i32::try_from(state.stages.len() + 1).expect("staging depth exceeds i32 handle range");
        let before = state.entries.clone();
        state.stages.push(Stage { handle, before });
        handle
    }

    /// 按栈顶句柄回滚到 Staging 前状态；越过当前深度无操作，非栈顶句柄 panic。
    pub fn Cleanup(&self, handle: i32) {
        if handle == 0 {
            return;
        }
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let depth = i32::try_from(state.stages.len()).expect("staging depth exceeds i32 range");
        if handle > depth {
            return;
        }
        assert_eq!(handle, depth, "cannot cleanup staging buffer");
        if let Some(stage) = state.stages.pop() {
            state.entries = stage.before;
        }
    }

    /// 释放 Staging 句柄但不回滚数据（提交暂存层）。
    pub fn Release(&self, handle: i32) {
        if handle == 0 {
            return;
        }
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let depth = i32::try_from(state.stages.len()).expect("staging depth exceeds i32 range");
        assert_eq!(handle, depth, "cannot release staging buffer");
        state.stages.pop();
    }

    /// 检查指定 Staging 层相对进入前的脏变更，回调 visitor。
    pub fn InspectStage<F>(&self, handle: i32, mut visitor: F)
    where
        F: FnMut(Key, KeyFlags, Vec<u8>),
    {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(stage) = state.stages.iter().find(|stage| stage.handle == handle) else {
            return;
        };
        for (key, value) in &state.entries {
            if stage.before.get(key) != Some(value) {
                visitor(key.clone(), value.flags, value.value.clone());
            }
        }
        for key in stage.before.keys() {
            if !state.entries.contains_key(key) {
                visitor(key.clone(), KeyFlags::default(), Vec::new());
            }
        }
    }

    /// 写入非空值；空值禁止（删除请用 Delete）。
    pub fn Set(&self, key: Key, value: Vec<u8>) -> Result<(), DriverError> {
        if value.is_empty() {
            return Err(DriverError::Backend("cannot set an empty value".to_owned()));
        }
        self.set_internal(key, value, &[])
    }

    /// 带标志写入非空值。
    pub fn SetWithFlags(
        &self,
        key: Key,
        value: Vec<u8>,
        operations: &[FlagsOp],
    ) -> Result<(), DriverError> {
        if value.is_empty() {
            return Err(DriverError::Backend("cannot set an empty value".to_owned()));
        }
        self.set_internal(key, value, operations)
    }

    /// 内部写入：保留旧标志并叠加 operations。
    fn set_internal(
        &self,
        key: Key,
        value: Vec<u8>,
        operations: &[FlagsOp],
    ) -> Result<(), DriverError> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_flags = state
            .entries
            .get(&key)
            .map(|entry| entry.flags)
            .unwrap_or_default();
        state.entries.insert(
            key,
            BufferValue {
                value,
                flags: apply_flag_ops(previous_flags, operations),
            },
        );
        Ok(())
    }

    /// 正向扫描缓冲范围。
    pub fn Iter(
        &self,
        key: &[u8],
        upper_bound: Option<&[u8]>,
    ) -> Result<Box<dyn KvIterator>, DriverError> {
        Ok(Box::new(tikvScanner::new(self.rows(
            key,
            upper_bound,
            false,
        ))))
    }

    /// 反向扫描缓冲范围。
    pub fn IterReverse(
        &self,
        key: Option<&[u8]>,
        lower_bound: Option<&[u8]>,
    ) -> Result<Box<dyn KvIterator>, DriverError> {
        Ok(Box::new(tikvScanner::new(
            self.rows_reverse(key, lower_bound),
        )))
    }

    /// 供快照语义使用的迭代：流水线 DML 下返回空扫描器。
    pub fn SnapshotIter(&self, key: &[u8], upper_bound: Option<&[u8]>) -> Box<dyn KvIterator> {
        if self.is_pipelined_dml {
            Box::new(tikvScanner::new(Vec::new()))
        } else {
            Box::new(tikvScanner::new(self.rows(key, upper_bound, false)))
        }
    }

    /// 反向 SnapshotIter，流水线模式下同样为空。
    pub fn SnapshotIterReverse(
        &self,
        key: Option<&[u8]>,
        lower_bound: Option<&[u8]>,
    ) -> Box<dyn KvIterator> {
        if self.is_pipelined_dml {
            Box::new(tikvScanner::new(Vec::new()))
        } else {
            Box::new(tikvScanner::new(self.rows_reverse(key, lower_bound)))
        }
    }

    /// 返回只读 Getter 视图；流水线模式下为空 Getter。
    pub fn SnapshotGetter(&self) -> Arc<dyn Getter> {
        if self.is_pipelined_dml {
            Arc::new(tikvGetter::default())
        } else {
            let entries = self
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entries
                .iter()
                .map(|(key, value)| (key.clone(), ValueEntry::new(value.value.clone(), 0)))
                .collect();
            Arc::new(tikvGetter { entries })
        }
    }

    /// 仅读本地缓冲原始字节值。
    pub fn GetLocal(&self, key: &[u8]) -> Result<Vec<u8>, DriverError> {
        Ok(self.Get(key, &[])?.value)
    }

    /// 批量读取缓冲中存在的键。
    pub fn BatchGet(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        self.batch_get_bytes(keys, options)
    }

    /// 导出当前条目为检查点。
    pub fn checkpoint(&self) -> MemDBCheckpoint {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        MemDBCheckpoint {
            entries: state
                .entries
                .iter()
                .map(|(key, value)| (key.clone(), (value.value.clone(), value.flags)))
                .collect(),
        }
    }

    /// 用检查点整表替换当前条目。
    pub fn revert_to_checkpoint(&self, checkpoint: &MemDBCheckpoint) {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries = checkpoint
            .entries
            .iter()
            .map(|(key, (value, flags))| {
                (
                    key.clone(),
                    BufferValue {
                        value: value.clone(),
                        flags: *flags,
                    },
                )
            })
            .collect();
    }

    /// 测试用：注入下一次 Flush 错误。
    pub fn set_flush_error(&self, error: DriverError) {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flush_error = Some(error);
    }

    /// 流水线 Flush：返回条目数或注入错误。
    pub fn Flush(&self) -> Result<usize, DriverError> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = state.flush_error.take() {
            return Err(error);
        }
        Ok(state.entries.len())
    }

    /// 供提交路径导出全部缓冲条目。
    pub(crate) fn entries(&self) -> BTreeMap<Key, (Vec<u8>, KeyFlags)> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .iter()
            .map(|(key, value)| (key.clone(), (value.value.clone(), value.flags)))
            .collect()
    }

    /// 收集正向范围行。
    fn rows(&self, key: &[u8], upper_bound: Option<&[u8]>, reverse: bool) -> Vec<(Key, Vec<u8>)> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut rows = state
            .entries
            .iter()
            .filter(|(candidate, _)| {
                candidate.as_slice() >= key
                    && upper_bound.is_none_or(|upper| candidate.as_slice() < upper)
            })
            .map(|(key, value)| (key.clone(), value.value.clone()))
            .collect::<Vec<_>>();
        if reverse {
            rows.reverse();
        }
        rows
    }

    /// 收集反向范围行。
    fn rows_reverse(&self, key: Option<&[u8]>, lower_bound: Option<&[u8]>) -> Vec<(Key, Vec<u8>)> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .entries
            .iter()
            .rev()
            .filter(|(candidate, _)| {
                key.is_none_or(|upper| candidate.as_slice() < upper)
                    && lower_bound.is_none_or(|lower| candidate.as_slice() >= lower)
            })
            .map(|(key, value)| (key.clone(), value.value.clone()))
            .collect()
    }
}

impl Getter for memBuffer {
    fn get(&self, key: &[u8], _options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(key)
            .map(|entry| ValueEntry::new(entry.value.clone(), 0))
            .ok_or(DriverError::NotFound)
    }
}

impl BatchBufferGetter for memBuffer {
    fn len(&self) -> usize {
        self.Len()
    }

    fn batch_get_bytes(
        &self,
        keys: &[Key],
        _options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(keys
            .iter()
            .filter_map(|key| {
                state
                    .entries
                    .get(key)
                    .map(|entry| (key.clone(), ValueEntry::new(entry.value.clone(), 0)))
            })
            .collect())
    }
}

/// 简单内存 Getter，用于 SnapshotGetter 等场景。
#[derive(Default)]
pub struct tikvGetter {
    entries: BTreeMap<Key, ValueEntry>,
}

/// 从条目映射构造 Getter。
pub fn newKVGetter(entries: BTreeMap<Key, ValueEntry>) -> Arc<dyn Getter> {
    Arc::new(tikvGetter { entries })
}

impl Getter for tikvGetter {
    fn get(&self, key: &[u8], _options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        self.entries.get(key).cloned().ok_or(DriverError::NotFound)
    }
}

/// 对 `KvIterator` 的薄包装，保留 Go 命名风格类型。
pub struct tikvIterator {
    iterator: Box<dyn KvIterator>,
}

impl tikvIterator {
    pub fn new(iterator: Box<dyn KvIterator>) -> Self {
        Self { iterator }
    }
}

impl KvIterator for tikvIterator {
    fn next(&mut self) -> Result<(), DriverError> {
        self.iterator.next()
    }

    fn key(&self) -> &[u8] {
        self.iterator.key()
    }

    fn value(&self) -> &[u8] {
        self.iterator.value()
    }

    fn valid(&self) -> bool {
        self.iterator.valid()
    }

    fn close(&mut self) {
        self.iterator.close();
    }
}

/// 规范化/复制 TiDB 侧关心的标志位子集。
pub fn getTiDBKeyFlags(flags: KeyFlags) -> KeyFlags {
    let mut result = KeyFlags::default();
    if flags.has_presume_key_not_exists() {
        result = apply_flag_ops(result, &[FlagsOp::SetPresumeKeyNotExists]);
    }
    if flags.has_need_locked() {
        result = apply_flag_ops(result, &[FlagsOp::SetNeedLocked]);
    }
    if flags.has_assert_exist() {
        result = apply_assertion(result, AssertionOp::AssertExist);
    } else if flags.has_assert_not_exist() {
        result = apply_assertion(result, AssertionOp::AssertNotExist);
    } else if flags.has_assert_unknown() {
        result = apply_assertion(result, AssertionOp::AssertUnknown);
    }
    if flags.has_need_constraint_check_in_prewrite() {
        result = apply_flag_ops(result, &[FlagsOp::SetNeedConstraintCheckInPrewrite]);
    }
    result
}

/// FlagsOp → TiKVFlagsOp。
pub fn getTiKVFlagsOp(operation: FlagsOp) -> TiKVFlagsOp {
    match operation {
        FlagsOp::SetPresumeKeyNotExists => TiKVFlagsOp::SetPresumeKeyNotExists,
        FlagsOp::SetNeedLocked => TiKVFlagsOp::SetNeedLocked,
        FlagsOp::SetNeedConstraintCheckInPrewrite => TiKVFlagsOp::SetNeedConstraintCheckInPrewrite,
        FlagsOp::SetPreviousPresumeKeyNotExists => TiKVFlagsOp::SetPreviousPresumeKNE,
    }
}

/// 批量映射 FlagsOp。
pub fn getTiKVFlagsOps(operations: &[FlagsOp]) -> Vec<TiKVFlagsOp> {
    operations.iter().copied().map(getTiKVFlagsOp).collect()
}

/// AssertionOp → 对应的 TiKV 断言 FlagsOp。
pub fn getTiKVAssertionOp(operation: AssertionOp) -> TiKVFlagsOp {
    match operation {
        AssertionOp::AssertExist => TiKVFlagsOp::SetAssertExist,
        AssertionOp::AssertNotExist => TiKVFlagsOp::SetAssertNotExist,
        AssertionOp::AssertUnknown => TiKVFlagsOp::SetAssertUnknown,
        AssertionOp::AssertNone => TiKVFlagsOp::SetAssertNone,
    }
}
