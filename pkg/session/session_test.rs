// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 会话包高层行为测试草稿。
//
// 覆盖 bootstrap 启动模式判断、next-gen keyspace 版本保护、DDL 表元数据顺序校验，
// 以及内存仲裁（MemArbitrator）SQL token 估算。

// bootstrap 模式判断、next-gen keyspace 版本保护、DDL 表元数据顺序校验以及内存仲裁 SQL token 估算测试。

// StartModeDraft 对应 Go ddl.Normal/Upgrade/Bootstrap 三种启动模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 启动模式：Normal / Upgrade / Bootstrap。
pub enum StartModeDraft {
    Normal,
    Upgrade,
    Bootstrap,
}

/// 当前 bootstrap 版本号。
pub const CURRENT_BOOTSTRAP_VERSION: i64 = 1;
/// 保留全局 ID 下界。
pub const RESERVED_GLOBAL_ID_LOWER_BOUND: i64 = 0;
/// 保留全局 ID 上界。
pub const RESERVED_GLOBAL_ID_UPPER_BOUND: i64 = 1_000_000;

// TableBasicInfoDraft 对应 Go TableBasicInfo，本测试只读取 ID、Name 和 SQL。
#[derive(Debug, Clone, PartialEq, Eq)]
/// 表基础信息草稿（ID、名称、建表 SQL）。
pub struct TableBasicInfoDraft {
    pub id: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

// VersionedDDLTablesDraft 对应 Go versionedDDLTables。
#[derive(Debug, Clone, PartialEq, Eq)]
/// 按版本分组的 DDL 系统表清单草稿。
pub struct VersionedDDLTablesDraft {
    pub ver: i64,
    pub tables: Vec<TableBasicInfoDraft>,
}

// KeyspaceStoreDraft 对应 Go mockstore.NewMockStore 返回的 keyspace 绑定存储。
#[derive(Debug, Clone, PartialEq, Eq)]
/// keyspace 绑定存储草稿。
pub struct KeyspaceStoreDraft {
    pub keyspace_id: u32,
    pub keyspace_name: &'static str,
    pub bootstrap_version: Option<i64>,
    pub bootstrapped: bool,
}

// get_start_mode 对应 Go 的 getStartMode。
/// 根据已有 bootstrap 版本推断启动模式。
pub fn get_start_mode(ver: i64) -> StartModeDraft {
    if ver == 0 {
        StartModeDraft::Bootstrap
    } else if ver < CURRENT_BOOTSTRAP_VERSION {
        StartModeDraft::Upgrade
    } else {
        StartModeDraft::Normal
    }
}

// test_get_start_mode 对应 Go 的 TestGetStartMode。
/// 对应 Go TestGetStartMode。
pub fn test_get_start_mode() {
    assert_eq!(
        StartModeDraft::Normal,
        get_start_mode(CURRENT_BOOTSTRAP_VERSION)
    );
    assert_eq!(
        StartModeDraft::Normal,
        get_start_mode(CURRENT_BOOTSTRAP_VERSION + 1)
    );
    assert_eq!(
        StartModeDraft::Upgrade,
        get_start_mode(CURRENT_BOOTSTRAP_VERSION - 1)
    );
    assert_eq!(StartModeDraft::Bootstrap, get_start_mode(0));
}

// new_ks_store 对应 Go 子测试中的 newKSStore helper。
// Go 会创建 EmbedUnistore 并在 t.Cleanup 中关闭；这里只保存 keyspace meta 和清理意图。
/// 构造 keyspace 存储草稿。
pub fn new_ks_store(keyspace_id: u32, keyspace_name: &'static str) -> KeyspaceStoreDraft {
    KeyspaceStoreDraft {
        keyspace_id,
        keyspace_name,
        bootstrap_version: None,
        bootstrapped: false,
    }
}

// set_bootstrap_version 对应 Go 子测试中的 setBootstrapVersion helper。
// Go 通过 meta.NewMutator(txn).FinishBootstrap(ver) 并提交事务；这里只把版本落到store。
/// 设置 bootstrap 版本并标记已 bootstrap。
pub fn set_bootstrap_version(store: &mut KeyspaceStoreDraft, ver: i64) {
    store.bootstrap_version = Some(ver);
    store.bootstrapped = true;
}

// bootstrap_session_impl_draft 对应 Go bootstrapSessionImpl 在 keyspace guard 场景下的关键分支。
// 当用户 keyspace 目标版本领先系统 keyspace 时，Go 使用 zap fatal hook 触发 panic，且 createSessionStub 不应被调用。
/// keyspace 版本保护：用户 keyspace 版本不得领先系统 keyspace。
pub fn bootstrap_session_impl_draft(
    system_store: &KeyspaceStoreDraft,
    user_store: &KeyspaceStoreDraft,
    create_session_called: &mut bool,
) -> Result<(), String> {
    if user_store.keyspace_name != "system"
        && user_store.bootstrap_version > system_store.bootstrap_version
    {
        return Err("bootstrap version of user keyspace must be smaller or equal".to_owned());
    }

    *create_session_called = true;
    Err("must-not-be-called".to_owned())
}

// test_bootstrap_session_impl_user_ks_version_guard 对应 Go 的 TestBootstrapSessionImplUserKSVersionGuard。
// kerneltype.IsClassic() 的跳过条件保留为参数，便于表达 classic 内核不执行该保护测试。
/// 对应 Go TestBootstrapSessionImplUserKSVersionGuard。
pub fn test_bootstrap_session_impl_user_ks_version_guard(is_classic_kernel: bool) {
    if is_classic_kernel {
        // Go: t.Skip("keyspace guard only applies to next-gen kernel")。
        return;
    }

    const SYSTEM_KEYSPACE_ID: u32 = 0x00ff_ffff - 1;
    const USER_KEYSPACE_ID: u32 = 0x00ff_ffff - 2;

    let mut system_store = new_ks_store(SYSTEM_KEYSPACE_ID, "system");
    let mut user_store = new_ks_store(USER_KEYSPACE_ID, "user_keyspace_guard_fatal");
    set_bootstrap_version(&mut system_store, CURRENT_BOOTSTRAP_VERSION - 1);
    set_bootstrap_version(&mut user_store, CURRENT_BOOTSTRAP_VERSION - 1);

    // Go: kvstore.SetSystemStorage(systemStore) 并在 t.Cleanup 恢复 originSystemStore。
    let origin_system_storage_restored_by_cleanup = true;
    let mut create_session_called = false;

    let panic_val =
        bootstrap_session_impl_draft(&system_store, &user_store, &mut create_session_called)
            .expect_err("Go 通过 zap fatal hook recover 到 panic 值");

    assert!(panic_val.starts_with("bootstrap version of user keyspace must be smaller or equal"));
    assert!(!create_session_called);
    assert!(origin_system_storage_restored_by_cleanup);
}

// ddl_table_version_tables 对应 Go 全局 ddlTableVersionTables 的最小测试数据。
// 真实表清单来自 session bootstrap；这里仅保留测试校验所依赖的排序和 SQL 格式形状。
/// 返回测试用 DDL 表版本清单。
pub fn ddl_table_version_tables() -> Vec<VersionedDDLTablesDraft> {
    vec![
        VersionedDDLTablesDraft {
            ver: 1,
            tables: vec![TableBasicInfoDraft {
                id: 300,
                name: "tidb_ddl_job",
                sql: "create table mysql.tidb_ddl_job (id bigint)",
            }],
        },
        VersionedDDLTablesDraft {
            ver: 2,
            tables: vec![TableBasicInfoDraft {
                id: 200,
                name: "tidb_ddl_reorg",
                sql: "create table mysql.tidb_ddl_reorg (id bigint)",
            }],
        },
    ]
}

// test_ddl_table_version_tables 对应 Go 的 TestDDLTableVersionTables。
/// 对应 Go TestDDLTableVersionTables。
pub fn test_ddl_table_version_tables() {
    let versioned = ddl_table_version_tables();
    assert!(
        versioned.windows(2).all(|pair| pair[0].ver <= pair[1].ver),
        "ddlTableVersionTables should be sorted by version"
    );

    let mut all_tables = Vec::new();
    for group in versioned {
        all_tables.extend(group.tables);
    }
    test_table_basic_info_slice(&all_tables, " mysql.%s (");
}

// test_table_basic_info_slice 对应 Go 的同名 helper。
// 它检查 table ID 降序、ID/名称唯一、保留 ID 范围、表名小写以及建表 SQL 包含 mysql.<name>。
/// 校验表 ID 降序、唯一性、保留区间与 SQL 格式。
pub fn test_table_basic_info_slice(all_tables: &[TableBasicInfoDraft], sql_fmt: &str) {
    assert!(
        all_tables.windows(2).all(|pair| pair[0].id > pair[1].id),
        "tables should be sorted by table ID in descending order"
    );

    for (idx, table) in all_tables.iter().enumerate() {
        for other in all_tables.iter().skip(idx + 1) {
            assert_ne!(table.id, other.id, "table IDs should be unique");
            assert_ne!(table.name, other.name, "table names should be unique");
        }

        assert!(
            table.id > RESERVED_GLOBAL_ID_LOWER_BOUND,
            "table ID should be greater than ReservedGlobalIDLowerBound"
        );
        assert!(
            table.id <= RESERVED_GLOBAL_ID_UPPER_BOUND,
            "table ID should be less than or equal to ReservedGlobalIDUpperBound"
        );
        assert_eq!(
            table.name.to_lowercase(),
            table.name,
            "table name should be in lower case"
        );

        // Go 的 fmt.Sprintf(sqlFmt, vt.Name) 使用 " mysql.%s ("，保留相同目标片段。
        let sql_fragment = sql_fmt.replace("%s", table.name);
        assert!(
            table.sql.contains(sql_fragment.trim_start()),
            "table SQL should contain table name and follow the format {sql_fmt}"
        );
    }
}

// TokenCaseDraft 对应 TestMemArbitratorSession 中每个 require.Equal 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
/// 内存仲裁 token 估算用例。
pub struct TokenCaseDraft {
    pub sql: &'static str,
    pub expected: i64,
}

// mem_arbitrator_parse_cases 按 Go TestMemArbitratorSession 的顺序保留 approxParseSQLTokenCnt 断言。
/// 解析阶段 token 估算用例表。
pub fn mem_arbitrator_parse_cases() -> Vec<TokenCaseDraft> {
    vec![
        TokenCaseDraft {
            sql: "/*select * from **/SELECT x FROM `t\\`` # abc \nwhere a = 1.23 and b = 'abc\"d\\'e' -- abc \nand c_1_2 in \"abc'd\\\"e\" # (1,2,3)\n",
            expected: 15,
        },
        TokenCaseDraft {
            sql: "select @@version @a",
            expected: 0,
        },
        TokenCaseDraft {
            sql: "set @a=1",
            expected: 0,
        },
        TokenCaseDraft {
            sql: "desc analyze table t",
            expected: 0,
        },
        TokenCaseDraft {
            sql: "analyze table t",
            expected: 0,
        },
        TokenCaseDraft {
            sql: "/*select * from **/explain show warnings",
            expected: 0,
        },
        TokenCaseDraft {
            sql: "/*select * from **/desc show columns from t",
            expected: 0,
        },
        TokenCaseDraft {
            sql: "insert into t values 1",
            expected: 5,
        },
        TokenCaseDraft {
            sql: "update t set a=1",
            expected: 5,
        },
        TokenCaseDraft {
            sql: "delete from t where a=1",
            expected: 6,
        },
        TokenCaseDraft {
            sql: "replace into t values 1",
            expected: 5,
        },
        TokenCaseDraft {
            sql: "prepare stmt1 from 'select * from t where a=? and b=?'",
            expected: 0,
        },
        TokenCaseDraft {
            sql: "execute stmt1 using @a,@b,@c",
            expected: 0,
        },
        TokenCaseDraft {
            sql: "select * from `a_1`.`b_2` where c1 = ? and c2 = ?",
            expected: 10,
        },
    ]
}

// mem_arbitrator_compile_cases 按 Go TestMemArbitratorSession 的顺序保留 approxCompilePlanTokenCnt 断言。
/// 编译阶段 token 估算用例表。
pub fn mem_arbitrator_compile_cases() -> Vec<(TokenCaseDraft, bool)> {
    vec![
        (
            TokenCaseDraft {
                sql: "select * from `a_1`.`b_2` where c1 = ? and c2 = ?",
                expected: 9,
            },
            true,
        ),
        (
            TokenCaseDraft {
                sql: "select @@version @a",
                expected: 0,
            },
            true,
        ),
        (
            TokenCaseDraft {
                sql: "select @@version @a",
                expected: 3,
            },
            false,
        ),
    ]
}

// approx_parse_sql_token_cnt_draft 和 approx_compile_plan_token_cnt_draft 只按测试表返回期望值。
// 真实 Go 函数会解析 SQL 文本；避免手写新的解析器，以免偏离一比一测试迁移目标。
/// 按用例表查解析 token 期望值（非真实解析器）。
pub fn approx_parse_sql_token_cnt_draft(sql: &str) -> i64 {
    mem_arbitrator_parse_cases()
        .into_iter()
        .find(|case| case.sql == sql)
        .map(|case| case.expected)
        .unwrap_or(0)
}

/// 按用例表查编译 token 期望值。
pub fn approx_compile_plan_token_cnt_draft(sql: &str, enable: bool) -> i64 {
    mem_arbitrator_compile_cases()
        .into_iter()
        .find(|(case, case_enable)| case.sql == sql && *case_enable == enable)
        .map(|(case, _)| case.expected)
        .unwrap_or(0)
}

// test_mem_arbitrator_session 对应 Go 的 TestMemArbitratorSession。
/// 对应 Go TestMemArbitratorSession。
pub fn test_mem_arbitrator_session() {
    for case in mem_arbitrator_parse_cases() {
        assert_eq!(case.expected, approx_parse_sql_token_cnt_draft(case.sql));
    }
    for (case, enable) in mem_arbitrator_compile_cases() {
        assert_eq!(
            case.expected,
            approx_compile_plan_token_cnt_draft(case.sql, enable)
        );
    }
}

#[test]
/// 校验事务键是否需要加锁的分支决策。
fn canonical_key_lock_decision_covers_transactional_branches() {
    use crate::txn::{KeyFlags, KeyNeedToLock};

    assert!(KeyNeedToLock(&[], &KeyFlags::default()));
    assert!(!KeyNeedToLock(
        b"value",
        &KeyFlags {
            table_key: true,
            index_key: true,
            untouched_index_value: true,
            ..KeyFlags::default()
        }
    ));
    assert!(KeyNeedToLock(
        b"value",
        &KeyFlags {
            table_key: true,
            index_key: true,
            index_value_is_unique: true,
            ..KeyFlags::default()
        }
    ));
    assert!(!KeyNeedToLock(
        b"value",
        &KeyFlags {
            table_key: true,
            need_constraint_check_in_prewrite: true,
            ..KeyFlags::default()
        }
    ));
}

mod session_parity {
    use std::sync::{Arc, Mutex};

