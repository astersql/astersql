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

use model_dependency::IndexInfo;
use physicalop_dependency::ColumnarIndexExtra;
use tipb::{
    AnnQueryInfo, AnnQueryType, ColumnInfo, ColumnarIndexInfo, ColumnarIndexType,
    VectorDistanceMetric,
};

/// Build the vector-index protobuf payload attached to a physical table scan.
///
/// This deliberately performs no validation: the Go helper is a lossless
/// constructor, and its callers validate or derive these values beforehand.
pub fn buildVectorIndexExtra(
    index_info: &IndexInfo,
    query_type: AnnQueryType,
    distance_metric: VectorDistanceMetric,
    top_k: u32,
    column_name: &str,
    ref_vec: &[u8],
    column: &ColumnInfo,
) -> ColumnarIndexExtra {
    let mut ann_query_info = AnnQueryInfo::new();
    ann_query_info.set_query_type(query_type);
    ann_query_info.set_distance_metric(distance_metric);
    ann_query_info.set_top_k(top_k);
    ann_query_info.set_column_name(column_name.to_owned());
    ann_query_info.set_index_id(index_info.ID);
    ann_query_info.set_ref_vec_f32(ref_vec.to_vec());
    ann_query_info.set_column(column.clone());

    let mut query_info = ColumnarIndexInfo::new();
    query_info.set_index_type(ColumnarIndexType::TypeVector);
    query_info.set_ann_query_info(ann_query_info);

    ColumnarIndexExtra {
        IndexInfo: index_info.Clone(),
        QueryInfo: query_info,
    }
}
