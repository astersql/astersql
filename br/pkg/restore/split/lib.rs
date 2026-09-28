// Copyright 2026 AsterSQL.

//! Region 分裂/散射辅助包入口：导出 client、splitter、region 等。
//! 对应 Go `br/pkg/restore/split`，restore 导入 SST 前按键切分 region。
//! 含 mock PD client 与 sum_sorted 工具；测试分文件挂载。
//! stubs 提供 PD/metapb 替身，避免单测依赖真集群。
//! 本文件只做模块装配与扁平再导出。
//! `region`：RegionInfo 与内部键判定。
//! `split`/`splitter`：批量分裂与等待散射。
//! `client`：对 PD 的 split/scatter RPC 封装。
//! `sum_sorted`：有序键区间体积累计，辅助选分裂点。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "region.rs"]
pub mod region;

#[path = "sum_sorted.rs"]
pub mod sum_sorted;

#[path = "client.rs"]
pub mod client;

#[path = "split.rs"]
pub mod split;

#[path = "splitter.rs"]
pub mod splitter;

#[path = "mock_pd_client.rs"]
pub mod mock_pd_client;

pub use client::*;
pub use mock_pd_client::*;
pub use region::*;
pub use split::*;
pub use splitter::*;
pub use sum_sorted::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "sum_sorted_test.rs"]
mod sum_sorted_test;

#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;

#[cfg(test)]
#[path = "mock_pd_client_test.rs"]
mod mock_pd_client_test;

#[cfg(test)]
#[path = "split_test.rs"]
mod split_test;

#[cfg(test)]
#[path = "region_test.rs"]
mod region_test;

#[cfg(test)]
#[path = "splitter_test.rs"]
mod splitter_test;
