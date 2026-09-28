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

// fix-control 会话变量字符串解析。
//
// 将 `tidb_opt_fix_control` 风格的 `key:value,key:value` 文本解析为
// `HashMap<u64, String>`，并收集重复键警告；语法错误时立即失败（与 Go 一致）。

#![allow(non_snake_case)]

/// 解析 fix-control 文本，返回 (编号→值 map, 警告列表)。
// ParseToMap 对应 Go 的 fix-control 解析器，输入格式为 key:value,key:value。
// 返回 map、重复键警告列表和错误；一旦语法错误，沿用 Go 立即返回 nil 结果的行为。
pub fn ParseToMap(
    mut s: &str,
) -> anyhow::Result<(std::collections::HashMap<u64, String>, Vec<String>)> {
    let mut m = std::collections::HashMap::new();
    let mut warnMsgs = Vec::new();

    while !s.is_empty() {
        // 先找冒号，冒号前是十进制 fix-control 编号。
        let Some(colonIdx) = s.find(':') else {
            return Err(anyhow::anyhow!(
                "invalid fix control: expected colon not found"
            ));
        };
        let keyStr = s[..colonIdx].trim();
        let key = keyStr.parse::<u64>()?;

        s = s[colonIdx + 1..].trim_start();
        let mut value = String::new();
        // 引号值可以包含逗号；先定位匹配的结束引号，再从剩余内容中找下一项。
        if let Some(quote) = s.chars().next().filter(|c| *c == '\'' || *c == '"') {
            let Some(relativeEnd) = s[quote.len_utf8()..].find(quote) else {
                return Err(anyhow::anyhow!(
                    "invalid fix control: expected quote not found"
                ));
            };
            let endIdx = quote.len_utf8() + relativeEnd;
            value = s[quote.len_utf8()..endIdx].to_string();
            s = &s[endIdx + quote.len_utf8()..];
        }

        // 没有逗号时，剩余文本全部属于当前 fix；否则逗号分隔下一项。
        let (endIdx, nextStartIdx) = match s.find(',') {
            Some(commaIdx) => (commaIdx, commaIdx + 1),
            None => (s.len(), s.len()),
        };
        if value.is_empty() {
            value = s[..endIdx].trim().to_string();
        }

        // 重复键且值不同才产生警告；随后仍用新值覆盖旧值，保持 Go map 赋值语义。
        if let Some(originalValue) = m.get(&key) {
            if originalValue != &value {
                warnMsgs.push(format!(
                    "repeated assignment for fix control: {}. existing value: {:?}. new value: {:?}.",
                    key, originalValue, value,
                ));
            }
        }
        m.insert(key, value);
        // 继续处理逗号后的内容；trim_space 语义在下一轮开头继续生效。
        s = &s[nextStartIdx..];
        s = s.trim();
    }
    Ok((m, warnMsgs))
}
