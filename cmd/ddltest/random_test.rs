// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! 对 `cmd/ddltest/random_test.go` 中随机辅助函数做 Go 等价覆盖。
//! Go 原文件只暴露辅助函数而没有 `Test*` 入口，因此这里用 Rust
//! 断言把可观察契约固定下来，避免后续重构改坏取值范围或输出形态。

use astersql_cmd_ddltest::stubs::{
    random_float, random_int, random_intn, random_num, random_string,
};

#[test]
fn test_random_helpers() {
    // Go 的 `rand.Int` 只返回非负整数。
    assert!(random_int() >= 0);
    // 重复采样固定 Go 半开区间契约，不绑定具体随机序列。
    for _ in 0..256 {
        assert!((0..10).contains(&random_intn(10)));
        let value = random_float();
        assert!((0.0..1.0).contains(&value), "random_float()={value}");
    }
    assert_eq!(random_string(0), "");
    let s = random_string(10);
    // 随机串长度必须稳定，且字符集限制在 Go 常量定义的字母数字集合内。
    assert_eq!(s.len(), 10);
    for b in s.bytes() {
        assert!(
            b.is_ascii_alphanumeric(),
            "non-alphanumeric byte in random_string: {b}"
        );
    }
    // 两参数分支复现 Go 的偏移逻辑：`min + Intn(max-min)`，因此是 `[1,10)`。
    let v = random_num(&[1, 10]);
    assert!((1..10).contains(&v), "random_num([1,10])={v}");
    // 单参数分支直接代理 `Intn(n)`，只验证半开区间，不绑定具体随机值。
    let v = random_num(&[7]);
    assert!((0..7).contains(&v), "random_num([7])={v}");
    // 无参数时退回任意整数生成器，测试只确认接口可用。
    let _ = random_num(&[]);
}
