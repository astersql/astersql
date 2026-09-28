// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载学习使用的指标与标识符类型。
//
// 定义大小写不敏感字符串 `CIStr`，以及按表汇总的表读代价指标
// `TableReadCostMetrics`（扫描时间、内存、频率与归一化代价）。

use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;

/// 大小写不敏感标识符：保留原始写法 `O`，并缓存小写形式 `L`。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CIStr {
    /// 原始大小写字符串。
    pub O: String,
    /// ASCII 小写形式，用于比较与查找。
    pub L: String,
}

impl CIStr {
    /// 由任意可转成 `String` 的值构造，自动填充小写副本。
    pub fn new(value: impl Into<String>) -> Self {
        let O = value.into();
        let L = O.to_lowercase();
        Self { O, L }
    }
}

impl<'de> Deserialize<'de> for CIStr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Representation {
            Object {
                #[serde(default)]
                O: String,
                #[serde(default)]
                L: String,
            },
            String(String),
            Null,
        }

        match Representation::deserialize(deserializer)? {
            Representation::Object { O, L } => Ok(Self { O, L }),
            Representation::String(value) => Ok(Self::new(value)),
            Representation::Null => Ok(Self::default()),
        }
    }
}

/// 单表的读代价指标：扫描耗时、内存用量、读频率与综合代价。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TableReadCostMetrics {
    /// 数据库名（大小写不敏感）。
    pub DbName: CIStr,
    /// 表名（大小写不敏感）。
    pub TableName: CIStr,
    /// 表扫描累计时间；序列化为纳秒整数。
    #[serde(with = "duration_nanos")]
    pub TableScanTime: Duration,
    /// 表读相关内存用量（字节量级整数）。
    pub TableMemUsage: i64,
    /// 读操作出现频率（由语句频率累加）。
    pub ReadFrequency: i64,
    /// 归一化后的表读代价（扫描占比 + 内存占比）。
    pub TableReadCost: f64,
}

/// `Duration` 与纳秒 `u64` 之间的 serde 适配。
mod duration_nanos {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(duration.as_nanos().min(u64::MAX as u128) as u64)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_nanos(u64::deserialize(deserializer)?))
    }
}
