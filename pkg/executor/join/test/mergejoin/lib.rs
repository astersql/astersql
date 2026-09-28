// Copyright 2026 AsterSQL.

// Merge Join（归并连接）相关集成测试的 crate 根模块。
//
// 归并连接要求两侧输入按连接键有序，然后像归并排序一样同步推进两侧游标完成匹配。
// 本目录通过 `merge_join_test` 对照 Go 版测试语义，验证串行与并发 shuffle 归并连接结果。

#![allow(dead_code)]

/// 归并连接（Merge Join）与 Shuffle Merge Join 的语义与边界用例测试。
#[cfg(test)]
mod merge_join_test;
