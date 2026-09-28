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

// 测试断言与固定夹具：Datum / Handle 相等、无序字符串多重集比较、慢日志连接属性等。

use std::collections::HashMap;

use rand::Rng;

// DatumEqual verifies that the actual value is equal to the expected value.
// String datums are compared with the binary collation.
/// 断言两个 Datum 在二进制排序规则（binary collation）下相等。
pub fn DatumEqual(expected: types::Datum, actual: types::Datum) {
    let collator = collate::GetBinaryCollator();
    let result = actual
        .Compare(
            (*types::DefaultStmtNoWarningContext).clone(),
            &expected,
            collator.as_ref(),
        )
        .expect("compare datums");
    assert_eq!(0, result, "expected and actual datums differ");
}

// HandleEqual verifies that the actual handle is equal to the expected handle.
/// 断言两个 Handle 的种类（是否整型）与字符串表示一致。
pub fn HandleEqual(expected: &dyn kv::Handle, actual: &dyn kv::Handle) {
    assert_eq!(
        expected.IsInt(),
        actual.IsInt(),
        "expected and actual handle kinds differ"
    );
    assert_eq!(
        expected.String(),
        actual.String(),
        "expected and actual handles differ"
    );
}

// CompareUnorderedStringSlice compares two string slices as multisets.
// None represents a nil Go slice and remains distinct from an empty slice.
/// 将两个字符串切片视为多重集比较；`None` 对应 Go 的 nil slice，与空切片不同。
pub fn CompareUnorderedStringSlice(a: Option<&[String]>, b: Option<&[String]>) -> bool {
    match (a, b) {
        (None, None) => return true,
        (None, Some(_)) | (Some(_), None) => return false,
        (Some(a), Some(b)) if a.len() != b.len() => return false,
        _ => {}
    }

    // 用计数表做多重集差分：a 中累加，b 中递减，最终应清空。
    let (a, b) = (a.expect("checked above"), b.expect("checked above"));
    let mut counts = HashMap::with_capacity(a.len());
    for item in a {
        *counts.entry(item.as_str()).or_insert(0_usize) += 1;
    }

    for item in b {
        let remove = match counts.get_mut(item.as_str()) {
            None => return false,
            Some(count) => {
                *count -= 1;
                *count == 0
            }
        };
        if remove {
            counts.remove(item.as_str());
        }
    }
    counts.is_empty()
}

/// 生成随机 ASCII 字母时使用的字符表。
static letterRunes: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L',
    'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

/// 慢日志测试共用的会话连接属性 JSON 固定夹具。
const defaultSessionConnectAttrsJSON: &str =
    r#"{"_client_name":"Go-MySQL-Driver","_os":"linux","app_name":"test_app"}"#;

// DefaultSessionConnectAttrsJSON returns the shared fixture JSON used in slow-log tests.
/// 返回慢日志测试共用的连接属性 JSON 夹具。
pub fn DefaultSessionConnectAttrsJSON() -> String {
    defaultSessionConnectAttrsJSON.to_owned()
}

// DefaultSessionConnectAttrsSlowLogLine returns the shared slow-log line for the fixture.
/// 返回夹具对应的慢日志 `# Session_connect_attrs:` 行。
pub fn DefaultSessionConnectAttrsSlowLogLine() -> String {
    format!("# Session_connect_attrs: {defaultSessionConnectAttrsJSON}")
}

// RequireContainsDefaultSessionConnectAttrs verifies that every fixture key/value is present.
/// 断言文本中包含夹具里的全部键值片段。
pub fn RequireContainsDefaultSessionConnectAttrs(attrsText: &str) {
    for required in [
        r#""_client_name""#,
        r#""Go-MySQL-Driver""#,
        r#""_os""#,
        r#""linux""#,
        r#""app_name""#,
        r#""test_app""#,
    ] {
        assert!(
            attrsText.contains(required),
            "connection attributes are missing {required}: {attrsText}"
        );
    }
}

// RandStringRunes generates a random ASCII-letter string of length n.
/// 生成长度为 `n` 的随机 ASCII 字母串。
pub fn RandStringRunes(n: isize) -> String {
    assert!(n >= 0, "negative random string length");
    let mut rng = rand::rng();
    (0..n)
        .map(|_| letterRunes[rng.random_range(0..letterRunes.len())])
        .collect()
}
