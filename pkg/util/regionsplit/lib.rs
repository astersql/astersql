// Copyright 2026 AsterSQL.

// `util/regionsplit` crate 入口：计算手动拆分 Region 用的切分键。
//
// 对应 Go `pkg/util/regionsplit`。Region 是 TiKV 中一段连续的键空间分片；
// 本 crate 根据表/索引上下界与拆分段数生成中间边界键，供 `SPLIT TABLE/INDEX`
// 等语句把过大的 Region 拆开。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// Region 切分键计算：表记录键、索引键编码与等分边界。
mod split_handle;

/// 再导出 split_handle 的公开 API。
pub use split_handle::*;

/// 使用真实表元信息与 Datum 的拆分句柄入口。
mod model_handle;
pub use model_handle::*;
