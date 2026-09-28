// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use std::time::Duration;

use crate::AtomicDuration;

#[test]
fn atomic_duration_preserves_go_nanosecond_precision() {
    let initial = Duration::from_nanos(1_234_567);
    let duration = AtomicDuration::new(initial);
    assert_eq!(duration.Load(), initial);

    let updated = Duration::from_nanos(9_876_543);
    duration.Store(updated);
    assert_eq!(duration.Load(), updated);
}
