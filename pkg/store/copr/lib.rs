// Copyright 2026 AsterSQL.

// Coprocessor（copr）客户端库入口。
//
// 汇聚批处理 coprocessor、缓存、KeyRanges、MPP（大规模并行处理）、
// Region 缓存与 store 适配等子模块，并向外 re-export 常用类型。
// Region 是 TiKV 的键空间分片；coprocessor 在存储端就地执行推送算子。

#![allow(dead_code)]

/// 批处理 coprocessor 请求编排。
pub mod batch_coprocessor;
/// 批请求发送与 Region 路由相关类型。
pub mod batch_request_sender;
/// Coprocessor 任务构建与迭代执行核心。
pub mod coprocessor;
/// Coprocessor 响应结果缓存。
pub mod coprocessor_cache;
/// KeyRanges 切片与按键拆分辅助。
pub mod key_ranges;
/// MPP（大规模并行处理）相关逻辑。
pub mod mpp;
/// MPP 探测（probe）辅助。
pub mod mpp_probe;
/// 标准 TiKV DAG 的 PD 元数据与 gRPC transport。
pub mod network_backend;
/// 键区间诊断工具。
pub mod range_diagnostics;
/// Region 元数据缓存（定位键所属分片）。
pub mod region_cache;
/// Store 层适配与封装。
pub mod store;

pub use batch_coprocessor::*;
pub use batch_request_sender::{
    Backoffer, BatchError, BatchRequest, BatchResponse, BatchResult, CancellationToken,
    CommandType, CoprocessorRegionInfo, KeyRange, KeyRanges, NewRegionBatchRequestSender, Peer,
    RegionBatchRequestSender, RegionFailureHandler, RegionInfo, RegionMeta, RegionVerId,
    RequestContext, RpcClient, RpcContext, RpcResponse, RpcRuntimeStats, SendResult,
    Store as RegionStore, TableRegions,
};
pub use coprocessor::*;
pub use coprocessor_cache::*;
pub use key_ranges::*;
pub use mpp::*;
pub use mpp_probe::*;
pub use network_backend::*;
pub use range_diagnostics::*;
pub use region_cache::*;
pub use store::*;

#[cfg(test)]
#[path = "batch_coprocessor_test.rs"]
mod batch_coprocessor_test;
#[cfg(test)]
#[path = "coprocessor_cache_test.rs"]
mod coprocessor_cache_test;
#[cfg(test)]
#[path = "coprocessor_test.rs"]
mod coprocessor_test;
#[cfg(test)]
#[path = "key_ranges_test.rs"]
mod key_ranges_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "mpp_probe_test.rs"]
mod mpp_probe_test;
#[cfg(test)]
#[path = "mpp_test.rs"]
mod mpp_test;
#[cfg(test)]
#[path = "network_backend_test.rs"]
mod network_backend_test;
#[cfg(test)]
#[path = "region_cache_test.rs"]
mod region_cache_test;
