// Copyright 2026 AsterSQL.

// Index Lookup Join 相关集成测试的 crate 根模块。
//
// Index Lookup Join（索引查找连接）是一种以外表驱动、按连接键批量回表查找内表行的连接算法。
// 本目录下的测试模块验证索引查找连接与索引归并连接在典型键匹配、NULL 安全匹配
// 以及重复键分组等场景下的语义；此处仅做测试子模块声明，不包含算法实现。

#![allow(dead_code)]
/// 索引查找连接（Index Lookup Join）的规范化语义测试。
#[cfg(test)]
mod index_lookup_join_test;
/// 索引查找归并连接（Index Lookup Merge Join）的规范化语义测试。
#[cfg(test)]
mod index_lookup_merge_join_test;
