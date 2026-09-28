// Copyright 2026 AsterSQL.

use crate::backend::BackendContext;
use crate::disk_root::DiskRoot;
use crate::engine::{Engine, Writer};
use crate::engine_mgr::{
    OPT_CHECK_DUP, OPT_CLEAN_DATA, OPT_CLOSE_ENGINES, finish_and_unregister_engines,
    register_engines,
};
use crate::mem_root::{MemRoot, MemRootImpl};
use std::sync::Arc;

fn backend() -> BackendContext {
    let memory: Arc<dyn MemRoot> = Arc::new(MemRootImpl::new(1_000));
    BackendContext::new(42, memory, DiskRoot::new("/tmp", 1_000, 1_000), None)
}

#[test]
fn unregister_options_match_go_bit_layout() {
    assert_eq!(OPT_CLOSE_ENGINES, 1);
    assert_eq!(OPT_CLEAN_DATA, 2);
    assert_eq!(OPT_CHECK_DUP, 4);
}

#[test]
fn register_engines_forwards_order_uniqueness_and_writer_memory() {
    let mut backend = backend();
    let engines = register_engines(&mut backend, &[20, 10], &[false, true], 64).unwrap();

    assert_eq!(
        engines
            .iter()
            .map(|engine| engine.index_id())
            .collect::<Vec<_>>(),
        [20, 10]
    );
    assert!(!engines[0].unique());
    assert!(engines[1].unique());
    let writer = engines[0].create_writer(7).unwrap();
    assert_eq!(writer.written_bytes(), 0);
}

#[test]
fn successful_unregister_closes_clears_and_honors_cleanup() {
    let mut keep_backend = backend();
    let keep_engine = register_engines(&mut keep_backend, &[10], &[false], 16)
        .unwrap()
        .pop()
        .unwrap();
    let mut writer = keep_engine.create_writer(1).unwrap();
    writer.write_row(b"key", b"value").unwrap();
    drop(writer);

    finish_and_unregister_engines(&mut keep_backend, OPT_CLOSE_ENGINES).unwrap();
    assert!(keep_backend.engines.is_empty());
    assert_eq!(
        keep_engine.rows().get(b"key".as_slice()),
        Some(&b"value".to_vec())
    );

    let mut clean_backend = backend();
    let clean_engine = register_engines(&mut clean_backend, &[10], &[false], 16)
        .unwrap()
        .pop()
        .unwrap();
    let mut writer = clean_engine.create_writer(1).unwrap();
    writer.write_row(b"key", b"value").unwrap();
    drop(writer);

    finish_and_unregister_engines(&mut clean_backend, OPT_CLEAN_DATA).unwrap();
    assert!(clean_backend.engines.is_empty());
    assert!(clean_engine.rows().is_empty());
}

#[test]
fn duplicate_error_keeps_go_closed_engines_registered() {
    let mut backend = backend();
    let engine = register_engines(&mut backend, &[10], &[true], 16)
        .unwrap()
        .pop()
        .unwrap();
    let mut writer = engine.create_writer(1).unwrap();
    writer.write_row(b"key-1", b"duplicate").unwrap();
    writer.write_row(b"key-2", b"duplicate").unwrap();
    drop(writer);

    let error = finish_and_unregister_engines(&mut backend, OPT_CHECK_DUP).unwrap_err();

    assert!(error.contains("duplicate rows for index 10"));
    assert!(backend.engines.contains_key(&10));
    match engine.create_writer(2) {
        Ok(_) => panic!("Go closes engines before returning a duplicate error"),
        Err(error) => assert_eq!(error, "engine closed"),
    }
}
