// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::txnmanager::*;
use crate::{SessionError, SessionResult};

#[derive(Default)]
struct MockSession {
    logged_events: Mutex<Vec<Vec<String>>>,
}

impl MockSession {
    fn last_event_names(&self) -> Vec<String> {
        self.logged_events
            .lock()
            .expect("logged events")
            .last()
            .cloned()
            .unwrap_or_default()
    }
}

impl TxnManagerSession for MockSession {
    fn LatestInfoSchema(&self) -> Option<String> {
        Some("latest".into())
    }
    fn DefaultTxnMode(&self) -> TxnMode {
        TxnMode::Optimistic
    }
    fn IsolationLevelForNewTxn(&self) -> IsolationLevel {
        IsolationLevel::RepeatableRead
    }
    fn SetStaleReadTS(&self, _timestamp: u64) {}
    fn SetInTxn(&self, _in_transaction: bool) {}
    fn RollbackTxn(&self) {}
    fn BulkDMLEnabled(&self) -> bool {
        false
    }
    fn EnableRedactLog(&self) -> &str {
        "ON"
    }
    fn SlowTxnThresholdMs(&self) -> u64 {
        1
    }
    fn ConnectionID(&self) -> u64 {
        1
    }
    fn TxnStartTS(&self) -> u64 {
        2
    }
    fn TxnStatementCount(&self) -> u64 {
        0
    }
    fn TraceTxnEnter(&self, _enter_type: EnterNewTxnType, _explicit: bool) {}
    fn TraceTxnEnd(&self, _duration: Duration, _slow: bool) {}
    fn LogSlowTxn(&self, _duration: Duration, events: &[Event]) {
        self.logged_events
            .lock()
            .expect("logged events")
            .push(events.iter().map(|event| event.event.clone()).collect());
    }
}

#[derive(Default)]
struct MockProvider;

impl TxnContextProvider for MockProvider {
    fn OnInitialize(&mut self, _enter_type: EnterNewTxnType) -> SessionResult {
        Ok(())
    }
    fn GetTxnInfoSchema(&self) -> Option<String> {
        None
    }
    fn GetTxnScope(&self) -> String {
        GlobalTxnScope.to_owned()
    }
    fn GetReadReplicaScope(&self) -> String {
        GlobalReplicaScope.to_owned()
    }
    fn GetStmtReadTS(&self) -> SessionResult<u64> {
        Ok(1)
    }
    fn GetStmtForUpdateTS(&self) -> SessionResult<u64> {
        Ok(1)
    }
    fn GetSnapshotWithStmtReadTS(&self) -> SessionResult<String> {
        Ok("read".into())
    }
    fn GetSnapshotWithStmtForUpdateTS(&self) -> SessionResult<String> {
        Ok("update".into())
    }
    fn OnStmtStart(&mut self, _statement: Option<&StatementNode>) -> SessionResult {
        Ok(())
    }
    fn OnStmtCommit(&mut self) -> SessionResult {
        Ok(())
    }
    fn OnStmtRollback(&mut self, _pessimistic_retry: bool) -> SessionResult {
        Ok(())
    }
    fn OnPessimisticStmtStart(&mut self) -> SessionResult {
        Ok(())
    }
    fn OnPessimisticStmtEnd(&mut self, _successful: bool) -> SessionResult {
        Ok(())
    }
    fn OnStmtErrorForNextAction(
        &mut self,
        _point: StmtErrorHandlePoint,
        _error: &SessionError,
    ) -> SessionResult<StmtErrorAction> {
        Ok(StmtErrorAction::NoIdea)
    }
    fn ActivateTxn(&mut self) -> SessionResult<String> {
        Ok("txn".into())
    }
    fn OnStmtRetry(&mut self) -> SessionResult {
        Ok(())
    }
    fn OnLocalTemporaryTableCreated(&mut self) {}
    fn AdviseWarmup(&mut self) -> SessionResult {
        Ok(())
    }
    fn AdviseOptimizeWithPlan(&mut self, _plan: &dyn Any) -> SessionResult {
        Ok(())
    }
    fn SetOptionsBeforeCommit(&mut self, _checker: &dyn Fn(u64) -> bool) -> SessionResult {
        Ok(())
    }
}

struct MockFactory;

impl TxnProviderFactory for MockFactory {
    fn NewStaleReadProvider(&self, _timestamp: u64) -> SessionResult<Box<dyn TxnContextProvider>> {
        Ok(Box::new(MockProvider))
    }
    fn NewOptimisticProvider(
        &self,
        _slot: usize,
        _causal_consistency_only: bool,
    ) -> SessionResult<Box<dyn TxnContextProvider>> {
        Ok(Box::new(MockProvider))
    }
    fn NewPessimisticRCProvider(
        &self,
        _causal: bool,
    ) -> SessionResult<Box<dyn TxnContextProvider>> {
        Ok(Box::new(MockProvider))
    }
    fn NewPessimisticSerializableProvider(
        &self,
        _causal: bool,
    ) -> SessionResult<Box<dyn TxnContextProvider>> {
        Ok(Box::new(MockProvider))
    }
    fn NewPessimisticRRProvider(
        &self,
        _causal: bool,
    ) -> SessionResult<Box<dyn TxnContextProvider>> {
        Ok(Box::new(MockProvider))
    }
}

fn manager() -> (TxnManager, Arc<MockSession>) {
    let session = Arc::new(MockSession::default());
    let manager = TxnManager::new(session.clone(), Arc::new(MockFactory));
    (manager, session)
}

#[test]
fn missing_provider_does_not_record_commit_or_rollback_events() {
    for rollback in [false, true] {
        let (mut manager, session) = manager();
        let result = if rollback {
            manager.OnStmtRollback(false)
        } else {
            manager.OnStmtCommit()
        };
        assert_eq!(
            result.expect_err("provider is required").to_string(),
            "context provider not set"
        );

        thread::sleep(Duration::from_millis(2));
        manager.OnTxnEnd();
        assert_eq!(session.last_event_names(), ["txn end"]);
    }
}

#[test]
fn statement_event_uses_parser_redaction_normalization() {
    let (mut manager, session) = manager();
    let mut request = EnterNewTxnRequest {
        Type: EnterNewTxnType::Default,
        Provider: None,
        StaleReadTS: 0,
        TxnMode: None,
        CausalConsistencyOnly: false,
    };
    manager
        .EnterNewTxn(&mut request)
        .expect("enter transaction");

    let sql = "SELECT * FROM users WHERE password = 'secret' AND id = 42";
    manager
        .OnStmtStart(Some(StatementNode {
            original_text: sql.into(),
        }))
        .expect("statement start");
    thread::sleep(Duration::from_millis(2));
    manager.OnTxnEnd();

    let expected = astersql_parser::Normalize(sql, "ON");
    assert_eq!(session.last_event_names()[1], expected);
}

#[test]
fn provider_schema_none_does_not_fall_back_to_latest_schema() {
    let (mut manager, _) = manager();
    assert_eq!(manager.GetTxnInfoSchema().as_deref(), Some("latest"));

    let mut request = EnterNewTxnRequest {
        Type: EnterNewTxnType::Default,
        Provider: Some(Box::new(MockProvider)),
        StaleReadTS: 0,
        TxnMode: None,
        CausalConsistencyOnly: false,
    };
    manager
        .EnterNewTxn(&mut request)
        .expect("enter transaction with provider");

    assert_eq!(manager.GetTxnInfoSchema(), None);
}
