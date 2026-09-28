// Copyright 2026 AsterSQL.

use super::domain_sysvars::{
    DomainSysVars, DynamicOption, PD_ENABLE_FOLLOWER_HANDLE_REGION, TIDB_ENABLE_BATCH_QUERY_REGION,
    TIDB_ENABLE_TSO_FOLLOWER_PROXY, TIDB_TSO_CLIENT_BATCH_MAX_WAIT_TIME, TIDB_TSO_CLIENT_RPC_MODE,
};

#[test]
fn pd_dynamic_options_match_go_parsing_and_duration_units() {
    let vars = DomainSysVars::default();

    vars.set_pd_client_dynamic_option(TIDB_TSO_CLIENT_BATCH_MAX_WAIT_TIME, "1.25")
        .unwrap();
    assert_eq!(
        vars.option(TIDB_TSO_CLIENT_BATCH_MAX_WAIT_TIME),
        Some(DynamicOption::DurationNanos(1_250_000))
    );

    vars.set_pd_client_dynamic_option(TIDB_TSO_CLIENT_BATCH_MAX_WAIT_TIME, "-0.5")
        .unwrap();
    assert_eq!(
        vars.option(TIDB_TSO_CLIENT_BATCH_MAX_WAIT_TIME),
        Some(DynamicOption::DurationNanos(-500_000))
    );

    for name in [
        TIDB_ENABLE_TSO_FOLLOWER_PROXY,
        PD_ENABLE_FOLLOWER_HANDLE_REGION,
        TIDB_ENABLE_BATCH_QUERY_REGION,
    ] {
        vars.set_pd_client_dynamic_option(name, "not-on").unwrap();
        assert_eq!(vars.option(name), Some(DynamicOption::Bool(false)));
    }
}

#[test]
fn rpc_mode_matches_go_case_sensitive_constants() {
    let vars = DomainSysVars::default();

    for (value, concurrency) in [("DEFAULT", 1), ("PARALLEL", 2), ("PARALLEL-FAST", 4)] {
        vars.set_pd_client_dynamic_option(TIDB_TSO_CLIENT_RPC_MODE, value)
            .unwrap();
        assert_eq!(
            vars.option(TIDB_TSO_CLIENT_RPC_MODE),
            Some(DynamicOption::Integer(concurrency))
        );
    }

    assert!(
        vars.set_pd_client_dynamic_option(TIDB_TSO_CLIENT_RPC_MODE, "parallel")
            .is_err()
    );
}

#[test]
fn scalar_callbacks_do_not_add_clamping_absent_from_go() {
    let vars = DomainSysVars::default();

    vars.set_stats_cache_capacity(-7);
    assert_eq!(vars.stats_cache_capacity(), -7);

    vars.set_circuit_breaker_error_rate_ratio(101);
    assert_eq!(vars.circuit_breaker_error_rate_ratio(), 101);
}
