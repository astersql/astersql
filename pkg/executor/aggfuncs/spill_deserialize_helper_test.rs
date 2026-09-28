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

use astersql_util_serialization::{chunk, types};

#[test]
fn constructor_defers_row_count_validation_like_go() {
    let source = chunk::NewChunkWithCapacity(vec![types::NewFieldType(16)], 1);

    // Go's newDeserializeHelper stores rowNum without inspecting the column.
    // This matters when the caller constructs the helper before rows arrive.
    let _helper = crate::DeserializeHelper::new(source.Column(0), 1);
}
