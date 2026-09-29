// Copyright 2026 AsterSQL.

use super::ruv2_metrics::*;

#[test]
fn go_merge_20_only_response_bytes_are_collected() {
    let mut raw = kvrpcpb::Ruv2::new();
    raw.set_read_rpc_count(3);
    raw.set_write_rpc_count(5);
    raw.set_kv_engine_cache_miss(7);
    raw.set_coprocessor_executor_iterations(11);
    raw.set_coprocessor_response_bytes(13);
    raw.set_raftstore_store_write_trigger_wb_bytes(17);
    raw.set_storage_processed_keys_batch_get(19);
    raw.set_storage_processed_keys_get(23);
    raw.mut_executor_inputs()
        .set_tikv_coprocessor_executor_work_total_batch_index_scan(29);

    let statement_metrics = NewRUV2Metrics();
    let global_response_bytes = metrics::RUV2TiKVCoprocessorResponseBytes.get();
    UpdateRUV2MetricsFromRUV2(Some(&statement_metrics), Some(&raw));
    assert_eq!(statement_metrics.TiKVCoprocessorResponseBytes(), 13);
    assert_eq!(
        metrics::RUV2TiKVCoprocessorResponseBytes.get(),
        global_response_bytes
    );
}

#[test]
fn go_merge_20_response_bytes_respect_nil_bypass_and_zero() {
    let mut raw = kvrpcpb::Ruv2::new();
    raw.set_coprocessor_response_bytes(4);
    UpdateRUV2MetricsFromRUV2(None, Some(&raw));
    let metrics = NewRUV2Metrics();
    UpdateRUV2MetricsFromRUV2(Some(&metrics), None);
    UpdateRUV2MetricsFromRUV2(Some(&metrics), Some(&kvrpcpb::Ruv2::new()));
    assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 0);

    metrics.SetBypass(true);
    UpdateRUV2MetricsFromRUV2(Some(&metrics), Some(&raw));
    metrics.AddTiKVCoprocessorResponseBytes(7);
    assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 0);

    metrics.SetBypass(false);
    UpdateRUV2MetricsFromRUV2(Some(&metrics), Some(&raw));
    metrics.AddTiKVCoprocessorResponseBytes(-2);
    assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 2);
}

#[test]
fn go_merge_20_ru_details_drain_transfers_only_response_bytes_once() {
    let details = tikvutil::NewRUDetails();
    let mut raw = kvrpcpb::Ruv2::new();
    raw.set_read_rpc_count(5);
    raw.set_coprocessor_response_bytes(8);
    details.AddRUV2(&raw);
    let metrics = NewRUV2Metrics();
    SyncRUV2MetricsFromRUDetails(Some(&metrics), Some(&details));
    SyncRUV2MetricsFromRUDetails(Some(&metrics), Some(&details));
    assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 8);
}
