// Copyright 2026 AsterSQL.

use crate::errors::{Error, eof};

#[test]
fn eof_detection_matches_go_identity_check() {
    assert!(eof().is_eof());
    assert!(!Error::new("not EOF").is_eof());
    assert!(!Error::annotate(eof(), "receiving failed").is_eof());
}
