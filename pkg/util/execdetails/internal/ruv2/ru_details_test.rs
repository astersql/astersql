// Copyright 2026 AsterSQL.
// Copyright 2026 TiKV Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// RUv2 指标与 RUDetails 的 Go 兼容性回归测试。
//
// 覆盖 Prometheus 命名约定、读写 RU 与等待时间的累计、TiFlash 计量，
// 以及原始 protobuf 计数在克隆、合并、排空和整数溢出时的语义。

use super::*;
use prometheus::core::Collector;
use std::time::Duration;

/// 指标名必须保留 Go 侧的 `tidb` namespace 与 `ruv2` subsystem，
/// 否则已有监控面板和告警规则将无法继续匹配时间序列。
#[test]
fn ruv2_prometheus_names_match_go_namespace_and_subsystem() {
    metrics::InitRUV2Metrics();
    assert_eq!(
        metrics::RUV2ResultChunkCells.collect()[0].get_name(),
        "tidb_ruv2_result_chunk_cells"
    );
    assert_eq!(
        metrics::RUV2TiKVCoprocessorWorkTotalCounter("BatchSelection")
            .expect("work total counter")
            .collect()[0]
            .get_name(),
        "tidb_ruv2_tikv_coprocessor_executor_work_total"
    );
}

/// Help 文本参与 Prometheus descriptor 的维度哈希，必须与 Go 指标契约一致；
/// 使用指标名充当 Help 会导致同名指标注册冲突。
#[test]
fn ruv2_prometheus_help_matches_go_descriptors() {
    macro_rules! assert_help {
        ($collector:expr, $expected:literal) => {
            assert_eq!($collector.desc()[0].help, $expected)
        };
    }

    assert_help!(
        metrics::RUV2ResultChunkCells,
        "Counter of result chunk cells for RU v2."
    );
    assert_help!(
        metrics::RUV2ExecutorCounter(1, "BatchPointGetExec").expect("L1 counter"),
        "Counter of executor L1 input/output for RU v2."
    );
    assert_help!(
        metrics::RUV2ExecutorCounter(2, "HashAggExec").expect("L2 counter"),
        "Counter of executor L2 input/output for RU v2."
    );
    assert_help!(
        metrics::RUV2ExecutorCounter(3, "SortExec").expect("L3 counter"),
        "Counter of executor L3 input/output for RU v2."
    );
    assert_help!(
        metrics::RUV2ExecutorL5InsertRows,
        "Counter of insert rows for RU v2."
    );
    assert_help!(
        metrics::RUV2PlanCnt,
        "Counter of plan builder executions for RU v2."
    );
    assert_help!(
        metrics::RUV2PlanDeriveStatsPaths,
        "Counter of derive stats paths for RU v2."
    );
    assert_help!(
        metrics::RUV2ResourceManagerReadCnt,
        "Counter of resource manager read requests for RU v2."
    );
    assert_help!(
        metrics::RUV2ResourceManagerWriteCnt,
        "Counter of resource manager write requests for RU v2."
    );
    assert_help!(
        metrics::RUV2WriteKeys,
        "Counter of commit write keys for RU v2."
    );
    assert_help!(
        metrics::RUV2WriteSize,
        "Shadow counter of commit write size for RU v2."
    );
    assert_help!(
        metrics::RUV2SessionParserTotal,
        "Counter of session parser executions for RU v2."
    );
    assert_help!(metrics::RUV2TxnCnt, "Counter of transactions for RU v2.");
    assert_help!(
        metrics::RUV2TiKVKVEngineCacheMiss,
        "Counter of TiKV KV engine cache miss for RU v2."
    );
    assert_help!(
        metrics::RUV2TiKVCoprocessorExecutorIterations,
        "Counter of TiKV coprocessor executor iterations for RU v2."
    );
    assert_help!(
        metrics::RUV2TiKVCoprocessorResponseBytes,
        "Counter of TiKV coprocessor response bytes for RU v2."
    );
    assert_help!(
        metrics::RUV2TiKVRaftstoreStoreWriteTriggerWB,
        "Counter of TiKV raftstore write trigger WB bytes for RU v2."
    );
    assert_help!(
        metrics::RUV2TiKVStorageProcessedKeysBatchGet,
        "Counter of TiKV storage processed keys (batch get) for RU v2."
    );
    assert_help!(
        metrics::RUV2TiKVStorageProcessedKeysGet,
        "Counter of TiKV storage processed keys (get) for RU v2."
    );
    assert_help!(
        metrics::RUV2TiKVCoprocessorWorkTotalCounter("BatchSelection").expect("work total counter"),
        "Counter of TiKV coprocessor executor work for RU v2."
    );
}

