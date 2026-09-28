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

// `FilterCore` 与字段编码相关单元测试。
//
// 覆盖无过滤直写、白名单命中/未命中、`with` 附加字段，以及 Contains 匹配边界；
// 并保留与 Go benchmark 形状对应的辅助函数。

use std::sync::Arc;

use astersql_lightning_log::filter::{Entry, Field, FilterCore, Level};
use astersql_lightning_log::log::Logger;
use astersql_lightning_log::testlogger::MakeTestLogger;

/// 验证测试 Logger 编码、FilterCore 过滤与 with 字段合并行为。
#[test]
fn test_filter() {
    // 无过滤器：直接写出 JSON，含级别、消息与字段。
    let (logger, buffer) = MakeTestLogger([]);
    logger.Warn(
        "the message",
        [
            Field::int("number", 123456),
            Field::ints("array", [7, 8, 9]),
        ],
    );
    assert_eq!(
        buffer.stripped(),
        r#"{"$lvl":"WARN","$msg":"the message","number":123456,"array":[7,8,9]}"#
    );

    // 白名单与默认调用方不匹配：输出应为空。
    let (base, buffer) = MakeTestLogger([]);
    let logger = Logger::Wrap(Arc::new(FilterCore::new(
        base.core(),
        ["github.com/pingcap/br/"],
    )));
    logger.Warn("the message", [Field::int("number", 123456)]);
    assert!(buffer.stripped().is_empty());

    // 白名单匹配测试调用路径，且 with 字段应出现在输出中。
    let (base, buffer) = MakeTestLogger([]);
    let core = FilterCore::new(base.core(), ["/lightning/"]).with([Field::string("a", "b")]);
    let logger = Logger::Wrap(Arc::new(core));
    logger.Warn(
        "the message",
        [
            Field::int("number", 123456),
            Field::ints("array", [7, 8, 9]),
        ],
    );
    assert_eq!(
        buffer.stripped(),
        r#"{"$lvl":"WARN","$msg":"the message","a":"b","number":123456,"array":[7,8,9]}"#
    );

    // 再次用不匹配白名单：仍应被丢弃（含字段中的 stack 字符串也不算 caller）。
    let (base, buffer) = MakeTestLogger([]);
    let core =
        FilterCore::new(base.core(), ["github.com/pingcap/br/"]).with([Field::string("a", "b")]);
    let logger = Logger::Wrap(Arc::new(core));
    logger.Warn(
        "the message",
        [
            Field::int("number", 123456),
            Field::ints("array", [7, 8, 9]),
        ],
    );
    assert!(buffer.stripped().is_empty());

    logger.Warn(
        "the message",
        [Field::string("stack", "github.com/pingcap/tidb/br/")],
    );
    assert!(buffer.stripped().is_empty());

    // 显式设置 caller：子串 `/ingestor/ingestctrl` 命中，带尾斜杠则不命中。
    let entry = Entry::new(Level::Warn, "retryable write").with_caller(
        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl.(*regionJobBaseWorker).runJob",
    );
    let (logger, buffer) = MakeTestLogger([]);
    FilterCore::new(logger.core(), ["/ingestor/ingestctrl"])
        .write(entry.clone(), [])
        .unwrap();
    assert!(buffer.stripped().contains(r#""retryable write""#));

    let (logger, buffer) = MakeTestLogger([]);
    FilterCore::new(logger.core(), ["/ingestor/ingestctrl/"])
        .write(entry, [])
        .unwrap();
    assert!(buffer.stripped().is_empty());
}

// Rust's stable test harness has no benchmark API. These helpers retain the two
// Go benchmark loop shapes so they can be wired to a benchmark harness later.
/// 模拟 Go `strings.Contains` 过滤的循环形态，供日后接入 benchmark。
#[allow(dead_code)]
fn benchmark_filter_strings_contains(iterations: usize) -> usize {
    let inputs = [
        "github.com/pingcap/tidb/some/package/path",
        "github.com/tikv/pd/some/package/path",
        "github.com/pingcap/tidb/br/some/package/path",
    ];
    let filters = ["github.com/pingcap/tidb/", "github.com/tikv/pd/"];
    let mut matches = 0;
    for _ in 0..iterations {
        for input in inputs {
            for filter in filters {
                matches += usize::from(input.contains(filter));
            }
        }
    }
    matches
}

/// 模拟 Go 正则匹配循环形态（此处用 Contains 等价实现，避免引入 regex crate）。
#[allow(dead_code)]
fn benchmark_filter_regex_match_string(iterations: usize) -> usize {
    let inputs = [
        "github.com/pingcap/tidb/some/package/path",
        "github.com/tikv/pd/some/package/path",
        "github.com/pingcap/tidb/br/some/package/path",
    ];
    // Retain Go's `regexp.MustCompile("github.com/(pingcap/tidb|tikv/pd)/")` match
    // shape without pulling an extra regex crate into this ported unit.
    let matches_filter = |input: &str| -> bool {
        input.contains("github.com/pingcap/tidb/") || input.contains("github.com/tikv/pd/")
    };
    let mut matches = 0;
    for _ in 0..iterations {
        for input in inputs {
            matches += usize::from(matches_filter(input));
        }
    }
    matches
}
