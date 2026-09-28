// Copyright 2026 AsterSQL.

// 优化器基数（cardinality）估算包的 crate 根。
//
// 汇总跨列相关性、指数退避、Join 行数、NDV、伪统计、行列宽与选择率等子模块，
// 并提供与 Go `planctx` 对接的 object-safe `CardinalityContext`，以及各依赖
// 包的 re-export 命名空间，便于机械迁移代码保持原有路径形状。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 基数估算所消费的规划器上下文的 object-safe 子集。
/// 上游 `PlanContext` 带有 InfoSchema 关联类型，无法直接作为 trait object；
/// 本 trait 只暴露会话变量、表达式上下文与 ranger 上下文。
/// Object-safe subset of planner context consumed by cardinality estimation.
/// The upstream `PlanContext` carries InfoSchema associated types that are not
/// relevant here and therefore cannot be used as a trait object directly.
pub trait CardinalityContext {
    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars;
    fn GetExprCtx(&self) -> &dyn planctx_dependency::exprctx::ExprContext;
    fn GetRangerCtx(&self) -> &planctx_dependency::rangerctx::RangerContext<'_>;
}

/// Go 允许通过可空指针调用 TopN 方法；在 Rust 边界保留 nil 安全契约，
/// 避免在调用处散布 unwrap。
/// Go permits calling TopN methods through a nullable pointer.  Keep that
/// nil-safe contract at the Rust boundary instead of scattering unwraps.
pub trait OptionalTopNExt {
    fn Num(&self) -> usize;
    fn MinCount(&self) -> u64;
    fn TotalCount(&self) -> u64;
}

impl OptionalTopNExt for Option<statistics_dependency::TopN> {
    fn Num(&self) -> usize {
        self.as_ref().map_or(0, statistics_dependency::TopN::Num)
    }
    fn MinCount(&self) -> u64 {
        self.as_ref()
            .map_or(0, statistics_dependency::TopN::MinCount)
    }
    fn TotalCount(&self) -> u64 {
        self.as_ref()
            .map_or(0, statistics_dependency::TopN::TotalCount)
    }
}

impl OptionalTopNExt for Option<&statistics_dependency::TopN> {
    fn Num(&self) -> usize {
        self.map_or(0, statistics_dependency::TopN::Num)
    }
    fn MinCount(&self) -> u64 {
        self.map_or(0, statistics_dependency::TopN::MinCount)
    }
    fn TotalCount(&self) -> u64 {
        self.map_or(0, statistics_dependency::TopN::TotalCount)
    }
}

impl<T> CardinalityContext for T
where
    T: planctx_dependency::PlanContext,
{
    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        planctx_dependency::Common::GetSessionVars(self)
    }

    fn GetExprCtx(&self) -> &dyn planctx_dependency::exprctx::ExprContext {
        planctx_dependency::Common::GetExprCtx(self)
    }

    fn GetRangerCtx(&self) -> &planctx_dependency::rangerctx::RangerContext<'_> {
        planctx_dependency::Common::GetRangerCtx(self)
    }
}

pub mod ast {
    pub use parser_ast_dependency::functions::*;
}
pub mod chunk {
    pub use chunk_dependency::iterator::NewIterator4Chunk;
    pub use chunk_dependency::*;
}
pub mod codec {
    pub use codec_dependency::*;
}
pub mod collate {
    pub use collate_dependency::*;
}
pub mod cost {
    pub use cost_dependency::factors_thresholds::*;
    pub use cost_dependency::*;
}
pub mod errors {
    pub use types_dependency::datum::errors::*;
    pub fn NewNoStackError(message: impl Into<String>) -> Error {
        New(message)
    }
}
pub mod expression {
    pub use expression_dependency::*;
}
pub mod kv {
    pub use kv_dependency::StoreType::{TiFlash, TiKV};
    pub use kv_dependency::*;
}
pub mod logutil {
    pub use logutil_dependency::*;
}
pub mod mathutil {
    pub use mathutil_dependency::*;
}
pub mod model {
    pub use model_dependency::*;
}
pub mod mysql {
    pub use mysql_dependency::r#type::*;
}
pub mod planctx {
    pub use crate::CardinalityContext as PlanContext;
}
pub mod planutil {
    pub use planutil_dependency::*;
}
pub mod property {
    pub use property_dependency::*;
}
pub mod ranger {
    pub use ranger_dependency::*;
}
pub mod stmtctx {
    pub use stmtctx_dependency::*;
}
pub mod statistics {
    pub use statistics_dependency::*;
}
pub mod tablecodec {
    pub use tablecodec_dependency::*;
}
pub mod types {
    pub use types_dependency::datum::{
        Context, Datum, FieldType, KindBinaryLiteral, KindBytes, KindInt64, KindMaxValue,
        KindMinNotNull, KindMysqlBit, KindNull, KindString, MaxValueDatum, NewBytesDatum,
        NewFieldType, NewIntDatum, UnspecifiedLength,
    };
    pub use types_dependency::field::IsString;
}
pub mod util {
    pub use planutil_dependency::*;
}
pub mod vardef {
    pub use vardef_dependency::*;
}
pub mod variable {
    pub use variable_dependency::session::SessionVars;
}

mod cross_estimation;
mod exponential;
mod join;
mod ndv;
mod pseudo;
mod row_count_column;
mod row_count_index;
mod row_size;
mod selectivity;
mod trace;

pub use cross_estimation::*;
pub use exponential::*;
pub use join::*;
pub use ndv::*;
pub use pseudo::*;
pub use row_count_column::*;
pub use row_count_index::*;
pub use row_size::*;
pub use selectivity::*;
pub use trace::*;

#[cfg(test)]
#[path = "cross_estimation_test.rs"]
mod cross_estimation_test;
#[cfg(test)]
#[path = "exponential_test.rs"]
mod exponential_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "ndv_test.rs"]
mod ndv_test;
#[cfg(test)]
#[path = "row_count_column_test.rs"]
mod row_count_column_test;
#[cfg(test)]
#[path = "row_count_index_test.rs"]
mod row_count_index_test;
#[cfg(test)]
#[path = "row_size_test.rs"]
mod row_size_test;
#[cfg(test)]
#[path = "selectivity_test.rs"]
mod selectivity_test;
#[cfg(test)]
#[path = "trace_test.rs"]
mod trace_test;
