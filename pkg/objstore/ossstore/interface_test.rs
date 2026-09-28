// Copyright 2026 AsterSQL.

use crate::content_range_for_download;

#[test]
fn ranged_download_reports_go_response_content_range() {
    assert_eq!(
        content_range_for_download(Some("bytes=0-9"), 10, 100).unwrap(),
        Some("bytes 0-9/100".to_owned())
    );
    assert_eq!(
        content_range_for_download(Some("bytes=50-"), 50, 100).unwrap(),
        Some("bytes 50-99/100".to_owned())
    );
    assert_eq!(content_range_for_download(None, 100, 100).unwrap(), None);
}

#[test]
fn ranged_download_rejects_inconsistent_sdk_responses() {
    let error = content_range_for_download(Some("bytes=90-99"), 5, 100).unwrap_err();
    assert!(error.to_string().contains("expected 10 bytes, received 5"));
}
