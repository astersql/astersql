// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TopSQL / TopRU 上报数据汇（DataSink）注册与分发。
//
// DataSink 接收 reporter 产出的 `ReportData`（SQL/Plan 元数据与耗时记录），
// 经 pubsub 或单目标通道外发。TopRU 指按 Request Unit（资源计量单位）聚合的
// 热点查询；注册器维护开关引用计数，在最后一个订阅者离开时关闭采集。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::tipb_protobuf as tipb;
use thiserror::Error;

/// 单个注册器允许挂载的 DataSink 上限。
const MAX_DATA_SINKS: usize = 10;

/// DataSink 注册、发送与配置相关错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DataSinkError {
    #[error("DefaultDataSinkRegisterer closed")]
    RegistererClosed,
    #[error("too many datasinks")]
    TooManyDataSinks,
    #[error("topru config is empty")]
    TopRuConfigEmpty,
    #[error("the channel of pubsub dataSink is full")]
    ChannelFull,
    #[error("pubsub dataSink closed")]
    Closed,
    #[error("pubsub send deadline exceeded")]
    DeadlineExceeded,
    #[error("invalid top ru item interval: {0}")]
    InvalidTopRuInterval(i32),
    #[error("subscriber stream send failed: {0}")]
    Stream(String),
}

/// 一次上报批次：TopSQL 记录、TopRU 记录及 SQL/Plan 元数据。
#[derive(Clone, Debug, Default)]
pub struct ReportData {
    pub data_records: Vec<tipb::TopSqlRecord>,
    pub ru_records: Vec<tipb::TopRuRecord>,
    pub sql_metas: Vec<tipb::SqlMeta>,
    pub plan_metas: Vec<tipb::PlanMeta>,
}

impl ReportData {
    /// 任一字段非空即视为有可上报数据。
    pub fn has_data(&self) -> bool {
        !self.data_records.is_empty()
            || !self.ru_records.is_empty()
            || !self.sql_metas.is_empty()
            || !self.plan_metas.is_empty()
    }
}

/// 订阅端开关：是否启用 TopSQL/TopRU 及 TopRU 采样间隔（秒）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubscriptionConfig {
    pub enable_top_sql: bool,
    pub enable_top_ru: bool,
    pub item_interval: i32,
}

/// 上报数据汇：尝试发送批次，并在 reporter 关闭时回调。
pub trait DataSink: Send + Sync + 'static {
    fn try_send(&self, data: Arc<ReportData>, deadline: Instant) -> Result<(), DataSinkError>;
    fn on_reporter_closing(&self);

    /// 无配置时视为 SingleTarget：默认启用 TopSQL、不参与 TopRU 引用计数。
    fn subscription_config(&self) -> Option<SubscriptionConfig> {
        None
    }
}

/// DataSink 的注册/注销接口。
pub trait DataSinkRegisterer: Send + Sync + 'static {
    fn register(&self, data_sink: Arc<dyn DataSink>) -> Result<(), DataSinkError>;
    fn deregister(&self, data_sink: &Arc<dyn DataSink>);
}

/// 注册器内部可变状态：sink 表与 TopSQL 启用引用计数。
struct RegistererState {
    data_sinks: HashMap<usize, Arc<dyn DataSink>>,
    top_sql_sink_count: usize,
}

/// 默认注册器：互斥保护 sink 集合，关闭时通知全部订阅者并关掉采集开关。
pub struct DefaultDataSinkRegisterer {
    closed: AtomicBool,
    state: Mutex<RegistererState>,
}

impl Default for DefaultDataSinkRegisterer {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultDataSinkRegisterer {
    /// 创建空注册器。
    pub fn new() -> Self {
        Self {
            closed: AtomicBool::new(false),
            state: Mutex::new(RegistererState {
                data_sinks: HashMap::with_capacity(MAX_DATA_SINKS),
                top_sql_sink_count: 0,
            }),
        }
    }

    /// 关闭注册器：清空 sink、按需关闭 TopSQL/TopRU，并回调 `on_reporter_closing`。
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let sinks = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let sinks: Vec<_> = state.data_sinks.drain().map(|(_, sink)| sink).collect();
            // 仍有 TopSQL 订阅时先关掉全局采集开关。
            if state.top_sql_sink_count > 0 {
                crate::topsql_state::DisableTopSQL();
                state.top_sql_sink_count = 0;
            }
            for sink in &sinks {
                if sink
                    .subscription_config()
                    .is_some_and(|config| config.enable_top_ru)
                {
                    crate::topsql_state::DisableTopRU();
                }
            }
            sinks
        };
        // 锁外通知，避免回调再入注册路径死锁。
        for sink in sinks {
            sink.on_reporter_closing();
        }
    }

    /// 当前已注册 DataSink 数量。
    pub fn sink_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_sinks
            .len()
    }

    /// 返回已注册 DataSink 的快照副本。
    pub fn sinks(&self) -> Vec<Arc<dyn DataSink>> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .data_sinks
            .values()
            .cloned()
            .collect()
    }
}

impl DataSinkRegisterer for DefaultDataSinkRegisterer {
    fn register(&self, data_sink: Arc<dyn DataSink>) -> Result<(), DataSinkError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(DataSinkError::RegistererClosed);
        }
        let key = data_sink_key(&data_sink);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        // 加锁后再查一次，避免 close 与 register 竞态。
        if self.closed.load(Ordering::SeqCst) {
            return Err(DataSinkError::RegistererClosed);
        }
        if state.data_sinks.contains_key(&key) {
            return Ok(());
        }
        if state.data_sinks.len() >= MAX_DATA_SINKS {
            return Err(DataSinkError::TooManyDataSinks);
        }

        let config = data_sink.subscription_config();
        // 启用 TopRU 时先写入采样间隔，再打开全局开关。
        if let Some(config) = config
            && config.enable_top_ru
        {
            crate::topsql_state::SetTopRUItemInterval(config.item_interval)
                .map_err(|_| DataSinkError::InvalidTopRuInterval(config.item_interval))?;
            crate::topsql_state::EnableTopRU();
        }

        let enable_top_sql = config.is_none_or(|config| config.enable_top_sql);
        state.data_sinks.insert(key, data_sink);
        if enable_top_sql {
            crate::topsql_state::EnableTopSQL();
            state.top_sql_sink_count += 1;
        }
        Ok(())
    }

    fn deregister(&self, data_sink: &Arc<dyn DataSink>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let key = data_sink_key(data_sink);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let Some(removed) = state.data_sinks.remove(&key) else {
            return;
        };
        let config = removed.subscription_config();
        let enable_top_sql = config.is_none_or(|config| config.enable_top_sql);
        // TopSQL 引用计数归零才关闭全局采集。
        if enable_top_sql {
            state.top_sql_sink_count = state.top_sql_sink_count.saturating_sub(1);
            if state.top_sql_sink_count == 0 {
                crate::topsql_state::DisableTopSQL();
            }
        }
        if config.is_some_and(|config| config.enable_top_ru) {
            crate::topsql_state::DisableTopRU();
        }
    }
}

impl Drop for DefaultDataSinkRegisterer {
    fn drop(&mut self) {
        self.close();
    }
}

/// 以 Arc 指针地址作为 DataSink 去重键。
fn data_sink_key(data_sink: &Arc<dyn DataSink>) -> usize {
    Arc::as_ptr(data_sink) as *const () as usize
}
