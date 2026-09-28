// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 名称合法性校验：服务范围（tidb_service_scope）与 Keyspace 名称规则。
//
// Keyspace（键空间）把物理集群切分为逻辑租户；名称写入 KV，长度难扩展，
// 故限制为最多 20 字符。校验仅允许字母、数字、连字符与下划线。

use regex::Regex;

// mostly we use an uint64 as the keyspace name, the max value is 20 characters.
// And there are at most 16,777,215 keyspace ID in a single physical cluster,
// as keyspace ID is encoded into KV, it's very hard to extend its length, so
// current keyspace name limit is more than enough for current usage and later
// extension.
// maxKeyspaceNameLength 对应 Go 常量，限制 keyspace 名称的最大长度。
/// Keyspace 名称最大长度（字符数），对齐 Go 常量。
const maxKeyspaceNameLength: isize = 20;

// Check if the name is valid.
// Valid name must be 64 characters or fewer and consist only of letters (a-z, A-Z),
// numbers (0-9), hyphens (-), and underscores (_).
// currently, we enforce this rule to tidb_service_scope and keyspace_name
// Check 对应 Go 的 Check，使用默认最大长度 64 校验服务范围等名称。
/// 使用默认最大长度 64 校验名称（如 tidb_service_scope）。
pub fn Check(name: &str) -> Result<(), String> {
    CheckWithMaxLen(name, 64)
}

// CheckKeyspaceName checks if the keyspace name is valid.
// CheckKeyspaceName 对应 Go 的 keyspace 名称校验入口，最大长度使用 maxKeyspaceNameLength。
/// 按 Keyspace 专用上限校验名称。
pub fn CheckKeyspaceName(name: &str) -> Result<(), String> {
    CheckWithMaxLen(name, maxKeyspaceNameLength)
}

// CheckWithMaxLen checks if the name is valid with the specified maximum length.
// CheckWithMaxLen 对应 Go 的通用校验函数，动态生成正则并匹配整个字符串。
/// 按指定最大长度构造正则并整串匹配；失败返回与 Go 一致的错误文案。
pub fn CheckWithMaxLen(name: &str, maxLen: isize) -> Result<(), String> {
    let namePattern = format!(r"^[a-zA-Z0-9_-]{{0,{}}}$", maxLen);
    // Go 的 regexp.MustCompile 在模式非法时 panic；这里用 expect 保留“模式应由代码保证有效”的语义。
    let nameRe = Regex::new(&namePattern).expect("invalid naming regular expression");
    if !nameRe.is_match(name) {
        // 错误文本保持 Go fmt.Errorf 的用户可见内容，方便后续人工核对行为。
        return Err(format!(
            "the value '{}' is invalid. It must be {} characters or fewer and consist only of letters (a-z, A-Z), numbers (0-9), hyphens (-), and underscores (_)",
            name, maxLen
        ));
    }
    Ok(())
}
