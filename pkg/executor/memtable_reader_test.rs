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

// memtable_reader 中 failpoint 服务地址解析的单元测试。
//
// Failpoint（故障注入点）用于在测试中模拟节点故障；服务信息字符串
// 描述 TiDB/TiKV 等组件的类型与监听地址，供内存表读取器定位探测目标。

use crate::memtable_reader::parseFailpointServerInfo;

/// 验证多段地址均被保留，且字段不足的行被拒绝。
#[test]
fn memtable_failpoint_server_parser_preserves_all_addresses_and_rejects_short_rows() {
    // 分号分隔多台服务器；逗号分隔类型与地址字段。
    let servers =
        parseFailpointServerInfo("tidb,db:4000,status:10080;tikv,kv:20160,kv:20180").unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0].serverType, "tidb");
    assert_eq!(servers[1].statusAddr, "kv:20180");
    // 缺少必需字段时应返回错误。
    assert!(parseFailpointServerInfo("tidb,missing").is_err());
}

use crate::memtable_reader::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct LogCancellation(AtomicBool);
impl cancellation for LogCancellation {
    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct LogFixtureStream(Option<Vec<logMessage>>);
impl logStream for LogFixtureStream {
    fn next_messages(&mut self) -> MemTableResult<Option<Vec<logMessage>>> {
        Ok(self.0.take())
    }
}
struct LogFixtureRuntime {
    messages: Vec<logMessage>,
    requests: Mutex<Vec<logSearchRequest>>,
}
impl memTableRuntime for LogFixtureRuntime {
    fn executor_open(&self) -> MemTableResult {
        panic!("unexpected executor_open call")
    }
    fn probe_transaction(&self) -> MemTableResult<bool> {
        panic!("unexpected probe_transaction call")
    }
    fn activate_transaction(&self) -> MemTableResult {
        panic!("unexpected activate_transaction call")
    }
    fn internal_http_schema(&self) -> String {
        panic!("unexpected internal_http_schema call")
    }
    fn has_config_privilege(&self) -> bool {
        panic!("unexpected has_config_privilege call")
    }
    fn has_process_privilege(&self) -> bool {
        true
    }
    fn cluster_servers(&self) -> MemTableResult<Vec<serverInfo>> {
        Ok(vec![serverInfo {
            serverType: "pd".into(),
            address: "pd:2379".into(),
            statusAddr: "pd:2379".into(),
        }])
    }
    fn pd_servers(&self) -> MemTableResult<Vec<serverInfo>> {
        panic!("unexpected pd_servers call")
    }
    fn fetch_cluster_config(
        &self,
        server: &serverInfo,
        url: &str,
    ) -> MemTableResult<Vec<configItem>> {
        panic!("unexpected fetch_cluster_config call")
    }
    fn fetch_server_info(
        &self,
        server: &serverInfo,
        info_type: serverInfoType,
    ) -> MemTableResult<Vec<datumRow>> {
        panic!("unexpected fetch_server_info call")
    }
    fn new_log_cancellation(&self) -> Arc<dyn cancellation> {
        Arc::new(LogCancellation::default())
    }
    fn open_log_stream(
        &self,
        server: &serverInfo,
        remote: &str,
        request: &logSearchRequest,
        cancellation: Arc<dyn cancellation>,
    ) -> MemTableResult<Box<dyn logStream>> {
        assert_eq!(server.serverType, "pd");
        assert_eq!(remote, "pd:2379");
        self.requests.lock().unwrap().push(request.clone());
        let messages = self
            .messages
            .iter()
            .filter(|message| {
                (request.startTime..=request.endTime).contains(&message.time)
                    && request.patterns.iter().all(|pattern| {
                        scalar_log_match("regexp_instr", &message.message, pattern, None)
                    })
            })
            .cloned()
            .collect();
        Ok(Box::new(LogFixtureStream(Some(messages))))
    }
    fn fetch_hot_regions(
        &self,
        pd_server: &serverInfo,
        request: &HistoryHotRegionsRequest,
    ) -> MemTableResult<HistoryHotRegions> {
        panic!("unexpected fetch_hot_regions call")
    }
    fn ensure_tikv_storage(&self) -> MemTableResult {
        panic!("unexpected ensure_tikv_storage call")
    }
    fn hot_region_table_mappings(
        &self,
        region: &HistoryHotRegion,
    ) -> MemTableResult<Vec<tableMapping>> {
        panic!("unexpected hot_region_table_mappings call")
    }
    fn format_timestamp(&self, unix_millis: i64) -> MemTableResult<String> {
        Ok(unix_millis.to_string())
    }
    fn all_regions(&self) -> MemTableResult<Vec<regionInfo>> {
        panic!("unexpected all_regions call")
    }
    fn regions_by_store(&self, store_id: u64) -> MemTableResult<Vec<regionInfo>> {
        panic!("unexpected regions_by_store call")
    }
    fn region_by_id(&self, region_id: u64) -> MemTableResult<Option<regionInfo>> {
        panic!("unexpected region_by_id call")
    }
    fn append_warning(&self, warning: MemTableError) {
        panic!("unexpected log warning: {warning:?}")
    }
    fn register_runtime_stats(&self, stats: runtimeStats) {
        panic!("unexpected register_runtime_stats call")
    }
}

fn scalar_log_match(function: &str, value: &str, pattern: &str, escape: Option<u8>) -> bool {
    let context = astersql_expression_exprstatic::NewExprContext(vec![]);
    let mut arguments: Vec<Box<dyn astersql_expression::Expression>> = vec![
        Box::new(astersql_expression::NewStrConst(value)),
        Box::new(astersql_expression::NewStrConst(pattern)),
    ];
    if let Some(escape) = escape {
        arguments.push(Box::new(astersql_expression::NewInt64Const(i64::from(
            escape,
        ))));
    }
    let expression = astersql_expression::NewFunctionBase(
        &context,
        function,
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
        arguments,
    )
    .unwrap();
    assert!(
        expression
            .as_any()
            .is::<astersql_expression::ScalarFunction>()
    );
    let (value, is_null) = expression
        .EvalInt(
            context.GetEvalCtx(),
            astersql_expression::chunk::Row::default(),
        )
        .unwrap();
    !is_null && value != 0
}

#[test]
fn cluster_log_ilike_prefilter_retrieves_nonempty_logs_and_rechecks_scalars() {
    use astersql_planner_core::{ClusterLogTableExtractor, LikeEscape, Predicate, PredicateValue};
    let mixed_pattern = "(?i:^.*pd.*$)|^.*tikv.*$";
    assert!(scalar_log_match("regexp_instr", "PD", mixed_pattern, None));
    assert!(!scalar_log_match(
        "regexp_instr",
        "TIKV",
        mixed_pattern,
        None
    ));
    for (pattern, escape, expected_pattern, expected_count) in [
        ("%FOO%", b'\\', "(?i:^.*FOO.*$)", 1),
        ("%NoSuchMessage%", b'\\', "(?i:^.*NoSuchMessage.*$)", 0),
        ("%FOO#%%", b'#', "(?i:^.*FOO%.*$)", 1),
        ("%k%", b'\\', "(?i:^.*k.*$)", 0),
    ] {
        let predicate = Predicate::Ilike(
            "message".into(),
            pattern.into(),
            LikeEscape::Constant(escape),
        );
        let mut planner = ClusterLogTableExtractor::default();
        let remaining = planner.ExtractPredicates(&[
            Predicate::Ge("time".into(), PredicateValue::I64(100)),
            Predicate::Le("time".into(), PredicateValue::I64(100)),
            predicate.clone(),
        ]);
        assert_eq!(remaining, [predicate]);
        assert_eq!(planner.Patterns, [expected_pattern]);
        let message = if pattern == "%k%" {
            "K"
        } else {
            "[test log message pd 5, foo%]"
        };
        let runtime = Arc::new(LogFixtureRuntime {
            messages: vec![
                logMessage {
                    time: 100,
                    level: "critical".into(),
                    message: message.into(),
                },
                logMessage {
                    time: 99,
                    level: "critical".into(),
                    message: "outside time window foo%".into(),
                },
                logMessage {
                    time: 100,
                    level: "info".into(),
                    message: "unrelated".into(),
                },
            ],
            requests: Mutex::new(Vec::new()),
        });
        let mut retriever = clusterLogRetriever {
            isDrained: false,
            retrieving: false,
            heap: logResponseHeap(Vec::new()),
            cancel: None,
            extractor: clusterLogTableExtractor {
                patterns: planner.Patterns.clone(),
                startTime: planner.StartTime,
                endTime: planner.EndTime,
                ..Default::default()
            },
            runtime: runtime.clone(),
        };
        let rows = retriever
            .retrieve()
            .expect("ILIKE-only message filter must pass the full-log scan guard");
        assert_eq!(rows.len(), usize::from(pattern != "%NoSuchMessage%"));
        let rows = rows
            .into_iter()
            .filter(|row| match &row[4] {
                datum::String(value) => scalar_log_match("ilike", value, pattern, Some(escape)),
                _ => panic!("log message must be a string"),
            })
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), expected_count);
        if let Some(row) = rows.first() {
            assert_eq!(row[3], datum::String("CRITICAL".into()));
            assert_eq!(
                row[4],
                datum::String("[test log message pd 5, foo%]".into())
            );
        }
        assert_eq!(
            runtime.requests.lock().unwrap()[0].patterns,
            [expected_pattern]
        );
        assert!(retriever.retrieve().unwrap().is_empty());
        retriever.close().unwrap();
        assert!(retriever.cancel.is_none());
    }
}
