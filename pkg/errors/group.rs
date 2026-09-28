// Copyright 2026 AsterSQL.

// 多原因错误组（ErrorGroup）与深度遍历。
//
// 与单链 `cause` 不同，组错误同时持有多个彼此独立的子错误；
// [`WalkDeep`] 先沿单原因链下钻，再递归访问各组子错误。

use std::error::Error as StdError;

use super::{SharedError, Unwrap};

/// An error containing multiple independent causes rather than one cause chain.
/// 持有多个独立原因的错误，而非单一 cause 链。
pub trait ErrorGroup: StdError + Send + Sync + 'static {
    /// 返回组内全部子错误（通常为克隆的 [`SharedError`] 列表）。
    fn Errors(&self) -> Vec<SharedError>;
}

/// Returns group children, or the error itself when it is not a group.
/// 若为组则返回其子错误；否则把该错误自身包成单元素列表。
pub fn Errors(error: &SharedError) -> Vec<SharedError> {
    error
        .error_group()
        .map(ErrorGroup::Errors)
        .unwrap_or_else(|| vec![error.clone()])
}

/// Traverses the single-cause chain first and each group's children afterward.
/// 深度优先访问：visitor 返回 `true` 时提前终止并向上传播。
pub fn WalkDeep<F>(error: Option<&SharedError>, mut visitor: F) -> bool
where
    F: FnMut(&SharedError) -> bool,
{
    /// 内部递归：先访问节点，再 Unwrap 原因链，最后遍历 ErrorGroup 子项。
    fn walk<F>(error: &SharedError, visitor: &mut F) -> bool
    where
        F: FnMut(&SharedError) -> bool,
    {
        if visitor(error) {
            return true;
        }

        // 单原因链优先于组内兄弟错误
        if let Some(cause) = Unwrap(Some(error)) {
            if walk(&cause, visitor) {
                return true;
            }
        }

        if let Some(group) = error.error_group() {
            for child in group.Errors() {
                if walk(&child, visitor) {
                    return true;
                }
            }
        }

        false
    }

    error.is_some_and(|error| walk(error, &mut visitor))
}
