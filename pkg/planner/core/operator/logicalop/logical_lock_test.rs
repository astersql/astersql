// Copyright 2026 AsterSQL.

use crate::*;
use std::any::Any;

#[derive(Default)]
struct PushDownProbe {
    base: BaseLogicalPlan,
    called: bool,
}

impl LogicalPlan for PushDownProbe {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn base(&self) -> &BaseLogicalPlan {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.base
    }

    fn PushDownTopN(&mut self, _top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        self.called = true;
        None
    }
}

#[test]
fn absent_top_n_does_not_visit_lock_child() {
    let mut lock = LogicalLock::default();
    lock.SetChildren(vec![Box::new(PushDownProbe::default())]);

    lock.PushDownTopN(None);

    let child = lock.Children()[0]
        .as_any()
        .downcast_ref::<PushDownProbe>()
        .expect("lock child remains unchanged");
    assert!(
        !child.called,
        "Go LogicalLock.PushDownTopN(nil) does not visit its child"
    );
}
