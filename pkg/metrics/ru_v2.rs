// Copyright 2026 PingCAP, Inc.
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

// RU v2（Request Unit 第二版）计费相关 Prometheus 指标。
//
// RU 将 CPU、读写、执行器开销等折算为统一请求单元，用于资源组配额与计费。
// 本文件构造计数器、热点标签缓存与按 executor/TiKV coprocessor 标签取句柄的辅助函数，
// 不执行实际计费或访问 TiKV。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;
use std::sync::Once;

// 仅构造 Prometheus 指标描述与句柄，不会注册采集器、连接数据库、访问 TiKV 或执行请求计费。

pub const LblRUV2Unit: &str = "unit";
pub const LblRUV2UnitCPUWork: &str = "cpu_work";
pub const LblRUV2UnitScanBytes: &str = "scan_bytes";
pub const LblRUV2UnitNetBytes: &str = "net_bytes";
pub const LblRUV2UnitCrossAZNetBytes: &str = "cross_az_net_bytes";
pub const LblRUV2UnitFrontendCompileBytes: &str = "frontend_compile_bytes";
pub const LblRUV2UnitHashStateRows: &str = "hash_state_rows";
pub const LblRUV2UnitJoinOutputRows: &str = "join_output_rows";
pub const LblRUV2UnitWriteStatement: &str = "write_statement";
pub const LblRUV2UnitOperatorNum: &str = "operator_num";
pub const LblRUV2UnitWriteKeys: &str = "write_keys";
pub const LblRUV2UnitWriteBytes: &str = "write_bytes";

pub static mut RUV2Total: Option<prometheus::Counter> = None;
pub static mut RUV2TTLTotal: Option<prometheus::Counter> = None;
pub static mut RUV2BySQLType: Option<prometheus::CounterVec> = None;
pub static mut RUV2BySQLTypeDDL: Option<prometheus::Counter> = None;
pub static mut RUV2ByEngine: Option<prometheus::CounterVec> = None;
pub static mut RUV2ByEngineTiKV: Option<prometheus::Counter> = None;
pub static mut RUV2Unit: Option<prometheus::CounterVec> = None;
pub static mut RUV2Statements: Option<prometheus::CounterVec> = None;
static mut ruv2TiDB: Option<prometheus::Counter> = None;
static mut ruv2TiFlash: Option<prometheus::Counter> = None;
static mut ruv2Select: Option<prometheus::Counter> = None;
static mut ruv2Insert: Option<prometheus::Counter> = None;
static mut ruv2Replace: Option<prometheus::Counter> = None;
static mut ruv2Update: Option<prometheus::Counter> = None;
static mut ruv2Delete: Option<prometheus::Counter> = None;
static mut ruv2Commit: Option<prometheus::Counter> = None;
static mut ruv2Analyze: Option<prometheus::Counter> = None;
static mut ruv2Other: Option<prometheus::Counter> = None;
static RUV2_METRICS_INIT: Once = Once::new();

// counter 对应 Go 中重复的 metricscommon.NewCounter(CounterOpts{...}) 构造形状。
/// 构造命名空间为 tidb、子系统为 ruv2 的 Counter。
fn counter(name: &'static str, help: &'static str) -> prometheus::Counter {
    metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "ruv2",
        Name: name,
        Help: help,
        ..Default::default()
    })
}

// counter_vec 构造只带 LblType 的 CounterVec，保持全部 RU v2 向量指标的标签维度。
/// 构造仅含 `type` 标签的 CounterVec。
fn counter_vec(name: &'static str, help: &'static str) -> prometheus::CounterVec {
    metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "ruv2",
            Name: name,
            Help: help,
            ..Default::default()
        },
        &[LblType],
    )
}

