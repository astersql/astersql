// Copyright 2026 AsterSQL.

use super::*;
use std::sync::Arc;

struct OverflowingTimestampPd;

impl PdClient for OverflowingTimestampPd {
    fn get_ts(&self) -> Result<(i64, i64), String> {
        Ok((i64::MAX, i64::MAX))
    }

    fn alloc_id(&self) -> Result<u64, String> {
        Ok(1)
    }
}

/// Go converts both timestamp components to uint64 and lets the addition wrap.
#[test]
fn new_composes_timestamp_with_go_uint64_wrapping() {
    let mut config = astersql_store_mockstore_unistore_config::DefaultConf.clone();
    config.Engine.VolatileMode = true;
    config.Server.Raft = false;

    let result = new(&config, Arc::new(OverflowingTimestampPd));
    assert!(result.is_ok());
}
