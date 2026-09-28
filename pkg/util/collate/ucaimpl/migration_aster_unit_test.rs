// Copyright 2023 PingCAP, Inc.
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

// `ucaimpl` 生成器与 Go 产物一致性的迁移期单元测试。
//
// 将临时文件上的 `generate_file` 输出与仓库内 Go 参考 `*_generated.go`
// 做全文比对，并确认重复生成会截断旧内容。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Data, generate_file};

/// 构造带纳秒后缀的临时输出路径，避免并行测试互相覆盖。
fn temporary_output(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("ucaimpl-migration-{nonce}-{name}"))
}

/// 生成到临时文件后读回并与期望全文相等，随后删除临时文件。
fn assert_generated_file(name: &str, impl_name: &str, expected: &str) {
    let output = temporary_output(name);
    generate_file(
        &output,
        &Data {
            name: name.to_owned(),
            impl_name: impl_name.to_owned(),
        },
    )
    .expect("generate collator source");

    let generated = fs::read_to_string(&output).expect("read generated collator source");
    fs::remove_file(output).expect("remove generated collator source");
    assert_eq!(generated, expected);
}

#[test]
/// 4.0.0 生成结果应与 Go `unicode_0400_ci_generated.go` 一致。
fn generates_unicode_0400_source_identical_to_go_generator() {
    assert_generated_file(
        "unicodeCICollator",
        "unicode0400Impl",
        include_str!("../unicode_0400_ci_generated.go"),
    );
}

#[test]
/// 9.0.0 生成结果应与 Go `unicode_0900_ai_ci_generated.go` 一致。
fn generates_unicode_0900_source_identical_to_go_generator() {
    assert_generated_file(
        "unicode0900AICICollator",
        "unicode0900Impl",
        include_str!("../unicode_0900_ai_ci_generated.go"),
    );
}

#[test]
/// 先写入陈旧内容再生成，确认输出被截断为完整新文件而非追加。
fn generating_again_truncates_the_existing_file_like_os_create() {
    let output = temporary_output("truncate");
    // 预置尾部脏数据，验证写回后不会残留。
    fs::write(&output, "stale trailing contents").expect("seed output file");

    generate_file(
        &output,
        &Data {
            name: "unicodeCICollator".to_owned(),
            impl_name: "unicode0400Impl".to_owned(),
        },
    )
    .expect("regenerate collator source");

    let generated = fs::read_to_string(&output).expect("read regenerated source");
    fs::remove_file(output).expect("remove regenerated source");
    assert_eq!(generated, include_str!("../unicode_0400_ci_generated.go"));
}
