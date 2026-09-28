// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 样本方差聚合函数（`VAR_SAMP`）类型别名导出。
//
// 对应 Go 的 `func_varsamp.go`：计算逻辑复用 `func_varpop` 中的
// `VarianceState` / `DistinctVariance`，最终结果取样本方差
// （`中间方差 / (n-1)`），此处仅提供与 Go 命名对齐的类型别名。

/// 原始阶段 `VAR_SAMP DISTINCT` 的部分结果。
pub use crate::func_varpop::DistinctVariance as VarSampOriginal4DistinctFloat64;
/// 部分聚合阶段 `VAR_SAMP DISTINCT` 的部分结果。
pub use crate::func_varpop::DistinctVariance as VarSampPartial4DistinctFloat64;
/// 兼容早期 Rust 调用方使用的无阶段 DISTINCT 名称。
pub use crate::func_varpop::DistinctVariance as VarSampDistinctFloat64;
/// float64 路径上 `VAR_SAMP` 的部分结果；最终求值走 `sample_variance`。
pub use crate::func_varpop::VarianceState as VarSamp4Float64;
