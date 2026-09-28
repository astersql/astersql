// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::builtin_regexp_util_kernel::*;

#[test]
fn released_buffers_remain_attached_to_parameters_like_go() {
    let mut params = [
        FuncParam::column(Column::new(vec![Some("value")])),
        FuncParam::constant(),
    ];
    let mut allocator = BufferAllocator::default();

    release_buffers(&mut allocator, &mut params);

    assert_eq!(allocator.returned_len(), 1);
    assert_eq!(get_buffers(&params).len(), 1);
}

#[test]
fn reserve_string_resets_existing_rows_before_filling_nulls() {
    let mut result = Column::new(vec![Some("old".to_owned())]);

    fill_null_string_into_result(&mut result, 2);

    assert_eq!(result.values(), &[None, None]);
}

#[test]
fn null_lookup_panics_for_an_out_of_range_row_like_chunk_column() {
    let columns = vec![Column::new(vec![Some("value")])];

    assert!(std::panic::catch_unwind(|| is_result_null(&columns, 1)).is_err());
}
