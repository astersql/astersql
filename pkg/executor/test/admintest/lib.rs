// Copyright 2026 AsterSQL.

// ADMIN CHECK / RECOVER / CLEANUP INDEX 测试用内存模型。
//
// 用行表 + 二级索引模拟表数据与索引一致性：可注入缺失/悬空索引项，
// 再通过 [`AdminTable::check`]、[`AdminTable::recover_index`]、
// [`AdminTable::cleanup_index`] 验证修复语义（含多值索引与全局索引）。

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引不一致类型：缺索引项或悬空索引项。
pub enum Inconsistency {
    /// 行存在但索引缺少对应 (partition, handle, value)。
    MissingIndex {
        partition: u64,
        handle: i64,
        value: i64,
    },
    /// 索引存在但行侧无对应值。
    DanglingIndex {
        partition: u64,
        handle: i64,
        value: i64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 模拟会话变量：lookup 批大小、Chunk 上限、并发度（供 fast check 传播）。
pub struct AdminSessionVars {
    pub index_lookup_size: usize,
    pub max_chunk_size: usize,
    pub concurrency: usize,
}

impl Default for AdminSessionVars {
    fn default() -> Self {
        Self {
            index_lookup_size: 20_000,
            max_chunk_size: 1_024,
            concurrency: 4,
        }
    }
}

#[derive(Clone, Debug, Default)]
/// 内存表：按 (partition, handle) 存行值，并维护 value → 句柄集合的索引。
pub struct AdminTable {
    /// 多值索引：一行可对应多个索引值（如 JSON 数组）。
    multi_valued: bool,
    /// 全局索引：跨分区共享同一索引空间。
    global: bool,
    rows: BTreeMap<(u64, i64), Vec<i64>>,
    index: BTreeMap<i64, BTreeSet<(u64, i64)>>,
}

impl AdminTable {
    /// 构造空表；`multi_valued` / `global` 控制索引语义。
    pub fn new(multi_valued: bool, global: bool) -> Self {
        Self {
            multi_valued,
            global,
            ..Self::default()
        }
    }

    /// 插入或覆盖一行，并同步维护索引。
    pub fn insert(&mut self, partition: u64, handle: i64, values: Vec<i64>) {
        let key = (partition, handle);
        // 覆盖旧行时先摘掉旧索引项。
        if let Some(old) = self.rows.insert(key, values.clone()) {
            for value in self.index_values(&old) {
                self.remove_entry(partition, handle, value);
            }
        }
        for value in self.index_values(&values) {
            self.index.entry(value).or_default().insert(key);
        }
    }

    /// 删除一行及其全部索引项。
    pub fn delete_row(&mut self, partition: u64, handle: i64) {
        if let Some(values) = self.rows.remove(&(partition, handle)) {
            for value in self.index_values(&values) {
                self.remove_entry(partition, handle, value);
            }
        }
    }

    /// 人为破坏：只删索引项（模拟缺失索引）。
    pub fn corrupt_remove(&mut self, partition: u64, handle: i64, value: i64) {
        self.remove_entry(partition, handle, value);
    }

    /// 人为破坏：只插索引项（模拟悬空索引）。
    pub fn corrupt_insert(&mut self, partition: u64, handle: i64, value: i64) {
        self.index
            .entry(value)
            .or_default()
            .insert((partition, handle));
    }

    /// 双向校验行↔索引一致性，发现首个不一致即返回。
    pub fn check(&self) -> Result<(), Inconsistency> {
        // 行 → 索引：缺项报 MissingIndex。
        for (&(partition, handle), values) in &self.rows {
            for value in self.index_values(values) {
                if !self
                    .index
                    .get(&value)
                    .is_some_and(|handles| handles.contains(&(partition, handle)))
                {
                    return Err(Inconsistency::MissingIndex {
                        partition,
                        handle,
                        value,
                    });
                }
            }
        }
        // 索引 → 行：悬空报 DanglingIndex。
        for (&value, handles) in &self.index {
            for &(partition, handle) in handles {
                let valid = self.rows.get(&(partition, handle)).is_some_and(|values| {
                    self.index_values(values)
                        .into_iter()
                        .any(|item| item == value)
                });
                if !valid {
                    return Err(Inconsistency::DanglingIndex {
                        partition,
                        handle,
                        value,
                    });
                }
            }
        }
        Ok(())
    }

    /// 按行重建缺失索引项；返回 (新插入索引数, 扫描行数)。
    pub fn recover_index(&mut self) -> (usize, usize) {
        let expected: Vec<_> = self
            .rows
            .iter()
            .flat_map(|(&(partition, handle), values)| {
                self.index_values(values)
                    .into_iter()
                    .map(move |value| (value, partition, handle))
            })
            .collect();
        let mut recovered = 0;
        for (value, partition, handle) in expected {
            if self
                .index
                .entry(value)
                .or_default()
                .insert((partition, handle))
            {
                recovered += 1;
            }
        }
        (recovered, self.rows.len())
    }

    /// 删除悬空/错误索引项；返回删除条数。
    pub fn cleanup_index(&mut self) -> usize {
        let rows = &self.rows;
        let multi_valued = self.multi_valued;
        let mut removed = 0;
        self.index.retain(|value, handles| {
            handles.retain(|&(partition, handle)| {
                let valid = rows.get(&(partition, handle)).is_some_and(|values| {
                    let mut expected = if multi_valued {
                        values.clone()
                    } else {
                        values.first().copied().into_iter().collect()
                    };
                    expected.sort_unstable();
                    expected.dedup();
                    expected.binary_search(value).is_ok()
                });
                removed += usize::from(!valid);
                valid
            });
            !handles.is_empty()
        });
        removed
    }

    /// 索引中出现过的去重行数（按 partition+handle）。
    pub fn indexed_row_count(&self) -> usize {
        self.index
            .values()
            .flat_map(|handles| handles.iter())
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// 快速检查：先把会话变量记入 propagated，再执行 [`check`]。
    pub fn fast_check(
        &self,
        vars: AdminSessionVars,
        propagated: &mut Vec<AdminSessionVars>,
    ) -> Result<(), Inconsistency> {
        propagated.push(vars);
        self.check()
    }

    /// 是否为全局索引表。
    pub fn is_global(&self) -> bool {
        self.global
    }

    /// 从行值推导索引键：多值取全部，单值取首元素；排序去重。
    fn index_values(&self, values: &[i64]) -> Vec<i64> {
        let mut result = if self.multi_valued {
            values.to_vec()
        } else {
            values.first().copied().into_iter().collect()
        };
        result.sort_unstable();
        result.dedup();
        result
    }

    /// 从索引中移除单个 (partition, handle) 条目，空集合则删键。
    fn remove_entry(&mut self, partition: u64, handle: i64, value: i64) {
        if let Some(handles) = self.index.get_mut(&value) {
            handles.remove(&(partition, handle));
            if handles.is_empty() {
                self.index.remove(&value);
            }
        }
    }
}

#[cfg(test)]
mod admin_test;
#[cfg(test)]
mod main_test;
