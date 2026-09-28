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

use model_dependency::IndexInfo;
use tipb::{AnnQueryType, ColumnInfo, ColumnarIndexType, VectorDistanceMetric};

use crate::buildVectorIndexExtra;

#[test]
fn build_vector_index_extra_matches_go_payload_without_validation() {
    let index = IndexInfo {
        ID: 42,
        ..IndexInfo::default()
    };
    let mut column = ColumnInfo::new();
    column.set_column_id(7);
    column.set_tp(15);
    let ref_vec = vec![0, 0, 128, 63, 0, 0, 0, 64];

    let extra = buildVectorIndexExtra(
        &index,
        AnnQueryType::OrderBy,
        VectorDistanceMetric::Cosine,
        0,
        "embedding",
        &ref_vec,
        &column,
    );

    assert_eq!(extra.IndexInfo.ID, 42);
    assert_eq!(
        extra.QueryInfo.get_index_type(),
        ColumnarIndexType::TypeVector
    );
    let query = extra.QueryInfo.get_ann_query_info();
    assert_eq!(query.get_query_type(), AnnQueryType::OrderBy);
    assert_eq!(query.get_distance_metric(), VectorDistanceMetric::Cosine);
    assert_eq!(query.get_top_k(), 0, "Go performs no top-k validation");
    assert_eq!(query.get_column_name(), "embedding");
    assert_eq!(query.get_index_id(), 42);
    assert_eq!(query.get_ref_vec_f32(), ref_vec);
    assert_eq!(query.get_column(), &column);
}
