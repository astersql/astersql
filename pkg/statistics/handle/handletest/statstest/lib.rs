// Copyright 2026 AsterSQL.

// `statstest` crate 入口：统计信息 handle 的集成/缓存相关测试包。
//
// 聚合统计缓存驱逐等行为用例（`stats_test`）。

#![allow(dead_code)]

#[cfg(test)]
/// 统计缓存容量超限时按最旧表驱逐的行为测试。
mod stats_test;
