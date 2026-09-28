// Copyright 2026 AsterSQL.

use crate::Error;
use crate::local_unix::{RawRLimit, verify_rlimit_with};

fn limit(current: u64, maximum: u64) -> RawRLimit {
    RawRLimit { current, maximum }
}

#[test]
fn setrlimit_error_is_returned_without_a_second_read() {
    let mut reads = 0;
    let result = verify_rlimit_with(
        200,
        || {
            reads += 1;
            Ok(RawRLimit {
                current: if reads == 1 { 100 } else { 200 },
                maximum: 100,
            })
        },
        |_| Err(std::io::Error::from_raw_os_error(1)),
    );

    assert_eq!(reads, 1, "Go returns immediately when setrlimit fails");
    let Error::Io(message) = result.expect_err("setrlimit failure must be returned") else {
        panic!("expected an I/O error");
    };
    assert!(message.contains("got 100"), "{message}");
    assert!(message.contains("greater or equal to 200"), "{message}");
}

#[test]
fn sufficient_limit_skips_set_and_second_read() {
    let mut reads = 0;
    let mut sets = 0;
    verify_rlimit_with(
        100,
        || {
            reads += 1;
            Ok(limit(100, 200))
        },
        |_| {
            sets += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!((reads, sets), (1, 0));
}

#[test]
fn request_is_capped_and_hard_limit_is_raised() {
    let mut reads = 0;
    let mut applied = None;
    verify_rlimit_with(
        crate::local_unix::maxRLimit + 1,
        || {
            reads += 1;
            Ok(if reads == 1 {
                limit(100, 500)
            } else {
                limit(crate::local_unix::maxRLimit, crate::local_unix::maxRLimit)
            })
        },
        |value| {
            applied = Some(*value);
            Ok(())
        },
    )
    .unwrap();

    let applied = applied.unwrap();
    assert_eq!(applied.current, crate::local_unix::maxRLimit);
    assert_eq!(applied.maximum, crate::local_unix::maxRLimit);
    assert_eq!(reads, 2);
}

#[test]
fn existing_hard_limit_is_preserved() {
    let mut reads = 0;
    let mut applied = None;
    verify_rlimit_with(
        200,
        || {
            reads += 1;
            Ok(if reads == 1 {
                limit(100, 300)
            } else {
                limit(200, 300)
            })
        },
        |value| {
            applied = Some(*value);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(applied.unwrap().maximum, 300);
}

#[test]
fn second_read_error_is_propagated() {
    let mut reads = 0;
    let result = verify_rlimit_with(
        200,
        || {
            reads += 1;
            if reads == 1 {
                Ok(limit(100, 300))
            } else {
                Err(Error::Io("second read failed".into()))
            }
        },
        |_| Ok(()),
    );
    assert_eq!(result, Err(Error::Io("second read failed".into())));
}

#[test]
fn ineffective_set_reports_manual_remediation() {
    let mut reads = 0;
    let result = verify_rlimit_with(
        200,
        || {
            reads += 1;
            Ok(limit(100, 300))
        },
        |_| Ok(()),
    );
    let Error::Io(message) = result.unwrap_err() else {
        panic!("expected an I/O error");
    };
    assert!(message.contains("expected: 200, got: 100"), "{message}");
    assert!(message.contains("ulimit -n 200"), "{message}");
}
