// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// CALIBRATE RESOURCE 相关测试草稿与可运行回归。
//
// `legacy_calibrate_resource`（`cfg(any())`）保留 Go 测试中的 SQL 阶段、mock metrics、
// failpoint 与资源组 provider 语义，便于人工对照；当前可执行用例覆盖静态/动态校准
// 的核心数值路径。

#[cfg(any())]
mod legacy_calibrate_resource {
    // 保留 CALIBRATE RESOURCE 测试阶段、mock metrics fixture、failpoint 和 PD resource group provider 语义；
    // 也不会执行 SQL，只把 Go 测试中关键输入、期望结果和错误路径转成便于人工核对的 Rust 结构。

    use std::collections::HashMap;

    /// 对应 Go 的 `TestCalibrateResource`。
    /// Go 版本会通过 testkit 创建 mock store/session，并真实调用 CALIBRATE RESOURCE executor；
    /// Rust 这里只记录每个阶段的 SQL、期望行和错误信息。
    #[test]
    pub fn test_calibrate_resource() {
        let mut tk = TestKitDraft::new();

        // 第一阶段验证 resource_control 关闭时 executor 返回错误。
        tk.must_exec("SET GLOBAL tidb_enable_resource_control='OFF';");
        tk.exec_expect_error("CALIBRATE RESOURCE", "Resource control feature is disabled");
        tk.must_exec("SET GLOBAL tidb_enable_resource_control='ON';");

        // Go 这里临时替换 domain 的 ResourceGroupsController，defer 中恢复旧 controller。
        let old_cfg = ResourceGroupConfigDraft {
            read_base_cost: 0.25,
            read_cost_per_byte: 0.0000152587890625,
            write_base_cost: 1.0,
            write_cost_per_byte: 0.0009765625,
            cpu_ms_cost: 0.3333333333333333,
        };
        let provider = MockResourceGroupProvider::new(old_cfg);
        tk.install_resource_group_controller(provider);

        // 没有集群 metrics 时，Go executor 在读取结果 chunk 时返回 tikv server 缺失。
        tk.exec_expect_error("CALIBRATE RESOURCE", "no server with type 'tikv' is found");
        tk.exec_expect_parse_error(
            "CALIBRATE RESOURCE WORKLOAD tpcc START_TIME '2020-02-12 10:35:00'",
        );

        // Mock information_schema.cluster_config；failpoint 表达式在 Go 中拼接为 `return("...")`。
        let mut instances = vec![
            "pd,127.0.0.1:32379,127.0.0.1:32380,mock-version,mock-githash,0",
            "tidb,127.0.0.1:34000,30080,mock-version,mock-githash,1001",
            "tikv,127.0.0.1:30160,30180,mock-version,mock-githash,0",
            "tikv,127.0.0.1:30161,30181,mock-version,mock-githash,0",
            "tikv,127.0.0.1:30162,30182,mock-version,mock-githash,0",
        ];
        tk.enable_failpoint(
            "github.com/pingcap/tidb/pkg/infoschema/mockClusterInfo",
            &instances.join(";"),
        );

        // Mock metrics table 和 Prometheus response；Go 使用 base64 绕过 failpoint 不支持空白/换行的限制。
        let metrics_data = r#"# HELP process_cpu_seconds_total Total user and system CPU time spent in seconds.
# TYPE process_cpu_seconds_total counter
process_cpu_seconds_total 49943
# HELP tikv_server_cpu_cores_quota Total CPU cores quota for TiKV server
# TYPE tikv_server_cpu_cores_quota gauge
tikv_server_cpu_cores_quota 8
# HELP tiflash_proxy_tikv_scheduler_write_flow The write flow passed through at scheduler level.
# TYPE tiflash_proxy_tikv_scheduler_write_flow gauge
tiflash_proxy_tikv_scheduler_write_flow 0
# HELP tiflash_proxy_tikv_server_cpu_cores_quota Total CPU cores quota for TiKV server
# TYPE tiflash_proxy_tikv_server_cpu_cores_quota gauge
tiflash_proxy_tikv_server_cpu_cores_quota 20
"#;
        tk.enable_failpoint(
            "github.com/pingcap/tidb/pkg/executor/mockMetricsTableData",
            "return",
        );
        tk.enable_failpoint(
            "github.com/pingcap/tidb/pkg/executor/internal/calibrateresource/mockMetricsDataFilter",
            "return(true)",
        );
        tk.enable_failpoint(
            "github.com/pingcap/tidb/pkg/executor/internal/calibrateresource/mockMetricsResponse",
            metrics_data,
        );
        tk.enable_failpoint(
            "github.com/pingcap/tidb/pkg/executor/internal/calibrateresource/mockGOMAXPROCS",
            "return(40)",
        );

        // 默认 workload 与不同 workload 类型的静态容量估算。
        for case in [
            QueryCase::row("CALIBRATE RESOURCE", "69768"),
            QueryCase::row("CALIBRATE RESOURCE WORKLOAD TPCC", "69768"),
            QueryCase::row("CALIBRATE RESOURCE WORKLOAD OLTP_READ_WRITE", "55823"),
            QueryCase::row("CALIBRATE RESOURCE WORKLOAD OLTP_READ_ONLY", "34926"),
            QueryCase::row("CALIBRATE RESOURCE WORKLOAD OLTP_WRITE_ONLY", "109776"),
        ] {
            tk.must_query(case);
        }

        // Go 修改 mockGOMAXPROCS 为 8，验证 TiDB CPU 小于 TiKV quota 时的路径。
        tk.enable_failpoint(
            "github.com/pingcap/tidb/pkg/executor/internal/calibrateresource/mockGOMAXPROCS",
            "return(8)",
        );
        tk.must_query(QueryCase::row("CALIBRATE RESOURCE", "38760"));

        let mut mock_data = MockMetricsData::default();
        mock_data.set("resource_manager_resource_unit", ru_recent_window());
        mock_data.set("process_cpu_usage", cpu_recent_window());

        // 最近 21 分钟内按不同 START_TIME/END_TIME/DURATION 组合选择窗口，期望相同窗口给出相同 RU。
        for case in recent_window_query_cases() {
            tk.must_query_with_metrics(&mock_data, case);
        }

        // 固定时间窗口数据，覆盖动态 calibrate 的等价 START_TIME/DURATION/END_TIME 表达。
        mock_data.set("resource_manager_resource_unit", ru_static_window());
        mock_data.set("process_cpu_usage", cpu_static_window(3.212));
        tk.must_query_with_metrics(
            &mock_data,
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' DURATION '10m'",
                "8161",
            ),
        );
        tk.must_query_with_metrics(
        &mock_data,
        QueryCase::row(
            "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' END_TIME '2020-02-12 10:45:00'",
            "8161",
        ),
    );

