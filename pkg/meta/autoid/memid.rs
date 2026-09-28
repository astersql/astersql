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

// 内存版 AutoID 分配器（对应 Go `memid`）。
//
// 用于临时表（temporary table）等无需持久化元数据水位的场景：ID 只存在于进程内
// `Mutex` 状态，不访问 `IdStore`。行为与本地缓存分配器类似，但不支持序列缓存
// 与序列 rebase。

use std::sync::{Arc, Mutex};

use crate::autoid::{
    Allocator, AllocatorType, Context, TableInfo, calc_needed_batch_size,
    valid_increment_and_offset,
};
use crate::errors::{AutoIdError, Result, autoinc_read_failed, invalid_increment_and_offset};

/// 根据临时表元信息创建内存分配器；无 row id / 无自增列时返回 `None`。
///
/// 若表已有 `auto_increment_id > 1`，会先 rebase 到该值减一，使下一次分配从该值开始。
pub fn new_allocator_from_temp_table_info(table: &TableInfo) -> Option<Arc<dyn Allocator>> {
    // 非聚簇主键且非 common handle 时需要隐式 _tidb_rowid。
    let has_row_id = !table.pk_is_handle && !table.is_common_handle;
    if !has_row_id && !table.has_auto_increment_column {
        return None;
    }
    let allocator = Arc::new(InMemoryAllocator::new(
        table.auto_increment_unsigned,
        AllocatorType::RowId,
    ));
    if table.auto_increment_id > 1
        && allocator
            .rebase(&Context::background(), table.auto_increment_id - 1, false)
            .is_err()
    {
        return None;
    }
    Some(allocator)
}

/// 纯内存 AutoID 分配器：本地 `base` 单调推进，不写存储。
pub struct InMemoryAllocator {
    state: Mutex<InMemoryState>,
    is_unsigned: bool,
    allocator_type: AllocatorType,
}

/// 当前已分配水位（不含 end 缓存区间，因内存分配器按需直接推进 base）。
#[derive(Clone, Copy, Debug, Default)]
struct InMemoryState {
    base: i64,
}

impl InMemoryAllocator {
    /// 创建内存分配器；`is_unsigned` 决定有符号/无符号算术路径。
    pub fn new(is_unsigned: bool, allocator_type: AllocatorType) -> Self {
        Self {
            state: Mutex::new(InMemoryState::default()),
            is_unsigned,
            allocator_type,
        }
    }

    /// 有符号路径：按 increment/offset 计算所需批大小并推进 base。
    fn alloc_signed(
        &self,
        state: &mut InMemoryState,
        n: u64,
        increment: i64,
        offset: i64,
    ) -> Result<(i64, i64)> {
        // 保证下一可用 ID 至少满足 offset 约束。
        if offset - 1 > state.base {
            state.base = offset - 1;
        }
        let needed =
            calc_needed_batch_size(state.base, n as i64, increment, offset, self.is_unsigned);
        if i64::MAX as i128 - state.base as i128 <= needed as i128 {
            return Err(autoinc_read_failed("signed auto ID exhausted"));
        }
        let minimum = state.base;
        state.base += needed;
        Ok((minimum, state.base))
    }

    /// 无符号路径：用 wrapping 算术处理接近 u64::MAX 的水位。
    fn alloc_unsigned(
        &self,
        state: &mut InMemoryState,
        n: u64,
        increment: i64,
        offset: i64,
    ) -> Result<(i64, i64)> {
        let offset_base = (offset as u64).wrapping_sub(1);
        if offset_base > state.base as u64 {
            state.base = offset_base as i64;
        }
        let needed = calc_needed_batch_size(state.base, n as i64, increment, offset, true);
        if u64::MAX - state.base as u64 <= needed as u64 {
            return Err(autoinc_read_failed("unsigned auto ID exhausted"));
        }
        let minimum = state.base;
        state.base = (state.base as u64).wrapping_add(needed as u64) as i64;
        Ok((minimum, state.base))
    }
}

impl Allocator for InMemoryAllocator {
    fn alloc(&self, _ctx: &Context, n: u64, increment: i64, offset: i64) -> Result<(i64, i64)> {
        if n == 0 {
            return Ok((0, 0));
        }
        if matches!(
            self.allocator_type,
            AllocatorType::AutoIncrement | AllocatorType::RowId
        ) && !valid_increment_and_offset(increment, offset)
        {
            return Err(invalid_increment_and_offset(increment, offset));
        }
        let mut state = self.state.lock().unwrap();
        if self.is_unsigned {
            self.alloc_unsigned(&mut state, n, increment, offset)
        } else {
            self.alloc_signed(&mut state, n, increment, offset)
        }
    }

    fn alloc_seq_cache(&self) -> Result<(i64, i64, i64)> {
        Err(AutoIdError::NotImplemented(
            "AllocSeqCache is not implemented for in-memory allocators".into(),
        ))
    }

    fn rebase(&self, _ctx: &Context, required_base: i64, _allocate_ids: bool) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        // 仅当 required_base 更大时上调本地水位。
        if self.is_unsigned {
            if required_base as u64 > state.base as u64 {
                state.base = required_base;
            }
        } else if required_base > state.base {
            state.base = required_base;
        }
        Ok(())
    }

    fn force_rebase(&self, required_base: i64) -> Result<()> {
        self.state.lock().unwrap().base = required_base;
        Ok(())
    }

    fn rebase_seq(&self, _new_base: i64) -> Result<(i64, bool)> {
        Err(AutoIdError::NotImplemented(
            "RebaseSeq is not implemented for in-memory allocators".into(),
        ))
    }

    fn transfer(&self, _database_id: i64, _table_id: i64) -> Result<()> {
        Ok(())
    }

    fn base(&self) -> i64 {
        self.state.lock().unwrap().base
    }

    fn end(&self) -> i64 {
        0
    }

    fn next_global_auto_id(&self) -> Result<i64> {
        let base = self.state.lock().unwrap().base;
        if self.is_unsigned {
            Ok((base as u64).wrapping_add(1) as i64)
        } else {
            Ok(base.wrapping_add(1))
        }
    }

    fn get_type(&self) -> AllocatorType {
        self.allocator_type
    }
}
