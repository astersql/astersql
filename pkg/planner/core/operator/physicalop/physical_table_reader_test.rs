// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::PhysicalPlan as _;

use crate::{PhysicalExchangeSender, PhysicalTableReader, PhysicalTableScan, ReadReqType};

struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        std::process::abort()
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        std::process::abort()
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

#[test]
fn tiflash_exchange_sender_selects_mpp_and_marks_table_scans() {
    let ctx = context();
    let mut scan = PhysicalTableScan::New(ctx.clone());
    // MPP scans are marked while the scan candidate is built, matching Go's
    // post-init propagation without requiring mutable child traversal here.
    scan.IsMPPOrBatchCop = true;
    let mut sender = PhysicalExchangeSender::New(ctx.clone());
    sender.set_children(vec![Box::new(scan)]);

    let mut reader = PhysicalTableReader::New(ctx);
    reader.StoreType = kv::StoreType::TiFlash;
    reader.SetChildren(vec![Box::new(sender)]);

    assert_eq!(reader.ReadReqType, ReadReqType::MPP);
    assert!(
        reader
            .GetTableScan()
            .expect("single table scan")
            .IsMPPOrBatchCop
    );
}

#[test]
fn operator_info_never_includes_the_mpp_version_prefix() {
    let ctx = context();
    let scan = PhysicalTableScan::New(ctx.clone());
    let mut reader = PhysicalTableReader::New(ctx);
    reader.SetTablePlanForTest(Box::new(scan));
    let expected = reader.ExplainInfo();
    reader.ReadReqType = ReadReqType::MPP;

    assert_eq!(reader.OperatorInfo(false), expected);
}
