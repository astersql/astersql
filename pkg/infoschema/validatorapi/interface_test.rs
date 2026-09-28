// Copyright 2026 AsterSQL.

use super::{Result, Validator};
use std::cell::Cell;

#[derive(Default)]
struct RecordingValidator {
    related_ids_were_nil: Cell<Option<bool>>,
}

impl Validator for RecordingValidator {
    type RelatedSchemaChange = ();

    fn Update(&self, _: u64, _: i64, _: i64, _: Option<&Self::RelatedSchemaChange>) {}

    fn Check(
        &self,
        _: u64,
        _: i64,
        related_physical_table_ids: Option<&[i64]>,
        _: bool,
    ) -> (Option<Self::RelatedSchemaChange>, Result) {
        self.related_ids_were_nil
            .set(Some(related_physical_table_ids.is_none()));
        (None, Result::ResultSucc)
    }

    fn Stop(&self) {}
    fn Restart(&self, _: i64) {}
    fn Reset(&self) {}
    fn IsStarted(&self) -> bool {
        true
    }
    fn IsLeaseExpired(&self) -> bool {
        false
    }
}

#[test]
fn check_preserves_go_nil_and_empty_slice_distinction() {
    let validator = RecordingValidator::default();

    validator.Check(1, 1, None, true);
    assert_eq!(validator.related_ids_were_nil.get(), Some(true));

    validator.Check(1, 1, Some(&[]), true);
    assert_eq!(validator.related_ids_were_nil.get(), Some(false));
}
