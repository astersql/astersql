// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use cascades_base::NewHashEqualer;

use crate::{JoinType, PossiblePropertiesInfo};

#[test]
fn full_outer_join_keeps_go_discriminant_and_outer_semantics() {
    assert_eq!(JoinType::FullOuterJoin as i32, 7);
    assert!(JoinType::FullOuterJoin.is_outer_join());
    assert_eq!(JoinType::FullOuterJoin.to_string(), "full outer join");
}

#[test]
fn possible_properties_hash_starts_with_non_nil_object_marker() {
    let info = PossiblePropertiesInfo {
        orders: None,
        has_tiflash: false,
    };

    let mut actual = NewHashEqualer();
    info.hash64(actual.as_mut());

    let mut expected = NewHashEqualer();
    expected.HashByte(cascades_base::NotNilFlag);
    expected.HashByte(cascades_base::NilFlag);

    assert_eq!(actual.Sum64(), expected.Sum64());
}
