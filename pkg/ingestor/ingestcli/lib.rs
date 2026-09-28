// Copyright 2026 AsterSQL.

// ingestcli crate 根：对外暴露写入/导入客户端与错误分类。
//
// 对应 Go `pkg/ingestor/ingestcli`。本包通过 HTTP 与 TiKV worker 交互：
// 先流式写入 KV 生成 SST（Sorted String Table，有序键值文件），再按 Region
// （TiKV 的键范围分片）执行 ingest（跳过常规写路径直接导入存储）。
//
// 子模块：`client`（HTTP 实现）、`ingest_err`（errorpb 分类）、`interface`（公共类型与 trait）。

#![allow(non_snake_case)]

/// HTTP 传输与 Client/WriteClient 实现。
mod client;
/// TiKV errorpb.Error 解码与 IngestAPIError 分类。
mod ingest_err;
/// 公共请求/响应类型与 Client、WriteClient、SplitClient 等接口。
mod interface;

pub use client::*;
pub use ingest_err::*;
pub use interface::*;

#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;
#[cfg(test)]
#[path = "ingest_err_test.rs"]
mod ingest_err_test;
#[cfg(test)]
#[path = "interface_test.rs"]
mod interface_test;
