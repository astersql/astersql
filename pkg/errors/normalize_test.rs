// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

use super::{Normalize, RFCCodeText};
use crate::{New, WithMessage};

/// Go `Error.Cause` calls `Unwrap` exactly once on the attached cause.
#[test]
fn cause_unwraps_only_one_attached_layer() {
    let root = New("root");
    let inner = WithMessage(Some(root), "inner").expect("inner wrapper");
    let outer = WithMessage(Some(inner.clone()), "outer").expect("outer wrapper");
    let normalized = Normalize("normalized", &[RFCCodeText("errors:normalized")])
        .Wrap(Some(outer))
        .expect("normalized wrapper");

    let cause = normalized.Cause().expect("one-level cause");
    assert!(cause.ptr_eq(&inner));
    assert_eq!(cause.to_string(), "inner: root");
}
