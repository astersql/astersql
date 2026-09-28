// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `TypeStr` / `StrToType` 双向转换对齐测试。
//
// 对照 Go `etc_test.go`：遍历完整 `u8` 类型码域，验证规范名称与类型码可往返；
// 并覆盖 BLOB/BINARY 别名在解析时归一化为 text/char 的行为。

use mysql::r#type::{TypeBlob, TypeString};
use parser_types::types::{StrToType, TypeStr};
use std::process::Command;

// Go iterates the package-private type2Str map. Scanning the complete byte domain
// exercises the same production entries without exposing test-only production APIs.
// Go 遍历包内私有 type2Str；扫描完整字节域可覆盖同一批生产条目，无需向测试暴露内部表。
/// 验证 TypeStr 与 StrToType 对已注册类型码可往返，并检查 blob/binary 别名。
#[test]
fn test_str_to_type() {
    // 对每个有规范名的类型码，确认反向解析得到同一码值。
    for tp in u8::MIN..=u8::MAX {
        let type_name = TypeStr(tp);
        if !type_name.is_empty() {
            assert_eq!(tp, StrToType(type_name), "type name {type_name}");
        }
    }

    // blob/binary 是显示侧别名，解析时应映射回 Blob/String 类型码。
    assert_eq!(TypeBlob, StrToType("blob"));
    assert_eq!(TypeString, StrToType("binary"));
}

/// Go 的包级错误变量会在 `RegisterFinish` 前完成注册；用独立进程验证同一时序。
#[test]
fn standard_errors_are_registered_during_package_initialization() {
    const HELPER_ENV: &str = "PARSER_TYPES_PACKAGE_INIT_HELPER";

    if std::env::var_os(HELPER_ENV).is_some() {
        parser_types::terror::RegisterFinish();
        assert_eq!(
            parser_types::types::ErrInvalidDefault.Code(),
            mysql::errcode::ErrInvalidDefault as i32
        );
        assert_eq!(
            parser_types::types::ErrDataOutOfRange.Code(),
            mysql::errcode::ErrDataOutOfRange as i32
        );
        assert_eq!(
            parser_types::types::ErrTruncatedWrongValue.Code(),
            mysql::errcode::ErrTruncatedWrongValue as i32
        );
        assert_eq!(
            parser_types::types::ErrIllegalValueForType.Code(),
            mysql::errcode::ErrIllegalValueForType as i32
        );
        return;
    }

    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("etc_test::standard_errors_are_registered_during_package_initialization")
        .env(HELPER_ENV, "1")
        .status()
        .expect("run parser/types package-initialization helper");
    assert!(
        status.success(),
        "parser/types package-initialization helper failed with {status}"
    );
}
