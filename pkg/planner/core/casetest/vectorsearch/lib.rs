// Copyright 2026 AsterSQL.

// 向量检索（vector search / ANN）规划器用例包入口。
//
// 聚合 `main_test` 与 `vector_index_test`：覆盖向量距离度量、VECTOR INDEX DDL、
// TiFlash 副本形状及隔离读引擎（isolation read engines）等与近似最近邻
// （ANN，Approximate Nearest Neighbor）索引相关的规划前置条件。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 向量索引 / ANN 相关生产 API 直连用例。
#[cfg(test)]
#[path = "vector_index_test.rs"]
mod vector_index_test;
