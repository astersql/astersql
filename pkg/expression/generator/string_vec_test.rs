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

use crate::string_vec::{TYPE_INT, TYPE_REAL, TYPE_STRING, generate_one_file};
use std::fs;

#[test]
fn type_contexts_match_go_order_and_shape() {
    assert_eq!(TYPE_INT.et_name, "Int");
    assert_eq!(TYPE_INT.type_name_in_column, "Int64");
    assert!(TYPE_INT.fixed);
    assert_eq!(TYPE_REAL.et_name, "Real");
    assert_eq!(TYPE_REAL.type_name_in_column, "Float64");
    assert!(TYPE_REAL.fixed);
    assert_eq!(TYPE_STRING.et_name, "String");
    assert!(!TYPE_STRING.fixed);
}

#[test]
fn generation_writes_formatted_implementation_and_test_pair() {
    let temp = tempfile::tempdir().expect("create temporary output directory");
    let prefix = temp.path().join("builtin_string_vec_generated");
    generate_one_file(&prefix).expect("generate implementation and tests");

    let implementation =
        fs::read_to_string(prefix.with_extension("go")).expect("read generated implementation");
    let tests =
        fs::read_to_string(format!("{}_test.go", prefix.display())).expect("read generated tests");
    assert!(implementation.contains("func (b *builtinFieldIntSig) vecEvalInt"));
    assert!(implementation.contains("b.ctor.Compare(buf0.GetString(j), buf1.GetString(j))"));
    assert!(tests.contains("TestVectorizedGeneratedBuiltinStringFunc"));
    assert!(!implementation.contains("{{"));
    assert!(!tests.contains("{{"));
}
