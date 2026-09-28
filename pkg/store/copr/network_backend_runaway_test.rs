// Copyright 2026 AsterSQL.

use kvproto::{coprocessor, kvrpcpb};

#[test]
fn real_cop_response_scan_details_reach_runaway_processed_key_threshold() {
    let mut scan = kvrpcpb::ScanDetailV2::new();
    scan.set_processed_versions(9);
    let mut details = kvrpcpb::ExecDetailsV2::new();
    details.set_scan_detail_v2(scan);
    let mut response = coprocessor::Response::new();
    response.set_exec_details_v2(details);
    let protocol = super::pb_response(response, &super::KeyCodec::v1()).unwrap();
    assert_eq!(protocol.scanned_keys, 9);
}

#[test]
fn legacy_and_store_batch_scan_details_keep_each_processed_key_count() {
    let mut legacy_scan = kvrpcpb::ScanInfo::new();
    legacy_scan.set_processed(3);
    let mut legacy_detail = kvrpcpb::ScanDetail::new();
    legacy_detail.set_write(legacy_scan);
    let mut legacy_exec = kvrpcpb::ExecDetails::new();
    legacy_exec.set_scan_detail(legacy_detail);
    let mut legacy = coprocessor::Response::new();
    legacy.set_exec_details(legacy_exec);
    assert_eq!(
        super::pb_response(legacy, &super::KeyCodec::v1())
            .unwrap()
            .scanned_keys,
        3
    );

    let mut child_scan = kvrpcpb::ScanDetailV2::new();
    child_scan.set_processed_versions(5);
    let mut child_exec = kvrpcpb::ExecDetailsV2::new();
    child_exec.set_scan_detail_v2(child_scan);
    let mut child = coprocessor::StoreBatchTaskResponse::new();
    child.set_task_id(12);
    child.set_exec_details_v2(child_exec);
    let mut parent = coprocessor::Response::new();
    parent.set_batch_responses(protobuf::RepeatedField::from_vec(vec![child]));
    let protocol = super::pb_response(parent, &super::KeyCodec::v1()).unwrap();
    assert_eq!(protocol.batch_responses.get(&12).unwrap().scanned_keys, 5);
}
