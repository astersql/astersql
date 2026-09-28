// Copyright 2026 AsterSQL.

use std::io::{self, Read};

use crate::test_storage::fill_random_from_reader;

struct FailingRandom;

impl Read for FailingRandom {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("entropy unavailable"))
    }
}

#[test]
fn random_generation_failure_is_propagated() {
    let err = fill_random_from_reader(&mut FailingRandom, &mut [0; 16]).unwrap_err();
    assert!(err.msg.contains("failed to generate test data"));
    assert!(err.msg.contains("entropy unavailable"));
}
