// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn error_preserves_go_proto_field_names_for_flashback_errors() {
    let mut recovery = errorpb::RecoveryInProgress::new();
    recovery.set_region_id(11);
    let mut flashback = errorpb::FlashbackInProgress::new();
    flashback.set_region_id(12);
    flashback.set_flashback_start_ts(13);
    let mut not_prepared = errorpb::FlashbackNotPrepared::new();
    not_prepared.set_region_id(14);

    let mut request_error = errorpb::Error::new();
    request_error.set_recovery_in_progress(recovery);
    request_error.set_flashback_in_progress(flashback);
    request_error.set_flashback_not_prepared(not_prepared);

    let wrapped = PBError {
        RequestErr: Some(request_error),
    };

    assert_eq!(
        wrapped.Error(),
        "RecoveryInProgress:<region_id:11 > FlashbackInProgress:<region_id:12 flashback_start_ts:13 > FlashbackNotPrepared:<region_id:14 > "
    );
}