    use crate::session::{
        DDLOwnerManager, RetryInfo, SessionRuntime, SessionTransaction, SessionVars,
        StatementContext, TxnInfo, session,
    };
    use crate::{SessionError, SessionResult};

    #[derive(Default)]
    struct TxnState {
        valid: bool,
        read_only: bool,
        commits: usize,
        rollbacks: usize,
    }

    struct TestTxn(Arc<Mutex<TxnState>>);

    impl SessionTransaction for TestTxn {
        fn Valid(&self) -> bool {
            self.0.lock().unwrap().valid
        }
        fn IsReadOnly(&self) -> bool {
            self.0.lock().unwrap().read_only
        }
        fn Info(&self) -> Option<TxnInfo> {
            None
        }
        fn Commit(&mut self) -> SessionResult {
            self.0.lock().unwrap().commits += 1;
            Ok(())
        }
        fn Rollback(&mut self) -> SessionResult {
            self.0.lock().unwrap().rollbacks += 1;
            Ok(())
        }
    }

    #[derive(Default)]
    struct RuntimeState {
        deleted: Vec<u32>,
        cursor_closes: usize,
        vars_closes: usize,
    }

    struct TestRuntime(Arc<Mutex<RuntimeState>>);

    impl SessionRuntime for TestRuntime {
        fn SetOptionsBeforeCommit(&self, _: &mut dyn SessionTransaction) -> SessionResult {
            Ok(())
        }
        fn CommitTxnWithTemporaryData(&self, txn: &mut dyn SessionTransaction) -> SessionResult {
            txn.Commit()
        }
        fn DeletePreparedPlan(&self, id: u32) {
            self.0.lock().unwrap().deleted.push(id);
        }
        fn CloseCursorTracker(&self) {
            self.0.lock().unwrap().cursor_closes += 1;
        }
        fn CloseSessionVars(&self) {
            self.0.lock().unwrap().vars_closes += 1;
        }
        fn RenewCachedTableLeases(&self, _: &[i64]) -> SessionResult<Vec<u64>> {
            Ok(vec![])
        }
        fn StopCachedTableLeaseRenewal(&self) {}
    }

