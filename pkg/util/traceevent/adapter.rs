// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// client-go 与 TiDB traceevent 之间的适配层。
//
// 将 client-go 侧的类别/事件映射到内核 `TraceCategory`，并根据飞行记录器
// （Flight Recorder）与已启用类别，计算下发给 TiKV 客户端的跟踪控制标志。

use crate::flightrecorder::get_flight_recorder;
use crate::traceevent::{
    Context, Field, KV_REQUEST, REGION_CACHE, TIKV_READ_DETAILS, TIKV_REQUEST, TIKV_WRITE_DETAILS,
    TXN_2PC, TXN_LOCK_RESOLVE, TraceCategory, UNKNOWN_CLIENT, get_enabled_categories, is_enabled,
    trace_event,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// client-go 上报的跟踪类别枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientCategory {
    /// 两阶段提交（2PC）相关事件。
    Txn2Pc,
    /// 锁解析（lock resolve）相关事件。
    TxnLockResolve,
    /// KV 请求。
    KvRequest,
    /// Region 缓存（Region 是 TiKV 数据分片单位）。
    RegionCache,
    /// 未知/未来扩展类别，保留原始 u32。
    Other(u32),
}

impl ClientCategory {
    /// 转为 client-go 侧原始类别编号。
    fn raw(self) -> u32 {
        match self {
            Self::Txn2Pc => 0,
            Self::TxnLockResolve => 1,
            Self::KvRequest => 2,
            Self::RegionCache => 3,
            Self::Other(value) => value,
        }
    }
}

/// 下发给 client-go 的跟踪控制位标志集合。
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TraceControlFlags(u32);
impl TraceControlFlags {
    /// 立即落盘日志（命中飞行记录器保留条件时置位）。
    pub const IMMEDIATE_LOG: Self = Self(1 << 0);
    /// 启用 TiKV 请求跟踪。
    pub const TIKV_REQUEST: Self = Self(1 << 1);
    /// 启用 TiKV 写详情跟踪。
    pub const TIKV_WRITE_DETAILS: Self = Self(1 << 2);
    /// 启用 TiKV 读详情跟踪。
    pub const TIKV_READ_DETAILS: Self = Self(1 << 3);
    /// 是否包含指定标志位。
    pub fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 != 0
    }
    /// 按位或合并标志。
    pub fn with(self, flag: Self) -> Self {
        Self(self.0 | flag.0)
    }
    /// 将 TiKV 相关标志映射为内部 `TraceCategory` 位图。
    pub fn category_bits(self) -> u64 {
        let mut bits = 0;
        if self.contains(Self::TIKV_REQUEST) {
            bits |= TIKV_REQUEST.0;
        }
        if self.contains(Self::TIKV_WRITE_DETAILS) {
            bits |= TIKV_WRITE_DETAILS.0;
        }
        if self.contains(Self::TIKV_READ_DETAILS) {
            bits |= TIKV_READ_DETAILS.0;
        }
        bits
    }
}

/// 是否已向 client-go 完成一次注册。
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// 标记已与 client-go 完成回调注册（具体接线由包级集成任务完成）。
pub fn register_with_client_go() {
    // The client-go crate is connected by the package integration task. This
    // task owns the callbacks and records that registration occurred once.
    REGISTERED.store(true, Ordering::Release);
}
/// 查询是否已注册。
pub fn client_go_registered() -> bool {
    REGISTERED.load(Ordering::Acquire)
}

/// 处理 client-go 跟踪事件：映射类别、过滤未启用项并写入 sink。
pub fn handle_client_go_trace_event(
    ctx: &Context,
    category: ClientCategory,
    name: &str,
    mut fields: Vec<Field>,
) {
    let mapped = map_category(category);
    if !is_enabled(mapped) {
        return;
    }
    // 未知类别时附加原始 client_go_category，便于排查未来扩展事件。
    if mapped == UNKNOWN_CLIENT {
        fields.push(Field::u32("client_go_category", category.raw()));
    }
    trace_event(ctx, mapped, name, fields);
}

/// 查询映射后的类别是否已启用。
pub fn handle_client_go_is_category_enabled(category: ClientCategory) -> bool {
    is_enabled(map_category(category))
}

/// 根据已启用类别与飞行记录器，组装 client-go 跟踪控制标志。
pub fn handle_trace_control_extractor(ctx: &Context) -> TraceControlFlags {
    let enabled = get_enabled_categories();
    let mut flags = TraceControlFlags::default();
    if enabled.contains(TIKV_REQUEST) {
        flags = flags.with(TraceControlFlags::TIKV_REQUEST);
    }
    if enabled.contains(TIKV_WRITE_DETAILS) {
        flags = flags.with(TraceControlFlags::TIKV_WRITE_DETAILS);
    }
    if enabled.contains(TIKV_READ_DETAILS) {
        flags = flags.with(TraceControlFlags::TIKV_READ_DETAILS);
    }

    let Some(trace) = ctx.sink() else {
        return flags;
    };
    let Some(recorder) = get_flight_recorder() else {
        return flags;
    };
    // 命中保留条件时要求 client-go 立即写日志。
    if recorder.should_keep(trace.bits()) {
        flags = flags.with(TraceControlFlags::IMMEDIATE_LOG);
    }
    flags
}

/// 将 client-go 类别映射为内核 `TraceCategory`。
pub fn map_category(category: ClientCategory) -> TraceCategory {
    match category {
        ClientCategory::Txn2Pc => TXN_2PC,
        ClientCategory::TxnLockResolve => TXN_LOCK_RESOLVE,
        ClientCategory::KvRequest => KV_REQUEST,
        ClientCategory::RegionCache => REGION_CACHE,
        ClientCategory::Other(_) => UNKNOWN_CLIENT,
    }
}

pub use handle_client_go_is_category_enabled as handleClientGoIsCategoryEnabled;
pub use handle_client_go_trace_event as handleClientGoTraceEvent;
pub use handle_trace_control_extractor as handleTraceControlExtractor;
pub use map_category as mapCategory;
pub use register_with_client_go as RegisterWithClientGo;
