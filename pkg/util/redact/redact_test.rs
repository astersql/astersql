// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 脱敏 API 基础单元测试（对齐 Go `redact_test`）。
//
// 覆盖：String/Stringer 三种模式、DeRedact 标记解析与未闭合边界，
// 以及 InitRedact 全局开关对 Value/Key 的影响（经 REDACT_TEST_LOCK 串行化）。

use std::io::Cursor;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

use crate::REDACT_TEST_LOCK;
use crate::{
    DeRedact, DeRedactFile, FmtStringer, InitRedact, Key, String as RedactString, Stringer, Value,
};

#[cfg(unix)]
unsafe extern "C" {
    fn umask(mask: u32) -> u32;
}

/// 测试用 Stringer，返回固定字符串值。
struct TestStringer {
    value: String,
}

impl FmtStringer for TestStringer {
    fn String(&self) -> String {
        self.value.clone()
    }
}

/// 验证 String 与 Stringer 在 OFF/ON/MARKER 下输出一致。
#[test]
fn test_redact() {
    let cases = [
        ("OFF", "fxcv", "fxcv"),
        ("OFF", "f‹xcv", "f‹xcv"),
        ("ON", "f‹xcv", ""),
        ("MARKER", "f‹xcv", "‹f‹‹xcv›"),
        ("MARKER", "f›xcv", "‹f››xcv›"),
    ];

    for (mode, input, output) in cases {
        assert_eq!(RedactString(mode, input), output);
        assert_eq!(
            Stringer(
                mode,
                &TestStringer {
                    value: input.to_owned()
                },
            )
            .String(),
            output
        );
    }
}

/// 验证 DeRedact 对成对/未闭合标记及转义边界的处理。
#[test]
fn test_de_redact() {
    let cases = [
        (true, "‹fxcv›ggg", "?ggg"),
        (false, "‹fxcv›ggg", "fxcvggg"),
        (true, "fxcv", "fxcv"),
        (false, "fxcv", "fxcv"),
        (true, "‹fxcv›ggg‹fxcv›eee", "?ggg?eee"),
        (false, "‹fxcv›ggg‹fxcv›eee", "fxcvgggfxcveee"),
        (true, "‹›", "?"),
        (false, "‹›", ""),
        (true, "gg‹ee", "gg‹ee"),
        (false, "gg‹ee", "gg‹ee"),
        (true, "gg›ee", "gg›ee"),
        (false, "gg›ee", "gg›ee"),
        (true, "gg‹ee‹ee", "gg‹ee‹ee"),
        (false, "gg‹ee‹gg", "gg‹ee‹gg"),
        (true, "gg›ee›gg", "gg›ee›gg"),
        (false, "gg›ee›ee", "gg›ee›ee"),
    ];

    for (remove, input, output) in cases {
        let mut writer = Vec::new();
        DeRedact(remove, Cursor::new(input), &mut writer, "").expect("DeRedact should not fail");
        assert_eq!(String::from_utf8(writer).unwrap(), output);
    }
}

/// 验证 InitRedact 后 Value/Key 在开关开闭时的输出差异。
#[test]
fn test_redact_init_and_value_and_key() {
    let _guard = REDACT_TEST_LOCK.lock().unwrap();
    let redacted = "?";
    let secret = "secret";

    InitRedact(false);
    assert_eq!(Value(secret), secret);
    assert_eq!(Key(secret.as_bytes()), "736563726574");

    InitRedact(true);
    assert_eq!(Value(secret), redacted);
    assert_eq!(Key(secret.as_bytes()), redacted);

    InitRedact(false);
}

/// Go 以 0644 创建输出文件；即使进程 umask 允许更多权限也不能放宽。
#[cfg(unix)]
#[test]
fn test_de_redact_file_uses_go_output_permissions() {
    let _guard = REDACT_TEST_LOCK.lock().unwrap();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("astersql-redact-{}-{unique}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let input = dir.join("input.log");
    let output = dir.join("output.log");
    std::fs::write(&input, "‹secret›").unwrap();

    // SAFETY: 测试锁串行化本 crate 中依赖进程全局状态的测试，并立即恢复原 umask。
    let previous_umask = unsafe { umask(0) };
    let result = DeRedactFile(false, input.to_str().unwrap(), output.to_str().unwrap());
    // SAFETY: 恢复上面保存的进程 umask。
    unsafe { umask(previous_umask) };
    result.unwrap();

    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o644
    );
    std::fs::remove_dir_all(dir).unwrap();
}
