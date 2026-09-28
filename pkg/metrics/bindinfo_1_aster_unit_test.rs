// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Bindinfo 指标迁移对照测试：校验指标全名、读写数值，并冒烟初始化其它子系统指标。

use astersql_metrics::{
    binding_cache_hit_counter, binding_cache_mem_limit, binding_cache_mem_usage,
    binding_cache_miss_counter, binding_cache_num_bindings, init_bind_info_metrics,
};
use prometheus::core::Collector;

/// 从 Collector 取出第一个 metric family 的全名。
fn metric_name<C: Collector>(collector: &C) -> String {
    collector.collect()[0].get_name().to_owned()
}

/// 核对 Go 侧元数据命名（`tidb_server_*`），并验证 Counter/Gauge 可写入与读回。
#[test]
fn bindinfo_metrics_match_go_metadata_and_record_values() {
    init_bind_info_metrics();

    assert_eq!(
        metric_name(binding_cache_hit_counter()),
        "tidb_server_binding_cache_hit_total"
    );
    assert_eq!(
        metric_name(binding_cache_miss_counter()),
        "tidb_server_binding_cache_miss_total"
    );
    assert_eq!(
        metric_name(binding_cache_mem_usage()),
        "tidb_server_binding_cache_mem_usage"
    );
    assert_eq!(
        metric_name(binding_cache_mem_limit()),
        "tidb_server_binding_cache_mem_limit"
    );
    assert_eq!(
        metric_name(binding_cache_num_bindings()),
        "tidb_server_binding_cache_num_bindings"
    );

    binding_cache_hit_counter().inc_by(2.0);
    binding_cache_miss_counter().inc();
    binding_cache_mem_usage().set(128.0);
    binding_cache_mem_limit().set(1024.0);
    binding_cache_num_bindings().set(3.0);

    assert_eq!(binding_cache_hit_counter().get(), 2.0);
    assert_eq!(binding_cache_miss_counter().get(), 1.0);
    assert_eq!(binding_cache_mem_usage().get(), 128.0);
    assert_eq!(binding_cache_mem_limit().get(), 1024.0);
    assert_eq!(binding_cache_num_bindings().get(), 3.0);
}

/// 冒烟调用多个子系统的 Go 风格 Init*Metrics，确认元数据可被接受（部分路径需 unsafe）。
#[test]
fn all_metric_initializers_accept_go_metadata() {
    if crate::main_test::run_in_isolated_process(
        "bindinfo_1_aster_unit_test::all_metric_initializers_accept_go_metadata",
    ) {
        return;
    }
    astersql_metrics::br::InitBRMetrics();
    astersql_metrics::ddl::InitDDLMetrics();
    astersql_metrics::distsql::InitDistSQLMetrics();
    astersql_metrics::domain::InitDomainMetrics();
    astersql_metrics::executor::InitExecutorMetrics();
    astersql_metrics::external_workload::InitExternalWorkloadMetrics();
    astersql_metrics::gc_worker::InitGCWorkerMetrics();
    unsafe {
        astersql_metrics::globalsort::InitGlobalSortMetrics();
        astersql_metrics::infoschema::InitInfoSchemaV2Metrics();
        astersql_metrics::log_backup::InitLogBackupMetrics();
        astersql_metrics::memory::InitMemoryMetrics();
        astersql_metrics::meta::init_meta_metrics();
    }
}
