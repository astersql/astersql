// Copyright 2026 AsterSQL.
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

use super::rand::bounded;

#[test]
fn bounded_random_uses_the_full_state_width() {
    assert_eq!(bounded(0, 10), 0);
    assert_eq!(bounded(u64::MAX, 10), 9);
    assert_eq!(
        (0..10)
            .map(|bucket| bounded((u64::MAX / 10) * bucket + u64::MAX / 20, 10))
            .collect::<Vec<_>>(),
        (0..10).collect::<Vec<_>>()
    );
}
