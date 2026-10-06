// Copyright 2026 AsterSQL.

// RUv2 指标与 TiFlash 统计的内部 crate 入口。
//
// 提供 Prometheus 计数器、tipb 生成代码、KV/资源管理 protobuf 再导出，
// 以及 `tikvutil::RUDetails` 对原始 RUv2/TiKV/TiFlash RU 的累加与排空。
// RU（Request Unit）衡量读写资源消耗；RUv2 为第二代细粒度计数。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    renamed_and_removed_lints,
    static_mut_refs
)]

use std::sync::Mutex;
use std::sync::atomic::AtomicI64;

pub mod kvrpcpb {
    pub use resourcegrouptag_dependency::kvproto::kvrpcpb::*;
}

/// 再导出资源组 protobuf（含 Consumption）。
pub mod resource_manager {
    pub use resourcegrouptag_dependency::kvproto::resource_manager::*;
}

#[allow(clippy::all, unknown_lints)]
/// build.rs 生成的 tipb 绑定。
pub mod tipb {
    include!(concat!(env!("OUT_DIR"), "/tipb/mod.rs"));
}

/// 测试用空 Context 桩。
pub mod context {
    #[derive(Default)]
    pub struct Context;

    impl Context {
        pub fn value<T: 'static>(&self, _key: impl Sized + 'static) -> Option<T> {
            None
        }
    }
}

#[derive(Clone, Copy)]
/// 语句执行明细上下文键类型。
pub struct StmtExecDetailKeyType;
/// 语句执行明细上下文键。
pub static StmtExecDetailKey: StmtExecDetailKeyType = StmtExecDetailKeyType;

#[derive(Default)]
/// 语句执行明细桩（本 crate 内 getRUV2Metrics 恒为 None）。
pub struct StmtExecDetails;

impl StmtExecDetails {
    pub fn getRUV2Metrics(&self) -> Option<RUV2Metrics> {
        None
    }
}

/// TiKV 侧提交/执行/RU 明细类型。
pub mod tikvutil {
    use super::*;
    use std::time::Duration;

    #[derive(Default)]
    /// 提交阶段写入键数与字节数。
    pub struct CommitDetails {
        pub WriteKeys: u64,
        pub WriteSize: u64,
    }

    #[derive(Default)]
    /// MPP 收发字节（含跨可用区）。
    pub struct ExecDetails {
        pub UnpackedBytesSentMPPCrossZone: AtomicI64,
        pub UnpackedBytesSentMPPTotal: AtomicI64,
        pub UnpackedBytesReceivedMPPCrossZone: AtomicI64,
        pub UnpackedBytesReceivedMPPTotal: AtomicI64,
    }

    #[derive(Default)]
    /// 聚合原始 RUv2、TiKV RUv2 与 TiFlash RU。
    pub struct RUDetails {
        ruv2: Mutex<kvrpcpb::Ruv2>,
        read_ru: Mutex<f64>,
        write_ru: Mutex<f64>,
        ru_wait_duration: Mutex<Duration>,
        tikv_ruv2: Mutex<f64>,
        tiflash_ru: Mutex<f64>,
    }

