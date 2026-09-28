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

// SEM 兼容层测试辅助：版本常量、切换开关与 Go 兼容的 SEM v2 配置 JSON。
//
// SEM（Security Enhanced Mode）测试需要在 v1/v2 之间切换并注入与 Go 一致的配置。
// 本模块提供 `SwitchToSEMForTest` 与 `compatibleSEMV2Config`，供单元/集成测试复用。

#![allow(non_snake_case, non_upper_case_globals)]

use std::io::Write;

use astersql_util_sem as semv1;
use astersql_util_sem_v2 as semv2;

// V1 represents SEM v1
// V1 表示测试需要切换到 SEM v1；保持 Go 常量名和字符串值不变。
pub const V1: &str = "v1";

// V2 represents SEM v2
// V2 表示测试需要切换到 SEM v2；保持 Go 常量名和字符串值不变。
pub const V2: &str = "v2";

// SwitchToSEMForTest switches to SEM v1 or v2 and returns its cleanup function.
/// 按版本启用 SEM，并返回用于关闭的清理闭包（对应 Go 的 defer cleanup）。
pub fn SwitchToSEMForTest(version: &str) -> Box<dyn Fn()> {
    match version {
        V1 => {
            semv1::Enable();
            Box::new(semv1::Disable)
        }
        V2 => {
            // The Go helper changes this build-time value before validating the config.
            // 校验配置前将发布版本抬到 v9.0.0，满足 JSON 中 tidb_version 的最低版本要求。
            unsafe {
                mysql::r#const::TiDBReleaseVersion = "v9.0.0";
            }

            // 将兼容配置写入临时 JSON，再经 EnableFromPathForTest 加载（与 Go 路径一致）。
            let mut file = tempfile::Builder::new()
                .prefix("semv2_config_")
                .suffix(".json")
                .tempfile()
                .unwrap_or_else(|err| panic!("failed to create temp file: {err}"));
            file.write_all(compatibleSEMV2Config.as_bytes())
                .unwrap_or_else(|err| panic!("failed to write SEM v2 config: {err}"));
            file.flush()
                .unwrap_or_else(|err| panic!("failed to flush SEM v2 config: {err}"));

            let config_path = file.path().to_string_lossy().into_owned();
            let cleanup = semv2::EnableFromPathForTest(&config_path)
                .unwrap_or_else(|err| panic!("failed to enable SEM v2: {err}"));
            // 启用成功后即可丢弃临时文件句柄；路径已读入配置，cleanup 负责关闭 SEM。
            drop(file);
            cleanup
        }
        _ => panic!("unknown SEM version: {version}"),
    }
}

