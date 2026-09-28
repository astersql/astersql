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

use super::cpu_windows::filetime_ticks_to_unix_nanos;

#[test]
fn filetime_conversion_matches_go_epoch_and_units() {
    const WINDOWS_TO_UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

    assert_eq!(filetime_ticks_to_unix_nanos(WINDOWS_TO_UNIX_EPOCH_TICKS), 0);
    assert_eq!(
        filetime_ticks_to_unix_nanos(WINDOWS_TO_UNIX_EPOCH_TICKS + 12_345_678),
        1_234_567_800
    );
}
