// Copyright 2026 AsterSQL.

use crate::write::{WriteRuntime, updateRecord};
use std::cmp::Ordering;

#[derive(Clone)]
struct Column;

struct Runtime;

impl WriteRuntime for Runtime {
    type Context = ();
    type Datum = i64;
    type Column = Column;
    type Assignment = ();
    type Handle = i64;
    type DuplicateKeyMode = ();
    type ForeignKeyCheck = ();
    type ForeignKeyCascade = ();
    type Error = &'static str;

    fn columns(&self) -> Vec<Column> {
        vec![Column]
    }
    fn column_is_generated(&self, _: &Column) -> bool {
        false
    }
    fn column_is_virtual_generated(&self, _: &Column) -> bool {
        false
    }
    fn column_is_on_update_now(&self, _: &Column) -> bool {
        true
    }
    fn column_is_auto_increment(&self, _: &Column) -> bool {
        false
    }
    fn column_is_pk_handle(&self, _: &Column) -> bool {
        true
    }
    fn column_is_common_handle(&self, _: &Column) -> bool {
        false
    }
    fn column_is_not_null(&self, _: &Column) -> bool {
        false
    }
    fn column_prevents_null_insert(&self, _: &Column) -> bool {
        false
    }
    fn column_name(&self, _: &Column) -> String {
        "ts".into()
    }
    fn datum_is_null(&self, _: &i64) -> bool {
        false
    }
    fn zero_datum(&self) -> i64 {
        0
    }
    fn compare_binary(&mut self, left: &i64, right: &i64) -> Result<Ordering, &'static str> {
        Ok(left.cmp(right))
    }
    fn rebase_auto_increment(
        &mut self,
        _: &mut (),
        _: &Column,
        _: &i64,
    ) -> Result<(), &'static str> {
        Ok(())
    }
    fn table_contains_auto_random_bits(&self) -> bool {
        false
    }
    fn auto_random_incremental_mask(&self, _: &Column) -> i64 {
        0
    }
    fn auto_record_id(&self, _: &Column, _: &i64) -> Result<i64, &'static str> {
        Ok(0)
    }
    fn rebase_auto_random(&mut self, _: &mut (), _: i64) -> Result<(), &'static str> {
        Ok(())
    }
    fn client_found_rows(&self) -> bool {
        false
    }
    fn lock_unchanged_keys(&self) -> bool {
        false
    }
    fn in_pessimistic_transaction(&self) -> bool {
        false
    }
    fn add_unchanged_keys_for_lock(
        &mut self,
        _: &mut (),
        _: &i64,
        _: &[i64],
        _: u8,
    ) -> Result<usize, &'static str> {
        Ok(0)
    }
    fn add_touched_rows(&mut self, _: u64) {}
    fn add_affected_rows(&mut self, _: u64) {}
    fn add_updated_rows(&mut self, _: u64) {}
    fn add_copied_rows(&mut self, _: u64) {}
    fn current_timestamp(&mut self, _: &mut (), _: &Column) -> Result<i64, &'static str> {
        Ok(3)
    }
    fn on_update_now_pk_handle_error(&self) -> &'static str {
        "on-update-now column should never be pk-is-handle"
    }
    fn evaluation_buffer_exists(&self) -> bool {
        false
    }
    fn set_evaluation_buffer_datum(&mut self, _: usize, _: i64) {}
    fn assignment_lazy_error(&mut self, _: &()) -> Option<&'static str> {
        None
    }
    fn assignment_column_index(&self, _: &()) -> usize {
        0
    }
    fn evaluate_assignment(&mut self, _: &mut (), _: &()) -> Result<i64, &'static str> {
        unreachable!()
    }
    fn cast_assignment(&mut self, _: &mut (), _: &(), _: &i64) -> Result<i64, &'static str> {
        unreachable!()
    }
    fn bad_null_error(&self, _: &Column) -> &'static str {
        "bad null"
    }
    fn handle_bad_null(&mut self, _: &Column, _: &mut i64) -> Result<(), &'static str> {
        Ok(())
    }
    fn check_fk_ignore_error(
        &mut self,
        _: &mut (),
        _: &mut [()],
        _: &[i64],
    ) -> Result<bool, &'static str> {
        Ok(false)
    }
    fn exchange_partition_check_required(&self) -> bool {
        false
    }
    fn check_exchange_partition_row(&mut self, _: &mut (), _: &[i64]) -> Result<(), &'static str> {
        Ok(())
    }
    fn replace_record_with_staging(
        &mut self,
        _: &mut (),
        _: &i64,
        _: &[i64],
        _: &[i64],
        _: &(),
    ) -> Result<(), &'static str> {
        Ok(())
    }
    fn in_transaction(&self) -> bool {
        false
    }
    fn in_foreign_key_trigger(&self) -> bool {
        false
    }
    fn has_foreign_key_cascades(&self) -> bool {
        false
    }
    fn update_record(
        &mut self,
        _: &mut (),
        _: &i64,
        _: &[i64],
        _: &[i64],
        _: &[bool],
        _: &(),
        _: bool,
    ) -> Result<(), &'static str> {
        Ok(())
    }
    fn handle_partition_error(&mut self, error: &'static str) -> &'static str {
        error
    }
    fn foreign_key_update_check(
        &mut self,
        _: &mut (),
        _: &[i64],
        _: &[i64],
    ) -> Result<(), &'static str> {
        Ok(())
    }
    fn foreign_key_update_cascade(
        &mut self,
        _: &mut (),
        _: &[i64],
        _: &[i64],
    ) -> Result<(), &'static str> {
        Ok(())
    }
}

#[test]
fn on_update_now_pk_handle_returns_error_instead_of_panicking() {
    let result = updateRecord(
        &mut Runtime,
        &mut (),
        1,
        vec![1],
        vec![2],
        0,
        &[],
        |_, _, _| Ok(()),
        vec![false],
        false,
        &mut [],
        &mut [],
        &(),
        false,
    );

    assert_eq!(
        result,
        Err("on-update-now column should never be pk-is-handle")
    );
}
