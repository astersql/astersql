// Copyright 2026 AsterSQL.

use crate::backend::{BackendContext, FlushDecision};
use crate::disk_root::DiskRoot;
use crate::mem_root::{MemRoot, MemRootImpl};
use std::sync::Arc;

fn backend(memory_quota: i64) -> (BackendContext, Arc<MemRootImpl>) {
    let memory = Arc::new(MemRootImpl::new(memory_quota));
    let mem_root: Arc<dyn MemRoot> = memory.clone();
    (
        BackendContext::new(42, mem_root, DiskRoot::new("/tmp", 1_000, 1_000), None),
        memory,
    )
}

#[test]
fn register_is_idempotent_and_rejects_partial_overlap_like_go() {
    let (mut backend, _) = backend(1_000);
    let first = backend.register(&[10, 20], &[true, false], 16).unwrap();

    let repeated = backend.register(&[10, 20], &[false, true], 128).unwrap();
    assert!(Arc::ptr_eq(&first[0], &repeated[0]));
    assert!(Arc::ptr_eq(&first[1], &repeated[1]));
    assert!(backend.register(&[10, 30], &[true, true], 16).is_err());
    assert_eq!(backend.engines.len(), 2);
}

#[test]
fn memory_pressure_alone_does_not_trigger_go_flush_policy() {
    let (backend, memory) = backend(100);
    memory.consume(80);

    assert_eq!(backend.check_flush(), FlushDecision::None);
}
