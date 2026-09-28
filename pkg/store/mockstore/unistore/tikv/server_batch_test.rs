// Copyright 2026 AsterSQL.

use crate::server_batch::{BatchCommandResponse, ResponseIdPair, collect_batch_response};

#[test]
fn collected_responses_preserve_channel_arrival_order_like_go() {
    let response = collect_batch_response([
        ResponseIdPair {
            request_id: 20,
            response: BatchCommandResponse::Value(Some(b"first".to_vec())),
        },
        ResponseIdPair {
            request_id: 10,
            response: BatchCommandResponse::Empty,
        },
    ]);

    assert_eq!(vec![20, 10], response.request_ids);
    assert_eq!(
        vec![
            BatchCommandResponse::Value(Some(b"first".to_vec())),
            BatchCommandResponse::Empty,
        ],
        response.responses
    );
}
