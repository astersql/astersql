// Copyright 2026 AsterSQL.

use std::any::Any;

use crate::txn_manager_test::MockSession;
use crate::*;

struct CountingAssertionContext {
    inner: MockSession,
    txn_local_identity_calls: usize,
}

impl TxnManagerContext for CountingAssertionContext {
    fn txn_manager(&mut self) -> &mut dyn TxnManager {
        self.inner.txn_manager()
    }
}

impl SessionValueStore for CountingAssertionContext {
    fn Value(&self, key: &str) -> Option<&dyn Any> {
        self.inner.Value(key)
    }

    fn ValueMut(&mut self, key: &str) -> Option<&mut dyn Any> {
        self.inner.ValueMut(key)
    }

    fn SetValue(&mut self, key: &'static str, value: SessionValue) {
        self.inner.SetValue(key, value);
    }
}

impl TxnAssertionContext for CountingAssertionContext {
    fn LocalTemporaryTablesIdentity(&self) -> Option<usize> {
        self.inner.local_temp_tables
    }

    fn TxnInfoSchemaLocalTemporaryTablesIdentity(&mut self) -> Option<usize> {
        self.txn_local_identity_calls += 1;
        self.inner.manager.provider.txn_local_temp_tables
    }
}

#[test]
fn info_schema_assertion_only_observes_txn_local_tables_when_session_has_them() {
    let (inner, _) = MockSession::new();
    let mut context = CountingAssertionContext {
        inner,
        txn_local_identity_calls: 0,
    };

    AssertTxnManagerInfoSchema(&mut context, None);

    assert_eq!(context.txn_local_identity_calls, 0);
}
