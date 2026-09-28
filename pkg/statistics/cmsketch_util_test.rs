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

use crate::{NewDatumMapCache, TopNMeta};

#[test]
fn datum_map_cache_decodes_double_through_the_generic_codec() {
    let encoded = codec::EncodeValue(
        codec::time::UTC,
        Vec::new(),
        vec![types::NewFloat64Datum(12_345.678)],
    )
    .unwrap();
    let value = TopNMeta {
        Encoded: encoded,
        Count: 1,
    };
    let mut cache = NewDatumMapCache();

    let datum = cache
        .Put(
            &value,
            b"double".to_vec(),
            codec::mysql::TypeDouble,
            false,
            codec::time::UTC,
        )
        .unwrap();

    assert_eq!(datum.GetFloat64(), 12_345.678);
    assert_eq!(cache.Get(b"double").unwrap().GetFloat64(), 12_345.678);
}
