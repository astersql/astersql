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

// `analyze_utils` / `analyze` 中与统计信息收集相关的辅助逻辑单元测试。
//
// ANALYZE 用于采集表/索引的直方图、NDV（不同值个数）等优化器统计信息。

#![allow(non_snake_case)]

use crate::analyze::{
    AnalyzeError, analyzeColumnsPlanTask, analyzeContext, analyzePlan,
    canBroadcastToTiDBRPCForTest, collectStatsDeltaFlushObjectsForAnalyze,
    isUnsupportedBroadcastQueryErr,
};
use crate::analyze_utils::{
    AnalyzeContext, AnalyzeError as UtilsAnalyzeError, AnalyzeErrorKind, AnalyzeResults,
    AnalyzeSessionContext, DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY,
    GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED, NewAnalyzeResultsNotifyWaitGroupWrapper, NotifyChannel,
    StoreStatusError, TIDB_BUILD_SAMPLING_STATS_CONCURRENCY, TIDB_BUILD_STATS_CONCURRENCY,
    adaptiveAnlayzeDistSQLConcurrency, getAnalyzePanicErr, getBuildSamplingStatsConcurrency,
    getBuildStatsConcurrency, normalizeCtxErrWithCause,
};
use std::sync::Mutex;

struct MockAnalyzeSessionContext {
    configured: i64,
    variable: Result<String, UtilsAnalyzeError>,
    stores: Result<Option<usize>, StoreStatusError>,
    warnings: Mutex<Vec<(String, Option<UtilsAnalyzeError>)>>,
}

impl AnalyzeSessionContext for MockAnalyzeSessionContext {
    fn analyze_dist_sql_scan_concurrency(&self) -> i64 {
        self.configured
    }

    fn get_session_or_global_system_var(&self, _name: &str) -> Result<String, UtilsAnalyzeError> {
        self.variable.clone()
    }

    fn tikv_store_count(&self, _ctx: &AnalyzeContext) -> Result<Option<usize>, StoreStatusError> {
        self.stores.clone()
    }

    fn warn(&self, message: &str, error: Option<&UtilsAnalyzeError>) {
        self.warnings
            .lock()
            .unwrap()
            .push((message.into(), error.cloned()));
    }
}

fn mock_context(
    configured: i64,
    stores: Result<Option<usize>, StoreStatusError>,
) -> MockAnalyzeSessionContext {
    MockAnalyzeSessionContext {
        configured,
        variable: Ok("7".into()),
        stores,
        warnings: Mutex::new(Vec::new()),
    }
}

#[test]
/// 校验「对端不支持该 exec 类型」类广播错误能被正确识别。
fn TestIsUnsupportedBroadcastQueryErr() {
    assert!(isUnsupportedBroadcastQueryErr(&AnalyzeError(
        "other error: this exec type 17 doesn't support yet".into(),
    )));
    assert!(isUnsupportedBroadcastQueryErr(&AnalyzeError(
        "this exec type 17 doesn't support yet".into(),
    )));

    for message in [
        "context canceled",
        "region unavailable",
        "exec type mismatch",
    ] {
        assert!(!isUnsupportedBroadcastQueryErr(&AnalyzeError(
            message.into()
        )));
    }
}

#[test]
/// 校验 panic 载荷能映射为 OOM 或普通 worker panic 错误种类。
fn TestGetAnalyzePanicErr() {
    let memory_error = getAnalyzePanicErr(&GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED);
    assert_eq!(memory_error.kind, AnalyzeErrorKind::AnalyzeOutOfMemory);
    assert!(!memory_error.to_string().contains("%!(EXTRA"));

    let ordinary = getAnalyzePanicErr(&"ordinary panic");
    assert_eq!(ordinary.kind, AnalyzeErrorKind::AnalyzeWorkerPanic);
}

