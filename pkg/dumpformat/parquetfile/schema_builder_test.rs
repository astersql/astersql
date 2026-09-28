// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::schema_builder::build_parquet_schema_from_columns;
use crate::{ColumnInfo, LogicalType, PhysicalType};

#[test]
fn out_of_range_decimal_precision_falls_back_to_utf8_regardless_of_scale() {
    for precision in [0, 39] {
        let info = ColumnInfo {
            name: format!("decimal_{precision}"),
            database_type_name: " decimal ".into(),
            nullable: false,
            precision,
            scale: -1,
        };

        let (schema, columns) = build_parquet_schema_from_columns(&[info]).unwrap();
        assert_eq!(schema.fields[0].physical, PhysicalType::ByteArray);
        assert_eq!(schema.fields[0].logical, LogicalType::String);
        assert_eq!(columns[0].column_type.physical, PhysicalType::ByteArray);
        assert_eq!(columns[0].column_type.logical, LogicalType::String);
    }
}
