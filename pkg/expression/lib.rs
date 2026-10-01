// Copyright 2026 AsterSQL.

// `pkg/expression` crate 根：表达式求值、内置函数与下推推断的对外入口。
//
// 对应 Go `pkg/expression`。本文件通过依赖再导出与 `#[path]` 挂载各内核模块，
// 并在测试配置下挂载大量 Aster/Go 对齐单元测试。表达式是 SQL 中可求值的树节点
//（列、常量、标量函数等）。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// AST / 解析器表达式节点再导出。
pub mod ast {
    pub use parser_ast_dependency::expressions::PositionExpr;
    pub use parser_ast_dependency::functions::*;
    pub use parser_ast_dependency::*;
}
/// 优化器 base 依赖再导出。
pub mod base {
    pub use planner_base_dependency::*;
}
/// 字符集相关。
pub mod charset {
    pub use parser_charset_dependency::charset::*;
    pub use parser_charset_dependency::*;
}
/// Chunk（列式批）与迭代器。
pub mod chunk {
    pub use chunk_dependency::iterator::{Iterator, Iterator4Chunk, NewIterator4Chunk};
    pub use chunk_dependency::*;
}
pub mod codec {
    pub use codec_dependency::*;
}
/// 校对规则（collation）再导出。
pub mod collate {
    pub use collate_dependency::*;
}
pub mod contextutil {
    pub use contextutil_dependency::*;
}
pub mod errctx {
    pub use errctx_dependency::errctx::*;
    pub use errctx_dependency::*;
    pub const LevelIgnore: Level = Level::LevelIgnore;
}
pub mod errors {
    pub use types_dependency::datum::errors::{Error, Errorf, New, Trace};
    pub use types_dependency::errors::*;
    pub const RedactLogDisable: &str = "OFF";
    pub const RedactLogEnable: &str = "ON";
    pub const RedactLogMarker: &str = "MARKER";
}
pub mod exprctx {
    pub use exprctx_dependency::*;
}
pub mod generatedexpr {
    pub use generatedexpr_dependency::*;
}
mod function_traits;
pub use function_traits::*;
/// Public policy boundary used by planner crates when deciding whether an
/// expression can be encoded for TiKV or TiFlash pushdown.
///
/// 规划器下推策略边界：再导出表达式 AST 片段与 `infer_pushdown` 判定 API。
pub mod infer_pushdown {
    pub use crate::fts_to_like_kernel::{
        CastFamily, Column, Datum, Expression, FieldKind, FieldType, ScalarFunction, Signature,
    };
    pub use crate::infer_pushdown_kernel::{
        EXPR_PUSH_DOWN_BLACKLIST_RELOAD_TIMESTAMP, PushDownContext, StoreType, WarningHandler,
        can_expr_push_down, can_exprs_push_down, can_exprs_push_down_with_extra_info,
        clear_pushdown_blacklist, is_push_down_enabled, push_down_exprs,
        push_down_exprs_with_extra_info, replace_pushdown_blacklist,
    };
}
pub mod model {
    pub use model_dependency::*;
}
/// MySQL 协议常量与类型码。
pub mod mysql {
    pub use parser_mysql_dependency::r#const::*;
    pub use parser_mysql_dependency::r#type::*;
    pub use parser_mysql_dependency::util::*;
    pub use parser_mysql_dependency::*;
}
pub mod opcode {
    pub use parser_opcode_dependency::*;
    pub const IsTruth: Op = Op::IsTruth;
}
pub mod parser {
    pub use parser_dependency::*;
}
pub mod intest {
    pub use intest_dependency::*;
}
pub mod logutil {
    pub use logutil_dependency::log::*;
}
pub mod kv {
    pub use kv_dependency::*;
}
pub mod variable {
    pub const OffInt: i32 = 0;
    pub const OnInt: i32 = 1;
}
pub mod terror {
    pub fn Log(error: impl std::fmt::Display) {
        crate::logutil::BgLogger().debug(error.to_string());
    }
}
/// 类型系统再导出，并补充参数类型推断辅助。
pub mod types {
    pub use types_dependency::datum::*;
    pub use types_dependency::field::{
        ETDatetime, ETDecimal, ETDuration, ETInt, ETJson, ETReal, ETString, ETTimestamp,
        ETVectorFloat32, EvalType,
    };
    pub use types_dependency::file_group::set::ParseSetName;
    pub use types_dependency::metadata::ParseEnumName;
    pub use types_dependency::metadata::{EmptyName, ExplainFormatPlanTree, FieldName, NameSlice};
    pub use types_dependency::time::ZeroTime;
    pub use types_dependency::vector::ZeroVectorFloat32;
    pub use types_dependency::{
        Context, DefaultStmtFlags, DefaultStmtNoWarningContext, Flags, NewContext, StrictContext,
    };

