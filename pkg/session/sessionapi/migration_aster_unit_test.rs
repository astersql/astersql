// Copyright 2026 AsterSQL.

// sessionapi 迁移基线单元测试。
//
// 校验身份未找到错误文案稳定，以及 `ExecuteInternal` 参数的类型擦除（Any）行为。

use crate::{Any, ErrIdentityNotFound};

/// 断言 `ErrIdentityNotFound` 的 Display 文案与 Go 侧一致且稳定。
#[test]
fn identity_not_found_error_is_stable() {
    assert_eq!(ErrIdentityNotFound.to_string(), "identity not found");
}

/// 断言内部执行参数经 `Any` 装箱后仍可按原类型 downcast。
#[test]
fn execute_internal_arguments_preserve_type_erasure() {
    let argument: Any = Box::new(42_u64);
    assert_eq!(argument.downcast_ref::<u64>(), Some(&42));
}