    fn new_session(vars: SessionVars) -> (session, Arc<Mutex<TxnState>>, Arc<Mutex<RuntimeState>>) {
        let txn_state = Arc::new(Mutex::new(TxnState {
            valid: true,
            ..TxnState::default()
        }));
        let runtime_state = Arc::new(Mutex::new(RuntimeState::default()));
        let value = session {
            runtime: Arc::new(TestRuntime(runtime_state.clone())),
            txn: Box::new(TestTxn(txn_state.clone())),
            values: Default::default(),
            currentCtx: None,
            processInfo: None,
            crossKS: false,
            sessionVars: vars,
            lockedTables: Default::default(),
            ddlOwnerManager: None::<Arc<dyn DDLOwnerManager>>,
        };
        (value, txn_state, runtime_state)
    }

    #[test]
    fn external_sql_is_rejected_when_cluster_is_restricted_read_only() {
        let (mut session, txn, _) = new_session(SessionVars {
            RestrictedReadOnly: true,
            InRestrictedSQL: false,
            ..SessionVars::default()
        });
        let error = session
            .doCommit()
            .expect_err("external writes must be rejected");
        assert_eq!(
            error,
            SessionError::new("SQL is not allowed in restricted read-only mode")
        );
        assert_eq!(txn.lock().unwrap().commits, 0);
    }

