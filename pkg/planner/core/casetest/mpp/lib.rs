// Copyright 2026 AsterSQL.

// `mpp` casetest crate 入口。
//
// 聚合 MPP（Massively Parallel Processing，大规模并行处理：将物理计划拆到
// TiFlash 等多节点执行）相关用例：TestMain 初始化语义，以及建表 / analyze /
// 会话变量等送入 MPP 优化规则前的真实输入构造测试。仅在 `cfg(test)` 下挂载子模块。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
/// MPP join / exchange / collation / decimal 等 fixture 与会话变量真实驱动测试。
#[cfg(test)]
mod mpp_test;
