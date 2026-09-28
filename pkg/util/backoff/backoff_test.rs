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

// 指数退避单元测试（对应 Go `backoff_test.go`）。
//
// 用纳秒整数构造 `Duration`，验证倍率恒为 1 时不增长，以及倍率 2 时按
// 1→2→4→8→10… 截断到上限的序列。

// 本文件由 pkg/util/backoff/backoff_test.go 迁移而来，保留指数退避测试结构。

use std::time::Duration;

use super::*;

// test_exponential 对应 Go 的 TestExponential。
// Go 使用 time.Duration 的整数纳秒值作为输入和期望；用 Duration::from_nanos 保留同一数值含义。
/// 覆盖固定退避、不增长、以及指数增长后受 `maxBackoff` 截断三种场景。
#[test]
fn test_exponential() {
    let one = Duration::from_nanos(1);
    let ten = Duration::from_nanos(10);

    let mut backoffer = NewExponential(one, 1.0, one);
    for i in 0..10 {
        // base、multiplier、max 全为 1 时，每次退避都被固定为 1。
        assert_eq!(one, backoffer.Backoff(i));
    }

    backoffer = NewExponential(one, 1.0, ten);
    for i in 0..10 {
        // multiplier 为 1 时不会增长，即使 maxBackoff 更大也仍返回 baseBackoff。
        assert_eq!(one, backoffer.Backoff(i));
    }

    backoffer = NewExponential(one, 2.0, ten);
    let res = [
        Duration::from_nanos(1),
        Duration::from_nanos(2),
        Duration::from_nanos(4),
        Duration::from_nanos(8),
        Duration::from_nanos(10),
        Duration::from_nanos(10),
        Duration::from_nanos(10),
        Duration::from_nanos(10),
        Duration::from_nanos(10),
        Duration::from_nanos(10),
    ];
    for i in 0..10 {
        // 倍增到超过 maxBackoff 后被截断为 10，后续调用继续保持上限。
        assert_eq!(res[i], backoffer.Backoff(i));
    }
}
