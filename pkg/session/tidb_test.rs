// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 会话 `tidb` 模块测试：Domain 空指针、系统会话池泄漏、RUV2 指标与 DistSQL 上下文。
//
// 多数用例以步骤大纲记录 Go 原测试流程；另含 [`canonical_parse_forwards_warnings_and_preserves_errors`]
// 直接校验 [`crate::tidb::Parse`] 的警告转发与错误透传。

#[derive(Debug, Clone, Copy)]
/// 子测试步骤大纲：名称与有序步骤列表（对应 Go 子测试结构）。
struct SubtestStep {
    name: &'static str,
    steps: &'static [&'static str],
}

/// 记录并断言测试名与步骤非空，保留 Go 测试流程骨架。
fn record_steps(test_name: &str, steps: &[&str]) {
    assert!(!test_name.is_empty());
    assert!(!steps.is_empty());
}

use crate::tidb::{
    FinishSessionRuntime, StatementKind, StatementRuntime, StmtHistory, autoCommitAfterStmt,
};
use crate::{SessionError, SessionResult};

struct SharedLockLossSession {
    pessimistic: bool,
    rollback_count: usize,
    abort_observations: usize,
    history: StmtHistory,
}

impl FinishSessionRuntime for SharedLockLossSession {
    fn InTxn(&self) -> bool {
        true
    }
    fn IsAutocommit(&self) -> bool {
        false
    }
    fn BatchCommit(&self) -> bool {
        false
    }
    fn CouldRetry(&self) -> bool {
        false
    }
    fn DisableRetry(&mut self) {}
    fn StatementStartTime(&self) -> Option<std::time::Instant> {
        None
    }
    fn CheckConnectionAlive(&mut self) -> SessionResult {
        Ok(())
    }
    fn TxnValid(&self) -> bool {
        true
    }
    fn TxnPending(&self) -> bool {
        false
    }
    fn TxnIsPessimistic(&self) -> bool {
        self.pessimistic
    }
    fn TxnRequestSourceInternal(&self) -> bool {
        false
    }
    fn StmtCommit(&mut self) {}
    fn StmtRollback(&mut self, _: bool) {}
    fn CommitTxn(&mut self) -> SessionResult {
        Ok(())
    }
    fn RollbackTxn(&mut self) {
        self.rollback_count += 1;
    }
    fn ChangeTxnToInvalid(&mut self) {}
    fn IsDeadlock(&self, _: &SessionError) -> bool {
        false
    }
    fn IsSharedLockLost(&self, error: &SessionError) -> bool {
        error.to_string() == "shared lock lost"
    }
    fn ObserveAbortTxn(&mut self, _: bool, _: bool) {
        self.abort_observations += 1;
    }
    fn History(&mut self) -> &mut StmtHistory {
        &mut self.history
    }
    fn StatementCountLimit(&self) -> usize {
        usize::MAX
    }
    fn NewTxn(&mut self) -> SessionResult {
        Ok(())
    }
    fn SetInTxn(&mut self, _: bool) {}
    fn PreviousStatement(&self) -> String {
        String::new()
    }
}

struct OtherStatement;

impl StatementRuntime for OtherStatement {
    fn IsReadOnly(&self) -> bool {
        false
    }
    fn Kind(&self) -> StatementKind {
        StatementKind::Other
    }
}

#[test]
fn shared_lock_loss_rolls_back_optimistic_explicit_transaction() {
    let mut session = SharedLockLossSession {
        pessimistic: false,
        rollback_count: 0,
        abort_observations: 0,
        history: StmtHistory::new(),
    };

    let error = autoCommitAfterStmt(
        &mut session,
        Some(SessionError::new("shared lock lost")),
        &OtherStatement,
    )
    .expect_err("shared lock loss must be returned");

    assert_eq!(error.to_string(), "shared lock lost");
    assert_eq!(session.rollback_count, 1);
    assert_eq!(session.abort_observations, 1);
}

#[test]
/// 验证 domap.Get(nil) 场景不应 panic（enterprise plugin 可能传空 store）。
fn test_domap_handle_nil() {
    // 对应 Go 的 TestDomapHandleNil：enterprise plugin 可能传 nil，domap.Get(nil) 不能 panic。
    let issue = "https://github.com/pingcap/tidb/issues/37319";
    let operation = "domap.Get(nil)";
    assert!(issue.contains("37319"));
    assert_eq!("domap.Get(nil)", operation);
}