    pub const StrictFlags: Flags = Flags(0);

    pub type AnyValue = Box<dyn std::any::Any>;

    /// 判断字段是否为 BIT 类型。
    pub fn IsTypeBit(field_type: &FieldType) -> bool {
        field_type.GetType() == crate::mysql::TypeBit
    }

    /// 由 Datum kind 推断并写回 FieldType（含无符号标志）。
    pub fn InferParamTypeFromDatum(datum: &Datum, field_type: &mut FieldType) {
        match datum.Kind() {
            KindNull => field_type.SetType(crate::mysql::TypeNull),
            KindInt64 | KindUint64 => field_type.SetType(crate::mysql::TypeLonglong),
            KindFloat32 => field_type.SetType(crate::mysql::TypeFloat),
            KindFloat64 => field_type.SetType(crate::mysql::TypeDouble),
            KindMysqlDecimal => field_type.SetType(crate::mysql::TypeNewDecimal),
            KindMysqlDuration => field_type.SetType(crate::mysql::TypeDuration),
            KindMysqlTime => field_type.SetType(datum.GetMysqlTime().Type()),
            KindMysqlJSON => field_type.SetType(crate::mysql::TypeJSON),
            KindVectorFloat32 => field_type.SetType(crate::mysql::TypeTiDBVectorFloat32),
            _ => field_type.SetType(crate::mysql::TypeVarString),
        }
        if datum.Kind() == KindUint64 {
            field_type.AddFlag(crate::mysql::UnsignedFlag);
        }
    }
}

/// 内存大小常量（指针/切片/接口），供估算用。
pub mod size {
    pub const SizeOfPointer: i64 = std::mem::size_of::<usize>() as i64;
    pub const SizeOfSlice: i64 = std::mem::size_of::<Vec<usize>>() as i64;
    pub const SizeOfInterface: i64 = std::mem::size_of::<*const dyn std::any::Any>() as i64;
}

