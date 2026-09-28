// Copyright 2026 AsterSQL.

use prometheus::proto::{Metric, MetricFamily};

fn family<'a>(families: &'a [MetricFamily], name: &str) -> Option<&'a MetricFamily> {
    families.iter().find(|family| family.name() == name)
}

fn label<'a>(metric: &'a Metric, name: &str) -> Option<&'a str> {
    metric
        .get_label()
        .iter()
        .find(|label| label.name() == name)
        .map(|label| label.value())
}

#[test]
fn skips_zero_ids_and_nodes_without_data_like_go_collector() {
    let families = crate::channelz::collect_snapshots_for_test(
        r#"{
            "channel": [
                {
                    "ref": {"channelId": "0"},
                    "data": {"target": "zero"},
                    "socketRef": [{"socketId": "5"}]
                },
                {
                    "ref": {"channelId": "1"},
                    "data": {"target": "root"},
                    "subchannelRef": [{"subchannelId": "2"}],
                    "socketRef": [{"socketId": "3"}]
                }
            ],
            "end": true
        }"#,
        &[
            (
                "subchannel",
                2,
                r#"{"subchannel":{"ref":{"subchannelId":"2"},"socketRef":[{"socketId":"4"}]}}"#,
            ),
            (
                "socket",
                3,
                r#"{"socket":{"ref":{"socketId":"3"},"remote":{"other_address":{"name":"peer"}}}}"#,
            ),
            (
                "socket",
                4,
                r#"{"socket":{"ref":{"socketId":"4"},"remote":{"other_address":{"name":"peer"}}}}"#,
            ),
            (
                "socket",
                5,
                r#"{"socket":{"ref":{"socketId":"5"},"data":{"streamsStarted":"1"},"remote":{"other_address":{"name":"peer"}}}}"#,
            ),
        ],
    );

    assert!(family(&families, "tidb_grpc_channelz_channel_calls_total").is_none());
    assert!(family(&families, "tidb_grpc_channelz_socket_streams_total").is_none());
}

#[test]
fn formats_socket_addresses_and_skips_zero_stream_timestamp_like_go_collector() {
    let families = crate::channelz::collect_snapshots_for_test(
        r#"{
            "channel": [{
                "ref": {"channelId": "1"},
                "data": {"target": "root"},
                "socketRef": [{"socketId": "10"}, {"socketId": "11"}]
            }],
            "end": true
        }"#,
        &[
            (
                "socket",
                10,
                r#"{"socket":{
                    "ref":{"socketId":"10"},
                    "data":{"streamsStarted":"1","lastLocalStreamCreatedTimestamp":"1970-01-01T00:00:00Z"},
                    "local":{"uds_address":{"filename":"/tmp/channelz.sock"}},
                    "remote":{"other_address":{"name":"peer"}}
                }}"#,
            ),
            (
                "socket",
                11,
                r#"{"socket":{
                    "ref":{"socketId":"11"},
                    "data":{"streamsStarted":"1"},
                    "local":{"tcpip_address":{"ip_address":"fwAAAQ==","port":8080}},
                    "remote":{"other_address":{"name":"peer"}}
                }}"#,
            ),
        ],
    );

    let streams = family(&families, "tidb_grpc_channelz_socket_streams_total").unwrap();
    let uds = streams
        .get_metric()
        .iter()
        .find(|metric| label(metric, "id") == Some("10"))
        .unwrap();
    assert_eq!(label(uds, "local"), Some("/tmp/channelz.sock"));
    let tcp = streams
        .get_metric()
        .iter()
        .find(|metric| label(metric, "id") == Some("11"))
        .unwrap();
    assert_eq!(label(tcp, "local"), Some("127.0.0.1:8080"));

    assert!(
        family(
            &families,
            "tidb_grpc_channelz_socket_last_stream_created_timestamp_seconds"
        )
        .is_none()
    );
}
