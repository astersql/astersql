// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL worker 会话侧校验逻辑的单元测试。
//
// 验证物理表（分区/非分区表在存储层的实际表实例）TTL 开关关闭后，
// 作业不应继续扫描或删除过期数据。

/// 当当前表元数据中 TTL 已关闭时，`validate_ttl_work` 应返回 `TtlDisabled`。
#[test]
fn ttl_validation_rejects_disabled_table() {
    use crate::session::{PhysicalTable, SessionError, validate_ttl_work};

    // 作业启动时快照：TTL 开启，过期列与保留时长已配置。
    let original = PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 2,
        schema: "test".to_owned(),
        table: "events".to_owned(),
        key_columns: vec!["id".to_owned()],
        ttl_column: "created_at".to_owned(),
        ttl_enabled: true,
        definition_version: 3,
        expire_after_seconds: 60,
    };
    // 运行中途表定义变更：TTL 被关闭，应拒绝继续执行。
    let mut current = original.clone();
    current.ttl_enabled = false;
    assert_eq!(
        validate_ttl_work(&original, Some(&current), 940, 1000),
        Err(SessionError::TtlDisabled)
    );
}

#[test]
fn successful_table_session_execution_is_not_retryable() {
    use crate::session::{Datum, SessionError, SessionState, TableSession, WorkerSession};

    struct Session;
    impl WorkerSession for Session {
        fn state(&self) -> &SessionState {
            static STATE: std::sync::OnceLock<SessionState> = std::sync::OnceLock::new();
            STATE.get_or_init(SessionState::default)
        }
        fn state_mut(&mut self) -> &mut SessionState {
            panic!("state mutation is not needed in this test")
        }
        fn execute(
            &mut self,
            _sql: &str,
            _args: &[Datum],
        ) -> Result<Vec<Vec<Datum>>, SessionError> {
            Ok(vec![vec![Datum::Integer(1)]])
        }
    }

    let mut session = Session;
    let table = crate::session::PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".into(),
        table: "t".into(),
        key_columns: vec!["id".into()],
        ttl_column: "created_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 10,
    };
    let mut table_session = TableSession {
        session: &mut session,
        table: table.clone(),
        expire_time: 90,
    };
    let (_, retryable) = table_session
        .execute_sql_with_check("select 1", &[], Some(&table), 100)
        .unwrap();
    assert!(!retryable);
}

fn table() -> crate::session::PhysicalTable {
    crate::session::PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 2,
        schema: "test".into(),
        table: "events".into(),
        key_columns: vec!["id".into()],
        ttl_column: "created_at".into(),
        ttl_enabled: true,
        definition_version: 3,
        expire_after_seconds: 60,
    }
}

#[test]
fn successful_execution_still_rejects_changed_ttl_metadata() {
    use crate::session::{Datum, SessionError, SessionState, TableSession, WorkerSession};

    #[derive(Default)]
    struct Session(SessionState);
    impl WorkerSession for Session {
        fn state(&self) -> &SessionState {
            &self.0
        }
        fn state_mut(&mut self) -> &mut SessionState {
            &mut self.0
        }
        fn execute(&mut self, _: &str, _: &[Datum]) -> Result<Vec<Vec<Datum>>, SessionError> {
            Ok(vec![])
        }
    }

    let original = table();
    let mut current = original.clone();
    current.ttl_enabled = false;
    let mut session = Session::default();
    let mut table_session = TableSession {
        session: &mut session,
        table: original,
        expire_time: 940,
    };
    assert_eq!(
        table_session.execute_sql_with_check("select 1", &[], Some(&current), 1000),
        Err(SessionError::TtlDisabled),
    );
}

#[test]
fn safe_non_ttl_metadata_changes_do_not_abort_work() {
    use crate::session::validate_ttl_work;

    let original = table();
    let mut current = original.clone();
    current.definition_version += 1;
    current.key_columns = vec!["new_primary_key".into()];
    assert_eq!(
        validate_ttl_work(&original, Some(&current), 940, 1000),
        Ok(())
    );
}

#[test]
fn prepare_session_preserves_an_already_complete_read_engine_setting() {
    use crate::session::{SessionState, prepare_session};
    use std::collections::BTreeMap;

    struct Session(SessionState);
    impl crate::session::WorkerSession for Session {
        fn state(&self) -> &SessionState {
            &self.0
        }
        fn state_mut(&mut self) -> &mut SessionState {
            &mut self.0
        }
        fn execute(
            &mut self,
            _: &str,
            _: &[crate::session::Datum],
        ) -> Result<Vec<crate::session::Row>, crate::session::SessionError> {
            Ok(vec![])
        }
    }

    let mut session = Session(SessionState {
        variables: BTreeMap::from([(
            "tidb_isolation_read_engines".into(),
            "tidb,tikv,tiflash,custom".into(),
        )]),
        in_transaction: true,
        timezone_offset_seconds: 8 * 60 * 60,
        ..SessionState::default()
    });
    let previous = prepare_session(&mut session);
    assert_eq!(
        session.0.variables["tidb_isolation_read_engines"],
        "tidb,tikv,tiflash,custom"
    );
    assert!(previous.in_transaction);
}