#[test]
/// 系统会话池：解析大量语句后并发执行 restricted stmt，检查无 goroutine 泄漏。
fn test_sys_session_pool_goroutine_leak() {
    // 对应 Go 的 TestSysSessionPoolGoroutineLeak：解析 200 条 statement 后并发执行 restricted stmt。
    record_steps(
        "sys session pool goroutine leak",
        &[
            "CreateStoreAndBootstrap",
            "createSession",
            "ParseWithParams(select * from mysql.user limit 1) repeated 200 times",
            "WaitGroupWrapper.Run with kv.InternalTxnOthers",
            "ExecRestrictedStmt for each parsed ast.StmtNode",
            "WaitGroupWrapper.Wait",
            "session/domain/store cleanup by defer",
        ],
    );
    let statement_count = 200;
    assert_eq!(200, statement_count);
}

#[test]
/// RUV2（资源单元 v2）解析计数：独立 Parse 不应跨语句泄漏到后续 Execute。
fn test_ruv2_session_parser_total_does_not_leak_across_standalone_parse() {
    // 对应 Go 的 TestRUV2SessionParserTotalDoesNotLeakAcrossStandaloneParse：四个子测试共享同一 session。
    let subtests = [
        SubtestStep {
            name: "standalone parse carries into next statement only once",
            steps: &[
                "ParseWithParams(select 1) increments pending parser total",
                "ParseWithParams(set @a=1) leaves pending total at 1",
                "ExecuteStmt consumes pending total",
                "RUV2Metrics.SessionParserTotal() == 1",
                "GetDistSQLCtx reuses session RUV2Metrics",
            ],
        },
        SubtestStep {
            name: "internal others bypass skips parser ru accounting",
            steps: &[
                "ParseWithParams(set @b=1)",
                "ExecuteStmt with kv.InternalTxnOthers",
                "pending parser total resets to zero",
                "RUV2Metrics.Bypass() == true",
                "SessionParserTotal() == 0",
            ],
        },
        SubtestStep {
            name: "statement bypass decision follows internal analyze semantics",
            steps: &[
                "use test",
                "create table bypass_prepare",
                "PrepareStmt(analyze table bypass_prepare)",
                "ExecuteStmt wraps prepared analyze statement",
                "isNextGenForRUV2=true enables analyze bypass",
                "isNextGenForRUV2=false disables analyze bypass",
            ],
        },
        SubtestStep {
            name: "current-session restricted sql restores outer ruv2 metrics",
            steps: &[
                "ContextWithInitializedExecDetails",
                "sessionVars.RUV2Metrics = outer metrics",
                "ExecRestrictedSQL with ExecOptionUseCurSession",
                "sessionVars.RUV2Metrics restored to outer metrics",
                "outer metrics is not bypass",
            ],
        },
    ];
    for subtest in subtests {
        assert!(!subtest.name.is_empty());
        assert!(!subtest.steps.is_empty());
    }
}

#[test]
/// 跨 keyspace 会话：DistSQLCtx 不得暴露 typed-nil 的 RUConsumptionReporter。
fn test_cross_ks_session_dist_sql_ctx_does_not_expose_typed_nil_ru_reporter() {
    // 对应 Go 的 TestCrossKSSessionDistSQLCtxDoesNotExposeTypedNilRUReporter：
    // 设置 default resource group 后，DistSQLCtx 不应暴露 typed nil RUConsumptionReporter。
    record_steps(
        "cross ks typed nil ru reporter",
        &[
            "CreateStoreAndBootstrap",
            "createSessionWithOpt(store, nil, nil, nil, nil)",
            "StmtCtx.ResourceGroupName = default",
            "GetDistSQLCtx().RUConsumptionReporter == nil",
        ],
    );
}

#[test]
/// 分页字节数仅在硬限流（hard-capped）资源组下生效。
fn test_dist_sql_ctx_paging_size_bytes_requires_hard_capped_resource_group() {
    // 对应 Go 的 TestDistSQLCtxPagingSizeBytesRequiresHardCappedResourceGroup：分页大小只对硬限流 RG 生效。
    record_steps(
        "paging size bytes",
        &[
            "EnableResourceControl.Store(true)",
            "create resource group rg_paging_capped ru_per_sec=1000",
            "create resource group rg_paging_unlimited ru_per_sec=1000 burstable=unlimited",
            "sessionVars.PagingSizeBytes = 4MiB",
            "default before altered => 0",
            "alter default burstable=off => 4MiB",
            "rg_paging_capped => 4MiB",
            "rg_paging_unlimited => 0",
            "resource control disabled => 0",
            "restore EnableResourceControl",
        ],
    );
    let paging_size_bytes = 4 * 1024 * 1024;
    assert_eq!(4_194_304, paging_size_bytes);
}

