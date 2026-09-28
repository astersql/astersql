// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Domain（域）子系统相关 Prometheus 指标。
//
// Domain 持有 schema、权限与系统变量等全局状态；租约（lease）到期后需重新加载。
// 本模块观测租约过期时间、schema/权限/系统变量加载、InfoCache 命中率以及
// Schema Validator（校验本地 schema 是否仍有效）的状态转换。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

//

// Metrics for the domain package.
/// 最近一次租约过期时间（秒）的 Gauge。
// LeaseExpireTime records the lease expire time.
pub static mut LeaseExpireTime: Option<prometheus::Gauge> = None;
/// 加载 schema 次数计数。
// LoadSchemaCounter records the counter of load schema.
pub static mut LoadSchemaCounter: Option<prometheus::CounterVec> = None;
/// 加载 schema 耗时直方图。
// LoadSchemaDuration records the duration of load schema.
pub static mut LoadSchemaDuration: Option<prometheus::HistogramVec> = None;
/// InfoCache 读取/命中计数。
// InfoCacheCounters are the counters of get/hit.
pub static mut InfoCacheCounters: Option<prometheus::CounterVec> = None;

// InfoCache 的 action 标签值保持 Go 字符串不变，供调用方区分读取总数与命中数。
/// InfoCache action：读取。
pub const InfoCacheCounterGet: &str = "get";
/// InfoCache action：命中。
pub const InfoCacheCounterHit: &str = "hit";

/// 加载权限信息次数计数。
// LoadPrivilegeCounter records the counter of load privilege.
pub static mut LoadPrivilegeCounter: Option<prometheus::CounterVec> = None;
/// 加载系统变量缓存次数计数。
// LoadSysVarCacheCounter records the counter of loading sysvars.
pub static mut LoadSysVarCacheCounter: Option<prometheus::CounterVec> = None;

// Schema validator 的状态标签逐项对应 Go 包级字符串。
/// Schema Validator 状态：停止。
pub const SchemaValidatorStop: &str = "stop";
/// Schema Validator 状态：重启。
pub const SchemaValidatorRestart: &str = "restart";
/// Schema Validator 状态：重置。
pub const SchemaValidatorReset: &str = "reset";
/// Schema Validator 状态：缓存为空。
pub const SchemaValidatorCacheEmpty: &str = "cache_empty";
/// Schema Validator 状态：缓存未命中。
pub const SchemaValidatorCacheMiss: &str = "cache_miss";
/// Schema 校验处理事件计数。
// HandleSchemaValidate records the counter of handling schema validate.
pub static mut HandleSchemaValidate: Option<prometheus::CounterVec> = None;

/// 初始化 Domain 相关全部指标 collector。
// InitDomainMetrics 对应 Go 初始化函数：构造租约、schema、缓存、权限和校验器指标。
pub fn InitDomainMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    let lease_expire_time = metricscommon::NewGauge(prometheus::GaugeOpts {
        Namespace: "tidb",
        Subsystem: "domain",
        Name: "lease_expire_time",
        Help: "When the last time the lease is expired, it is in seconds",
    });

    let load_schema_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "domain",
            Name: "load_schema_total",
            Help: "Counter of load schema",
        },
        vec![LblType],
    );

    let load_schema_duration = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "domain",
            Name: "load_schema_duration_seconds",
            Help: "Bucketed histogram of processing time (s) in load schema.",
            // 1ms 起始、倍数 2、20 个桶，保持 Go 的约 524 秒上界。
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 20),
        },
        vec![LblAction],
    );

    let info_cache_counters = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "domain",
            Name: "infocache_counters",
            Help: "Counters of infoCache: get/hit.",
        },
        // 标签顺序会影响 Prometheus 时序身份，因此严格保留 action、type 顺序。
        vec![LblAction, LblType],
    );

    let load_privilege_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "domain",
            Name: "load_privilege_total",
            Help: "Counter of load privilege",
        },
        vec![LblType],
    );

    let load_sysvar_cache_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "domain",
            Name: "load_sysvarcache_total",
            Help: "Counter of load sysvar cache",
        },
        vec![LblType],
    );

    let handle_schema_validate = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "domain",
            Name: "handle_schema_validate",
            Help: "Counter of handle schema validate",
        },
        vec![LblType],
    );

    // Go 依赖 InitMetrics 单线程初始化这些包变量；这里保留写入形状，不承诺 static mut 的并发安全性。
    unsafe {
        LeaseExpireTime = Some(lease_expire_time);
        LoadSchemaCounter = Some(load_schema_counter);
        LoadSchemaDuration = Some(load_schema_duration);
        InfoCacheCounters = Some(info_cache_counters);
        LoadPrivilegeCounter = Some(load_privilege_counter);
        LoadSysVarCacheCounter = Some(load_sysvar_cache_counter);
        HandleSchemaValidate = Some(handle_schema_validate);
    }
}
