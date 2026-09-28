// Copyright 2026 AsterSQL.

use super::super::{GENERATED_MAIN_ACTION_REQUIRED, RULE_IDS_BY_REDUCTION};
use super::{admin, ddl, dml, expression, misc, query, security};

#[test]
fn all_action_rules_have_one_owner() {
    assert_eq!(
        RULE_IDS_BY_REDUCTION.len(),
        GENERATED_MAIN_ACTION_REQUIRED.len(),
        "every generated reduction must have exactly one stable RuleId action marker"
    );

    let mut security_count = 0;
    let mut admin_count = 0;
    let mut misc_count = 0;

    for (&rule_id, &required) in RULE_IDS_BY_REDUCTION
        .iter()
        .zip(GENERATED_MAIN_ACTION_REQUIRED)
    {
        let security_owned = security::owns(rule_id);
        let admin_owned = admin::owns(rule_id);
        let misc_owned = misc::owns(rule_id);
        security_count += usize::from(security_owned);
        admin_count += usize::from(admin_owned);
        misc_count += usize::from(misc_owned);

        let owners = usize::from(ddl::owns(rule_id))
            + usize::from(dml::owns(rule_id))
            + usize::from(expression::owns(rule_id))
            + usize::from(query::owns(rule_id))
            + usize::from(security_owned)
            + usize::from(admin_owned)
            + usize::from(misc_owned);
        assert_eq!(
            owners,
            usize::from(required),
            "unexpected semantic owner count for {}",
            rule_id.as_str()
        );
    }

    assert_eq!(security_count, 181, "security RuleId inventory changed");
    assert_eq!(admin_count, 545, "admin RuleId inventory changed");
    assert_eq!(misc_count, 153, "misc RuleId inventory changed");
}

#[test]
fn remaining_modules_have_no_numeric_fallback() {
    for source in [
        include_str!("security.rs"),
        include_str!("admin.rs"),
        include_str!("misc.rs"),
    ] {
        assert!(!source.contains("legacy_rule_number"));
        assert!(!source.contains("apply_numeric"));
        assert!(!source.contains("rhs_index"));
    }

    let dispatcher = include_str!("mod.rs");
    assert!(!dispatcher.contains("mod legacy"));
    assert!(!dispatcher.contains("legacy::"));
}