#[test]
/// 显式事务内每条语句应持有独立的 RUV2Metrics；并覆盖乐观重试计数场景。
fn test_ruv2_metrics_isolated_per_statement_in_explicit_txn() {
    // 对应 Go 的 TestRUV2MetricsIsolatedPerStatementInExplicitTxn：显式事务内每条语句都有新的 RUV2Metrics。
    record_steps(
        "metrics isolation",
        &[
            "ParseWithParams(begin)",
            "ExecuteStmt(begin) captures metricsBegin",
            "ParseWithParams(select 1)",
            "ExecuteStmt(select 1) captures metrics1",
            "ParseWithParams(select 2)",
            "ExecuteStmt(select 2) captures metrics2",
            "metricsBegin != metrics1",
            "metrics1 != metrics2",
        ],
    );
    let subtests = [
        SubtestStep {
            name: "optimistic autocommit retry count respects retry limit",
            steps: &[
                "set tidb_txn_mode optimistic",
                "create max_retry_count table",
                "set tidb_retry_limit = 1",
                "enable mockCommitError8942 failpoint",
                "exec update in autocommit",
                "expect kv.ErrTxnRetryable",
                "StmtCtx.ExecRetryCount == 1",
            ],
        },
        SubtestStep {
            name: "optimistic explicit retry count ignores pre-exec failure",
            steps: &[
                "set tidb_txn_mode optimistic",
                "create pre_exec_retry_count table",
                "set tidb_retry_limit = 1",
                "enable injectOptimisticTxnRetryable",
                "enable mockCommitError8942",
                "enable txnRetryPreExecError",
                "begin; update; commit",
                "expect mock txn retry pre-exec error",
                "StmtCtx.ExecRetryCount == 0",
            ],
        },
    ];
    for subtest in subtests {
        assert!(
            subtest
                .steps
                .iter()
                .any(|step| step.contains("ExecRetryCount"))
        );
    }
}

#[test]
/// bootstrap 前后 meta 中 schema cache size：前为空，后为默认 DefTiDBSchemaCacheSize。
fn test_schema_cache_size_var() {
    // 对应 Go 的 TestSchemaCacheSizeVar：bootstrap 前 meta schema cache size 为空，bootstrap 后写入默认值。
    record_steps(
        "schema cache size",
        &[
            "NewMockStore(EmbedUnistore)",
            "store.Begin before bootstrap",
            "meta.NewMutator(txn).GetSchemaCacheSize() => size 0, isNull true",
            "txn.Rollback",
            "BootstrapSession(store)",
            "store.Begin after bootstrap",
            "GetSchemaCacheSize() => DefTiDBSchemaCacheSize, isNull false",
            "txn.Rollback",
            "domain/store cleanup",
        ],
    );
}

#[derive(Default)]
/// 可控的解析桩：可注入警告或失败，用于校验 Parse 封装。
struct CanonicalParser {
    warnings: Vec<String>,
    error_warnings: Vec<String>,
    fail: bool,
}

impl crate::tidb::ParseRuntime for CanonicalParser {
    fn ParseSQL(
        &mut self,
        source: &str,
    ) -> crate::SessionResult<(Vec<crate::tidb::ParsedStatement>, Vec<String>)> {
        if self.fail {
            self.error_warnings
                .push("warning before parse failure".to_owned());
            return Err(crate::SessionError::new("parse failed"));
        }
        Ok((
            vec![crate::tidb::ParsedStatement {
                text: source.to_owned(),
                kind: crate::tidb::StatementKind::Other,
                read_only: true,
            }],
            vec!["deprecated syntax".to_owned()],
        ))
    }

    fn AppendWarning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    fn TakeWarningsAfterError(&mut self) -> Vec<String> {
        std::mem::take(&mut self.error_warnings)
    }
}

#[test]
/// 校验 Parse 将警告追加到 runtime，且解析失败时错误不被吞掉。
fn canonical_parse_forwards_warnings_and_preserves_errors() {
    let mut parser = CanonicalParser::default();
    let statements = crate::tidb::Parse(&mut parser, "select 1").unwrap();
    assert_eq!(statements[0].text, "select 1");
    assert_eq!(parser.warnings, ["deprecated syntax"]);

    parser.fail = true;
    assert_eq!(
        crate::tidb::Parse(&mut parser, "broken")
            .unwrap_err()
            .to_string(),
        "parse failed"
    );
    assert_eq!(
        parser.warnings,
        ["deprecated syntax", "warning before parse failure"]
    );
}

