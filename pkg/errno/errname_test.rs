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

// 错误码与消息表完整性测试。
//
// 对应 Go `errname_test.go`：解析同目录 `errcode.go` 源文件中的 `ErrXxx = N` 定义，
// 断言每个错误码均出现在 `MySQLErrName` 中，且不得落入预留区间 `[8800, 8900)`。

use crate::errname::MySQLErrName;

// 对应 Go 的 //go:embed errcode.go；用 include_str! 表达同目录源码作为测试输入。
/// 嵌入的 Go 错误码源文件文本，供解析常量定义。
const ERR_CODE_SRC: &str = include_str!("errcode.go");

/// 每个在 errcode.go 中声明的错误码都必须能在 MySQLErrName 中查到消息。
#[test]
fn test_all_err_code_has_msg() {
    let lines: Vec<&str> = ERR_CODE_SRC.split('\n').collect();
    let mut err_codes: Vec<u16> = Vec::with_capacity(lines.len());

    for line in lines {
        let l = line.trim();
        if !l.starts_with("Err") {
            continue;
        }

        // Go 直接 Split(l, "=")[1]；这里保留“每个 Err 定义必须含等号和值”的断言语义。
        let code_str = l
            .split('=')
            .nth(1)
            .expect("parse code definition should have rhs")
            .trim();
        let code = code_str
            .parse::<i32>()
            .unwrap_or_else(|_| panic!("parse code definition: {}", code_str));
        err_codes.push(code as u16);
    }

    for code in err_codes {
        let ok = MySQLErrName.contains_key(&code);
        assert!(ok, "ErrCode: {} is unknown", code);
    }
}

/// 禁止在下游 fork 预留区间 `[8800, 8900)` 内定义错误码。
#[test]
fn test_reserved_err_code_range() {
    const RESERVED_START: i32 = 8800;
    const RESERVED_END: i32 = 8900;

    for line in ERR_CODE_SRC.split('\n') {
        let l = line.trim();
        if !l.starts_with("Err") {
            continue;
        }

        let parts: Vec<&str> = l.split('=').collect();
        assert_eq!(parts.len(), 2, "parse code definition: {}", l);

        let err_name = parts[0].trim();
        let code_str = parts[1].trim();
        let code = code_str
            .parse::<i32>()
            .unwrap_or_else(|_| panic!("parse code definition: {}", code_str));

        // TiDB 预留 [8800, 8900) 给其他用途；Go 测试禁止 errcode.go 中定义落入该范围。
        assert!(
            !(code >= RESERVED_START && code < RESERVED_END),
            "{} must not be in reserved range [{}, {}), but got {}",
            err_name,
            RESERVED_START,
            RESERVED_END,
            code
        );
    }
}
