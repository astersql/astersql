// Copyright 2026 AsterSQL.

use astersql_timer_api::{WatchTimerEventCreate, WatchTimerEventUpdate};
use astersql_timer_tablestore::EtcdNotifyEvent;

use super::ttl_timer_etcd::{decode_timer_notify_message, encode_timer_notify_message};

#[test]
fn go_merge_43_timer_etcd_notice_matches_go_wire_shape() {
    let encoded = encode_timer_notify_message(&[
        EtcdNotifyEvent {
            tp: "create".into(),
            timer_id: "timer-1".into(),
            timestamp: 42,
        },
        EtcdNotifyEvent {
            tp: "update".into(),
            timer_id: "timer-2".into(),
            timestamp: 43,
        },
    ]);
    let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(json["events"][0]["tp"], "create");
    assert_eq!(json["events"][0]["timer_id"], "timer-1");
    assert_eq!(json["events"][0]["timestamp"], 42);
    let decoded = decode_timer_notify_message(encoded.as_bytes()).unwrap();
    assert_eq!(decoded.Events[0].Tp, WatchTimerEventCreate);
    assert_eq!(decoded.Events[1].Tp, WatchTimerEventUpdate);
    assert_eq!(decoded.Events[1].TimerID, "timer-2");
    let mixed =
        br#"{"events":[{"tp":"unknown","timer_id":"bad"},{"tp":"delete","timer_id":"timer-3"}]}"#;
    let decoded = decode_timer_notify_message(mixed).unwrap();
    assert_eq!(decoded.Events.len(), 1);
    assert_eq!(decoded.Events[0].TimerID, "timer-3");
}