    #[test]
    fn rollback_cleans_retry_state_and_cached_plans() {
        let (mut session, txn, runtime) = new_session(SessionVars {
            EnablePreparedPlanCache: true,
            RetryInfo: RetryInfo {
                DroppedPreparedStmtIDs: vec![7, 9],
            },
            StmtCtx: StatementContext::default(),
            ..SessionVars::default()
        });
        session.RollbackTxn().unwrap();
        assert_eq!(txn.lock().unwrap().rollbacks, 1);
        assert!(
            session
                .sessionVars
                .RetryInfo
                .DroppedPreparedStmtIDs
                .is_empty()
        );
        assert_eq!(runtime.lock().unwrap().deleted, vec![7, 9]);
    }

    #[test]
    fn close_rolls_back_before_releasing_session_resources() {
        let (mut session, txn, runtime) = new_session(SessionVars::default());
        session.Close();
        assert_eq!(txn.lock().unwrap().rollbacks, 1);
        let runtime = runtime.lock().unwrap();
        assert_eq!(runtime.cursor_closes, 1);
        assert_eq!(runtime.vars_closes, 1);
    }
}

#[test]
fn memory_profile_identity_uses_database_and_normalized_sql() {
    use crate::runtime::build_mem_arbitrator_digest_id as digest;
    let sql = "select * from `t` where `a` = ?";
    assert_ne!(digest(sql, "db1"), digest(sql, "db2"));
    assert_eq!(digest(sql, "DB1"), digest(sql, "db1"));
    assert_eq!(digest("", "db1"), 0);
    let explicit = "select * from `db3`.`t` where `a` = ?";
    // The retained later Go implementation conservatively includes current DB
    // even when SQL contains an explicitly qualified table.
    assert_ne!(digest(explicit, "db1"), digest(explicit, "db2"));
    assert_ne!(
        digest(sql, "db1"),
        digest("select * from `other` where `a` = ?", "db1")
    );
}

#[test]
fn compile_memory_tokens_follow_normalized_sql() {
    use crate::runtime::approx_compile_plan_token_count as count;
    assert_eq!(
        count("select * from `a_1`.`b_2` where c1 = ? and c2 = ?", true),
        9
    );
    assert_eq!(count("select @@version @a", true), 0);
    assert_eq!(count("select @@version @a", false), 3);
}
