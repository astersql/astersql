// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use crate::{Conds2TableDual, IsConstFalse};

#[test]
fn constant_false_uses_mysql_boolean_conversion() {
    assert!(IsConstFalse(&expression::NewStrConst("0")));
    assert!(!IsConstFalse(&expression::NewStrConst("1")));
    assert!(!IsConstFalse(&expression::NewStrConst("not-a-number")));
}

#[test]
fn multiple_conditions_only_fold_for_constant_null() {
    assert!(!Conds2TableDual(&[
        Box::new(expression::NewZero()),
        Box::new(expression::NewOne()),
    ]));
    assert!(!Conds2TableDual(&[
        Box::new(expression::NewNull()),
        Box::new(expression::NewOne()),
    ]));
    assert!(Conds2TableDual(&[Box::new(expression::NewNull())]));
}
