// Copyright 2026 AsterSQL.

// 规划器约束（constraint）crate 的库入口。
//
// 聚合 AST 函数名、MySQL 类型标志、语句上下文与表达式桩类型，并导出
// `exprs` 中的恒真谓词删除逻辑。本 crate 用于迁移期隔离依赖边界。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as constraint;

/// 再导出 AST 侧一元 NOT / IS NULL 函数名常量。
pub mod ast {
    pub use ast_crate::functions::{IsNull, UnaryNot};
}

/// 再导出 MySQL 类型与 NOT NULL 标志检查。
pub mod mysql {
    pub use mysql_crate::r#type::{HasNotNullFlag, NotNullFlag, TypeLonglong};
}

/// 再导出语句上下文（StatementContext）类型。
pub mod stmtctx {
    pub use stmtctx_crate::*;
}

/// 表达式桩模块：常量、列、标量函数与 Schema，供约束清理逻辑单测与联调。
pub mod expression {
    use types_crate::{Datum, FieldType};

    /// 表达式求值上下文桩。
    #[derive(Clone, Copy, Debug, Default)]
    pub struct EvalContext;

    /// 表达式构建上下文；`use_plan_cache` 表示是否启用计划缓存路径。
    #[derive(Clone, Copy, Debug, Default)]
    pub struct BuildContext {
        pub use_plan_cache: bool,
    }

    /// 常量表达式；`mutable` 为 true 时表示参数占位或延迟常量，执行期可变。
    #[derive(Clone)]
    pub struct Constant {
        pub Value: Datum,
        /// Parameter markers and deferred constants are execution mutable.
        /// 参数占位符与延迟常量在执行阶段可变。
        pub mutable: bool,
    }

    /// 列引用：以 UniqueID 在 Schema 中定位，并携带返回类型。
    #[derive(Clone)]
    pub struct Column {
        pub UniqueID: i64,
        pub RetType: Box<FieldType>,
    }

    impl Column {
        /// 返回列的字段类型（FieldType）。
        pub fn GetType(&self, _ctx: &EvalContext) -> &FieldType {
            &self.RetType
        }
    }

    /// 函数名（小写形式存于 `L`）。
    #[derive(Clone, Debug, Default)]
    pub struct FuncName {
        pub L: String,
    }

    /// 标量函数调用：函数名加参数列表。
    #[derive(Clone)]
    pub struct ScalarFunction {
        pub FuncName: FuncName,
        pub Args: Vec<Expression>,
    }

    impl ScalarFunction {
        /// 返回参数切片。
        pub fn GetArgs(&self) -> &[Expression] {
            &self.Args
        }
    }

    /// 表达式枚举：常量、列、标量函数或其它占位。
    #[derive(Clone)]
    pub enum Expression {
        Constant(Constant),
        Column(Column),
        ScalarFunction(ScalarFunction),
        Other,
    }

    impl Expression {
        /// 若为常量则返回引用。
        pub fn as_constant(&self) -> Option<&Constant> {
            match self {
                Self::Constant(value) => Some(value),
                _ => None,
            }
        }

        /// 若为列则返回引用。
        pub fn as_column(&self) -> Option<&Column> {
            match self {
                Self::Column(value) => Some(value),
                _ => None,
            }
        }

        /// 若为标量函数则返回引用。
        pub fn as_scalar_function(&self) -> Option<&ScalarFunction> {
            match self {
                Self::ScalarFunction(value) => Some(value),
                _ => None,
            }
        }
    }

    /// 逻辑 Schema：列集合，用于按 UniqueID 找回带标志的列元信息。
    #[derive(Clone, Default)]
    pub struct Schema {
        pub Columns: Vec<Column>,
    }

    impl Schema {
        /// 按 UniqueID 在 Schema 中查找列。
        pub fn RetrieveColumn(&self, column: &Column) -> Option<&Column> {
            self.Columns
                .iter()
                .find(|candidate| candidate.UniqueID == column.UniqueID)
        }
    }

    /// 计划缓存场景下常量是否可能被过优化（启用缓存且常量可变）。
    pub fn MaybeOverOptimized4PlanCache(build_ctx: &BuildContext, constant: &Constant) -> bool {
        build_ctx.use_plan_cache && constant.mutable
    }
}

mod exprs;
pub use exprs::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