        // 提高 TiDB CPU 样本后，Go 期望结果降到 5616；相同窗口的参数顺序也要覆盖。
        mock_data.set("process_cpu_usage", cpu_static_window(3.212));
        for sql in [
            "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' DURATION '10m'",
            "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' END_TIME '2020-02-12 10:45:00'",
            "CALIBRATE RESOURCE END_TIME '2020-02-12 10:45:00' START_TIME '2020-02-12 10:35:00'",
            "CALIBRATE RESOURCE END_TIME '2020-02-12 10:45:00' DURATION '5m' START_TIME '2020-02-12 10:35:00' ",
        ] {
            tk.must_query_with_metrics(&mock_data, QueryCase::row(sql, "5616"));
        }

        // 统计时间点不完全对应：Go 分别验证可对齐、乱序/缺点和 20s 时间差对结果的影响。
        mock_data.set("resource_manager_resource_unit", ru_modify_1());
        tk.must_query_with_metrics(
            &mock_data,
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:25:00' DURATION '20m'",
                "5616",
            ),
        );
        mock_data.set("resource_manager_resource_unit", ru_modify_2());
        mock_data.set("process_cpu_usage", cpu_modify_2());
        tk.must_query_with_metrics(
            &mock_data,
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:25:00' DURATION '20m'",
                "5631",
            ),
        );
        mock_data.set("resource_manager_resource_unit", ru_modify_3());
        tk.must_query_with_metrics(
            &mock_data,
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:25:00' DURATION '20m'",
                "5633",
            ),
        );

        // 低 workload、过长/过短 duration 以及单分钟窗口错误分支。
        mock_data.set("resource_manager_resource_unit", ru_low_workload());
        tk.exec_with_metrics_expect_error(
            &mock_data,
            "CALIBRATE RESOURCE START_TIME '2020-02-12 10:25:00' DURATION '20m'",
            "The workload in selected time window is too low",
        );
        mock_data.set("resource_manager_resource_unit", ru_sparse_window());
        mock_data.set("process_cpu_usage", cpu_sparse_window());
        tk.exec_with_metrics_expect_error(
            &mock_data,
            "CALIBRATE RESOURCE START_TIME '2020-02-12 10:25:00' DURATION '20m'",
            "The workload in selected time window is too low",
        );
        mock_data.set("resource_manager_resource_unit", ru_static_window());
        mock_data.set("process_cpu_usage", cpu_static_window(3.212));
        for err_case in [
            ErrorCase::new(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00'",
                "the duration of calibration is too long",
            ),
            ErrorCase::new(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' END_TIME '2020-02-12 10:35:40'",
                "the duration of calibration is too short",
            ),
            ErrorCase::new(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' DURATION '25h'",
                "the duration of calibration is too long",
            ),
            ErrorCase::new(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' END_TIME '2020-02-13 10:46:00'",
                "the duration of calibration is too long",
            ),
        ] {
            tk.exec_with_metrics_expect_error(&mock_data, err_case.sql, err_case.error_contains);
        }
        tk.must_query_with_metrics(
            &mock_data,
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' DURATION '1m'",
                "5337",
            ),
        );

        // CPU 样本多数接近 0 时，Go 返回更长的低 workload 错误信息。
        mock_data.set("process_cpu_usage", cpu_mostly_idle_window());
        tk.exec_with_metrics_expect_error(
        &mock_data,
        "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' END_TIME '2020-02-13 10:35:01'",
        "The workload in selected time window is too low, with which TiDB is unable to reach a capacity estimation",
    );

        // 样本不足 10 分钟但仍可估算的分支。
        mock_data.set("process_cpu_usage", cpu_short_window());
        tk.must_query_with_metrics(
        &mock_data,
        QueryCase::row(
            "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' END_TIME '2020-02-12 10:45:00'",
            "5492",
        ),
    );

        // TiFlash 路径：先因 TiFlash workload 低报错，再补齐 TiFlash CPU/RU 后成功。
        mock_data.set(
            "resource_manager_resource_unit",
            tiflash_ru_without_tiflash_cpu(),
        );
        mock_data.set("process_cpu_usage", tiflash_tidb_tikv_cpu());
        mock_data.set("tidb_server_maxprocs", tidb_server_maxprocs());
        instances.push("tiflash,127.0.0.1:3930,33940,mock-version,mock-githash,0");
        tk.enable_failpoint(
            "github.com/pingcap/tidb/pkg/infoschema/mockClusterInfo",
            &instances.join(";"),
        );
        tk.exec_with_metrics_expect_error(
            &mock_data,
            "CALIBRATE RESOURCE START_TIME '2023-09-19 19:50:39' DURATION '10m'",
            "The workload in selected time window is too low",
        );
        mock_data.set("tiflash_process_cpu_usage", tiflash_cpu());
        mock_data.set("tiflash_resource_manager_resource_unit", tiflash_ru());
        tk.must_query_with_metrics(
            &mock_data,
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME '2023-09-19 19:50:39' DURATION '10m'",
                "729439",
            ),
        );
        mock_data.delete("process_cpu_usage");
        tk.must_query_with_metrics(
        &mock_data,
        QueryCase::row(
            "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' END_TIME '2020-02-12 10:45:00'",
            "729439",
        ),
    );
        mock_data.delete("tiflash_process_cpu_usage");
        tk.exec_with_metrics_expect_error(
            &mock_data,
            "CALIBRATE RESOURCE START_TIME '2020-02-12 10:35:00' END_TIME '2020-02-12 10:45:00'",
            "query metric error: pd unavailable",
        );
    }

    /// 一条 metric 表行，对应 Go 的 `types.MakeDatums(...)`。
    #[derive(Clone)]
    pub struct MetricRow {
        pub values: Vec<MetricValue>,
    }

    impl MetricRow {
        pub fn new(values: Vec<MetricValue>) -> Self {
            Self { values }
        }
    }

    #[derive(Clone)]
    pub enum MetricValue {
        Time(&'static str),
        Str(&'static str),
        Float(f64),
    }

    fn ru(time: &'static str, value: f64) -> MetricRow {
        MetricRow::new(vec![MetricValue::Time(time), MetricValue::Float(value)])
    }

    fn cpu(time: &'static str, instance: &'static str, typ: &'static str, value: f64) -> MetricRow {
        MetricRow::new(vec![
            MetricValue::Time(time),
            MetricValue::Str(instance),
            MetricValue::Str(typ),
            MetricValue::Float(value),
        ])
    }

    /// Mock metrics table 数据，对应 Go 的 `map[string][][]types.Datum`。
    #[derive(Default)]
    pub struct MockMetricsData {
        pub tables: HashMap<&'static str, Vec<MetricRow>>,
    }

    impl MockMetricsData {
        pub fn set(&mut self, table: &'static str, rows: Vec<MetricRow>) {
            self.tables.insert(table, rows);
        }

        pub fn delete(&mut self, table: &'static str) {
            self.tables.remove(table);
        }
    }

    /// 对应 Go 的 testkit 操作；所有方法都是记录型，不执行 SQL。
    pub struct TestKitDraft {
        pub executed: Vec<String>,
    }

    impl TestKitDraft {
        pub fn new() -> Self {
            Self {
                executed: Vec::new(),
            }
        }

        pub fn must_exec(&mut self, sql: &str) {
            self.executed.push(sql.to_string());
        }

        pub fn exec_expect_error(&mut self, sql: &str, error_contains: &str) {
            self.executed
                .push(format!("{sql} -- expect error contains {error_contains}"));
        }

        pub fn exec_expect_parse_error(&mut self, sql: &str) {
            self.executed.push(format!("{sql} -- expect parse error"));
        }

        pub fn must_query(&mut self, case: QueryCase) {
            self.executed
                .push(format!("{} -- expect row {}", case.sql, case.expected_row));
        }

        pub fn must_query_with_metrics(&mut self, metrics: &MockMetricsData, case: QueryCase) {
            // Go 通过 context.WithValue 注入 "__mockMetricsTableData"，并用 failpoint hook 限定 mock 表。
            assert!(!metrics.tables.is_empty());
            self.must_query(case);
        }

        pub fn exec_with_metrics_expect_error(
            &mut self,
            metrics: &MockMetricsData,
            sql: &str,
            error_contains: &str,
        ) {
            assert!(!metrics.tables.is_empty());
            self.exec_expect_error(sql, error_contains);
        }

        pub fn enable_failpoint(&mut self, name: &str, expr: &str) {
            self.executed
                .push(format!("enable failpoint {name} => {expr}"));
        }

        pub fn install_resource_group_controller(&mut self, provider: MockResourceGroupProvider) {
            // Go 的 controller 会从 PD provider 读取 JSON config；这里只确认 provider 可返回 payload。
            assert!(provider.get(CONTROLLER_CONFIG_PATH_PREFIX).is_ok());
        }
    }

    pub struct QueryCase {
        pub sql: &'static str,
        pub expected_row: &'static str,
    }

    impl QueryCase {
        pub const fn row(sql: &'static str, expected_row: &'static str) -> Self {
            Self { sql, expected_row }
        }
    }

    pub struct ErrorCase {
        pub sql: &'static str,
        pub error_contains: &'static str,
    }

    impl ErrorCase {
        pub const fn new(sql: &'static str, error_contains: &'static str) -> Self {
            Self {
                sql,
                error_contains,
            }
        }
    }

    /// 对应 rmclient.Config 中测试覆盖的 RU cost 字段。
    pub struct ResourceGroupConfigDraft {
        pub read_base_cost: f64,
        pub read_cost_per_byte: f64,
        pub write_base_cost: f64,
        pub write_cost_per_byte: f64,
        pub cpu_ms_cost: f64,
    }

    const CONTROLLER_CONFIG_PATH_PREFIX: &[u8] = b"/controller/config";

    /// 对应 Go 的 mockResourceGroupProvider。
    pub struct MockResourceGroupProvider {
        pub cfg: ResourceGroupConfigDraft,
    }

    impl MockResourceGroupProvider {
        pub fn new(cfg: ResourceGroupConfigDraft) -> Self {
            Self { cfg }
        }

        /// 对应 `Get(ctx, key, opts...)`：只支持 controller config path，并返回 JSON payload。
        pub fn get(&self, key: &[u8]) -> Result<GetResponseDraft, String> {
            if key != CONTROLLER_CONFIG_PATH_PREFIX {
                return Err("unsupported configPath".to_string());
            }
            Ok(GetResponseDraft {
            count: 1,
            kvs: vec![KeyValueDraft {
                key: key.to_vec(),
                value: format!(
                    "{{\"read_base_cost\":{},\"read_cost_per_byte\":{},\"write_base_cost\":{},\"write_cost_per_byte\":{},\"cpu_ms_cost\":{}}}",
                    self.cfg.read_base_cost,
                    self.cfg.read_cost_per_byte,
                    self.cfg.write_base_cost,
                    self.cfg.write_cost_per_byte,
                    self.cfg.cpu_ms_cost
                )
                .into_bytes(),
            }],
        })
        }
    }

    pub struct GetResponseDraft {
        pub count: i64,
        pub kvs: Vec<KeyValueDraft>,
    }

    pub struct KeyValueDraft {
        pub key: Vec<u8>,
        pub value: Vec<u8>,
    }

    fn recent_window_query_cases() -> Vec<QueryCase> {
        vec![
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 11 minute",
                "8161",
            ),
            QueryCase::row("CALIBRATE RESOURCE DURATION '11m'", "8161"),
            QueryCase::row("CALIBRATE RESOURCE DURATION interval 11 minute", "8161"),
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 11 minute END_TIME now()",
                "8161",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE END_TIME now() START_TIME now() - interval 11 minute",
                "8161",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 11 minute DURATION interval 11 minute",
                "8161",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE DURATION interval 11 minute START_TIME now() - interval 11 minute",
                "8161",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE END_TIME now() DURATION interval 11 minute",
                "8161",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 21 minute END_TIME now() - interval 1 minute",
                "8141",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE DURATION '20m' START_TIME now() - interval 21 minute",
                "8141",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE DURATION interval 20 minute START_TIME now() - interval 21 minute",
                "8141",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 21 minute END_TIME now() - interval 20 minute",
                "7978",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 4 minute",
                "8297",
            ),
            QueryCase::row("CALIBRATE RESOURCE DURATION interval 4 minute", "8297"),
            QueryCase::row(
                "CALIBRATE RESOURCE DURATION interval 4 minute END_TIME now()",
                "8297",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 8 minute",
                "8223",
            ),
            QueryCase::row("CALIBRATE RESOURCE DURATION interval 8 minute", "8223"),
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 8 minute END_TIME now() - interval 4 minute",
                "8147",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE START_TIME now() - interval 8 minute DURATION interval 4 minute",
                "8147",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE DURATION interval 4 minute START_TIME now() - interval 8 minute",
                "8147",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE DURATION interval 4 minute END_TIME now() - interval 4 minute",
                "8147",
            ),
            QueryCase::row(
                "CALIBRATE RESOURCE END_TIME date_sub(now(), interval 4 minute ) DURATION '4m'",
                "8147",
            ),
        ]
    }

    fn ru_recent_window() -> Vec<MetricRow> {
        vec![
            ru("-20m40s", 2250.0),
            ru("-20m10s", 2200.0),
            ru("-10m10s", 2200.0),
            ru("-9m10s", 2100.0),
            ru("-8m10s", 2250.0),
            ru("-7m10s", 2300.0),
            ru("-6m10s", 2230.0),
            ru("-5m10s", 2210.0),
            ru("-4m10s", 2250.0),
            ru("-3m10s", 2330.0),
            ru("-2m10s", 2330.0),
            ru("-1m10s", 2300.0),
            ru("-10s", 2280.0),
        ]
    }

    fn cpu_recent_window() -> Vec<MetricRow> {
        let mut rows = Vec::new();
        for (instance, typ, base) in [
            ("tidb-0", "tidb", 1.2),
            ("tikv-1", "tikv", 2.2),
            ("tikv-0", "tikv", 2.28),
            ("tikv-2", "tikv", 2.11),
        ] {
            for (time, offset) in [
                ("-20m40s", 0.034),
                ("-20m10s", 0.012),
                ("-10m10s", 0.012),
                ("-9m10s", 0.033),
                ("-8m10s", 0.034),
                ("-7m10s", 0.013),
                ("-6m10s", 0.009),
                ("-5m10s", 0.013),
                ("-4m10s", 0.036),
                ("-3m10s", 0.028),
                ("-2m10s", 0.019),
                ("-1m10s", 0.020),
                ("-10s", 0.081),
            ] {
                rows.push(cpu(time, instance, typ, base + offset));
            }
        }
        rows
    }

    fn ru_static_window() -> Vec<MetricRow> {
        vec![
            ru("2020-02-12 10:35:00", 2200.0),
            ru("2020-02-12 10:36:00", 2100.0),
            ru("2020-02-12 10:37:00", 2250.0),
            ru("2020-02-12 10:38:00", 2300.0),
            ru("2020-02-12 10:39:00", 2230.0),
            ru("2020-02-12 10:40:00", 2210.0),
            ru("2020-02-12 10:41:00", 2250.0),
            ru("2020-02-12 10:42:00", 2330.0),
            ru("2020-02-12 10:43:00", 2330.0),
            ru("2020-02-12 10:44:00", 2300.0),
            ru("2020-02-12 10:45:00", 2280.0),
        ]
    }

    fn cpu_static_window(tidb_base: f64) -> Vec<MetricRow> {
        let mut rows = Vec::new();
        for minute in 35..=45 {
            rows.push(cpu(
                time_2020(minute),
                "tidb-0",
                "tidb",
                tidb_base + 0.001 * f64::from(minute - 35),
            ));
        }
        for instance in ["tikv-1", "tikv-0", "tikv-2"] {
            for minute in 35..=45 {
                rows.push(cpu(
                    time_2020(minute),
                    instance,
                    "tikv",
                    2.2 + 0.001 * f64::from(minute - 35),
                ));
            }
        }
        rows
    }

    fn ru_modify_1() -> Vec<MetricRow> {
        let mut rows = vec![
            ru("2020-02-12 10:25:00", 5.0),
            ru("2020-02-12 10:26:00", 5.0),
            ru("2020-02-12 10:27:00", 4.0),
            ru("2020-02-12 10:28:00", 6.0),
            ru("2020-02-12 10:29:00", 3.0),
            ru("2020-02-12 10:30:00", 5.0),
            ru("2020-02-12 10:31:00", 7.0),
            ru("2020-02-12 10:32:00", 5.0),
            ru("2020-02-12 10:33:00", 7.0),
            ru("2020-02-12 10:34:00", 8.0),
        ];
        rows.extend(ru_static_window());
        rows.extend([
            ru("2020-02-12 10:46:00", 5.0),
            ru("2020-02-12 10:47:00", 7.0),
            ru("2020-02-12 10:48:00", 8.0),
        ]);
        rows
    }

    fn ru_modify_2() -> Vec<MetricRow> {
        vec![
            ru("2020-02-12 10:25:00", 5.0),
            ru("2020-02-12 10:26:00", 5.0),
            ru("2020-02-12 10:27:00", 4.0),
            ru("2020-02-12 10:28:00", 6.0),
            ru("2020-02-12 10:29:00", 2200.0),
            ru("2020-02-12 10:30:00", 5.0),
            ru("2020-02-12 10:31:00", 7.0),
            ru("2020-02-12 10:32:00", 5.0),
            ru("2020-02-12 10:33:00", 7.0),
            ru("2020-02-12 10:34:00", 8.0),
            ru("2020-02-12 10:35:00", 29.0),
            ru("2020-02-12 10:36:00", 2100.0),
            ru("2020-02-12 10:37:00", 49.0),
            ru("2020-02-12 10:38:00", 2300.0),
            ru("2020-02-12 10:39:00", 2230.0),
            ru("2020-02-12 10:40:00", 2210.0),
            ru("2020-02-12 10:41:00", 47.0),
            ru("2020-02-12 10:42:00", 2330.0),
            ru("2020-02-12 10:43:00", 2330.0),
            ru("2020-02-12 10:44:00", 2300.0),
            ru("2020-02-12 10:45:00", 2280.0),
            ru("2020-02-12 10:47:00", 2250.0),
            ru("2020-02-12 10:49:00", 2250.0),
        ]
    }

    fn cpu_modify_2() -> Vec<MetricRow> {
        let mut rows = Vec::new();
        for (time, value) in [
            ("2020-02-12 10:29:00", 3.212),
            ("2020-02-12 10:36:00", 3.233),
            ("2020-02-12 10:38:00", 3.213),
            ("2020-02-12 10:39:00", 3.209),
            ("2020-02-12 10:40:00", 3.213),
            ("2020-02-12 10:42:00", 3.228),
            ("2020-02-12 10:43:00", 3.219),
            ("2020-02-12 10:44:00", 3.220),
            ("2020-02-12 10:45:00", 3.221),
            ("2020-02-12 10:46:00", 3.220),
            ("2020-02-12 10:47:00", 3.236),
            ("2020-02-12 10:48:00", 3.220),
            ("2020-02-12 10:49:00", 3.234),
        ] {
            rows.push(cpu(time, "tidb-0", "tidb", value));
        }
        for instance in ["tikv-1", "tikv-0", "tikv-2"] {
            for time in [
                "2020-02-12 10:29:00",
                "2020-02-12 10:36:00",
                "2020-02-12 10:49:00",
                "2020-02-12 10:38:00",
                "2020-02-12 10:39:00",
                "2020-02-12 10:46:00",
                "2020-02-12 10:40:00",
                "2020-02-12 10:47:00",
                "2020-02-12 10:42:00",
                "2020-02-12 10:43:00",
                "2020-02-12 10:44:00",
                "2020-02-12 10:45:00",
            ] {
                rows.push(cpu(time, instance, "tikv", 2.22));
            }
        }
        rows
    }

    fn ru_modify_3() -> Vec<MetricRow> {
        let mut rows = ru_modify_2();
        // Go 版本把 10:36 和 10:42 两个 RU 点改成带 20 秒偏移，用来验证插值结果会变化。
        rows.retain(|row| {
            !matches!(
                row.values.first(),
                Some(MetricValue::Time(
                    "2020-02-12 10:36:00" | "2020-02-12 10:42:00"
                ))
            )
        });
        rows.push(ru("2020-02-12 10:36:20", 2100.0));
        rows.push(ru("2020-02-12 10:42:20", 2330.0));
        rows
    }

    fn ru_low_workload() -> Vec<MetricRow> {
        vec![
            ru("2020-02-12 10:25:00", 2200.0),
            ru("2020-02-12 10:26:00", 2100.0),
            ru("2020-02-12 10:27:00", 2250.0),
            ru("2020-02-12 10:28:00", 2300.0),
            ru("2020-02-12 10:29:00", 2230.0),
            ru("2020-02-12 10:30:00", 2210.0),
            ru("2020-02-12 10:31:00", 2250.0),
            ru("2020-02-12 10:32:00", 2330.0),
            ru("2020-02-12 10:33:00", 2330.0),
            ru("2020-02-12 10:34:00", 2300.0),
            ru("2020-02-12 10:35:00", 2280.0),
        ]
    }

    fn ru_sparse_window() -> Vec<MetricRow> {
        vec![
            ru("2020-02-12 10:25:00", 2200.0),
            ru("2020-02-12 10:27:00", 2100.0),
            ru("2020-02-12 10:28:00", 2250.0),
            ru("2020-02-12 10:30:00", 2300.0),
            ru("2020-02-12 10:31:00", 2230.0),
            ru("2020-02-12 10:33:00", 2210.0),
            ru("2020-02-12 10:34:00", 2250.0),
            ru("2020-02-12 10:36:00", 2330.0),
            ru("2020-02-12 10:37:00", 2330.0),
            ru("2020-02-12 10:39:00", 2280.0),
            ru("2020-02-12 10:40:00", 2280.0),
            ru("2020-02-12 10:42:00", 2280.0),
            ru("2020-02-12 10:43:00", 2280.0),
        ]
    }

    fn cpu_sparse_window() -> Vec<MetricRow> {
        let mut rows = Vec::new();
        for time in [
            "2020-02-12 10:26:00",
            "2020-02-12 10:29:00",
            "2020-02-12 10:32:00",
            "2020-02-12 10:35:00",
            "2020-02-12 10:38:00",
            "2020-02-12 10:41:00",
            "2020-02-12 10:44:00",
        ] {
            rows.push(cpu(time, "tidb-0", "tidb", 3.2));
            rows.push(cpu(time, "tikv-0", "tikv", 2.28));
        }
        rows
    }

    fn cpu_mostly_idle_window() -> Vec<MetricRow> {
        let mut rows = Vec::new();
        for minute in 35..=45 {
            let tidb_value = if minute == 38 { 3.213 } else { 0.2 };
            rows.push(cpu(time_2020(minute), "tidb-0", "tidb", tidb_value));
            rows.push(cpu(
                time_2020(minute),
                "tikv-1",
                "tikv",
                if minute == 35 || minute == 40 {
                    2.2
                } else {
                    0.2
                },
            ));
            rows.push(cpu(
                time_2020(minute),
                "tikv-0",
                "tikv",
                if minute == 38 { 2.283 } else { 0.28 },
            ));
            rows.push(cpu(
                time_2020(minute),
                "tikv-2",
                "tikv",
                if minute == 35 { 2.112 } else { 0.12 },
            ));
        }
        rows
    }

    fn cpu_short_window() -> Vec<MetricRow> {
        let mut rows = Vec::new();
        for minute in 35..=38 {
            rows.push(cpu(time_2020(minute), "tidb-0", "tidb", 3.2));
            rows.push(cpu(time_2020(minute), "tikv-1", "tikv", 2.2));
            rows.push(cpu(time_2020(minute), "tikv-0", "tikv", 2.28));
            rows.push(cpu(time_2020(minute), "tikv-2", "tikv", 2.11));
        }
        rows
    }

    fn tiflash_ru_without_tiflash_cpu() -> Vec<MetricRow> {
        vec![
            ru("2023-09-19 19:50:39.322000", 465919.8102127319),
            ru("2023-09-19 19:51:39.322000", 819764.9742611333),
            ru("2023-09-19 19:52:39.322000", 520180.7089147462),
            ru("2023-09-19 19:53:39.322000", 790496.4071700446),
            ru("2023-09-19 19:54:39.322000", 545216.2174551424),
            ru("2023-09-19 19:55:39.322000", 714332.5760632281),
            ru("2023-09-19 19:56:39.322000", 577119.1037253677),
            ru("2023-09-19 19:57:39.322000", 678005.0740038564),
            ru("2023-09-19 19:58:39.322000", 592239.6784597588),
            ru("2023-09-19 19:59:39.322000", 666552.6950822703),
            ru("2023-09-19 20:00:39.322000", 689703.5663975218),
        ]
    }

    fn tiflash_tidb_tikv_cpu() -> Vec<MetricRow> {
        let mut rows = Vec::new();
        for minute in 50..=60 {
            rows.push(cpu(
                time_2023_cpu(minute, 324),
                "127.0.0.1:10080",
                "tidb",
                0.11,
            ));
            rows.push(cpu(
                time_2023_cpu(minute, 325),
                "127.0.0.1:20180",
                "tikv",
                0.04,
            ));
        }
        rows
    }

    fn tidb_server_maxprocs() -> Vec<MetricRow> {
        (50..=60)
            .map(|minute| {
                MetricRow::new(vec![
                    MetricValue::Time(time_2023_cpu(minute, 329)),
                    MetricValue::Str("127.0.0.1:10080"),
                    MetricValue::Float(20.0),
                ])
            })
            .collect()
    }

    fn tiflash_cpu() -> Vec<MetricRow> {
        vec![
            cpu(
                "2023-09-19 19:50:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                18.577777777777776,
            ),
            cpu(
                "2023-09-19 19:51:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                17.666666666666668,
            ),
            cpu(
                "2023-09-19 19:52:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                18.339038812074868,
            ),
            cpu(
                "2023-09-19 19:53:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                17.82222222222222,
            ),
            cpu(
                "2023-09-19 19:54:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                18.177777777777774,
            ),
            cpu(
                "2023-09-19 19:55:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                17.911111111111108,
            ),
            cpu(
                "2023-09-19 19:56:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                17.177777777777774,
            ),
            cpu(
                "2023-09-19 19:57:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                16.17957550838982,
            ),
            cpu(
                "2023-09-19 19:58:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                16.844444444444445,
            ),
            cpu(
                "2023-09-19 19:59:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                17.71111111111111,
            ),
            cpu(
                "2023-09-19 20:00:39.327000",
                "127.0.0.1:20292",
                "tiflash",
                18.066666666666666,
            ),
        ]
    }

    fn tiflash_ru() -> Vec<MetricRow> {
        vec![
            ru("2023-09-19 19:50:39.318000", 487049.3164728853),
            ru("2023-09-19 19:51:39.318000", 821600.8181867122),
            ru("2023-09-19 19:52:39.318000", 507566.26041673025),
            ru("2023-09-19 19:53:39.318000", 771038.8122556474),
            ru("2023-09-19 19:54:39.318000", 529128.4530634031),
            ru("2023-09-19 19:55:39.318000", 777912.9275530444),
            ru("2023-09-19 19:56:39.318000", 557595.6206041124),
            ru("2023-09-19 19:57:39.318000", 688658.1706168016),
            ru("2023-09-19 19:58:39.318000", 556400.2766714202),
            ru("2023-09-19 19:59:39.318000", 712467.4348424983),
            ru("2023-09-19 20:00:39.318000", 659167.0340155548),
        ]
    }

    fn time_2020(minute: i32) -> &'static str {
        match minute {
            35 => "2020-02-12 10:35:00",
            36 => "2020-02-12 10:36:00",
            37 => "2020-02-12 10:37:00",
            38 => "2020-02-12 10:38:00",
            39 => "2020-02-12 10:39:00",
            40 => "2020-02-12 10:40:00",
            41 => "2020-02-12 10:41:00",
            42 => "2020-02-12 10:42:00",
            43 => "2020-02-12 10:43:00",
            44 => "2020-02-12 10:44:00",
            _ => "2020-02-12 10:45:00",
        }
    }

    fn time_2023_cpu(minute: i32, micros: i32) -> &'static str {
        match (minute, micros) {
            (50, 324) => "2023-09-19 19:50:39.324000",
            (51, 324) => "2023-09-19 19:51:39.324000",
            (52, 324) => "2023-09-19 19:52:39.324000",
            (53, 324) => "2023-09-19 19:53:39.324000",
            (54, 324) => "2023-09-19 19:54:39.324000",
            (55, 324) => "2023-09-19 19:55:39.324000",
            (56, 324) => "2023-09-19 19:56:39.324000",
            (57, 324) => "2023-09-19 19:57:39.324000",
            (58, 324) => "2023-09-19 19:58:39.324000",
            (59, 324) => "2023-09-19 19:59:39.324000",
            (60, 324) => "2023-09-19 20:00:39.324000",
            (50, 325) => "2023-09-19 19:50:39.325000",
            (51, 325) => "2023-09-19 19:51:39.325000",
            (52, 325) => "2023-09-19 19:52:39.325000",
            (53, 325) => "2023-09-19 19:53:39.325000",
            (54, 325) => "2023-09-19 19:54:39.325000",
            (55, 325) => "2023-09-19 19:55:39.325000",
            (56, 325) => "2023-09-19 19:56:39.325000",
            (57, 325) => "2023-09-19 19:57:39.325000",
            (58, 325) => "2023-09-19 19:58:39.325000",
            (59, 325) => "2023-09-19 19:59:39.325000",
            (60, 325) => "2023-09-19 20:00:39.325000",
            (50, 329) => "2023-09-19 19:50:39.329000",
            (51, 329) => "2023-09-19 19:51:39.329000",
            (52, 329) => "2023-09-19 19:52:39.329000",
            (53, 329) => "2023-09-19 19:53:39.329000",
            (54, 329) => "2022-09-19 19:54:39.329000",
            (55, 329) => "2023-09-19 19:55:39.329000",
            (56, 329) => "2023-09-19 19:56:39.329000",
            (57, 329) => "2023-09-19 19:57:39.329000",
            (58, 329) => "2023-09-19 19:58:39.329000",
            (59, 329) => "2023-09-19 19:59:39.329000",
            _ => "2023-09-19 20:00:39.329000",
        }
    }
}

