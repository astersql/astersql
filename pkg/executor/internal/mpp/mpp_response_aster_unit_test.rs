// Copyright 2026 AsterSQL.

// `mppResponse` 与 `kv::ResultSubset` / 恢复缓冲内存契约的单元测试。
//
// 验证成功包的数据、MemSize（含 CopRuntimeStats 时加上 ExecDetails）、
// RespTime；错误响应无数据且内存为 0；以及 HoldResult/PopFront 对
// 父 Tracker 的占用与释放是否与 MemSize 一致。

use std::mem;
use std::time::Duration;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_util_execdetails::execdetails::{CopRuntimeStats, ExecDetails};
use astersql_util_memory as memory;
use kvproto::mpp::MppDataPacket;
use protobuf::Message;

use crate::local_mpp_coordinator::mppResponse;
use crate::recovery_handler::NewRecoveryHandler;

/// 成功响应实现 ResultSubset：数据、空 StartKey、MemSize 与 RespTime。
#[test]
fn response_implements_the_real_kv_result_subset_contract() {
    let mut packet = MppDataPacket::new();
    packet.set_data(vec![1, 2, 3, 4].into());
    let packet_size = packet.compute_size() as i64;
    let response_time = Duration::from_millis(17);
    let response = mppResponse::new(packet, Some(CopRuntimeStats::default()), response_time);

    let subset: &dyn kv::ResultSubset = &response;
    assert_eq!(subset.GetData(), &[1, 2, 3, 4]);
    assert_eq!(subset.GetStartKey(), kv::Key::default());
    // MemSize = protobuf 包大小 + 可选 CopRuntimeStats 对应的 ExecDetails。
    assert_eq!(
        subset.MemSize(),
        packet_size + mem::size_of::<ExecDetails>() as i64
    );
    assert_eq!(subset.RespTime(), response_time);
    assert!(response.GetCopRuntimeStats().is_some());
    assert!(response.Error().is_none());
}

/// 恢复缓冲 HoldResult/PopFront 时，父 Tracker 占用与 MemSize 一致。
#[test]
fn response_uses_the_same_memory_contract_inside_the_recovery_buffer() {
    let mut packet = MppDataPacket::new();
    packet.set_data(vec![9, 8, 7].into());
    let packet_size = packet.compute_size() as i64;
    let response = mppResponse::new(packet, None, Duration::ZERO);

    let mut parent = memory::tracker::NewTracker(77, 0);
    let mut recovery = NewRecoveryHandler(false, 1, true, &mut parent);
    // 持有响应后父 Tracker 应计入 packet 内存。
    recovery.HoldResult(Box::new(response));
    assert_eq!(parent.BytesConsumed(), packet_size);

    let response = recovery.PopFrontResp().expect("buffered response exists");
    assert_eq!(response.MemSize(), packet_size);
    // 弹出后占用归零。
    assert_eq!(parent.BytesConsumed(), 0);
}

/// 错误响应无包数据，MemSize 为 0，但保留错误原因与 RespTime。
#[test]
fn error_response_has_no_data_or_packet_memory() {
    let response =
        mppResponse::from_error(errors::New("MPP stream failed"), Duration::from_secs(1));
    let subset: &dyn kv::ResultSubset = &response;

    assert!(subset.GetData().is_empty());
    assert_eq!(subset.MemSize(), 0);
    assert_eq!(subset.RespTime(), Duration::from_secs(1));
    assert_eq!(
        response
            .Error()
            .expect("error response retains cause")
            .to_string(),
        "MPP stream failed"
    );
    assert!(response.GetCopRuntimeStats().is_none());
}