// —— 内置函数与表达式内核：经 #[path] 挂载各 .rs 实现文件 ——
#[path = "builtin_arithmetic.rs"]
mod builtin_arithmetic_kernel;
#[path = "builtin_arithmetic_vec.rs"]
mod builtin_arithmetic_vec_kernel;
#[path = "builtin_cast.rs"]
mod builtin_cast_kernel;
#[path = "builtin_cast_vec.rs"]
mod builtin_cast_vec_kernel;
#[path = "builtin_compare.rs"]
mod builtin_compare_kernel;
#[path = "builtin_compare_vec_generated.rs"]
mod builtin_compare_vec_generated_kernel;
#[path = "builtin_compare_vec.rs"]
mod builtin_compare_vec_kernel;
#[path = "builtin_control.rs"]
mod builtin_control_kernel;
#[path = "builtin_control_vec_generated.rs"]
mod builtin_control_vec_generated_kernel;
#[path = "builtin_convert_charset.rs"]
mod builtin_convert_charset_kernel;
#[cfg(test)]
#[path = "builtin_convert_charset_test.rs"]
mod builtin_convert_charset_test;
#[path = "builtin_core.rs"]
mod builtin_core;
#[path = "builtin_encryption.rs"]
mod builtin_encryption_kernel;
#[path = "builtin_encryption_vec.rs"]
mod builtin_encryption_vec_kernel;
#[path = "builtin_fts.rs"]
mod builtin_fts_kernel;
#[cfg(test)]
#[path = "builtin_fts_test.rs"]
mod builtin_fts_test;
#[path = "cache_snapshot.rs"]
mod cache_snapshot;
#[cfg(test)]
#[path = "cache_snapshot_test.rs"]
mod cache_snapshot_test;
pub mod builtin_fts {
    pub use crate::builtin_fts_kernel::*;
}
#[path = "builtin_func_param.rs"]
mod builtin_func_param_kernel;
#[path = "builtin_grouping.rs"]
mod builtin_grouping_kernel;
#[path = "builtin_ilike.rs"]
mod builtin_ilike_kernel;
#[path = "builtin_ilike_vec.rs"]
mod builtin_ilike_vec_kernel;
#[cfg(test)]
#[path = "builtin_ilike_vec_test.rs"]
mod builtin_ilike_vec_test;
#[path = "builtin_info.rs"]
mod builtin_info_kernel;
#[path = "builtin_info_vec.rs"]
mod builtin_info_vec_kernel;
#[path = "builtin_json.rs"]
mod builtin_json_kernel;
pub mod builtin_json {
    pub use crate::builtin_json_kernel::*;
}
#[path = "builtin_json_vec.rs"]
mod builtin_json_vec_kernel;
#[path = "builtin_like.rs"]
mod builtin_like_kernel;
#[path = "builtin_like_vec.rs"]
mod builtin_like_vec_kernel;
#[path = "builtin_math.rs"]
mod builtin_math_kernel;
#[path = "builtin_math_vec.rs"]
mod builtin_math_vec_kernel;
#[path = "builtin_miscellaneous.rs"]
mod builtin_miscellaneous_kernel;
#[path = "builtin_miscellaneous_vec.rs"]
mod builtin_miscellaneous_vec_kernel;
#[path = "builtin_op.rs"]
mod builtin_op_kernel;
#[path = "builtin_op_vec.rs"]
mod builtin_op_vec_kernel;
#[path = "builtin_other.rs"]
mod builtin_other_kernel;
#[path = "builtin_other_vec_generated.rs"]
mod builtin_other_vec_generated_kernel;
#[path = "builtin_other_vec.rs"]
mod builtin_other_vec_kernel;
#[path = "builtin_regexp.rs"]
mod builtin_regexp_kernel;
#[path = "builtin_regexp_util.rs"]
mod builtin_regexp_util_kernel;
#[path = "builtin_registry.rs"]
mod builtin_registry_kernel;
#[path = "builtin_string.rs"]
mod builtin_string_kernel;
#[path = "builtin_string_vec_generated.rs"]
mod builtin_string_vec_generated_kernel;
#[path = "builtin_string_vec.rs"]
mod builtin_string_vec_kernel;
#[path = "builtin_threadsafe_generated.rs"]
mod builtin_threadsafe_generated_kernel;
#[path = "builtin_threadunsafe_generated.rs"]
mod builtin_threadunsafe_generated_kernel;
#[path = "builtin_time.rs"]
mod builtin_time_kernel;
#[path = "builtin_time_vec_generated.rs"]
mod builtin_time_vec_generated_kernel;
#[path = "builtin_time_vec.rs"]
mod builtin_time_vec_kernel;
#[path = "builtin_vec.rs"]
mod builtin_vec_kernel;
#[path = "builtin_vec_vec.rs"]
mod builtin_vec_vec_kernel;
#[path = "builtin_vectorized.rs"]
mod builtin_vectorized_kernel;
#[path = "chunk_executor.rs"]
mod chunk_executor_kernel;
#[path = "constant_fold.rs"]
mod constant_fold_kernel;
#[path = "constant_propagation.rs"]
mod constant_propagation_kernel;
#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
#[path = "core_impl.rs"]
mod core_impl;
#[path = "core_support.rs"]
mod core_support;
#[cfg(test)]
#[path = "core_support_test.rs"]
mod core_support_test;
#[path = "distsql_builtin.rs"]
mod distsql_builtin_kernel;
#[path = "evaluator.rs"]
mod evaluator_kernel;
#[path = "explain.rs"]
mod explain_kernel;
#[cfg(test)]
#[path = "explain_test.rs"]
mod explain_test;
#[path = "explicit_collation.rs"]
mod explicit_collation;
#[path = "expr_to_pb.rs"]
mod expr_to_pb_kernel;
#[path = "builtin.rs"]
mod expression_builtin;
#[path = "collation.rs"]
mod expression_collation;
#[path = "column.rs"]
mod expression_column;
#[path = "constant.rs"]
mod expression_constant;
#[path = "context.rs"]
mod expression_context;
#[path = "expression.rs"]
mod expression_core;
#[path = "errors.rs"]
mod expression_errors_kernel;
#[path = "scalar_function.rs"]
mod expression_scalar_function;
#[path = "schema.rs"]
mod expression_schema;
#[path = "simple_rewriter.rs"]
mod expression_simple_rewriter;
#[path = "extension.rs"]
mod extension_kernel;
#[path = "fts_helper.rs"]
mod fts_helper_kernel;
#[path = "fts_to_like.rs"]
mod fts_to_like_kernel;
#[path = "function_traits.rs"]
mod function_traits_kernel;
#[path = "grouping_sets.rs"]
mod grouping_sets_kernel;
#[path = "helper.rs"]
mod helper_kernel;
#[path = "infer_pushdown.rs"]
mod infer_pushdown_kernel;
#[path = "legacy_vectorized_runtime.rs"]
mod legacy_vectorized_runtime;
#[cfg(test)]
#[path = "legacy_vectorized_runtime_test.rs"]
mod legacy_vectorized_runtime_test;
#[path = "pb_to_expr_runtime.rs"]
mod pb_to_expr_runtime;
#[path = "planner_bridge.rs"]
mod planner_bridge_kernel;
#[path = "pushdown_context.rs"]
mod pushdown_context_kernel;
#[path = "util.rs"]
mod util_kernel;
#[path = "vectorized.rs"]
mod vectorized_kernel;
#[path = "vs_helper.rs"]
mod vs_helper_kernel;