#[test]
fn analyze_dist_sql_concurrency_matches_go_thresholds_and_fallbacks() {
    let ctx = AnalyzeContext::default();
    for (stores, expected) in [
        (0, 15),
        (5, 15),
        (6, 6),
        (10, 10),
        (11, 22),
        (20, 40),
        (21, 63),
        (50, 150),
        (51, 204),
    ] {
        let session = mock_context(0, Ok(Some(stores)));
        assert_eq!(adaptiveAnlayzeDistSQLConcurrency(&ctx, &session), expected);
        assert!(session.warnings.lock().unwrap().is_empty());
    }

    let configured = mock_context(
        23,
        Err(StoreStatusError::GetStores(UtilsAnalyzeError::other(
            "unused",
        ))),
    );
    assert_eq!(adaptiveAnlayzeDistSQLConcurrency(&ctx, &configured), 23);
    assert!(configured.warnings.lock().unwrap().is_empty());

    for stores in [
        Ok(None),
        Err(StoreStatusError::PdHttpClient(UtilsAnalyzeError::other(
            "pd",
        ))),
        Err(StoreStatusError::GetStores(UtilsAnalyzeError::other(
            "stores",
        ))),
    ] {
        let session = mock_context(0, stores);
        assert_eq!(
            adaptiveAnlayzeDistSQLConcurrency(&ctx, &session),
            DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY
        );
        assert_eq!(session.warnings.lock().unwrap().len(), 1);
    }
}

#[test]
fn session_concurrency_and_context_cause_match_go_error_behavior() {
    for name in [
        TIDB_BUILD_STATS_CONCURRENCY,
        TIDB_BUILD_SAMPLING_STATS_CONCURRENCY,
    ] {
        let mut session = mock_context(0, Ok(Some(1)));
        session.variable = Ok("19".into());
        let actual = if name == TIDB_BUILD_STATS_CONCURRENCY {
            getBuildStatsConcurrency(&session)
        } else {
            getBuildSamplingStatsConcurrency(&session)
        };
        assert_eq!(actual.unwrap(), 19);
    }

    let mut invalid = mock_context(0, Ok(Some(1)));
    invalid.variable = Ok("not-an-int".into());
    assert!(getBuildStatsConcurrency(&invalid).is_err());

    let cause = UtilsAnalyzeError::other("root cause");
    let ctx = AnalyzeContext {
        cause: Some(cause.clone()),
    };
    assert_eq!(
        normalizeCtxErrWithCause(&ctx, Some(UtilsAnalyzeError::canceled("canceled"))),
        Some(cause.clone())
    );
    assert_eq!(
        normalizeCtxErrWithCause(&ctx, Some(UtilsAnalyzeError::deadline_exceeded("deadline"))),
        Some(cause)
    );
    let other = UtilsAnalyzeError::other("other");
    assert_eq!(
        normalizeCtxErrWithCause(&ctx, Some(other.clone())),
        Some(other)
    );
    assert_eq!(normalizeCtxErrWithCause(&ctx, None), None);
}

#[test]
fn analyze_result_wrapper_closes_only_after_every_registered_worker() {
    let notify = NotifyChannel::<AnalyzeResults>::new();
    let wrapper = NewAnalyzeResultsNotifyWaitGroupWrapper(notify.clone());
    wrapper.Add(2);
    let first = wrapper.Run(|| {});
    let second = wrapper.Run(|| {});
    first.join().unwrap();
    second.join().unwrap();
    assert!(notify.is_closed());
    assert_eq!(notify.recv(), None);
}

#[test]
/// 带点号库/表名时，收集待 FLUSH 的 stats delta 对象应去重且排序稳定。
/// stats delta：相对上次落盘的统计增量。
fn TestCollectStatsDeltaFlushObjectsForAnalyzeDottedNames() {
    let plan = analyzePlan {
        columnTasks: vec![
            analyzeColumnsPlanTask {
                databaseName: "a.b".into(),
                tableName: "c".into(),
                ..Default::default()
            },
            analyzeColumnsPlanTask {
                databaseName: "a".into(),
                tableName: "b.c".into(),
                ..Default::default()
            },
            analyzeColumnsPlanTask {
                databaseName: "a".into(),
                tableName: "b.c".into(),
                ..Default::default()
            },
        ],
    };

    let mut targets = collectStatsDeltaFlushObjectsForAnalyze(&plan)
        .into_iter()
        .map(|object| (object.databaseName, object.tableName))
        .collect::<Vec<_>>();
    targets.sort();
    assert_eq!(
        targets,
        vec![("a".into(), "b.c".into()), ("a.b".into(), "c".into())]
    );
}

#[test]
/// 空地址或非法地址不得被视为可到达的 TiDB RPC 端点。
fn TestCanBroadcastToTiDBRPCForTestRejectsInvalidEndpoints() {
    let context = analyzeContext::default();
    assert!(!canBroadcastToTiDBRPCForTest(
        &context,
        &[String::new(), String::new()],
    ));
    assert!(!canBroadcastToTiDBRPCForTest(
        &context,
        &["not-an-address".into()],
    ));
}