use crate::calibrate_resource::{
    Executor, MAX_DURATION, MIN_DURATION, MetricsResponse, MetricsServer, RuConfig, ServerInfo,
    TimePointValue, WorkloadType, dynamic_tidb_quota, dynamic_tiflash_quota,
    fetch_server_cpu_quota, get_component_cpu_query, get_ru_query, get_tiflash_cpu_query,
    get_tiflash_ru_query, get_values_from_metrics, parse_calibrate_duration,
    parse_calibrate_duration_text, setup_quotas, static_calibrate,
};
use std::time::{Duration, SystemTime};

/// 与 Go 测试 fixture 一致的 RU 单价配置。
fn config() -> RuConfig {
    RuConfig {
        read_base_cost: 0.25,
        cpu_ms_cost: 1.0 / 3.0,
        read_bytes_cost: 1.0 / 65_536.0,
        write_base_cost: 1.0,
        write_bytes_cost: 1.0 / 1024.0,
    }
}

/// 1 个 TiDB(40 核) + 3 个 TiKV(各 8 核) 的集群拓扑。
fn servers() -> Vec<ServerInfo> {
    vec![
        ServerInfo {
            server_type: "tidb".into(),
            cpu_cores: 40.0,
        },
        ServerInfo {
            server_type: "tikv".into(),
            cpu_cores: 8.0,
        },
        ServerInfo {
            server_type: "tikv".into(),
            cpu_cores: 8.0,
        },
        ServerInfo {
            server_type: "tikv".into(),
            cpu_cores: 8.0,
        },
    ]
}

