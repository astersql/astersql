// Copyright 2026 AsterSQL.

// `pkg/executor/test/distsqltest` crate 根：挂接 DistSQL（分布式 SQL 下推）测试模块。
//
// DistSQL 把扫描/计算请求下发到 TiKV 等存储节点；本 crate 仅在
// `#[cfg(test)]` 下编译测试源，不导出生产 API。

#![allow(dead_code)]

#[cfg(test)]
mod distsql_test;
#[cfg(test)]
mod main_test;
