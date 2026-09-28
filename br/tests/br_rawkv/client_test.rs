// Copyright 2026 AsterSQL.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::client::{createClient, parse_flags, put, randGen, randGenWithDuration};

#[test]
fn integer_flags_accept_go_base_zero_syntax() {
    let flags = parse_flags(&[
        "--key-max-len=0x20".into(),
        "--concurrency=010".into(),
        "--duration=-0x2".into(),
    ]);

    assert_eq!(flags.key_max_len, 32);
    assert_eq!(flags.concurrency, 8);
    assert_eq!(flags.duration, -2);

    let wide = parse_flags(&[
        "--concurrency=0x80000000".into(),
        "--duration=-0x8000000000000000".into(),
    ]);
    assert_eq!(wide.concurrency as i128, 2_147_483_648);
    assert_eq!(wide.duration as i128, -9_223_372_036_854_775_808);

    let overflow = catch_unwind(AssertUnwindSafe(|| {
        parse_flags(&["--concurrency=0x8000000000000000".into()]);
    }));
    assert!(
        overflow.is_err(),
        "out-of-range native int flag must fail parsing"
    );
}

#[test]
fn put_trims_only_ascii_spaces_like_go() {
    let client = createClient("client-test-trim:2379", "", "", "").expect("create client");

    put(&client, " 61 : 62 ").expect("Go strings.Trim accepts surrounding ASCII spaces");

    for data in ["\t61\t:62", "61:\n62"] {
        let err = put(&client, data).expect_err("non-space whitespace must reach hex decoder");
        assert!(
            err.msg.contains("invalid kv pair string"),
            "unexpected error for {data:?}: {}",
            err.msg
        );
    }
}

#[test]
fn rand_gen_rejects_negative_channel_capacity_like_go() {
    let client =
        createClient("client-test-negative-concurrency:2379", "", "", "").expect("create client");
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = randGen(&client, b"aa", b"zz", 8, -1);
    }));
    assert!(panicked.is_err(), "negative Go channel capacity must panic");

    let wrapper_panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = randGenWithDuration(&client, b"aa", b"zz", 8, -1, 10);
    }));
    assert!(
        wrapper_panicked.is_err(),
        "a worker panic must not become successful channel disconnection"
    );

    randGenWithDuration(&client, b"aa", b"zz", 8, 0, 0)
        .expect("duration wrapper must stop a zero-concurrency wait");
}

#[test]
fn rand_gen_duration_uses_go_time_duration_overflow() {
    let client =
        createClient("client-test-duration-overflow:2379", "", "", "").expect("create client");
    let started = std::time::Instant::now();

    randGenWithDuration(&client, b"aa", b"zz", 8, 0, isize::MAX)
        .expect("overflowed Go duration must time out");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "Go's overflowed time.Duration fires immediately"
    );
}
