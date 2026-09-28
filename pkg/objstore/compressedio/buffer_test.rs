// Copyright 2026 AsterSQL.

use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{CompressType, new_buffer};

#[test]
fn no_compression_buffer_write_panics_like_go_nil_writer() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut buffer = new_buffer(16, CompressType::NoCompression);
        let _ = buffer.write(b"payload");
    }));

    assert!(result.is_err());
}

#[test]
fn no_compression_buffer_flush_panics_like_go_nil_writer() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut buffer = new_buffer(16, CompressType::NoCompression);
        let _ = buffer.flush();
    }));

    assert!(result.is_err());
}

#[test]
fn no_compression_buffer_close_panics_like_go_nil_writer() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut buffer = new_buffer(16, CompressType::NoCompression);
        let _ = buffer.close();
    }));

    assert!(result.is_err());
}
