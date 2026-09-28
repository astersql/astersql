// Copyright 2026 AsterSQL.

use super::provider_mock::{
    MockCredentialsProvider, MockCredentialsProviderMockRecorder, NewMockCredentialsProvider,
};

/// GoMock exposes a named recorder type; keep that public contract in Rust.
#[test]
fn expect_returns_the_named_go_mock_recorder_type() {
    let mut mock = NewMockCredentialsProvider();
    let recorder: &mut MockCredentialsProviderMockRecorder = mock.EXPECT();
    let _: &mut MockCredentialsProvider = recorder;
}