/// 对照 Go：各 workload 静态校准 RU 与 fixture 一致。
#[test]
fn static_calibration_matches_go_workload_fixture() {
    assert_eq!(
        static_calibrate(WorkloadType::Tpcc, &servers(), config(), 40.0).unwrap(),
        69_768
    );
    assert_eq!(
        static_calibrate(WorkloadType::OltpReadWrite, &servers(), config(), 40.0).unwrap(),
        55_823
    );
    assert_eq!(
        static_calibrate(WorkloadType::OltpReadOnly, &servers(), config(), 40.0).unwrap(),
        34_926
    );
    assert_eq!(
        static_calibrate(WorkloadType::OltpWriteOnly, &servers(), config(), 40.0).unwrap(),
        109_776
    );
}

/// TiDB 总 CPU 不足时会压低有效 TiKV 核数；无 TiKV 时报错。
#[test]
fn static_calibration_limits_tikv_by_total_tidb_cpu() {
    assert_eq!(
        static_calibrate(WorkloadType::Tpcc, &servers(), config(), 8.0).unwrap(),
        38_760
    );
    assert!(
        static_calibrate(
            WorkloadType::Tpcc,
            &[ServerInfo {
                server_type: "tidb".into(),
                cpu_cores: 8.0
            }],
            config(),
            8.0,
        )
        .unwrap_err()
        .contains("tikv")
    );
}

