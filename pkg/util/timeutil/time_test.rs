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

// `Sleep` 单元测试（对齐 Go timeutil 用例）。
//
// 验证取消令牌先于长睡眠触发时返回 Cancelled，且实际耗时介于超时与睡眠时长之间。

use std::time::{Duration, Instant};

use astersql_util_timeutil::time::{CancellationToken, Sleep, SleepError};

/// 短超时取消长 Sleep：结果为 Cancelled，耗时落在合理区间。
#[tokio::test(flavor = "current_thread")]
async fn test_sleep() {
    let context_timeout = Duration::from_millis(10);
    let sleep_time = Duration::from_secs(10);
    let now = Instant::now();
    let context = CancellationToken::new();
    let timeout_context = context.clone();

    let timeout = tokio::spawn(async move {
        tokio::time::sleep(context_timeout).await;
        timeout_context.cancel();
    });

    let result = Sleep(&context, sleep_time).await;
    timeout.await.expect("timeout task must finish");

    let elapsed = now.elapsed();
    assert_eq!(result, Err(SleepError::Cancelled));
    assert!(elapsed > context_timeout, "elapsed {elapsed:?}");
    assert!(elapsed < sleep_time, "elapsed {elapsed:?}");
}
