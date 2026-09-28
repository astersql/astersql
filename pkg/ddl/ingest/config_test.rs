// Copyright 2026 AsterSQL.

use crate::config::{
    IngestConfig, adjust_import_memory, cop_read_batch_size, generate_config,
    generate_local_engine_config, try_aggressive_memory,
};
use crate::mem_root::{MemRoot, MemRootImpl};

fn config(engine: i64, writer: i64, workers: usize) -> IngestConfig {
    IngestConfig {
        worker_concurrency: workers,
        range_concurrency: workers / 2,
        engine_memory_cache_size: engine,
        local_writer_memory_cache_size: writer,
        max_open_files: 1024,
        sorted_kv_dir: "/tmp/ddl-ingest".into(),
    }
}

#[test]
fn config_defaults_match_go_backend_config() {
    let cfg = generate_config("/tmp/sort", 3, 1);
    assert_eq!(cfg.worker_concurrency, 6);
    assert_eq!(cfg.range_concurrency, 3);
    assert_eq!(cfg.engine_memory_cache_size, 512 * 1024 * 1024);
    assert_eq!(cfg.local_writer_memory_cache_size, 128 * 1024 * 1024);
    assert_eq!(cfg.sorted_kv_dir, "/tmp/sort");
}

#[test]
fn cop_batch_uses_positive_hint_or_go_default() {
    assert_eq!(cop_read_batch_size(1), 1);
    assert_eq!(cop_read_batch_size(2048), 2048);
    assert_eq!(cop_read_batch_size(0), 10 * 256);
}

#[test]
fn local_engine_config_preserves_go_defaults() {
    let cfg = generate_local_engine_config(42);
    assert_eq!(cfg["ts"], "42");
    assert_eq!(cfg["compact"], "true");
    assert_eq!(
        cfg["compact_threshold"],
        (1024_i64 * 1024 * 1024).to_string()
    );
    assert_eq!(cfg["compact_concurrency"], "4");
    assert_eq!(cfg["block_size"], (16 * 1024).to_string());
    assert_eq!(cfg["keep_sort_dir"], "true");
}

#[test]
fn aggressive_check_matches_go_formula_without_consuming() {
    let mem_root = MemRootImpl::new(1_000);
    mem_root.consume(100);
    let mut cfg = config(400, 100, 4);

    assert!(try_aggressive_memory(&mem_root, &mut cfg));
    assert_eq!(mem_root.current_usage(), 100);
    assert_eq!(cfg, config(400, 100, 4));
}

#[test]
fn adjustment_matches_go_integer_scale_rules() {
    let mem_root = MemRootImpl::new(1_000);
    mem_root.consume(900);
    let mut unchanged = config(400, 100, 4);
    adjust_import_memory(&mem_root, &mut unchanged);
    assert_eq!(unchanged, config(400, 100, 4));

    let mem_root = MemRootImpl::new(500);
    let mut scaled = config(400, 100, 4);
    adjust_import_memory(&mem_root, &mut scaled);
    assert_eq!(scaled.engine_memory_cache_size, 400 / 3);
    assert_eq!(scaled.local_writer_memory_cache_size, 100 / 3);
}
