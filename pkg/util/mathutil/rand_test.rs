// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// MysqlRng 单元测试：时间种子、固定种子序列与 SetSeed 会话态恢复。

use std::thread;
use std::time::Duration;

use super::{NewWithSeed, NewWithTime};

/// NewWithTime 两次构造应得到不同序列，且 Gen 落在 [0,1)。
#[test]
fn test_rand_with_time() {
    let rng1 = NewWithTime();
    // 间隔至少 1ms，降低纳秒种子碰撞概率。
    thread::sleep(Duration::from_millis(1));
    let rng2 = NewWithTime();
    let got1 = rng1.Gen();
    let got2 = rng2.Gen();
    assert!((0.0..1.0).contains(&got1));
    assert_ne!(got1, rng1.Gen());
    assert!((0.0..1.0).contains(&got2));
    assert_ne!(got2, rng2.Gen());
    assert_ne!(got1, got2);
}

/// 固定种子下前两次 Gen 必须与 Go 黄金值完全一致。
#[test]
fn test_rand_with_seed() {
    let tests = [
        (0, 0.15522042769493574, 0.620881741513388),
        (1, 0.40540353712197724, 0.8716141803857071),
        (-1, 0.9050373219931845, 0.37014932126752037),
        (i64::MAX, 0.9050373219931845, 0.37014932126752037),
    ];
    for (seed, once, twice) in tests {
        let rng = NewWithSeed(seed);
        assert_eq!(rng.Gen(), once);
        assert_eq!(rng.Gen(), twice);
    }
}

/// 手动设置 seed1/seed2 后，输出与 GetSeed 应与 Go 会话态样例一致。
#[test]
fn test_rand_with_seed_1_and_seed_2() {
    let rng = NewWithTime();
    rng.SetSeed1(10_000_000);
    rng.SetSeed2(1_000_000);
    assert_eq!(rng.Gen(), 0.028870999839968048);
    assert_eq!(rng.Gen(), 0.11641535266900002);
    assert_eq!(rng.Gen(), 0.49546379455874096);
    assert_eq!(rng.GetSeed1(), 532_000_198);
    assert_eq!(rng.GetSeed2(), 689_000_330);
}
