// Copyright 2026 AsterSQL.

use std::collections::BTreeSet;

use astersql_sessionctx_variable::slow_log::{GlobalSlowLogRules, SlowQueryLogItems, Threshold};

use crate::adapter_slow_log::{
    PrepareSlowLogItemsForRules, SessionSlowLogRules, SlowLogRuleContext,
};

#[derive(Default)]
struct TestRuleContext {
    state: SessionSlowLogRules,
}

impl SlowLogRuleContext for TestRuleContext {
    fn connection_id(&self) -> u64 {
        42
    }

    fn slow_log_rules(&self) -> &SessionSlowLogRules {
        &self.state
    }

    fn slow_log_rules_mut(&mut self) -> &mut SessionSlowLogRules {
        &mut self.state
    }

    fn registered_rule_fields(&self) -> Vec<String> {
        vec!["conn_id".to_owned()]
    }

    fn rule_field_has_setter(&self, _field: &str) -> bool {
        false
    }

    fn set_rule_field(&self, _field: &str, _items: &mut SlowQueryLogItems) {
        panic!("a field without a setter must not be invoked")
    }

    fn match_rule_field(
        &self,
        _field: &str,
        _items: &SlowQueryLogItems,
        _threshold: &Threshold,
    ) -> bool {
        false
    }
}

#[test]
fn prepare_does_not_allocate_for_a_registered_field_without_a_setter() {
    let mut session = TestRuleContext::default();
    session.state.effective_fields = BTreeSet::from(["conn_id".to_owned()]);

    let items = PrepareSlowLogItemsForRules(&GlobalSlowLogRules::default(), &mut session);

    assert!(items.is_none());
}
