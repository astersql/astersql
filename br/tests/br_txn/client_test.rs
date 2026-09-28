// Copyright 2026 AsterSQL.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::client::{createClient, parse_flags, randGenWithDuration};

#[test]
fn integer_flags_accept_go_base_zero_and_native_width() {
    let flags = parse_flags(&[
        "--key-max-len=0x_20".into(),
        "--concurrency=010".into(),
        "--duration=-0x2".into(),
    ]);

    assert_eq!(flags.key_max_len, 32);
    assert_eq!(flags.concurrency, 8);
    assert_eq!(flags.duration, -2);

    if isize::BITS == 64 {
        let wide = parse_flags(&[
            "--concurrency=0x80000000".into(),
            "--duration=-0x8000000000000000".into(),
        ]);
        assert_eq!(wide.concurrency as i128, 2_147_483_648);
        assert_eq!(wide.duration as i128, -9_223_372_036_854_775_808);

        let overflow = catch_unwind(AssertUnwindSafe(|| {
            parse_flags(&["--concurrency=0x8000000000000000".into()]);
        }));
        assert!(overflow.is_err(), "out-of-range native int must fail");
    }

    for invalid in ["08", "0x", "_1", "1__0"] {
        let invalid = catch_unwind(AssertUnwindSafe(|| {
            parse_flags(&[format!("--duration={invalid}")]);
        }));
        assert!(invalid.is_err(), "Go rejects {invalid:?}");
    }
}

#[test]
fn duration_uses_go_time_duration_overflow() {
    if isize::BITS != 64 {
        return;
    }

    let flags = parse_flags(&["--duration=0x7fffffffffffffff".into()]);
    let client =
        createClient("client-test-duration-overflow:2379", "", "", "").expect("create client");
    let started = std::time::Instant::now();

    randGenWithDuration(&client, b"aa", b"zz", 8, 0, flags.duration)
        .expect("overflowed Go duration must time out");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "Go's overflowed time.Duration fires immediately"
    );
}