// InitRUV2Metrics 按 Go 源码顺序初始化 RU v2 指标，最后建立热点标签缓存。
/// 初始化全部 RU v2 指标并预热热点 executor/coprocessor 标签缓存。
pub fn InitRUV2Metrics() {
    RUV2_METRICS_INIT.call_once(|| unsafe {
        RUV2TTLTotal = Some(counter(
            "ttl_ru_total",
            "Counter of RU v2 consumption from TTL user-table scans and deletes, including their commits; included in ru_total.",
        ));
        RUV2Total = Some(counter(
            "ru_total",
            "Counter of resource unit consumption for RU v2.",
        ));
        let sql = metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ruv2",
                Name: "ru_by_sql_type_total",
                Help: "Counter of resource unit consumption by SQL type for RU v2.",
                ..Default::default()
            },
            &[LblSQLType],
        );
        RUV2BySQLTypeDDL = Some(sql.WithLabelValues(&[LblSQLTypeDDL]));
        ruv2Select = Some(sql.WithLabelValues(&["select"]));
        ruv2Insert = Some(sql.WithLabelValues(&["insert"]));
        ruv2Replace = Some(sql.WithLabelValues(&["replace"]));
        ruv2Update = Some(sql.WithLabelValues(&["update"]));
        ruv2Delete = Some(sql.WithLabelValues(&["delete"]));
        ruv2Commit = Some(sql.WithLabelValues(&["commit"]));
        ruv2Analyze = Some(sql.WithLabelValues(&["analyze"]));
        ruv2Other = Some(sql.WithLabelValues(&["other"]));
        RUV2BySQLType = Some(sql);
        let engine = metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ruv2",
                Name: "ru_by_engine_total",
                Help: "Counter of resource unit consumption by engine for RU v2.",
                ..Default::default()
            },
            &[LblEngine],
        );
        ruv2TiDB = Some(engine.WithLabelValues(&["tidb"]));
        ruv2TiFlash = Some(engine.WithLabelValues(&[LblEngineTiFlash]));
        RUV2ByEngineTiKV = Some(engine.WithLabelValues(&[LblEngineTiKV]));
        RUV2ByEngine = Some(engine);
        RUV2Unit = Some(metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ruv2",
                Name: "unit_total",
                Help: "Counter of raw statement units for RU v2.",
                ..Default::default()
            },
            &[LblEngine, "opclass", LblRUV2Unit],
        ));
        RUV2Statements = Some(metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ruv2",
                Name: "statements_total",
                Help: "Counter of RU v2 calculation outcomes in full report mode; success with incomplete evidence remains best effort.",
                ..Default::default()
            },
            &["status", "reason"],
        ));
    });
}

/// Record RU totals by SQL type and execution engine using prebound counters.
pub fn AddRUV2Results(tikv_ru: f64, tidb_ru: f64, tiflash_ru: f64, total_ru: f64, sql_type: &str) {
    InitRUV2Metrics();
    unsafe {
        let sql_counter = match sql_type {
            "select" => &ruv2Select,
            "insert" => &ruv2Insert,
            "replace" => &ruv2Replace,
            "update" => &ruv2Update,
            "delete" => &ruv2Delete,
            "commit" => &ruv2Commit,
            "analyze" => &ruv2Analyze,
            _ => &ruv2Other,
        };
        RUV2Total
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(total_ru);
        sql_counter
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(total_ru);
        RUV2ByEngineTiKV
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(tikv_ru);
        ruv2TiDB
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(tidb_ru);
        ruv2TiFlash
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(tiflash_ru);
    }
}

/// Record a completed DDL job as TiKV RU, matching the three Go counters.
pub fn AddDDLJobRU(ru: f64) {
    if ru <= 0.0 {
        return;
    }
    InitRUV2Metrics();
    unsafe {
        RUV2Total
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(ru);
        RUV2BySQLTypeDDL
            .as_ref()
            .expect("RU v2 SQL type metrics initialized")
            .Add(ru);
        RUV2ByEngineTiKV
            .as_ref()
            .expect("RU v2 engine metrics initialized")
            .Add(ru);
    }
}
