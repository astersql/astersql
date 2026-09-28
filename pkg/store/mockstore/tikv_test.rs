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

// MockTiKVDriver 相关单元测试：校验事务本地闩配置与 URI 解析。
//
// 事务本地闩（txn local latch）用于在客户端侧做写写冲突的粗粒度检测。

use crate::MockTiKVDriver;
use astersql_config::{TxnLocalLatches, update_global};

/// 验证 latch 开关随全局配置生效，以及非法/错误 scheme URI 被拒绝。
#[test]
fn test_config() {
    // 启用 latch：打开 mocktikv 后应报告 latch 已启用。
    update_global(|conf| {
        conf.txn_local_latches = TxnLocalLatches {
            enabled: true,
            capacity: 10240,
        };
    });

    let driver = MockTiKVDriver;
    let store = driver.open("mocktikv://").expect("open mocktikv");
    assert!(store.is_latch_enabled());
    store.close().expect("close store");

    // 关闭 latch：新打开的 store 应报告未启用。
    update_global(|conf| {
        conf.txn_local_latches = TxnLocalLatches {
            enabled: false,
            capacity: 10240,
        };
    });
    let store = driver.open("mocktikv://").expect("open mocktikv");
    assert!(!store.is_latch_enabled());
    store.close().expect("close store");

    // 缺少 scheme 的非法 URI。
    match driver.open(":") {
        Ok(_) => panic!("invalid URI must fail"),
        Err(err) => assert!(!err.to_string().is_empty()),
    }

    // 非 mocktikv scheme 应被拒绝。
    match driver.open("faketikv://") {
        Ok(_) => panic!("unsupported scheme must fail"),
        Err(err) => assert!(err.to_string().contains("mocktikv")),
    }
}
