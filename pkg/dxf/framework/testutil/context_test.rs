// Copyright 2026 AsterSQL.

use super::*;
use std::sync::Arc;

struct Runtime;

impl DxfRuntime for Runtime {
    fn set_node_resource(&self, resource: NodeResource) -> Result<NodeResource, DxfError> {
        Ok(resource)
    }

    fn start_executor(&self, _: &str, _: NodeResource) -> Result<(), DxfError> {
        Ok(())
    }

    fn stop_executor(&self, _: &str) -> Result<(), DxfError> {
        Ok(())
    }

    fn cancel_executor(&self, _: &str) -> Result<(), DxfError> {
        Ok(())
    }

    fn start_scheduler(&self, _: &str, _: NodeResource) -> Result<(), DxfError> {
        Ok(())
    }

    fn stop_scheduler(&self, _: &str) -> Result<(), DxfError> {
        Ok(())
    }

    fn cancel_scheduler(&self, _: &str) -> Result<(), DxfError> {
        Ok(())
    }

    fn update_live_executor_ids(&self, _: &[String]) -> Result<(), DxfError> {
        Ok(())
    }

    fn set_check_intervals(&self, intervals: CheckIntervals) -> Result<CheckIntervals, DxfError> {
        Ok(intervals)
    }
}

#[test]
fn recycled_node_id_pool_matches_go_capacity() {
    let context = NewTestDXFContext(Arc::new(Runtime), 101, 1, false).unwrap();

    context.ScaleIn(101).unwrap();
    context.ScaleOut(101).unwrap();

    assert_eq!(context.GetNodeIDByIdx(100), ":4101");
}