/// Go 从首个可用 TiKV metrics 响应读取单机 quota，再乘实例数；不会把各
/// `ServerInfo` 中可能不同的 quota 求和。
#[test]
fn static_calibration_uses_first_server_quota_for_all_instances() {
    let heterogeneous = vec![
        ServerInfo {
            server_type: "tidb".into(),
            cpu_cores: 40.0,
        },
        ServerInfo {
            server_type: "tikv".into(),
            cpu_cores: 8.0,
        },
        ServerInfo {
            server_type: "tikv".into(),
            cpu_cores: 64.0,
        },
    ];
    assert_eq!(
        static_calibrate(WorkloadType::Tpcc, &heterogeneous, config(), 40.0).unwrap(),
        46_512
    );
}

/// 校验 duration 相对窗口与最短/最长边界。
#[test]
fn calibrate_duration_enforces_minimum_maximum_and_relative_window() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000);
    let (start, end) =
        parse_calibrate_duration(None, None, Some(Duration::from_secs(600)), now).unwrap();
    assert_eq!(end.duration_since(start).unwrap(), Duration::from_secs(600));
    assert!(
        parse_calibrate_duration(
            Some(now),
            Some(now + MIN_DURATION - Duration::from_secs(1)),
            None,
            now,
        )
        .is_err()
    );
    assert!(
        parse_calibrate_duration(
            Some(now),
            Some(now + MAX_DURATION + Duration::from_secs(61)),
            None,
            now,
        )
        .is_err()
    );
}

