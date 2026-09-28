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

// 退避模块迁移补充单元测试。
//
// 覆盖指数序列与上限截断、`retryCnt == 0` 复位复用实例，以及通过
// `dyn Backoffer` 动态分发调用。

use std::time::Duration;

use super::backoff::{Backoffer, NewExponential};

/// 验证倍率 2、上限 10 时的完整纳秒序列与封顶行为。
#[test]
fn migration_exponential_matches_go_sequence_and_cap() {
    let mut backoffer = NewExponential(Duration::from_nanos(1), 2.0, Duration::from_nanos(10));
    let expected = [1, 2, 4, 8, 10, 10, 10, 10, 10, 10];

    for (retry_count, expected_nanos) in expected.into_iter().enumerate() {
        assert_eq!(
            Duration::from_nanos(expected_nanos),
            backoffer.Backoff(retry_count)
        );
    }
}

/// 复用同一实例时，再次 `Backoff(0)` 必须复位到 `baseBackoff`。
#[test]
fn migration_retry_zero_resets_a_reused_backoffer() {
    let mut backoffer = NewExponential(Duration::from_nanos(3), 2.0, Duration::from_nanos(20));
    assert_eq!(Duration::from_nanos(3), backoffer.Backoff(0));
    assert_eq!(Duration::from_nanos(6), backoffer.Backoff(1));
    assert_eq!(Duration::from_nanos(12), backoffer.Backoff(2));
    assert_eq!(Duration::from_nanos(3), backoffer.Backoff(0));
    assert_eq!(Duration::from_nanos(6), backoffer.Backoff(1));
}

/// 通过 trait 对象调用时，浮点倍率截断语义仍与具体类型一致。
#[test]
fn migration_backoffer_trait_is_usable_dynamically() {
    let mut backoffer: Box<dyn Backoffer> = Box::new(NewExponential(
        Duration::from_nanos(2),
        1.5,
        Duration::from_nanos(10),
    ));

    assert_eq!(Duration::from_nanos(2), backoffer.Backoff(0));
    assert_eq!(Duration::from_nanos(3), backoffer.Backoff(1));
    assert_eq!(Duration::from_nanos(4), backoffer.Backoff(2));
}
