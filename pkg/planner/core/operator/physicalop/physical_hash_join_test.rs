// Copyright 2026 AsterSQL.

use base::JoinType;
use expression::{Column, NewSchema};

use crate::physical_hash_join::{
    PhysicalHashJoin, can_tiflash_use_hash_join_v2, can_use_hash_join_v2_with_non_ga,
};

struct GoMerge46PlanContext(
    std::sync::atomic::AtomicI32,
    base::BuiltinFunctionUsageCounter,
);

impl base::PlanContext for GoMerge46PlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("unused")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("unused")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("unused")
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

struct GoMerge46Client;

impl kv::Client for GoMerge46Client {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("protobuf encoding must not send a request")
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        true
    }
}

#[test]
fn go_merge_46_tiflash_join_pb_preserves_null_eq_and_full_join_type() {
    use std::sync::Arc;

    let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
    let mut build_pb = base::BuildPBContext {
        ExprCtx: expression,
        Client: Some(Arc::new(GoMerge46Client)),
        TiFlashFastScan: false,
        TiFlashFineGrainedShuffleBatchSize: 0,
        GroupConcatMaxLen: 0,
        InExplainStmt: false,
        WarnHandler: None,
        ExtraWarnghandler: None,
    };
    let context: base::ContextRef = Arc::new(GoMerge46PlanContext(
        std::sync::atomic::AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ));
    let plan = crate::BasePhysicalPlan::New(context, "HashJoin", 0);
    let producer = crate::PhysicalSchemaProducer::New(plan);
    let mut base = crate::BasePhysicalJoin::New(producer, JoinType::FullOuterJoin);
    base.IsNullEQ = vec![true];
    let join = crate::NewPhysicalHashJoin(base, 1, false);
    let clone_context: base::ContextRef = Arc::new(GoMerge46PlanContext(
        std::sync::atomic::AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ));
    let cloned = join
        .Clone(clone_context)
        .expect("clone hash join candidate");
    assert_eq!(cloned.BasePhysicalJoin.IsNullEQ, [true]);
    let encoded = join
        .ToPB(&mut build_pb, kv::StoreType::TiFlash)
        .expect("encode TiFlash join");
    assert_eq!(
        encoded.get_join().get_join_type(),
        tipb::JoinType::TypeFullOuterJoin
    );
    assert_eq!(encoded.get_join().get_is_null_eq(), &[true]);
}

#[test]
fn repeated_hash_join_output_columns_are_deduplicated_before_resolution() {
    let source = Column::default();
    let mut output = NewSchema(vec![source.Clone(), source.Clone()]);
    let children = NewSchema(vec![source]);

    assert_eq!(
        PhysicalHashJoin::DeduplicateOutputColumns(&mut output, 2),
        1
    );
    assert_eq!(output.Columns.len(), 1);
    assert_eq!(
        PhysicalHashJoin::ResolveOutputColumns(&mut output, &children, 1),
        1
    );
    assert_eq!(output.Columns[0].Index, 0);
}

#[test]
fn non_ga_hash_join_v2_respects_feature_gate() {
    let key = expression::Column::default();
    assert!(!can_use_hash_join_v2_with_non_ga(
        JoinType::LeftOuterSemiJoin,
        std::slice::from_ref(&key),
        &[],
        &[],
        false,
    ));
    assert!(can_use_hash_join_v2_with_non_ga(
        JoinType::LeftOuterSemiJoin,
        std::slice::from_ref(&key),
        &[],
        &[],
        true,
    ));
    assert!(can_use_hash_join_v2_with_non_ga(
        JoinType::InnerJoin,
        &[key],
        &[],
        &[],
        false,
    ));
}

#[test]
fn tiflash_hash_join_v2_rejects_legacy_spill_and_unsupported_shapes() {
    assert!(can_tiflash_use_hash_join_v2(
        "optimized",
        -1,
        -1,
        0.0,
        JoinType::InnerJoin,
        true,
        false,
        false,
    ));
    assert!(!can_tiflash_use_hash_join_v2(
        "legacy",
        -1,
        -1,
        0.0,
        JoinType::InnerJoin,
        true,
        false,
        false,
    ));
    assert!(!can_tiflash_use_hash_join_v2(
        "optimized",
        1,
        -1,
        0.0,
        JoinType::InnerJoin,
        true,
        false,
        false,
    ));
    assert!(!can_tiflash_use_hash_join_v2(
        "optimized",
        -1,
        1,
        0.5,
        JoinType::InnerJoin,
        true,
        false,
        false,
    ));
    assert!(!can_tiflash_use_hash_join_v2(
        "optimized",
        -1,
        -1,
        0.0,
        JoinType::LeftOuterJoin,
        true,
        false,
        false,
    ));
}