// compatibleSEMV2Config 保存 Go 测试使用的 SEM v2 兼容配置 JSON。
// 该字符串保留原始字段顺序和内容；在 Go 代码中它会被写入临时文件供 semv2.EnableFromPathForTest 读取。
pub static compatibleSEMV2Config: &str = r#"{
	"version": "1.0",
	"tidb_version": "v9.0.0",
	"restricted_databases": ["metrics_schema"],
	"restricted_tables": [
		{"schema": "mysql", "name": "expr_pushdown_blacklist", "hidden": true},
		{"schema": "mysql", "name": "gc_delete_range", "hidden": true},
		{"schema": "mysql", "name": "gc_delete_range_done", "hidden": true},
		{"schema": "mysql", "name": "opt_rule_blacklist", "hidden": true},
		{"schema": "mysql", "name": "tidb", "hidden": true},
		{"schema": "mysql", "name": "global_variables", "hidden": true},
		{"schema": "information_schema", "name": "cluster_config", "hidden": true},
		{"schema": "information_schema", "name": "cluster_hardware", "hidden": true},
		{"schema": "information_schema", "name": "cluster_load", "hidden": true},
		{"schema": "information_schema", "name": "cluster_log", "hidden": true},
		{"schema": "information_schema", "name": "cluster_systeminfo", "hidden": true},
		{"schema": "information_schema", "name": "inspection_result", "hidden": true},
		{"schema": "information_schema", "name": "inspection_rules", "hidden": true},
		{"schema": "information_schema", "name": "inspection_summary", "hidden": true},
		{"schema": "information_schema", "name": "metrics_summary", "hidden": true},
		{"schema": "information_schema", "name": "metrics_summary_by_label", "hidden": true},
		{"schema": "information_schema", "name": "metrics_tables", "hidden": true},
		{"schema": "information_schema", "name": "tidb_hot_regions", "hidden": true},
		{"schema": "performance_schema", "name": "pd_profile_allocs", "hidden": true},
		{"schema": "performance_schema", "name": "pd_profile_block", "hidden": true},
		{"schema": "performance_schema", "name": "pd_profile_cpu", "hidden": true},
		{"schema": "performance_schema", "name": "pd_profile_goroutines", "hidden": true},
		{"schema": "performance_schema", "name": "pd_profile_memory", "hidden": true},
		{"schema": "performance_schema", "name": "pd_profile_mutex", "hidden": true},
		{"schema": "performance_schema", "name": "tidb_profile_allocs", "hidden": true},
		{"schema": "performance_schema", "name": "tidb_profile_block", "hidden": true},
		{"schema": "performance_schema", "name": "tidb_profile_cpu", "hidden": true},
		{"schema": "performance_schema", "name": "tidb_profile_goroutines", "hidden": true},
		{"schema": "performance_schema", "name": "tidb_profile_memory", "hidden": true},
		{"schema": "performance_schema", "name": "tidb_profile_mutex", "hidden": true},
		{"schema": "performance_schema", "name": "tikv_profile_cpu", "hidden": true}
	],
	"restricted_status_variables": [
		"tidb_gc_leader_desc"
	],
	"restricted_variables": [
		{"name": "hostname", "hidden": false, "value": "localhost"},
		{"name": "tidb_enable_enhanced_security", "hidden": false, "value": "ON"},
		{"name": "ddl_slow_threshold", "hidden": true},
		{"name": "tidb_check_mb4_value_in_utf8", "hidden": true},
		{"name": "tidb_config", "hidden": true},
		{"name": "tidb_enable_slow_log", "hidden": true},
		{"name": "tidb_enable_telemetry", "hidden": true},
		{"name": "tidb_expensive_query_time_threshold", "hidden": true},
		{"name": "tidb_force_priority", "hidden": true},
		{"name": "tidb_general_log", "hidden": true},
		{"name": "tidb_metric_query_range_duration", "hidden": true},
		{"name": "tidb_metric_query_step", "hidden": true},
		{"name": "tidb_opt_write_row_id", "hidden": true},
		{"name": "tidb_pprof_sql_cpu", "hidden": true},
		{"name": "tidb_record_plan_in_slow_log", "hidden": true},
		{"name": "tidb_row_format_version", "hidden": true},
		{"name": "tidb_slow_query_file", "hidden": true},
		{"name": "tidb_slow_log_threshold", "hidden": true},
		{"name": "tidb_enable_collect_execution_info", "hidden": true},
		{"name": "tidb_memory_usage_alarm_ratio", "hidden": true},
		{"name": "tidb_redact_log", "hidden": true},
		{"name": "tidb_restricted_read_only", "hidden": true},
		{"name": "tidb_top_sql_max_time_series_count", "hidden": true},
		{"name": "tidb_top_sql_max_meta_count", "hidden": true}
	],
	"restricted_privileges": [
		"FILE",
		"BACKUP_ADMIN"
	],
	"restricted_sql": {
		"rule": [
			"time_to_live",
			"alter_table_attributes",
			"import_with_external_id"
		],
		"sql": [
			"BACKUP",
			"RESTORE",
			"ALTER RESOURCE GROUP"
		]
	}
}"#;