/// 丢弃两端极端样本后取均值；样本过少则拒绝。
#[test]
fn setup_quotas_discards_outliers_and_rejects_low_sample_count() {
    let mut samples = vec![100.0; 8];
    samples.extend([1.0, 1000.0]);
    assert_eq!(setup_quotas(samples).unwrap(), 100.0);
    assert!(setup_quotas(vec![100.0]).is_err());
}

/// 动态 TiDB 校准对齐 RU/CPU 时序，并过滤过低利用率点。
#[test]
fn dynamic_tidb_calibration_aligns_metrics_and_filters_low_usage() {
    let base = SystemTime::UNIX_EPOCH;
    let ru: Vec<_> = (0..4)
        .map(|second| TimePointValue {
            timestamp: base + Duration::from_secs(second),
            value: 1000.0,
        })
        .collect();
    let tikv: Vec<_> = (0..4)
        .map(|second| TimePointValue {
            timestamp: base + Duration::from_secs(second),
            // 首点过低，后续点进入有价值区间。
            value: if second == 0 { 0.1 } else { 4.0 },
        })
        .collect();
    let tidb: Vec<_> = (0..4)
        .map(|second| TimePointValue {
            timestamp: base + Duration::from_secs(second),
            value: if second == 0 { 0.1 } else { 2.0 },
        })
        .collect();
    assert_eq!(
        dynamic_tidb_quota(&ru, &tikv, &tidb, 8.0, 8.0).unwrap(),
        2000.0
    );
    assert!(dynamic_tidb_quota(&ru, &tikv, &tidb, 0.0, 8.0).is_err());
}

