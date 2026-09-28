// Copyright 2026 AsterSQL.

use std::cell::RefCell;
use std::io;

use super::rlimit_other::gen_rlimit_with;

#[test]
fn gen_rlimit_preserves_success_and_error_fallback() {
    let limit = gen_rlimit_with(
        "success",
        || {
            Ok(libc::rlimit {
                rlim_cur: 4096,
                rlim_max: 8192,
            })
        },
        |_, _| panic!("success must not warn"),
    );
    assert_eq!(limit, 4096);

    let warning = RefCell::new(String::new());
    let limit = gen_rlimit_with(
        "failure",
        || Err(io::Error::from_raw_os_error(libc::EINVAL)),
        |source, err| *warning.borrow_mut() = format!("[{source}] {err}"),
    );
    assert_eq!(limit, 1024);
    let warning = warning.into_inner();
    assert!(warning.contains("[failure]"));
    assert!(warning.contains("Invalid argument"));
}
