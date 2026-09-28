// Copyright 2026 AsterSQL.

use std::time::Duration;

use astersql_sessionctx_vardef::AdvancerCheckPointLagLimit;

use crate::{Config, DefaultTiDBConfig, tidb_conf::ADVANCER_LAG_LIMIT_TEST_LOCK};

/// Go `TiDBConfig.GetCheckPointLagLimit` reads the vardef global updated by SET GLOBAL.
#[test]
fn tidb_config_reads_the_shared_vardef_lag_limit() {
    let _guard = ADVANCER_LAG_LIMIT_TEST_LOCK.lock().unwrap();
    let original = AdvancerCheckPointLagLimit.Load();
    let updated = Duration::from_secs(100 * 3600);

    AdvancerCheckPointLagLimit.Store(updated.as_nanos() as i64);
    assert_eq!(DefaultTiDBConfig().GetCheckPointLagLimit(), updated);

    AdvancerCheckPointLagLimit.Store(original);
}