/// Go 的 `timeSeriesValues.advance` 接受严格小于 10 秒的时间偏差，
/// 不能把只在同一时间戳相等的样本才视为可对齐。
#[test]
fn dynamic_calibration_matches_go_ten_second_alignment() {
    let base = SystemTime::UNIX_EPOCH;
    let ru = vec![
        TimePointValue {
            timestamp: base + Duration::from_secs(10),
            value: 100.0,
        },
        TimePointValue {
            timestamp: base + Duration::from_secs(20),
            value: 200.0,
        },
    ];
    let tikv = vec![
        TimePointValue {
            timestamp: base + Duration::from_secs(1),
            value: 8.0,
        },
        TimePointValue {
            timestamp: base + Duration::from_secs(11),
            value: 8.0,
        },
    ];
    let tidb = vec![
        TimePointValue {
            timestamp: base + Duration::from_secs(9),
            value: 8.0,
        },
        TimePointValue {
            timestamp: base + Duration::from_secs(19),
            value: 8.0,
        },
    ];

    assert_eq!(
        dynamic_tidb_quota(&ru, &tikv, &tidb, 8.0, 8.0).unwrap(),
        150.0
    );
}

/// TiFlash 动态配额与 Executor 单次产出、禁用路径。
#[test]
fn tiflash_calibration_and_executor_are_real_and_single_shot() {
    let base = SystemTime::UNIX_EPOCH;
    let ru = vec![
        TimePointValue {
            timestamp: base,
            value: 1000.0,
        },
        TimePointValue {
            timestamp: base + Duration::from_secs(1),
            value: 1200.0,
        },
    ];
    let cpu = vec![
        TimePointValue {
            timestamp: base,
            value: 4.0,
        },
        TimePointValue {
            timestamp: base + Duration::from_secs(1),
            value: 4.0,
        },
    ];
    assert_eq!(dynamic_tiflash_quota(&ru, &cpu, 8.0).unwrap(), 2200.0);
    let mut executor = Executor::new(WorkloadType::Tpcc, true);
    assert_eq!(
        executor.next_static(&servers(), config(), 40.0).unwrap(),
        Some(69_768)
    );
    // 第二次 Next 应返回空，表示单次产出完成。
    assert_eq!(
        executor.next_static(&servers(), config(), 40.0).unwrap(),
        None
    );
    let mut disabled = Executor::new(WorkloadType::Tpcc, false);
    assert!(disabled.next_static(&servers(), config(), 40.0).is_err());
}

