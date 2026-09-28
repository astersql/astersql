// Copyright 2026 AsterSQL.

// Insert 物理计划相关的单元测试：验证 Schema 延迟初始化、生成列深克隆与外键子计划所有权。

use crate::{FKCascade, FKCascadeType, FKCheck, Insert, InsertGeneratedColumns};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// 最小 PlanContext 桩：只实现分配计划 ID 与内置函数计数，其余访问直接 panic。
struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("insert ownership test does not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("insert ownership test does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("insert ownership test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("insert ownership test does not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("insert ownership test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

/// 构造可共享的测试用 PlanContext。
fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

/// 新建 Insert 时表 Schema / OnDuplicate Schema / Table / SelectPlan 应仍为空，
/// 由后续 DML builder 负责填充。
#[test]
fn insert_preserves_nil_schema_until_dml_builder_initializes_it() {
    let insert = Insert::New(context());
    assert!(insert.TableSchema.is_none());
    assert!(insert.Schema4OnDuplicate.is_none());
    assert!(insert.Table.is_none());
    assert!(insert.SelectPlan.is_none());
}

/// 生成列表达式与 ON DUPLICATE 赋值在 Clone 后必须是深拷贝（指针不相等）。
#[test]
fn generated_columns_clone_expression_and_assignment_trees() {
    // 构造带 UniqueID 的列表达式工厂。
    let column = |unique_id| {
        expression::Column::new(
            *expression::types::NewFieldType(expression::mysql::TypeLonglong),
            unique_id,
            unique_id,
            0,
        )
    };
    let expression: expression::ExprBox = Box::new(column(11));
    let assignment = expression::Assignment {
        Col: column(12),
        ColName: parser_ast::CIStr::default(),
        Expr: Box::new(column(13)),
        LazyErr: None,
    };
    let generated = InsertGeneratedColumns {
        Exprs: vec![expression],
        OnDuplicates: vec![Box::new(assignment)],
    };
    let cloned = generated.clone();

    assert!(!std::ptr::eq(
        generated.Exprs[0].as_ref(),
        cloned.Exprs[0].as_ref()
    ));
    assert!(!std::ptr::eq(
        generated.OnDuplicates[0].as_ref(),
        cloned.OnDuplicates[0].as_ref()
    ));
    let cloned_column = cloned.Exprs[0]
        .as_any()
        .downcast_ref::<expression::Column>()
        .expect("generated expression remains a column");
    assert_eq!(cloned_column.UniqueID, 11);
    assert_eq!(cloned.OnDuplicates[0].Col.UniqueID, 12);
}

/// Insert 应能直接持有 FKCheck / FKCascade 物理子计划，并保留级联类型。
#[test]
fn insert_owns_exact_foreign_key_plan_types() {
    let ctx = context();
    let checks: Vec<Box<FKCheck>> = vec![Box::new(FKCheck::New(ctx.clone()))];
    let cascades: Vec<Box<FKCascade>> = vec![Box::new(FKCascade::New(
        ctx.clone(),
        FKCascadeType::OnUpdate,
    ))];
    let mut insert = Insert::New(ctx);
    insert.FKChecks = checks;
    insert.FKCascades = cascades;

    assert_eq!(insert.FKChecks.len(), 1);
    assert_eq!(insert.FKCascades.len(), 1);
    assert_eq!(insert.FKCascades[0].Tp, FKCascadeType::OnUpdate);
}
