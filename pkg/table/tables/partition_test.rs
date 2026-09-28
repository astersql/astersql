// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use crate::mutation_checker::Datum;
use crate::partition::{ForRangePruning, PartitionError};

/// Go `ForRangePruning.Compare` preserves the unsigned value's full `uint64`
/// range instead of narrowing it to a signed integer before comparison.
#[test]
fn unsigned_range_values_do_not_wrap_into_the_first_partition() {
    let pruning = ForRangePruning {
        column_offset: 0,
        upper_bounds: vec![Some(10), Some(i64::MAX)],
    };

    assert_eq!(
        pruning.locate(&[Datum::Uint(u64::MAX)]),
        Err(PartitionError::NoPartitionForValue)
    );
}
