// Copyright 2026 AsterSQL.

use std::sync::Arc;

use crate::{Request, RequestPayload, coprHandler};

#[test]
fn checksum_response_matches_go_protobuf_wire_format() {
    let handler = coprHandler::new(Arc::new(crate::copr_handler::MemoryReader::default()));
    let request = Request {
        ranges: Vec::new(),
        start_ts: 0,
        payload: RequestPayload::Checksum,
    };

    let response = handler.handleCopChecksumRequest(&request);

    // tipb.ChecksumResponse{Checksum: 1, TotalKvs: 1, TotalBytes: 1}
    // uses three protobuf varint fields numbered 1, 2, and 3.
    assert_eq!(response.data, [0x08, 0x01, 0x10, 0x01, 0x18, 0x01]);
    assert_eq!(response.other_error, None);
}
