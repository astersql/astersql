// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// JSON_OBJECTAGG 聚合：将组内 (key, value) 收集为 JSON 对象。
//
// 对应 SQL `JSON_OBJECTAGG(key, value)`；key 不可为 NULL（MySQL/TiDB 会报错）。
// 重复 key 后写覆盖先写；merge 时对侧条目同样按 key 覆盖写入。

use crate::aggfuncs::{
    AggError, DEF_BOOL_SIZE, DEF_DURATION_SIZE, DEF_FLOAT64_SIZE, DEF_INT64_SIZE,
    DEF_INTERFACE_SIZE, DEF_TIME_SIZE, DEF_UINT64_SIZE, SpillValue,
};
use std::collections::HashMap;

/// JSON_OBJECTAGG 的 partial 状态：字符串键到 SpillValue 的映射。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonObjectAgg {
    /// 对象成员表；重复 key 以 HashMap::insert 语义覆盖。
    entries: HashMap<String, SpillValue>,
}

impl JsonObjectAgg {
    /// 追加若干 (key, value)；key 为 None 时返回 AggError（禁止 NULL 成员名）。
    pub fn update(
        &mut self,
        v: impl IntoIterator<Item = (Option<String>, SpillValue)>,
    ) -> Result<i64, AggError> {
        let mut memory_delta = 0;
        for (k, v) in v {
            // JSON 规范与 MySQL 均不允许 NULL 作为 object member name。
            let k = k.ok_or_else(|| {
                AggError("JSON documents may not contain NULL member names".into())
            })?;
            let is_insert = !self.entries.contains_key(&k);
            if is_insert {
                memory_delta += k.len() as i64 + value_memory_delta(&v);
            }
            self.entries.insert(k, v);
        }
        Ok(memory_delta)
    }
    /// 合并对侧成员：同名 key 以对侧值覆盖。
    pub fn merge(&mut self, s: &Self) -> i64 {
        let mut memory_delta = 0;
        for (k, v) in &s.entries {
            // Go MergePartialResult 会为源 partial 的每个条目报告键值内存。
            memory_delta += k.len() as i64 + value_memory_delta(v);
            self.entries.insert(k.clone(), v.clone());
        }
        memory_delta
    }
    /// 清空全部成员。
    pub fn reset(&mut self) {
        // Go 重新初始化 map；替换而非 clear，释放旧表持有的容量。
        self.entries = HashMap::new();
    }
    /// 非空时返回成员表引用；空对象视为无结果（None）。
    pub fn result(&self) -> Option<&HashMap<String, SpillValue>> {
        (!self.entries.is_empty()).then_some(&self.entries)
    }
}

/// 与 Go `getValMemDelta` 相同的值内存口径（不含 map 桶扩容增量）。
fn value_memory_delta(value: &SpillValue) -> i64 {
    DEF_INTERFACE_SIZE
        + match value {
            SpillValue::Bool(_) => DEF_BOOL_SIZE,
            SpillValue::Int64(_) => DEF_INT64_SIZE,
            SpillValue::Uint64(_) => DEF_UINT64_SIZE,
            SpillValue::Float64(_) => DEF_FLOAT64_SIZE,
            SpillValue::String(value) => value.len() as i64,
            SpillValue::BinaryJson(value) => value.Value.len() as i64 + 1,
            SpillValue::Opaque(value) => value.Buf.len() as i64 + 1,
            SpillValue::Time(_) => DEF_TIME_SIZE,
            SpillValue::Duration(_) => DEF_DURATION_SIZE,
        }
}