/// TiFlash 与 TiDB/TiKV 使用相同的严格 10 秒时序对齐规则；没有 TiFlash
/// CPU quota 时，Go 的 `setupQuotas` 会返回低 workload 错误，而不是伪造 0。
#[test]
fn tiflash_calibration_matches_go_alignment_and_empty_quota_error() {
    let base = SystemTime::UNIX_EPOCH;
    let ru = vec![
        TimePointValue {
            timestamp: base + Duration::from_secs(10),
            value: 100.0,
        },
        TimePointValue {
            timestamp: base + Duration::from_secs(20),
            value: 200.0,
        },
    ];
    let cpu = vec![
        TimePointValue {
            timestamp: base + Duration::from_secs(1),
            value: 8.0,
        },
        TimePointValue {
            timestamp: base + Duration::from_secs(11),
            value: 8.0,
        },
    ];
    assert_eq!(dynamic_tiflash_quota(&ru, &cpu, 8.0).unwrap(), 150.0);
    assert!(
        dynamic_tiflash_quota(&ru, &cpu, 0.0)
            .unwrap_err()
            .contains("workload")
    );
}

#[test]
fn executor_reports_go_resource_control_error() {
    let mut executor = Executor::new(WorkloadType::Tpcc, false);
    assert_eq!(
        executor
            .next_static(&servers(), config(), 40.0)
            .unwrap_err(),
        "Resource control feature is disabled"
    );
}

#[test]
fn duration_text_and_metric_queries_match_go_inputs() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000);
    let (_, end) = parse_calibrate_duration_text(None, None, Some("11m"), now).unwrap();
    assert_eq!(end, now);
    assert!(parse_calibrate_duration_text(None, None, Some("25h"), now).is_err());
    let (start, end) = parse_calibrate_duration_text(None, None, Some("1h30m"), now).unwrap();
    assert_eq!(
        end.duration_since(start).unwrap(),
        Duration::from_secs(5_400)
    );
    assert!(get_ru_query("start", "end").contains("resource_manager_resource_unit"));
    assert!(get_component_cpu_query("tikv", "start", "end").contains("job like '%tikv'"));
    assert!(get_tiflash_ru_query("start", "end").contains("tiflash_resource_manager"));
    assert!(get_tiflash_cpu_query("start", "end").contains("job = 'tiflash'"));
}

#[test]
fn metrics_collection_ignores_bad_rows_and_reads_first_available_server() {
    let base = SystemTime::UNIX_EPOCH;
    let series = get_values_from_metrics(Ok(vec![
        Ok(TimePointValue {
            timestamp: base + Duration::from_secs(2),
            value: 2.0,
        }),
        Err("bad timestamp".to_string()),
        Ok(TimePointValue {
            timestamp: base + Duration::from_secs(1),
            value: 1.0,
        }),
    ]))
    .unwrap();
    assert_eq!(series.values.len(), 2);
    assert!(get_values_from_metrics(Err("query metric error".to_string())).is_err());

    let servers = vec![
        MetricsServer {
            server_type: "tikv".into(),
            address: "first".into(),
            status_address: "first-status".into(),
        },
        MetricsServer {
            server_type: "tikv".into(),
            address: "second".into(),
            status_address: "second-status".into(),
        },
    ];
    let mut requests = Vec::new();
    let quota = fetch_server_cpu_quota(&servers, "tikv", "tikv_server_cpu_cores_quota", |addr| {
        requests.push(addr.to_string());
        if addr == "first-status" {
            Err("first unavailable".to_string())
        } else {
            Ok(MetricsResponse {
                status_code: 200,
                status: "200 OK".into(),
                body: "# HELP ignored\ntikv_server_cpu_cores_quota 8\n".into(),
            })
        }
    })
    .unwrap();
    assert_eq!(quota, 8.0);
    assert_eq!(requests, ["first-status", "second-status"]);
}
