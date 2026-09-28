// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use crate::builtin_ilike_kernel::IlikeSig;
use crate::builtin_ilike_vec_kernel::{EscapeParam, StringParam};

#[test]
fn constant_null_string_argument_short_circuits_escape_validation() {
    let signature = IlikeSig::new("binary", true, false);

    for (expression, pattern) in [
        (
            StringParam::Constant(None),
            StringParam::Column(vec![Some("%".into()), Some("_".into())]),
        ),
        (
            StringParam::Column(vec![Some("a".into()), Some("b".into())]),
            StringParam::Constant(None),
        ),
    ] {
        assert_eq!(
            signature
                .vec_eval_int(&expression, &pattern, EscapeParam::Column, 2)
                .unwrap(),
            vec![None, None],
        );
    }
}
