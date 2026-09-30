// Copyright 2026 AsterSQL.

use crate::NewContext;

#[test]
fn go_merge_30_dist_sql_context_has_query_scoped_store_limiter() {
    let mut context = NewContext();
    assert_eq!(
        context
            .GetDistSQLCtx()
            .QueryCopStoreLimiter
            .unwrap()
            .Capacity(),
        15
    );
    context.GetSessionVarsMut().QueryCopStoreLimit = 3;
    let first = context.GetDistSQLCtx();
    let limiter = first.QueryCopStoreLimiter.expect("positive limit");
    assert_eq!(limiter.Capacity(), 3);
    assert!(limiter.GetStoreLimiter(1).is_some());
    let second = context.GetDistSQLCtx().QueryCopStoreLimiter.unwrap();
    assert!(!std::sync::Arc::ptr_eq(&limiter, &second));
    context.GetSessionVarsMut().QueryCopStoreLimit = 0;
    assert!(context.GetDistSQLCtx().QueryCopStoreLimiter.is_none());
}