    impl RUDetails {
        /// 合并一批原始 RUv2 计数（RPC、缓存未命中、Cop 迭代等）。
        pub fn AddRUV2(&self, incoming: &kvrpcpb::Ruv2) {
            let mut pending = self.ruv2.lock().expect("ruv2 lock poisoned");
            let read_rpc_count = pending
                .get_read_rpc_count()
                .wrapping_add(incoming.get_read_rpc_count());
            let write_rpc_count = pending
                .get_write_rpc_count()
                .wrapping_add(incoming.get_write_rpc_count());
            let kv_engine_cache_miss = pending
                .get_kv_engine_cache_miss()
                .wrapping_add(incoming.get_kv_engine_cache_miss());
            let coprocessor_executor_iterations = pending
                .get_coprocessor_executor_iterations()
                .wrapping_add(incoming.get_coprocessor_executor_iterations());
            let coprocessor_response_bytes = pending
                .get_coprocessor_response_bytes()
                .wrapping_add(incoming.get_coprocessor_response_bytes());
            let raftstore_store_write_trigger_wb_bytes = pending
                .get_raftstore_store_write_trigger_wb_bytes()
                .wrapping_add(incoming.get_raftstore_store_write_trigger_wb_bytes());
            let storage_processed_keys_batch_get = pending
                .get_storage_processed_keys_batch_get()
                .wrapping_add(incoming.get_storage_processed_keys_batch_get());
            let storage_processed_keys_get = pending
                .get_storage_processed_keys_get()
                .wrapping_add(incoming.get_storage_processed_keys_get());
            pending.set_read_rpc_count(read_rpc_count);
            pending.set_write_rpc_count(write_rpc_count);
            pending.set_kv_engine_cache_miss(kv_engine_cache_miss);
            pending.set_coprocessor_executor_iterations(coprocessor_executor_iterations);
            pending.set_coprocessor_response_bytes(coprocessor_response_bytes);
            pending
                .set_raftstore_store_write_trigger_wb_bytes(raftstore_store_write_trigger_wb_bytes);
            pending.set_storage_processed_keys_batch_get(storage_processed_keys_batch_get);
            pending.set_storage_processed_keys_get(storage_processed_keys_get);

            // 同步 Coprocessor 各 batch 算子的 work_total。
            let source = incoming.get_executor_inputs();
            let target = pending.mut_executor_inputs();
            target.set_tikv_coprocessor_executor_work_total_batch_index_scan(
                target
                    .get_tikv_coprocessor_executor_work_total_batch_index_scan()
                    .wrapping_add(
                        source.get_tikv_coprocessor_executor_work_total_batch_index_scan(),
                    ),
            );
            target.set_tikv_coprocessor_executor_work_total_batch_table_scan(
                target
                    .get_tikv_coprocessor_executor_work_total_batch_table_scan()
                    .wrapping_add(
                        source.get_tikv_coprocessor_executor_work_total_batch_table_scan(),
                    ),
            );
            target.set_tikv_coprocessor_executor_work_total_batch_selection(
                target
                    .get_tikv_coprocessor_executor_work_total_batch_selection()
                    .wrapping_add(
                        source.get_tikv_coprocessor_executor_work_total_batch_selection(),
                    ),
            );
            target.set_tikv_coprocessor_executor_work_total_batch_top_n(
                target
                    .get_tikv_coprocessor_executor_work_total_batch_top_n()
                    .wrapping_add(source.get_tikv_coprocessor_executor_work_total_batch_top_n()),
            );
            target.set_tikv_coprocessor_executor_work_total_batch_limit(
                target
                    .get_tikv_coprocessor_executor_work_total_batch_limit()
                    .wrapping_add(source.get_tikv_coprocessor_executor_work_total_batch_limit()),
            );
            target.set_tikv_coprocessor_executor_work_total_batch_simple_aggr(
                target
                    .get_tikv_coprocessor_executor_work_total_batch_simple_aggr()
                    .wrapping_add(
                        source.get_tikv_coprocessor_executor_work_total_batch_simple_aggr(),
                    ),
            );
            target.set_tikv_coprocessor_executor_work_total_batch_fast_hash_aggr(
                target
                    .get_tikv_coprocessor_executor_work_total_batch_fast_hash_aggr()
                    .wrapping_add(
                        source.get_tikv_coprocessor_executor_work_total_batch_fast_hash_aggr(),
                    ),
            );
        }

        /// 取出并清空已累计的原始 RUv2。
        pub fn DrainRUV2(&self) -> kvrpcpb::Ruv2 {
            std::mem::take(&mut *self.ruv2.lock().expect("ruv2 lock poisoned"))
        }

        /// 累加 TiKV 侧已换算的 RUv2。
        pub fn AddTiKVRUV2(&self, delta: f64) {
            if delta == 0.0 {
                return;
            }
            *self.tikv_ruv2.lock().expect("tikv RUv2 lock poisoned") += delta;
        }

        /// 当前 TiKV RUv2。
        pub fn TiKVRUV2(&self) -> f64 {
            *self.tikv_ruv2.lock().expect("tikv RUv2 lock poisoned")
        }

        /// 按 Consumption 的读写 RU 更新 TiFlash 合计。
        pub fn UpdateTiFlash(&self, consumption: &resource_manager::Consumption) {
            let read = consumption.get_r_r_u();
            let write = consumption.get_w_r_u();
            *self.read_ru.lock().expect("read RU lock poisoned") += read;
            *self.write_ru.lock().expect("write RU lock poisoned") += write;
            *self.tiflash_ru.lock().expect("tiflash RU lock poisoned") += read + write;
        }