// —— 核心 API 再导出 ——
pub use builtin_core::*;
pub use builtin_vectorized_kernel::{GetColumn, PutColumn};
pub use cache_snapshot::*;
pub use core_support::*;
pub use core_support::{
    CheckArgsNotMultiColumnRow, DatumToConstant, ExtractColumnsMapFromExpressions, GetFuncArg,
    GetIntFromConstant, GetRowLen, MaybeOverOptimized4PlanCache, SetExprColumnInOperand,
    logicalOps,
};
pub use explain_kernel::*;
pub use explicit_collation::*;
pub use expr_to_pb_kernel::*;
pub use expression_builtin::formal_registry;
pub use expression_builtin::formal_registry::{
    BuildCastFunction, BuildFromBinaryFunction, BuildGetVarFunction, BuildToBinaryFunction,
    BuiltinFactory, FunctionClassMetadata, GeneratedBuiltinFactoryOutput, InternalFuncFromBinary,
    InternalFuncToBinary, baseFunctionClass, extensionFuncs, funcs, functionClass,
    isTrueOrFalseFunctionClass, registerBuiltinFactory, removeBuiltinFactory, valuesFunctionClass,
};
pub use expression_collation::*;
pub use expression_column::*;
pub use expression_constant::*;
pub use expression_context::*;
pub use expression_core::*;
pub use expression_scalar_function::*;
pub use expression_schema::*;
pub use expression_simple_rewriter::*;
pub use pb_to_expr_runtime::{
    FieldTypeFromPB, PBSignatureFunctionName, PBToExpr, PBToExprs, PbTypeToFieldType,
};
pub use util_kernel::*;

pub type Error = types::errors::Error;

// —— 测试模块挂载（cfg(test)）——
#[cfg(test)]
#[path = "core_impl_test.rs"]
mod core_impl_test;

#[cfg(test)]
#[path = "chunk_executor_test.rs"]
mod chunk_executor_test;

#[cfg(test)]
#[path = "builtin_registry_aster_unit_test.rs"]
mod builtin_registry_aster_unit_test;

