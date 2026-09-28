// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use crate::backend::BackendContext;
use crate::checkpoint::{CheckpointManager, MemoryCheckpointStorage};
use crate::disk_root::DiskRoot;
use crate::engine::Engine;
use crate::mem_root::{MemRoot, MemRootImpl};
use crate::mock::{MockBackendContext, MockEngineInfo};

fn backend_with_checkpoint() -> MockBackendContext {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    let checkpoint = CheckpointManager::new(storage, b"a-start".to_vec(), 99, "node-1")
        .expect("create checkpoint");
    let memory: Arc<dyn MemRoot> = Arc::new(MemRootImpl::new(1_000));
    MockBackendContext::new(BackendContext::new(
        42,
        memory,
        DiskRoot::new("/tmp", 1_000, 1_000),
        Some(checkpoint),
    ))
}

#[test]
fn mock_backend_forwards_the_complete_checkpoint_contract() {
    let mut mock = backend_with_checkpoint();
    assert_eq!(mock.next_start_key(), b"a-start");
    assert_eq!(mock.total_key_count(), 0);
    assert_eq!(mock.import_ts(), 99);

    mock.add_chunk(0, b"end".to_vec());
    mock.update_chunk(0, 2, true);
    mock.finish_chunk(0, 2);
    assert_eq!(mock.written_chunks, [(0, 2)]);
    assert_eq!(mock.total_key_count(), 2);

    mock.advance_watermark(true).expect("advance watermark");
    assert_eq!(mock.next_start_key(), b"end");
}

#[test]
fn hook_replaces_the_default_write_path_like_go() {
    let engine = MockEngineInfo::new(7);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let hook_observed = Arc::clone(&observed);
    engine.set_hook(Arc::new(move |key, value| {
        hook_observed
            .lock()
            .unwrap()
            .push((key.to_vec(), value.to_vec()));
    }));
    let mut writer = engine.create_writer(1).expect("create writer");

    writer.write_row(b"key", b"value").expect("write row");

    assert_eq!(
        *observed.lock().unwrap(),
        [(b"key".to_vec(), b"value".to_vec())]
    );
    assert!(engine.rows().is_empty());
    assert_eq!(writer.written_bytes(), 0);
}

#[test]
fn mock_engine_close_is_a_noop_like_go() {
    let engine = MockEngineInfo::new(7);
    let mut writer = engine.create_writer(1).expect("create writer");
    writer.write_row(b"key", b"value").expect("write row");

    engine.close(true);

    assert_eq!(engine.rows(), [(b"key".to_vec(), b"value".to_vec())]);
}
