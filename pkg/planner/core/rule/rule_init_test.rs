// Copyright 2026 AsterSQL.

use super::{FLAG_PREDICATE_PUSH_DOWN, rule_init};

#[test]
fn init_registers_all_go_rule_hooks_idempotently() {
    rule_init::init();
    rule_init::init();

    assert!(astersql_planner_core_rule_util::RuleInitHooksRegistered());
    assert_eq!(
        astersql_planner_core_rule_util::SetPredicatePushDownFlag(0),
        FLAG_PREDICATE_PUSH_DOWN
    );
    assert_eq!(
        astersql_planner_core_rule_util::SetPredicatePushDownFlag(1 << 63),
        (1 << 63) | FLAG_PREDICATE_PUSH_DOWN
    );
}
