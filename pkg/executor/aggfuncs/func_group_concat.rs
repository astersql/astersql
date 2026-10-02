// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// GROUP_CONCAT 聚合：按分隔符拼接组内字符串，支持 DISTINCT 去重与最大长度截断。
//
// 对应 SQL `GROUP_CONCAT([DISTINCT] expr SEPARATOR sep)`；`group_concat_max_len`
// 控制结果上限，超长时截断并置 truncated 标志。并行 merge 时，DISTINCT 模式
// 按去重集合并入，非 DISTINCT 则直接拼接对侧已拼好的缓冲。

use std::collections::HashMap;

/// GROUP_CONCAT 的 partial 状态：分隔符、长度上限、结果缓冲、可选去重集与截断标志。
#[derive(Clone, Debug, PartialEq)]
pub struct GroupConcat {
    /// 行与行之间插入的分隔符字节序列。
    separator: Vec<u8>,
    /// 结果最大字节长度（对齐系统变量 group_concat_max_len）。
    maximum_len: usize,
    /// 当前已拼接的结果缓冲。
    pub(crate) value: Vec<u8>,
    /// 启用 DISTINCT 时用于去重的集合；None 表示不去重。
    pub(crate) distinct: Option<HashMap<Vec<u8>, Vec<u8>>>,
    /// 是否因超过 maximum_len 发生过截断。
    truncated: bool,
    /// 是否已经接收过至少一个非 NULL 值。
    ///
    /// Go uses `buffer == nil` for this distinction, so an empty string is
    /// still a present (non-NULL) GROUP_CONCAT result.
    pub(crate) has_value: bool,
}

impl GroupConcat {
    /// 构造聚合器；`distinct == true` 时分配去重集合。
    pub fn new(separator: Vec<u8>, maximum_len: usize, distinct: bool) -> Self {
        Self {
            separator,
            maximum_len,
            value: Vec::new(),
            distinct: distinct.then(HashMap::new),
            truncated: false,
            has_value: false,
        }
    }
    /// 清空 partial 结果与去重集，便于在下一个分组复用。
    ///
    /// `truncated` 属于聚合函数的生命周期哨兵，和 Go 的
    /// `baseGroupConcat4String.truncated` 一样不会随 partial reset 清零。
    pub fn reset(&mut self) {
        // Go resets the partial buffer to nil, releasing its retained
        // capacity; clear the DISTINCT set the same way for memory parity.
        self.value = Vec::new();
        if let Some(s) = &mut self.distinct {
            *s = HashMap::new()
        }
        self.has_value = false
    }
    /// 追加若干行（跳过 NULL）；DISTINCT 时重复值忽略，超长则截断。
    pub fn update(&mut self, rows: impl IntoIterator<Item = Option<Vec<u8>>>) {
        for row in rows.into_iter().flatten() {
            let was_present = self.has_value;
            self.has_value = true;
            // DISTINCT：insert 返回 false 表示已见过，跳过该行。
            if self
                .distinct
                .as_mut()
                .is_some_and(|s| s.insert(row.clone(), row.clone()).is_some())
            {
                continue;
            }
            // 非首段前写入分隔符。
            if was_present {
                self.value.extend_from_slice(&self.separator)
            }
            self.value.extend_from_slice(&row);
            // 超过上限则截断并标记 truncated。
            if self.maximum_len > 0 && self.value.len() > self.maximum_len {
                self.value.truncate(self.maximum_len);
                self.truncated = true
            }
        }
    }
    /// Restore/evaluate a DISTINCT entry with its collation key retained.
    pub fn update_keyed(&mut self, key: Vec<u8>, value: Vec<u8>) {
        if self.distinct.as_ref().is_some_and(|s| s.contains_key(&key)) {
            return;
        }
        // Reuse ordinary concatenation, then replace the raw-value key with
        // the caller's evaluated collation key.
        let mut entries = self.distinct.take();
        self.update([Some(value.clone())]);
        if let Some(entries) = &mut entries {
            entries.insert(key, value);
        }
        self.distinct = entries;
    }
    /// 合并对侧 partial：DISTINCT 重放对侧集合；否则拼接对侧整段缓冲。
    pub fn merge(&mut self, source: &Self) {
        if self.distinct.is_some() {
            if let Some(entries) = &source.distinct {
                for (key, value) in entries {
                    self.update_keyed(key.clone(), value.clone());
                }
            }
        } else if source.has_value {
            self.update([Some(source.value.clone())])
        }
    }
    /// 无非 NULL 输入时返回 None；非 NULL 空字符串仍返回空结果。
    pub fn result(&self) -> Option<&[u8]> {
        self.has_value.then_some(&self.value)
    }
    /// 是否曾因长度上限截断。
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}
