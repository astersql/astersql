// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// `execdetails` 内部 util 支撑 crate：上下文键、流量明细、执行明细与 RU/RUv2 指标桩。
//
// Context 用 TypeId 模拟 Go `context.WithValue`；RU（Request Unit）是资源组计费单位；
// RUv2 是第二代资源计量指标，可挂在语句执行明细上。

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

use std::sync::Mutex;

/// 轻量 Context：用 TypeId 键存任意值，模拟 Go context.Value。
pub mod context {
    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    /// 类型键到共享值的映射。
    pub struct Context {
        values: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    }

    impl Context {
        /// 按键类型取出并克隆值。
        pub fn value<T, K>(&self, _key: &K) -> Option<T>
        where
            T: Any + Clone + Send + Sync,
            K: Any + Send + Sync,
        {
            self.values
                .get(&TypeId::of::<K>())
                .and_then(|value| value.downcast_ref::<T>())
                .cloned()
        }
    }

    /// 写入键值对，返回更新后的 Context。
    pub fn WithValue<T, K>(mut ctx: Context, _key: &K, value: T) -> Context
    where
        T: Any + Send + Sync,
        K: Any + Send + Sync,
    {
        ctx.values.insert(TypeId::of::<K>(), Arc::new(value));
        ctx
    }
}

/// 执行/流量/RU 明细结构及测试辅助方法。
pub mod util {
    use std::sync::atomic::{AtomicI64, Ordering};

    /// Context 中存放 ExecDetails 的键类型。
    pub struct execDetailsKeyType;
    /// Context 中存放 RUDetails 的键类型。
    pub struct ruDetailsKeyType;

    /// 全局 ExecDetails 上下文键。
    pub static ExecDetailsKey: execDetailsKeyType = execDetailsKeyType;
    /// 全局 RUDetails 上下文键。
    pub static RUDetailsCtxKey: ruDetailsKeyType = ruDetailsKeyType;

    #[derive(Default)]
    /// KV/MPP 收发字节统计（含跨可用区）。MPP 为 TiFlash 分布式执行流量。
    pub struct TrafficDetails {
        pub UnpackedBytesSentKVTotal: AtomicI64,
        pub UnpackedBytesReceivedKVTotal: AtomicI64,
        pub UnpackedBytesSentKVCrossZone: AtomicI64,
        pub UnpackedBytesReceivedKVCrossZone: AtomicI64,
        pub UnpackedBytesSentMPPTotal: AtomicI64,
        pub UnpackedBytesReceivedMPPTotal: AtomicI64,
        pub UnpackedBytesSentMPPCrossZone: AtomicI64,
        pub UnpackedBytesReceivedMPPCrossZone: AtomicI64,
    }

    #[derive(Default)]
    /// 语句级执行明细：退避、等待 KV/PD 与流量。
    pub struct ExecDetails {
        pub BackoffCount: AtomicI64,
        pub BackoffDuration: AtomicI64,
        pub WaitKVRespDuration: AtomicI64,
        pub WaitPDRespDuration: AtomicI64,
        pub TrafficDetails: TrafficDetails,
    }

    impl ExecDetails {
        /// 测试：一次性写入全部计数器字段。
        pub fn set_all_for_test(&self, values: [i64; 12]) {
            let fields = [
                &self.BackoffCount,
                &self.BackoffDuration,
                &self.WaitKVRespDuration,
                &self.WaitPDRespDuration,
                &self.TrafficDetails.UnpackedBytesSentKVTotal,
                &self.TrafficDetails.UnpackedBytesReceivedKVTotal,
                &self.TrafficDetails.UnpackedBytesSentKVCrossZone,
                &self.TrafficDetails.UnpackedBytesReceivedKVCrossZone,
                &self.TrafficDetails.UnpackedBytesSentMPPTotal,
                &self.TrafficDetails.UnpackedBytesReceivedMPPTotal,
                &self.TrafficDetails.UnpackedBytesSentMPPCrossZone,
                &self.TrafficDetails.UnpackedBytesReceivedMPPCrossZone,
            ];
            for (field, value) in fields.into_iter().zip(values) {
                field.store(value, Ordering::Relaxed);
            }
        }

