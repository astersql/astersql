// Copyright 2026 AsterSQL.

// 向量化分组检查器（VecGroupChecker）crate 入口。
//
// 将已按 GROUP BY 键有序的 Chunk（列式批量行块）切分为连续等值分组，
// 供 HashAgg / StreamAgg 等聚合执行器按组消费。本文件负责依赖重导出，
// 真实表达式求值与切分逻辑在 `vec_group_checker` 模块。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as vecgroupchecker;

/// Chunk（列式行批）相关类型重导出。
pub mod chunk {
    pub use chunk_crate::*;
}

/// 编解码与按 collation（字符序）比较字符串的辅助重导出。
pub mod codec {
    pub use codec_crate::{ConvertByCollationStr, EncodeKey};
}

/// 共享错误类型重导出。
pub mod errors {
    pub use codec_crate::errors::{Errorf, SharedError};
}

/// MySQL 常量与类型标记重导出。
pub mod mysql {
    pub use mysql_crate::r#const::*;
    pub use mysql_crate::r#type::*;
}

/// 求值类型（EvalType）与 Datum 等类型系统重导出。
pub mod types {
    pub use types_crate::types::{
        ETDatetime, ETDecimal, ETDuration, ETInt, ETJson, ETReal, ETString, ETTimestamp,
        ETVectorFloat32, EvalType,
    };
    pub use types_crate::*;
}

/// 真实表达式接口、求值上下文与临时列池重导出。
pub mod expression {
    pub use expression_crate::*;
}

mod vec_group_checker;
pub use vec_group_checker::*;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "vec_group_checker_test.rs"]
mod vec_group_checker_test;