#[cfg(test)]
#[path = "rewriter_support_aster_unit_test.rs"]
mod rewriter_support_aster_unit_test;

#[cfg(test)]
#[path = "explicit_collation_test.rs"]
mod explicit_collation_test;

#[cfg(test)]
#[path = "extension_runtime_aster_unit_test.rs"]
mod extension_runtime_aster_unit_test;

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "builtin_arithmetic_test.rs"]
mod builtin_arithmetic_test;
#[cfg(test)]
#[path = "builtin_arithmetic_vec_test.rs"]
mod builtin_arithmetic_vec_test;
#[cfg(test)]
#[path = "builtin_cast_bench_test.rs"]
mod builtin_cast_bench_test;

#[cfg(test)]
#[path = "builtin_cast_test.rs"]
mod builtin_cast_test;
#[cfg(test)]
#[path = "builtin_cast_vec_3_aster_unit_test.rs"]
mod builtin_cast_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_cast_vec_test.rs"]
mod builtin_cast_vec_test;
#[cfg(test)]
#[path = "builtin_compare_7_aster_unit_test.rs"]
mod builtin_compare_aster_unit_test;
#[cfg(test)]
#[path = "builtin_compare_test.rs"]
mod builtin_compare_test;
#[cfg(test)]
#[path = "builtin_compare_vec_6_aster_unit_test.rs"]
mod builtin_compare_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_compare_vec_generated_5_aster_unit_test.rs"]
mod builtin_compare_vec_generated_aster_unit_test;
#[cfg(test)]
#[path = "builtin_compare_vec_generated_test.rs"]
mod builtin_compare_vec_generated_test;
#[cfg(test)]
#[path = "builtin_compare_vec_test.rs"]
mod builtin_compare_vec_test;
#[cfg(test)]
#[path = "builtin_control_9_aster_unit_test.rs"]
mod builtin_control_aster_unit_test;
#[cfg(test)]
#[path = "builtin_control_test.rs"]
mod builtin_control_test;
#[cfg(test)]
#[path = "builtin_control_vec_generated_8_aster_unit_test.rs"]
mod builtin_control_vec_generated_aster_unit_test;
#[cfg(test)]
#[path = "builtin_control_vec_generated_test.rs"]
mod builtin_control_vec_generated_test;
#[cfg(test)]
#[path = "builtin_encryption_11_aster_unit_test.rs"]
mod builtin_encryption_aster_unit_test;
#[cfg(test)]
#[path = "builtin_encryption_test.rs"]
mod builtin_encryption_test;
#[cfg(test)]
#[path = "builtin_encryption_vec_10_aster_unit_test.rs"]
mod builtin_encryption_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_encryption_vec_test.rs"]
mod builtin_encryption_vec_test;
#[cfg(test)]
#[path = "builtin_grouping_test.rs"]
mod builtin_grouping_test;
#[cfg(test)]
#[path = "builtin_ilike_test.rs"]
mod builtin_ilike_test;
#[cfg(test)]
#[path = "builtin_ilike_vec_12_aster_unit_test.rs"]
mod builtin_ilike_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_info_test.rs"]
mod builtin_info_test;
#[cfg(test)]
#[path = "builtin_info_vec_test.rs"]
mod builtin_info_vec_test;
#[cfg(test)]
#[path = "builtin_json_14_aster_unit_test.rs"]
mod builtin_json_aster_unit_test;
#[cfg(test)]
#[path = "builtin_json_test.rs"]
mod builtin_json_test;
#[cfg(test)]
#[path = "builtin_json_vec_13_aster_unit_test.rs"]
mod builtin_json_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_json_vec_test.rs"]
mod builtin_json_vec_test;
#[cfg(test)]
#[path = "builtin_like_test.rs"]
mod builtin_like_test;
#[cfg(test)]
#[path = "builtin_like_vec_15_aster_unit_test.rs"]
mod builtin_like_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_like_vec_test.rs"]
mod builtin_like_vec_test;
#[cfg(test)]
#[path = "builtin_math_16_aster_unit_test.rs"]
mod builtin_math_aster_unit_test;
#[cfg(test)]
#[path = "builtin_math_test.rs"]
mod builtin_math_test;
#[cfg(test)]
#[path = "builtin_math_vec_test.rs"]
mod builtin_math_vec_test;
#[cfg(test)]
#[path = "builtin_miscellaneous_18_aster_unit_test.rs"]
mod builtin_miscellaneous_aster_unit_test;
#[cfg(test)]
#[path = "builtin_miscellaneous_test.rs"]
mod builtin_miscellaneous_test;
#[cfg(test)]
#[path = "builtin_miscellaneous_vec_17_aster_unit_test.rs"]
mod builtin_miscellaneous_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_miscellaneous_vec_test.rs"]
mod builtin_miscellaneous_vec_test;
#[cfg(test)]
#[path = "builtin_op_20_aster_unit_test.rs"]
mod builtin_op_aster_unit_test;
#[cfg(test)]
#[path = "builtin_op_test.rs"]
mod builtin_op_test;
#[cfg(test)]
#[path = "builtin_op_vec_19_aster_unit_test.rs"]
mod builtin_op_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_op_vec_test.rs"]
mod builtin_op_vec_test;
#[cfg(test)]
#[path = "builtin_other_22_aster_unit_test.rs"]
mod builtin_other_aster_unit_test;
#[cfg(test)]
#[path = "builtin_other_test.rs"]
mod builtin_other_test;
#[cfg(test)]
#[path = "builtin_other_vec_generated_21_aster_unit_test.rs"]
mod builtin_other_vec_generated_aster_unit_test;
#[cfg(test)]
#[path = "builtin_other_vec_generated_test.rs"]
mod builtin_other_vec_generated_test;
#[cfg(test)]
#[path = "builtin_other_vec_test.rs"]
mod builtin_other_vec_test;
#[cfg(test)]
#[path = "builtin_regexp_test.rs"]
mod builtin_regexp_test;
#[cfg(test)]
#[path = "builtin_regexp_util_23_aster_unit_test.rs"]
mod builtin_regexp_util_aster_unit_test;
#[cfg(test)]
#[path = "builtin_regexp_util_test.rs"]
mod builtin_regexp_util_test;
#[cfg(test)]
#[path = "builtin_regexp_vec_const_test.rs"]
mod builtin_regexp_vec_const_test;
#[cfg(test)]
#[path = "builtin_string_25_aster_unit_test.rs"]
mod builtin_string_aster_unit_test;
#[cfg(test)]
#[path = "builtin_string_test.rs"]
mod builtin_string_test;
#[cfg(test)]
#[path = "builtin_string_vec_24_aster_unit_test.rs"]
mod builtin_string_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_string_vec_generated_test.rs"]
mod builtin_string_vec_generated_test;
#[cfg(test)]
#[path = "builtin_string_vec_test.rs"]
mod builtin_string_vec_test;
#[cfg(test)]
#[path = "builtin_test.rs"]
mod builtin_test;
#[cfg(test)]
#[path = "builtin_threadsafe_generated_26_aster_unit_test.rs"]
mod builtin_threadsafe_generated_aster_unit_test;
#[cfg(test)]
#[path = "builtin_threadunsafe_generated_27_aster_unit_test.rs"]
mod builtin_threadunsafe_generated_aster_unit_test;
#[cfg(test)]
#[path = "builtin_time_30_aster_unit_test.rs"]
mod builtin_time_aster_unit_test;
#[cfg(test)]
#[path = "builtin_time_test.rs"]
mod builtin_time_test;
#[cfg(test)]
#[path = "builtin_time_vec_29_aster_unit_test.rs"]
mod builtin_time_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_time_vec_generated_28_aster_unit_test.rs"]
mod builtin_time_vec_generated_aster_unit_test;
#[cfg(test)]
#[path = "builtin_time_vec_generated_test.rs"]
mod builtin_time_vec_generated_test;
#[cfg(test)]
#[path = "builtin_time_vec_test.rs"]
mod builtin_time_vec_test;
#[cfg(test)]
#[path = "builtin_vec_vec_31_aster_unit_test.rs"]
mod builtin_vec_vec_aster_unit_test;
#[cfg(test)]
#[path = "builtin_vec_vec_test.rs"]
mod builtin_vec_vec_test;
#[cfg(test)]
#[path = "builtin_vectorized_test.rs"]
mod builtin_vectorized_test;
#[cfg(test)]
#[path = "collation_33_aster_unit_test.rs"]
mod collation_aster_unit_test;
#[cfg(test)]
#[path = "collation_test.rs"]
mod collation_test;
#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;
#[cfg(test)]
#[path = "constant_fold_38_aster_unit_test.rs"]
mod constant_fold_aster_unit_test;
#[cfg(test)]
#[path = "constant_propagation_test.rs"]
mod constant_propagation_test;
#[cfg(test)]
#[path = "constant_test.rs"]
mod constant_test;
#[cfg(test)]
#[path = "distsql_builtin_34_aster_unit_test.rs"]
mod distsql_builtin_aster_unit_test;
#[cfg(test)]
#[path = "distsql_builtin_test.rs"]
mod distsql_builtin_test;
#[cfg(test)]
#[path = "errors_35_aster_unit_test.rs"]
mod errors_aster_unit_test;
#[cfg(test)]
#[path = "evaluator_test.rs"]
mod evaluator_test;
#[cfg(test)]
#[path = "expr_to_pb_test.rs"]
mod expr_to_pb_test;
#[cfg(test)]
#[path = "expression_test.rs"]
mod expression_test;
#[cfg(test)]
#[path = "fts_to_like_36_aster_unit_test.rs"]
mod fts_to_like_aster_unit_test;
#[cfg(test)]
#[path = "fts_to_like_test.rs"]
mod fts_to_like_test;
#[cfg(test)]
#[path = "function_traits_test.rs"]
mod function_traits_test;
#[cfg(test)]
#[path = "grouping_sets_runtime_aster_unit_test.rs"]
mod grouping_sets_runtime_aster_unit_test;
#[cfg(test)]
#[path = "grouping_sets_test.rs"]
mod grouping_sets_test;
#[cfg(test)]
#[path = "helper_test.rs"]
mod helper_test;
#[cfg(test)]
#[path = "infer_pushdown_test.rs"]
mod infer_pushdown_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "planner_bridge_test.rs"]
mod planner_bridge_test;
#[cfg(test)]
#[path = "scalar_function_37_aster_unit_test.rs"]
mod scalar_function_aster_unit_test;
#[cfg(test)]
#[path = "scalar_function_test.rs"]
mod scalar_function_test;
#[cfg(test)]
#[path = "schema_test.rs"]
mod schema_test;
#[cfg(test)]
#[path = "simple_rewriter_test.rs"]
mod simple_rewriter_test;
#[cfg(test)]
#[path = "typeinfer_test.rs"]
mod typeinfer_test;
#[cfg(test)]
#[path = "util_runtime_parity_aster_unit_test.rs"]
mod util_runtime_parity_aster_unit_test;
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;