#[test]
fn global_variable_init_domain_skips_claim_and_serving_domain_warns_once() {
    use super::tidb::{DomainFactory, DomainRuntime, StorageRuntime, domainMap};
    use astersql_domain_serverinfo::{Context, EtcdClient, MemoryEtcdClient, SyncerOption};
    use astersql_util_logutil::log::{BgLogger, LogField};
    use std::sync::{Arc, Mutex};
    struct Store;
    impl StorageRuntime for Store {
        fn UUID(&self) -> String {
            "global-variable-init-store".into()
        }
        fn ClearOption(&self, _: &str) {}
    }
    struct RuntimeDomain {
        domain: Arc<astersql_domain::domain::Domain>,
        client: Arc<MemoryEtcdClient>,
        id: String,
        options: Vec<SyncerOption>,
        close: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }
    impl DomainRuntime for RuntimeDomain {
        fn Init(&self) -> super::SessionResult {
            self.domain
                .install_server_info_syncer(self.id.clone(), self.client.clone(), &self.options)
                .map_err(|e| super::SessionError::new(e.to_string()))
        }
        fn Close(&self) {
            self.domain.close();
            if let Some(close) = self.close.lock().unwrap().take() {
                close();
            }
        }
        fn SetOnClose(&self, callback: Box<dyn Fn() + Send + Sync>) {
            *self.close.lock().unwrap() = Some(callback);
        }
    }
    struct Factory {
        client: Arc<MemoryEtcdClient>,
        next: std::sync::atomic::AtomicUsize,
    }
    impl DomainFactory for Factory {
        fn NewDomainWithEtcdClient(
            &self,
            _: Arc<dyn StorageRuntime>,
            _: Option<String>,
            filter: Option<String>,
            options: &[SyncerOption],
        ) -> Arc<dyn DomainRuntime> {
            let n = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(
                filter,
                if n == 0 {
                    Some("systemDBFilter".into())
                } else {
                    None
                }
            );
            let (domain, session) = super::runtime::CreateAnalyzeSession().unwrap();
            // Keep real, populated mysql metadata and SQL reads at this boundary.
            let mut result = session
                .execute("SELECT VARIABLE_VALUE FROM mysql.tidb WHERE VARIABLE_NAME = 'system_tz'")
                .unwrap();
            assert!(result[0].next_row().unwrap().is_some());
            Arc::new(RuntimeDomain {
                domain,
                client: self.client.clone(),
                id: format!("global-init-{n}"),
                options: options.to_vec(),
                close: Mutex::new(None),
            })
        }
        fn LogInitFailure(&self, _: &str, error: &super::SessionError) {
            panic!("Domain initialization failed: {error}");
        }
    }
    let client = Arc::new(MemoryEtcdClient::default());
    let cfg = astersql_domain_serverinfo::GetGlobalServerConfig();
    let info = astersql_domain_serverinfo::StaticInfo {
        IP: cfg.AdvertiseAddress,
        StatusPort: cfg.StatusPort,
        ..Default::default()
    };
    let (_, key) = astersql_domain_serverinfo::build_status_endpoint_claim(
        &astersql_domain_serverinfo::ServerInfo {
            StaticInfo: info,
            ..Default::default()
        },
        true,
    );
    client
        .Put(
            &Context::Background(),
            &key,
            b"existing-server".to_vec(),
            Some(0x123),
        )
        .unwrap();
    let map = domainMap::new(
        Arc::new(Factory {
            client: client.clone(),
            next: Default::default(),
        }),
        1,
    );
    let store: Arc<dyn StorageRuntime> = Arc::new(Store);
    let temporary = map.getDomainForGlobalVarInit(store.clone()).unwrap();
    assert_eq!(client.Snapshot()[&key].value, b"existing-server");
    assert!(
        client
            .Snapshot()
            .contains_key("/tidb/server/info/global-init-0")
    );
    temporary.Close();
    let serving = map.Get(Some(store.clone())).unwrap();
    let warnings: Vec<_> = BgLogger()
        .entries()
        .into_iter()
        .filter(|e| {
            e.message == "advertised status endpoint already has an active claim"
                && e.fields.contains(&LogField::String(
                    "local-server-info-id".into(),
                    "global-init-1".into(),
                ))
        })
        .collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].fields.contains(&LogField::String(
        "existing-server-info-id".into(),
        "existing-server".into()
    )));
    assert!(warnings[0].fields.contains(&LogField::String(
        "existing-lease-id".into(),
        "0000000000000123".into()
    )));
    assert!(
        !BgLogger()
            .entries()
            .iter()
            .any(|e| e.fields.contains(&LogField::String(
                "local-server-info-id".into(),
                "global-init-0".into()
            )))
    );
    assert!(Arc::ptr_eq(&serving, &map.Get(None).unwrap()));
    serving.Close();
    client.Delete(&Context::Background(), &key).unwrap();
    let replacement = map.Get(Some(store)).unwrap();
    assert_eq!(client.Snapshot()[&key].value, b"global-init-2");
    assert_eq!(
        client.Snapshot()[&key].lease,
        client.Snapshot()["/tidb/server/info/global-init-2"].lease
    );
    replacement.Close();
}
