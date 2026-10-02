// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::mockstore::{
    RunTestUnderCascades, RunTestUnderCascadesAndDomainWithSchemaLease,
    RunTestUnderCascadesWithDomain,
};

static CALLBACKS: AtomicUsize = AtomicUsize::new(0);

fn assert_cascades_disabled(test_kit: &crate::TestKit, round: &str, caller: &str) {
    CALLBACKS.fetch_add(1, Ordering::SeqCst);
    assert_eq!(round, "off");
    assert_eq!(
        caller,
        "runs_each_compatibility_helper_only_with_cascades_off"
    );
    test_kit
        .MustQuery("select @@session.tidb_enable_cascades_planner", Vec::new())
        .Check(vec![vec!["OFF".to_owned()]]);
}

#[test]
fn runs_each_compatibility_helper_only_with_cascades_off() {
    CALLBACKS.store(0, Ordering::SeqCst);

    RunTestUnderCascades(|test_kit, round, caller| {
        assert_cascades_disabled(test_kit, round, caller);
    });
    RunTestUnderCascadesWithDomain(|test_kit, _domain, round, caller| {
        assert_cascades_disabled(test_kit, round, caller);
    });
    RunTestUnderCascadesAndDomainWithSchemaLease(
        Duration::from_millis(10),
        |test_kit, _domain, round, caller| {
            assert_cascades_disabled(test_kit, round, caller);
        },
    );

    assert_eq!(CALLBACKS.load(Ordering::SeqCst), 3);
}
