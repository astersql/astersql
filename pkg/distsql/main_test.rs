// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// DistSQL 包测试入口（main_test）相关断言。
//
// 校验 crate 错误类型的标准接口。

use super::*;

/// DistSqlError 应可作为标准 Error 使用，且 to_string 返回原始消息。
#[test]
fn error_is_a_standard_error_with_stable_message() {
    let error: Box<dyn std::error::Error> = Box::new(DistSqlError("boom".into()));
    assert_eq!(error.to_string(), "boom");
}
