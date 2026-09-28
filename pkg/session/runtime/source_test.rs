// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn distinct_is_applied_before_order_by_and_limit() {
    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("create source parity session");
    let mut result = session
        .execute(
            "select distinct value \
             from (select 1 value union all select 1 union all select 2) source_rows \
             order by value limit 2",
        )
        .expect("execute DISTINCT ORDER BY LIMIT")
        .remove(0);

    assert_eq!(
        result.Next().expect("read first distinct row"),
        Some(vec!["1".to_owned()])
    );
    assert_eq!(
        result.Next().expect("read second distinct row"),
        Some(vec!["2".to_owned()])
    );
    assert_eq!(result.Next().expect("distinct rows exhausted"), None);

    let mut aggregate = session
        .execute(
            "select distinct count(*) \
             from (select 1 group_id union all select 2 union all \
                   select 3 union all select 3) source_rows \
             group by group_id order by count(*) limit 2",
        )
        .expect("execute aggregate DISTINCT ORDER BY LIMIT")
        .remove(0);
    assert_eq!(
        aggregate.Next().expect("read first aggregate row"),
        Some(vec!["1".to_owned()])
    );
    assert_eq!(
        aggregate.Next().expect("read second aggregate row"),
        Some(vec!["2".to_owned()])
    );
    assert_eq!(aggregate.Next().expect("aggregate rows exhausted"), None);
}
