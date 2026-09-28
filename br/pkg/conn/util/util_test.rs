// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicUsize, Ordering};

use astersql_errors::SharedError;

use crate::util::{CancelContext, GetCurrentTsFromPD, PdClient, StatusUrl};

#[test]
fn join_path_cleans_dot_segments_like_go_url_join_path() {
    let url = StatusUrl::parse("http://tikv.example:20180/status/../admin?token=x")
        .expect("parse TiKV status URL");

    assert_eq!(
        "http://tikv.example:20180/admin/config?token=x",
        url.join_path("config")
    );
}

struct ActiveContext;

impl CancelContext for ActiveContext {
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct FixedTimestamp {
    calls: AtomicUsize,
}

impl PdClient for FixedTimestamp {
    fn GetTS(&self, _ctx: &dyn CancelContext) -> Result<(i64, i64), SharedError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok((1, 1 << 18))
    }
}

#[test]
fn compose_timestamp_uses_go_oracle_addition_semantics() {
    let pd = FixedTimestamp {
        calls: AtomicUsize::new(0),
    };

    assert_eq!(
        1_u64 << 19,
        GetCurrentTsFromPD(&ActiveContext, &pd).unwrap()
    );
    assert_eq!(1, pd.calls.load(Ordering::Relaxed));
}
