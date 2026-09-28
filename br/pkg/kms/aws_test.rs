// Copyright 2026 AsterSQL.

use crate::{AwsDecryptError, classifyDecryptError};

#[test]
fn non_service_sdk_error_is_annotated_once_like_go() {
    let error = classifyDecryptError(&AwsDecryptError {
        code: "KMS error".into(),
        message: "dispatch failure".into(),
    });

    assert_eq!(error, "KMS error: dispatch failure");
}
