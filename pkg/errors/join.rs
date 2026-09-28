// Copyright 2026 AsterSQL.

// 多错误合并（Join）。
//
// 对应 Go `errors.Join`：过滤掉 `None` 后按输入顺序聚合为 [`ErrorGroup`]；
// 全部缺失时返回 `None`。展示时各子错误以换行分隔。

use std::error::Error as StdError;
use std::fmt;

use super::{ErrorGroup, SharedError};

/// 将多个独立错误合并后的载体；通过 [`ErrorGroup`] 暴露子错误列表。
#[derive(Debug)]
struct JoinError {
    errors: Vec<SharedError>,
}

impl fmt::Display for JoinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 多错误展示：逐行拼接，中间以换行分隔
        for (index, error) in self.errors.iter().enumerate() {
            if index != 0 {
                formatter.write_str("\n")?;
            }
            fmt::Display::fmt(error, formatter)?;
        }
        Ok(())
    }
}

impl StdError for JoinError {}

impl ErrorGroup for JoinError {
    fn Errors(&self) -> Vec<SharedError> {
        self.errors.clone()
    }
}

/// Joins present errors in order, returning `None` when all inputs are absent.
/// 按顺序合并存在的错误；输入全为 `None` 时返回 `None`。
pub fn Join(errors: &[Option<SharedError>]) -> Option<SharedError> {
    let errors: Vec<_> = errors.iter().flatten().cloned().collect();
    if errors.is_empty() {
        return None;
    }
    Some(SharedError::new_group(JoinError { errors }))
}