        /// 测试：读出全部计数器字段。
        pub fn values_for_test(&self) -> [i64; 12] {
            [
                self.BackoffCount.load(Ordering::Relaxed),
                self.BackoffDuration.load(Ordering::Relaxed),
                self.WaitKVRespDuration.load(Ordering::Relaxed),
                self.WaitPDRespDuration.load(Ordering::Relaxed),
                self.TrafficDetails
                    .UnpackedBytesSentKVTotal
                    .load(Ordering::Relaxed),
                self.TrafficDetails
                    .UnpackedBytesReceivedKVTotal
                    .load(Ordering::Relaxed),
                self.TrafficDetails
                    .UnpackedBytesSentKVCrossZone
                    .load(Ordering::Relaxed),
                self.TrafficDetails
                    .UnpackedBytesReceivedKVCrossZone
                    .load(Ordering::Relaxed),
                self.TrafficDetails
                    .UnpackedBytesSentMPPTotal
                    .load(Ordering::Relaxed),
                self.TrafficDetails
                    .UnpackedBytesReceivedMPPTotal
                    .load(Ordering::Relaxed),
                self.TrafficDetails
                    .UnpackedBytesSentMPPCrossZone
                    .load(Ordering::Relaxed),
                self.TrafficDetails
                    .UnpackedBytesReceivedMPPCrossZone
                    .load(Ordering::Relaxed),
            ]
        }
    }

    #[derive(Default)]
    /// RU 明细桩：用 pending 累加待同步的计量值。
    pub struct RUDetails {
        pending: AtomicI64,
    }

    impl RUDetails {
        /// 测试：累加 pending RU。
        pub fn add_for_test(&self, value: i64) {
            self.pending.fetch_add(value, Ordering::Relaxed);
        }

        /// 测试：取出并清零 pending RU。
        pub fn drain_for_test(&self) -> i64 {
            self.pending.swap(0, Ordering::Relaxed)
        }
    }

    /// 创建空的 RUDetails。
    pub fn NewRUDetails() -> std::sync::Arc<RUDetails> {
        std::sync::Arc::new(RUDetails::default())
    }
}

/// StmtExecDetails 的上下文键类型。
pub struct stmtExecDetailKeyType;
/// RUV2Metrics 的上下文键类型。
pub struct ruv2MetricsKeyType;

/// 语句执行明细上下文键。
pub static StmtExecDetailKey: stmtExecDetailKeyType = stmtExecDetailKeyType;
/// 直接挂在 Context 上的 RUv2 指标键。
pub static RUV2MetricsCtxKey: ruv2MetricsKeyType = ruv2MetricsKeyType;
/// 单条查询保留的明细条数上限。
pub const MaxDetailsNumsForOneQuery: i32 = 1000;

#[derive(Default)]
/// 语句级 RUv2 指标桩（测试用 total）。
pub struct RUV2Metrics {
    total: std::sync::atomic::AtomicI64,
}

impl RUV2Metrics {
    /// 测试：读取累计 total。
    pub fn total_for_test(&self) -> i64 {
        self.total.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Default)]
/// 语句执行明细：写回客户端耗时与可选 RUv2 指标。
pub struct StmtExecDetails {
    pub WriteSQLRespDuration: std::time::Duration,
    pub metrics: Mutex<Option<Arc<RUV2Metrics>>>,
}

impl StmtExecDetails {
    /// 若尚未挂载则创建 RUv2 指标并返回。
    pub fn ensureRUV2Metrics(&self) -> Arc<RUV2Metrics> {
        let mut metrics = self.metrics.lock().expect("metrics lock poisoned");
        metrics
            .get_or_insert_with(|| Arc::new(RUV2Metrics::default()))
            .clone()
    }

    /// 获取已挂载的 RUv2 指标（可能为空）。
    pub fn getRUV2Metrics(&self) -> Option<Arc<RUV2Metrics>> {
        self.metrics.lock().expect("metrics lock poisoned").clone()
    }

    /// 设置/替换 RUv2 指标。
    pub fn setRUV2Metrics(&self, metrics: Arc<RUV2Metrics>) {
        *self.metrics.lock().expect("metrics lock poisoned") = Some(metrics);
    }
}

/// 从 Context 取 RUv2：优先 StmtExecDetails，其次独立键。
pub fn RUV2MetricsFromContext(ctx: &context::Context) -> Option<Arc<RUV2Metrics>> {
    if let Some(details) = ctx.value::<Arc<StmtExecDetails>, _>(&StmtExecDetailKey) {
        if let Some(metrics) = details.getRUV2Metrics() {
            return Some(metrics);
        }
    }
    ctx.value::<Arc<RUV2Metrics>, _>(&RUV2MetricsCtxKey)
}

/// 将 RUDetails 中 pending 值排空并累加到 RUv2 指标。
pub fn SyncRUV2MetricsFromRUDetails(
    metrics: Option<&RUV2Metrics>,
    details: Option<&util::RUDetails>,
) {
    if let (Some(metrics), Some(details)) = (metrics, details) {
        metrics.total.fetch_add(
            details.drain_for_test(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../util.rs"));

#[cfg(test)]
#[path = "../../util_3_aster_unit_test.rs"]
mod util_3_aster_unit_test;
