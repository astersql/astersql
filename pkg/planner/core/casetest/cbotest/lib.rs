// Copyright 2026 AsterSQL.

// `cbotest` casetest crate 入口。
//
// CBO（Cost-Based Optimizer，基于代价的优化器）相关用例：验证送入优化器之前的
// DDL/DML/统计信息输入是否被真实构造，以及 TestMain 初始化语义。

#![allow(dead_code)]

/// CBO 建表、灌数、索引与会话变量等前置输入面测试。
#[cfg(test)]
mod cbo_test;
#[cfg(test)]
mod main_test;