        /// 合并另一份 RUDetails 的全部计量值与待排空原始 RUv2。
        pub fn Merge(&self, other: &RUDetails) {
            let read = other.RRU();
            let write = other.WRU();
            let wait = other.RUWaitDuration();
            let tikv = other.TiKVRUV2();
            let tiflash = other.TiFlashRU();
            let raw = other.ruv2.lock().expect("ruv2 lock poisoned").clone();
            *self.read_ru.lock().expect("read RU lock poisoned") += read;
            *self.write_ru.lock().expect("write RU lock poisoned") += write;
            *self
                .ru_wait_duration
                .lock()
                .expect("RU wait duration lock poisoned") += wait;
            *self.tikv_ruv2.lock().expect("tikv RUv2 lock poisoned") += tikv;
            *self.tiflash_ru.lock().expect("tiflash RU lock poisoned") += tiflash;
            self.AddRUV2(&raw);
        }

        /// 当前 TiFlash RU。
        pub fn TiFlashRU(&self) -> f64 {
            *self.tiflash_ru.lock().expect("tiflash RU lock poisoned")
        }

        /// TiFlashRU 的别名（拼写兼容）。
        pub fn TiflashRU(&self) -> f64 {
            self.TiFlashRU()
        }

        /// 当前读取 RU。
        pub fn RRU(&self) -> f64 {
            *self.read_ru.lock().expect("read RU lock poisoned")
        }

        /// 当前写入 RU。
        pub fn WRU(&self) -> f64 {
            *self.write_ru.lock().expect("write RU lock poisoned")
        }

        /// 当前等待资源组配额的总时长。
        pub fn RUWaitDuration(&self) -> Duration {
            *self
                .ru_wait_duration
                .lock()
                .expect("RU wait duration lock poisoned")
        }

        /// 累加资源组读写 RU 与等待时长。
        pub fn Update(&self, consumption: &resource_manager::Consumption, wait_duration: Duration) {
            *self.read_ru.lock().expect("read RU lock poisoned") += consumption.get_r_r_u();
            *self.write_ru.lock().expect("write RU lock poisoned") += consumption.get_w_r_u();
            *self
                .ru_wait_duration
                .lock()
                .expect("RU wait duration lock poisoned") += wait_duration;
        }

        /// 返回与当前值互不共享可变状态的快照。
        pub fn Clone(&self) -> RUDetails {
            RUDetails {
                ruv2: Mutex::new(self.ruv2.lock().expect("ruv2 lock poisoned").clone()),
                read_ru: Mutex::new(self.RRU()),
                write_ru: Mutex::new(self.WRU()),
                ru_wait_duration: Mutex::new(self.RUWaitDuration()),
                tikv_ruv2: Mutex::new(self.TiKVRUV2()),
                tiflash_ru: Mutex::new(self.TiFlashRU()),
            }
        }

        /// Go `fmt.Stringer` 兼容摘要。
        pub fn String(&self) -> String {
            let wait = self.RUWaitDuration();
            let wait_text = if wait.is_zero() {
                "0s".to_string()
            } else {
                format!("{wait:?}")
            };
            format!(
                "RRU:{:.6}, WRU:{:.6}, WaitDuration:{wait_text}",
                self.RRU(),
                self.WRU()
            )
        }
    }

    /// 创建空的 RUDetails。
    pub fn NewRUDetails() -> RUDetails {
        RUDetails::default()
    }

    /// 创建带初始读写 RU 与等待时长的 RUDetails（测试兼容 API）。
    pub fn NewRUDetailsWith(rru: f64, wru: f64, wait_duration: Duration) -> RUDetails {
        RUDetails {
            read_ru: Mutex::new(rru),
            write_ru: Mutex::new(wru),
            ru_wait_duration: Mutex::new(wait_duration),
            ..RUDetails::default()
        }
    }
}

/// protobuf 错误别名。
pub mod error {
    pub type Error = protobuf::ProtobufError;
}

mod ruv2_metrics {
    use crate::*;
    include!("../../ruv2_metrics.rs");
}
pub use ruv2_metrics::*;

mod tiflash_stats {
    use crate::*;
    include!("../../tiflash_stats.rs");
}
pub use tiflash_stats::*;
