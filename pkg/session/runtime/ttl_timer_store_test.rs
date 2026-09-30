// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::time::Duration;

use astersql_timer_api::{
    Context, NewOptionalVal, TimerRecord, TimerSpec, TimerUpdate, WatchTimerEventCreate,
    WatchTimerEventDelete, WatchTimerEventUpdate,
};
use astersql_timer_tablestore::NewTableTimerStore;

use super::{CreateAnalyzeSession, ttl_timer_store::new_ttl_timer_session_pool};

#[test]
fn go_merge_43_ttl_table_timer_store_real_sql_roundtrip() {
    let (domain, writer) = CreateAnalyzeSession().expect("SQL domain");
    let key = "go-merge-43-table-store-roundtrip";
    let watermark = chrono::DateTime::from_timestamp(1_700_000_000, 0)
        .expect("valid time")
        .fixed_offset();
    let pool = new_ttl_timer_session_pool(Arc::clone(&domain), 2);
    let store = NewTableTimerStore(1, pool.clone(), "mysql", "tidb_timers", None);
    let ctx = Context::background();
    let watch = store.Watch(&ctx);
    let id = store
        .Create(
            &ctx,
            Some(TimerRecord {
                TimerSpec: TimerSpec {
                    Namespace: "default".into(),
                    Key: key.into(),
                    Data: vec![0, 255],
                    TimeZone: "UTC".into(),
                    SchedPolicyType: "INTERVAL".into(),
                    SchedPolicyExpr: "1h".into(),
                    HookClass: "tidb.ttl".into(),
                    Watermark: Some(watermark),
                    Enable: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
        )
        .expect("create timer through real SQL pool");
    assert!(
        watch
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .Events
            .iter()
            .any(|event| event.Tp == WatchTimerEventCreate && event.TimerID == id)
    );
    let reader = super::ConcreteSession::new(domain.clone());
    let mut result = reader
        .execute(&format!(
            "SELECT TIMER_KEY, TIMER_DATA, TIMER_EXT FROM mysql.tidb_timers WHERE TIMER_KEY='{key}'"
        ))
        .expect("read timer from another real session")
        .remove(0);
    let row = result
        .next_row()
        .expect("fetch timer")
        .expect("timer exists");
    assert_eq!(row[0], key);
    assert_eq!(row[1], "__astersql_binary_hex__:00FF");
    let list = store.List(&ctx, None).expect("list timer via real SQL");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].Data, vec![0, 255]);
    assert_eq!(
        list[0].Watermark.unwrap().timestamp(),
        watermark.timestamp()
    );
    store
        .Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                SchedPolicyExpr: NewOptionalVal("2h".into()),
                ..Default::default()
            }),
        )
        .expect("update timer");
    assert!(
        watch
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .Events
            .iter()
            .any(|event| event.Tp == WatchTimerEventUpdate && event.TimerID == id)
    );
    assert_eq!(store.GetByID(&ctx, &id).unwrap().SchedPolicyExpr, "2h");
    assert!(store.Delete(&ctx, &id).expect("delete timer"));
    assert!(
        watch
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .Events
            .iter()
            .any(|event| event.Tp == WatchTimerEventDelete && event.TimerID == id)
    );
    store.Close();
    pool.Close();
    assert!(pool.Get().is_err(), "closed pool must reject new leases");
    drop(writer);
    domain.close();
}
