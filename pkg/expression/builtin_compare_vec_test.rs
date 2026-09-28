// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 向量比较内核测试入口：通过 `#[path]` 引入 Go 对齐实现文件。

#[path = "builtin_compare_vec_6_aster_unit_test.rs"]
mod go_parity;

use crate::builtin_compare_vec_kernel::{NullableColumn, greatest_string_by, least_string_by};

#[test]
fn collation_equal_string_extrema_choose_later_argument_like_go() {
    let columns = [
        NullableColumn::new(vec!["Alpha".to_owned()]),
        NullableColumn::new(vec!["alpha".to_owned()]),
    ];
    let case_insensitive =
        |left: &str, right: &str| left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase());

    assert_eq!(
        greatest_string_by(&columns, case_insensitive)
            .unwrap()
            .values,
        vec!["alpha"]
    );
    assert_eq!(
        least_string_by(&columns, case_insensitive).unwrap().values,
        vec!["alpha"]
    );
}