/// 普通更新累计读写 RU 和等待时间，TiFlash 更新还需计入 TiFlash 专属总量；
/// 字符串格式也属于对外兼容契约。
#[test]
fn ru_details_update_and_tiflash_match_go() {
    let details = tikvutil::NewRUDetailsWith(1.0, 2.0, Duration::from_millis(3));
    let mut consumption = resource_manager::Consumption::new();
    consumption.set_r_r_u(4.0);
    consumption.set_w_r_u(5.0);

    details.Update(&consumption, Duration::from_millis(6));
    details.UpdateTiFlash(&consumption);

    assert_eq!(details.RRU(), 9.0);
    assert_eq!(details.WRU(), 12.0);
    assert_eq!(details.RUWaitDuration(), Duration::from_millis(9));
    assert_eq!(details.TiflashRU(), 9.0);
    assert_eq!(details.Clone().TiflashRU(), details.TiflashRU());
    assert_eq!(
        details.String(),
        "RRU:9.000000, WRU:12.000000, WaitDuration:9ms"
    );
}

/// DrainRUV2 必须累加 Go 上游测试覆盖的每类基础计数，并在排空后返回零值消息。
#[test]
fn ru_details_drain_raw_ruv2_matches_go() {
    let details = tikvutil::NewRUDetails();
    let mut first = kvrpcpb::Ruv2::new();
    first.set_read_rpc_count(1);
    first.set_storage_processed_keys_batch_get(2);
    first
        .mut_executor_inputs()
        .set_tikv_coprocessor_executor_work_total_batch_selection(3);
    details.AddRUV2(&first);

    let mut second = kvrpcpb::Ruv2::new();
    second.set_write_rpc_count(4);
    second.set_storage_processed_keys_get(5);
    second.set_raftstore_store_write_trigger_wb_bytes(6);
    second
        .mut_executor_inputs()
        .set_tikv_coprocessor_executor_work_total_batch_selection(7);
    details.AddRUV2(&second);

    let drained = details.DrainRUV2();
    assert_eq!(drained.get_read_rpc_count(), 1);
    assert_eq!(drained.get_write_rpc_count(), 4);
    assert_eq!(drained.get_storage_processed_keys_batch_get(), 2);
    assert_eq!(drained.get_storage_processed_keys_get(), 5);
    assert_eq!(drained.get_raftstore_store_write_trigger_wb_bytes(), 6);
    assert_eq!(
        drained
            .get_executor_inputs()
            .get_tikv_coprocessor_executor_work_total_batch_selection(),
        10
    );
    assert_eq!(details.DrainRUV2(), kvrpcpb::Ruv2::new());
}

/// Clone 生成独立快照，后续增量不能回写原对象；Merge 读取来源但不排空来源，
/// 只有 DrainRUV2 会取走并清空各自暂存的原始计数。
#[test]
fn ru_details_clone_and_merge_raw_ruv2_match_go() {
    let original = tikvutil::NewRUDetails();
    let mut original_raw = kvrpcpb::Ruv2::new();
    original_raw.set_read_rpc_count(1);
    original_raw
        .mut_executor_inputs()
        .set_tikv_coprocessor_executor_work_total_batch_index_scan(2);
    original.AddRUV2(&original_raw);

    let cloned = original.Clone();
    let mut clone_delta = kvrpcpb::Ruv2::new();
    clone_delta.set_write_rpc_count(3);
    cloned.AddRUV2(&clone_delta);

    let original_drained = original.DrainRUV2();
    assert_eq!(original_drained.get_read_rpc_count(), 1);
    assert_eq!(original_drained.get_write_rpc_count(), 0);
    assert_eq!(
        original_drained
            .get_executor_inputs()
            .get_tikv_coprocessor_executor_work_total_batch_index_scan(),
        2
    );

    let cloned_drained = cloned.DrainRUV2();
    assert_eq!(cloned_drained.get_read_rpc_count(), 1);
    assert_eq!(cloned_drained.get_write_rpc_count(), 3);
    assert_eq!(
        cloned_drained
            .get_executor_inputs()
            .get_tikv_coprocessor_executor_work_total_batch_index_scan(),
        2
    );

    let left = tikvutil::NewRUDetails();
    let mut left_raw = kvrpcpb::Ruv2::new();
    left_raw.set_read_rpc_count(5);
    left.AddRUV2(&left_raw);
    let right = tikvutil::NewRUDetails();
    let mut right_raw = kvrpcpb::Ruv2::new();
    right_raw.set_write_rpc_count(7);
    right.AddRUV2(&right_raw);
    left.Merge(&right);

    let merged = left.DrainRUV2();
    assert_eq!(merged.get_read_rpc_count(), 5);
    assert_eq!(merged.get_write_rpc_count(), 7);
    assert_eq!(right.DrainRUV2().get_write_rpc_count(), 7);
}

/// 原始 RUv2 计数沿用 Go `uint64` 的模 2^64 加法，溢出时应回绕而非报错。
#[test]
fn ru_details_raw_counters_wrap_like_go_uint64() {
    let details = tikvutil::NewRUDetails();
    let mut first = kvrpcpb::Ruv2::new();
    first.set_read_rpc_count(u64::MAX);
    details.AddRUV2(&first);
    let mut second = kvrpcpb::Ruv2::new();
    second.set_read_rpc_count(1);
    details.AddRUV2(&second);
    assert_eq!(details.DrainRUV2().get_read_rpc_count(), 0);
}