#[test]
fn ttl_validation_matches_go_change_classification() {
    use crate::session::{SessionError, validate_ttl_work};

    let original = table();
    assert_eq!(
        validate_ttl_work(&original, None, 940, 1000),
        Err(SessionError::TableChanged)
    );

    let mut changed = original.clone();
    changed.table_id += 1;
    assert_eq!(
        validate_ttl_work(&original, Some(&changed), 940, 1000),
        Err(SessionError::TableChanged)
    );

    let mut changed = original.clone();
    changed.physical_id += 1;
    assert_eq!(
        validate_ttl_work(&original, Some(&changed), 940, 1000),
        Err(SessionError::TableChanged)
    );

    let mut changed = original.clone();
    changed.ttl_column = "updated_at".into();
    assert_eq!(
        validate_ttl_work(&original, Some(&changed), 940, 1000),
        Err(SessionError::TableChanged)
    );

    let mut changed = original.clone();
    changed.expire_after_seconds = 120;
    assert_eq!(
        validate_ttl_work(&original, Some(&changed), 940, 1000),
        Err(SessionError::ExpireIntervalChanged)
    );

    changed.expire_after_seconds = 30;
    assert_eq!(
        validate_ttl_work(&original, Some(&changed), 940, 1000),
        Ok(())
    );
}

#[test]
fn execution_checks_global_switch_timezone_and_retryable_errors() {
    use crate::session::{Datum, SessionError, SessionState, TableSession, WorkerSession};

    struct Session {
        state: SessionState,
        enabled: bool,
        resets: usize,
        result: Result<Vec<Vec<Datum>>, SessionError>,
    }
    impl WorkerSession for Session {
        fn state(&self) -> &SessionState {
            &self.state
        }
        fn state_mut(&mut self) -> &mut SessionState {
            &mut self.state
        }
        fn execute(&mut self, _: &str, _: &[Datum]) -> Result<Vec<Vec<Datum>>, SessionError> {
            self.result.clone()
        }
        fn ttl_jobs_enabled(&self) -> bool {
            self.enabled
        }
        fn reset_with_global_timezone(&mut self) -> Result<(), SessionError> {
            self.resets += 1;
            Ok(())
        }
    }

    let original = table();
    let mut session = Session {
        state: SessionState::default(),
        enabled: false,
        resets: 0,
        result: Err(SessionError::Execute("temporary".into())),
    };
    {
        let mut table_session = TableSession {
            session: &mut session,
            table: original.clone(),
            expire_time: 940,
        };
        assert_eq!(
            table_session.execute_sql_with_check("select 1", &[], Some(&original), 1000),
            Err(SessionError::TtlDisabled)
        );
    }
    assert_eq!(session.resets, 0);

    session.enabled = true;
    {
        let mut table_session = TableSession {
            session: &mut session,
            table: original.clone(),
            expire_time: 940,
        };
        assert_eq!(
            table_session.execute_sql_with_check("select 1", &[], Some(&original), 1000),
            Err(SessionError::Execute("temporary".into()))
        );
    }
    assert_eq!(session.resets, 1);

    let mut changed = original.clone();
    changed.ttl_enabled = false;
    let mut table_session = TableSession {
        session: &mut session,
        table: original.clone(),
        expire_time: 940,
    };
    assert_eq!(
        table_session.execute_sql_with_check("select 1", &[], Some(&changed), 1000),
        Err(SessionError::TtlDisabled)
    );
}

#[test]
fn session_prepare_and_scan_state_restore_exactly() {
    use crate::session::{
        Datum, SessionError, SessionState, WorkerSession, prepare_scan_session, prepare_session,
        restore_scan_session, restore_session,
    };

    #[derive(Default)]
    struct Session(SessionState);
    impl WorkerSession for Session {
        fn state(&self) -> &SessionState {
            &self.0
        }
        fn state_mut(&mut self) -> &mut SessionState {
            &mut self.0
        }
        fn execute(&mut self, _: &str, _: &[Datum]) -> Result<Vec<Vec<Datum>>, SessionError> {
            Ok(vec![])
        }
    }

    let mut session = Session(SessionState {
        in_transaction: true,
        timezone_offset_seconds: 28_800,
        internal_sql_scan_user_table: false,
        distsql_scan_concurrency: 16,
        enable_paging: true,
        ..SessionState::default()
    });
    let original = prepare_session(&mut session);
    assert!(!session.0.in_transaction);
    assert_eq!(session.0.timezone_offset_seconds, 0);
    assert_eq!(session.0.variables["time_zone"], "UTC");
    restore_session(&mut session, original.clone());
    assert!(session.0.in_transaction);
    assert_eq!(session.0.timezone_offset_seconds, 28_800);

    let scan = prepare_scan_session(&mut session);
    assert!(session.0.internal_sql_scan_user_table);
    assert_eq!(session.0.distsql_scan_concurrency, 1);
    assert!(!session.0.enable_paging);
    restore_scan_session(&mut session, scan);
    assert_eq!(
        session.0.internal_sql_scan_user_table,
        original.internal_sql_scan_user_table
    );
    assert_eq!(
        session.0.distsql_scan_concurrency,
        original.distsql_scan_concurrency
    );
    assert_eq!(session.0.enable_paging, original.enable_paging);
}