/// 测试用比较内核别名。
#[cfg(test)]
pub(crate) use builtin_compare_kernel as builtin_compare;
#[cfg(test)]
mod expression_builtin_cast_vec {
    pub use crate::builtin_cast_vec_kernel::*;
}

/// 向量化过滤器与规划器桥接 API。
pub use chunk_executor_kernel::VectorizedFilter;
pub use constant_propagation_kernel::{PropagateConstantForJoinRef, PropagateConstantRef};
pub use planner_bridge_kernel::{
    BuildFTSToILikeExpression, BuildFTSToILikeExpressionFromBuiltin,
    BuildJSONSumCrc32FunctionWithCheck, BuiltinGroupingImplSig, GetAccurateCmpType, GetTimeValue,
    InferType4ControlFuncs, InferType4ControlFuncsVariadic, PlannerGroupingMode,
    RefineComparedConstant, ResolveType4Between, SetFTSMysqlMatchAgainstModifier,
    ValidateFTSSearchStringForLikeFallback,
};
pub use pushdown_context_kernel::{
    NewPushDownContext, PushDownBuildContextRef, PushDownClientRef, PushDownContext,
    PushDownWarnAppenderRef,
};
// —— 任务/测试门面模块：为分批迁移的测试提供稳定路径 ——
#[cfg(test)]
mod expression_compare_vec_generated {
    pub use crate::builtin_compare_vec_generated_kernel::*;
}
#[cfg(test)]
mod expression_encryption {
    pub mod builtin_encryption {
        pub use crate::builtin_encryption_kernel::*;
    }
    pub mod builtin_fts {
        pub use crate::builtin_fts_kernel::*;
    }
    pub mod builtin_func_param {
        pub use crate::builtin_func_param_kernel::*;
    }
    pub mod builtin_grouping {
        pub use crate::builtin_grouping_kernel::*;
    }
}
#[cfg(test)]
mod expression_encryption_vec {
    pub use crate::builtin_encryption_vec_kernel::*;
}
#[cfg(test)]
mod expression_builtin_json {
    pub use crate::builtin_json_kernel::*;
}
#[cfg(test)]
mod expression_json_vec {
    pub use crate::builtin_json_vec_kernel::*;
}
#[cfg(test)]
mod expression_group_15 {
    pub use crate::builtin_like_kernel::*;
    pub use crate::builtin_like_vec_kernel::*;
    pub use crate::builtin_math_vec_kernel::*;
}
#[cfg(test)]
mod builtin_math {
    pub use crate::builtin_math_kernel::*;
}
#[cfg(test)]
mod expression_builtin_miscellaneous {
    pub use crate::builtin_miscellaneous_kernel::*;
}
#[cfg(test)]
mod builtin_miscellaneous_vec {
    pub use crate::builtin_miscellaneous_vec_kernel::*;
}
#[cfg(test)]
mod builtin_op {
    pub use crate::builtin_op_kernel::*;
}
#[cfg(test)]
mod builtin_op_vec {
    pub use crate::builtin_op_vec_kernel::*;
}
#[cfg(test)]
mod types_test_support {
    pub use types_dependency::*;
}
#[cfg(test)]
mod util_chunk {
    pub use chunk_dependency::*;
}
#[cfg(test)]
mod expression_other {
    pub use crate::builtin_other_kernel::*;
}
#[cfg(test)]
mod expression_other_vec {
    pub use crate::builtin_other_vec_generated_kernel::*;
    pub use crate::builtin_other_vec_kernel::*;
}
#[cfg(test)]
mod expression_regexp {
    pub use crate::builtin_regexp_kernel::*;
    pub use crate::builtin_regexp_util_kernel::*;
}
#[cfg(test)]
mod expression_builtin_string {
    pub use crate::builtin_string_kernel::*;
}
#[cfg(test)]
mod string_vec {
    pub use crate::builtin_string_vec_kernel::*;
}
#[cfg(test)]
mod builtin_time {
    pub use crate::builtin_time_kernel::*;
}
#[cfg(test)]
mod expression_builtin_time_vec {
    pub use crate::builtin_time_vec_kernel::*;
}
#[cfg(test)]
mod expression_vector {
    pub use crate::builtin_vec_vec_kernel::*;
}
#[cfg(test)]
mod expression_collation_test_support {
    pub use crate::expression_collation::*;
}
#[cfg(test)]
mod expression_distsql_builtin {
    pub use crate::distsql_builtin_kernel::*;
}
#[cfg(test)]
mod expression_errors {
    pub use crate::expression_errors_kernel::*;
}
#[cfg(test)]
mod expression_files_36 {
    pub use crate::fts_to_like_kernel::*;
    pub mod function_traits {
        pub use crate::function_traits_kernel::*;
    }
    pub mod grouping_sets {
        pub use crate::grouping_sets_kernel::*;
    }
    pub mod helper {
        pub use crate::helper_kernel::*;
    }
    pub mod infer_pushdown {
        pub use crate::infer_pushdown_kernel::*;
    }
}

#[cfg(test)]
#[path = "builtin_now_test.rs"]
mod builtin_now_test;
