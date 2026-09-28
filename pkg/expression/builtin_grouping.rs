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

// GROUPING metadata validation and scalar/vector evaluation.
//
// SQL `GROUPING` 函数：判断 GROUP BY 扩展（ROLLUP/CUBE/GROUPING SETS）中
// 某列是否因聚合层次被置为 NULL。本模块校验元数据并按 BitAnd / NumericCmp /
// NumericSet 三种模式计算标量或向量结果。

use std::collections::HashSet;

use thiserror::Error;

/// GROUPING 求值模式，由优化器重写时写入元数据。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GroupingMode {
    #[default]
    /// 尚未初始化的非法模式。
    Invalid,
    /// 按位与：`grouping_id & mark == 0` 时该维记 1。
    BitAnd,
    /// 数值比较：`grouping_id <= mark` 时该维记 1。
    NumericCmp,
    /// 集合判定：`grouping_id` 不在 mark 集合中时记 1。
    NumericSet,
}

/// 对外导出的 GROUPING 元数据快照（marks 已排序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupingMetadata {
    pub mode: GroupingMode,
    /// 每个 GROUPING 参数对应一组 mark 值。
    pub grouping_marks: Vec<Vec<u64>>,
}

/// GROUPING 元数据未就绪或模式非法时的错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GroupingError {
    #[error("Meta data hasn't been initialized")]
    MetadataNotInitialized,
    #[error("Mode of meta data in grouping function is invalid")]
    InvalidMode,
    #[error("Invalid number of groupingID for {mode:?}: {count}")]
    InvalidGroupingIdCount { mode: GroupingMode, count: usize },
}

/// Runtime state carried by the rewritten GROUPING expression.
///
/// 重写后的 GROUPING 表达式运行时状态：模式、marks 集合与初始化标志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupingSig {
    mode: GroupingMode,
    grouping_marks: Vec<HashSet<u64>>,
    is_meta_inited: bool,
}

impl Default for GroupingSig {
    fn default() -> Self {
        Self::new()
    }
}

impl GroupingSig {
    /// 创建未初始化的空签名。
    pub fn new() -> Self {
        Self {
            mode: GroupingMode::Invalid,
            grouping_marks: Vec::new(),
            is_meta_inited: false,
        }
    }

    /// Sets metadata atomically. Invalid metadata leaves the signature unusable,
    /// matching Go's reset of `isMetaInited` on validation failure.
    ///
    /// 原子写入元数据；校验失败时清零 `is_meta_inited`，与 Go 行为一致。
    pub fn set_metadata(
        &mut self,
        mode: GroupingMode,
        grouping_marks: Vec<HashSet<u64>>,
    ) -> Result<(), GroupingError> {
        self.mode = mode;
        self.grouping_marks = grouping_marks;
        self.is_meta_inited = true;
        // 先标记已初始化再校验；失败则回滚标志，避免半初始化状态被求值。
        if let Err(error) = self.check_metadata() {
            self.is_meta_inited = false;
            return Err(error);
        }
        Ok(())
    }

    /// 当前求值模式。
    pub fn grouping_mode(&self) -> GroupingMode {
        self.mode
    }

    /// 各参数对应的 mark 集合切片。
    pub fn grouping_marks(&self) -> &[HashSet<u64>] {
        &self.grouping_marks
    }

    /// 元数据是否已通过校验并可用。
    pub fn is_metadata_initialized(&self) -> bool {
        self.is_meta_inited
    }

    /// 导出已排序的元数据快照；未初始化则报错。
    pub fn metadata(&self) -> Result<GroupingMetadata, GroupingError> {
        self.check_metadata()?;
        let grouping_marks = self
            .grouping_marks
            .iter()
            .map(|mark| {
                let mut values: Vec<_> = mark.iter().copied().collect();
                values.sort_unstable();
                values
            })
            .collect();
        Ok(GroupingMetadata {
            mode: self.mode,
            grouping_marks,
        })
    }

    /// 校验初始化标志与模式对 mark 个数的约束。
    fn check_metadata(&self) -> Result<(), GroupingError> {
        if !self.is_meta_inited {
            return Err(GroupingError::MetadataNotInitialized);
        }
        match self.mode {
            // BitAnd / NumericCmp 每个 mark 必须恰好一个 id。
            GroupingMode::BitAnd | GroupingMode::NumericCmp => {
                for mark in &self.grouping_marks {
                    if mark.len() != 1 {
                        return Err(GroupingError::InvalidGroupingIdCount {
                            mode: self.mode,
                            count: mark.len(),
                        });
                    }
                }
            }
            GroupingMode::NumericSet => {}
            GroupingMode::Invalid => return Err(GroupingError::InvalidMode),
        }
        Ok(())
    }

    /// BitAnd：逐维左移拼位；与 mask 无交集则该维为 1。
    fn grouping_impl_bit_and(&self, grouping_id: u64) -> i64 {
        let mut result = 0_u64;
        for mark in &self.grouping_marks {
            // Metadata validation guarantees exactly one item for this mode.
            // 元数据校验保证该模式下每个 mark 恰好一项。
            for value in mark {
                result <<= 1;
                if grouping_id & value == 0 {
                    result += 1;
                }
            }
        }
        result as i64
    }

    /// NumericCmp：`grouping_id <= mark` 时该维记 1。
    fn grouping_impl_numeric_cmp(&self, grouping_id: u64) -> i64 {
        let mut result = 0_u64;
        for mark in &self.grouping_marks {
            for value in mark {
                result <<= 1;
                if grouping_id <= *value {
                    result += 1;
                }
            }
        }
        result as i64
    }

    /// NumericSet：id 不在集合中则该维记 1。
    fn grouping_impl_numeric_set(&self, grouping_id: u64) -> i64 {
        let mut result = 0_u64;
        for mark in &self.grouping_marks {
            result <<= 1;
            if !mark.contains(&grouping_id) {
                result += 1;
            }
        }
        result as i64
    }

    /// 按当前模式分发到具体实现；Invalid 返回 0。
    fn grouping(&self, grouping_id: u64) -> i64 {
        match self.mode {
            GroupingMode::BitAnd => self.grouping_impl_bit_and(grouping_id),
            GroupingMode::NumericCmp => self.grouping_impl_numeric_cmp(grouping_id),
            GroupingMode::NumericSet => self.grouping_impl_numeric_set(grouping_id),
            GroupingMode::Invalid => 0,
        }
    }

    /// 标量求值：先校验元数据再计算 GROUPING 位图结果。
    pub fn eval(&self, grouping_id: u64) -> Result<i64, GroupingError> {
        self.check_metadata()?;
        Ok(self.grouping(grouping_id))
    }

    /// 向量求值：对一批 grouping_id 逐行调用同一内核。
    pub fn eval_many(&self, grouping_ids: &[u64]) -> Result<Vec<i64>, GroupingError> {
        self.check_metadata()?;
        Ok(grouping_ids
            .iter()
            .map(|grouping_id| self.grouping(*grouping_id))
            .collect())
    }
}
