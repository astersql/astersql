// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// JSON_ARRAYAGG 聚合：按输入顺序将组内值收集为 JSON 数组。
//
// 对应 SQL `JSON_ARRAYAGG(expr)`；元素以 `SpillValue` 存储，便于 spill（溢写到磁盘）
// 序列化。merge 时按分片顺序追加，保持相对次序。

use crate::aggfuncs::{
    DEF_BOOL_SIZE, DEF_DURATION_SIZE, DEF_FLOAT64_SIZE, DEF_INT64_SIZE, DEF_INTERFACE_SIZE,
    DEF_TIME_SIZE, DEF_UINT64_SIZE, SpillValue,
};

/// JSON_ARRAYAGG 的 partial 状态：有序元素列表。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonArrayAgg {
    /// 已收集的数组元素（SpillValue 可跨类型表示可溢写值）。
    entries: Vec<SpillValue>,
}

impl JsonArrayAgg {
    /// 按输入顺序追加元素，并返回与 Go `getValMemDelta` 相同口径的内存增量。
    pub fn update(&mut self, values: impl IntoIterator<Item = SpillValue>) -> i64 {
        let mut memory_delta = 0;
        for value in values {
            memory_delta += value_memory_delta(&value);
            self.entries.push(value);
        }
        memory_delta
    }
    /// 将对侧 partial 的元素追加到本侧末尾。
    pub fn merge(&mut self, s: &Self) {
        self.entries.extend(s.entries.clone())
    }
    /// 清空已收集元素。
    pub fn reset(&mut self) {
        self.entries.clear()
    }
    /// 非空时返回元素切片；空数组在 SQL 语义下视为无结果（None）。
    pub fn result(&self) -> Option<&[SpillValue]> {
        (!self.entries.is_empty()).then_some(&self.entries)
    }
}

/// Go 的 JSON 聚合把每个元素保存为 interface，因此固定计入两个机器字；
/// 变长值按当前有效长度计量，BinaryJSON/Opaque 另含一个类型码字节。
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
