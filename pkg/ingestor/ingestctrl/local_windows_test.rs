// Copyright 2026 AsterSQL.

use crate::Error;
use crate::local_windows::{GetSystemRLimit, VerifyRLimit};

#[test]
fn windows_rlimit_matches_go_contract() {
    assert_eq!(GetSystemRLimit(), Ok(i32::MAX as u64));

    let expected = "local-backend is not tested on Windows. Run with --check-requirements=false to disable this check, but you are on your own risk";
    assert_eq!(
        VerifyRLimit(0),
        Err(Error::InvalidData(expected.to_owned()))
    );
    assert_eq!(
        VerifyRLimit(u64::MAX),
        Err(Error::InvalidData(expected.to_owned()))
    );
}
