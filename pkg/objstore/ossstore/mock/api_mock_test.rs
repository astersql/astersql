// Copyright 2026 AsterSQL.

use super::api_mock::{MockAPI, MockAPIMockRecorder, NewMockAPI};

/// GoMock exposes a named recorder type; keep that public contract in Rust.
#[test]
fn expect_returns_the_named_go_mock_recorder_type() {
    let mut mock = NewMockAPI();
    let recorder: &mut MockAPIMockRecorder = mock.EXPECT();
    let _: &mut MockAPI = recorder;
}
