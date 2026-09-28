// Copyright 2026 AsterSQL.

// PhysicalExchangeSender 的单元测试：验证 MPPSink 接口与克隆时不复制已调度任务。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::MPPSink;

use crate::PhysicalExchangeSender;

/// 测试用 PlanContext：只实现计划 ID 分配与内置函数计数，其余路径 panic。
struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("exchange sender test does not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("pass-through exchange does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("exchange sender test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("exchange sender test does not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("exchange sender test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

/// 静态地址实现的 MPPTaskMeta，用于填充任务 Meta。
#[derive(Clone)]
struct TaskAddress(&'static str);

impl kv::MPPTaskMeta for TaskAddress {
    fn GetAddress(&self) -> String {
        self.0.to_owned()
    }

    fn CloneBox(&self) -> Box<dyn kv::MPPTaskMeta> {
        Box::new(self.clone())
    }
}

/// 构造带默认计数器的测试上下文。
fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

/// Sender 应作为真正的 MPPSink：set/append 后保持任务顺序，EXPLAIN 含本端任务 ID。
#[test]
fn exchange_sender_is_a_real_mpp_sink_and_keeps_task_order() {
    let mut sender = PhysicalExchangeSender::New(context());
    // 通过 MPPSink trait 写入本端与目标任务。
    MPPSink::set_self_tasks(
        &mut sender,
        vec![kv::MPPTask {
            Meta: Some(Box::new(TaskAddress("tiflash-1:3930"))),
            ID: 11,
            ..kv::MPPTask::default()
        }],
    );
    MPPSink::append_target_tasks(
        &mut sender,
        vec![kv::MPPTask {
            Meta: Some(Box::new(TaskAddress("tiflash-2:3930"))),
            ID: 12,
            ..kv::MPPTask::default()
        }],
    );

    assert_eq!(MPPSink::get_self_tasks(&sender)[0].ID, 11);
    assert_eq!(sender.TargetTasks[0].ID, 12);
    assert_eq!(
        sender.ExplainInfo(),
        "ExchangeType: PassThrough, tasks: [11]"
    );
}

/// Clone 保留压缩等配置，但清空已调度的 Tasks/TargetTasks（与 Go 语义一致）。
#[test]
fn cloning_a_sender_preserves_plan_configuration_but_not_scheduled_tasks() {
    let mut sender = PhysicalExchangeSender::New(context());
    sender.Tasks.push(kv::MPPTask {
        ID: 21,
        ..kv::MPPTask::default()
    });
    sender.TargetTasks.push(kv::MPPTask {
        ID: 22,
        ..kv::MPPTask::default()
    });
    sender.CompressionMode = vardef::ExchangeCompressionModeFast;

    let clone = sender.Clone(context()).expect("exchange sender clones");
    assert!(clone.Tasks.is_empty());
    assert!(clone.TargetTasks.is_empty());
    assert_eq!(clone.CompressionMode, vardef::ExchangeCompressionModeFast);
}
