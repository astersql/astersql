// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use crate::Options;
use regex::Regex;
use std::sync::LazyLock;

pub fn decode_float32_array_bytes(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() % 4 != 0 {
        return Err("invalid embedding data".into());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

pub fn json_fields_with_options(fields: Options, opts: &Options) -> Options {
    let mut merged = opts.clone();
    merged.extend(fields);
    merged
}

pub fn sanitize_error_text(text: &str, secrets: &[&str]) -> String {
    static JSON: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)("(?:authorization|api[_-]?key|token|access[_-]?token|credentials)"\s*:\s*")([^"]*)(")"#).unwrap()
    });
    static BEARER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)Bearer\s+[A-Za-z0-9._~+/=-]+").unwrap());
    static KEY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bsk-[A-Za-z0-9_-]{8,}\b").unwrap());
    let mut value = text.to_owned();
    for secret in secrets {
        if !secret.is_empty() {
            value = value.replace(secret, "[REDACTED]");
        }
    }
    value = JSON.replace_all(&value, "${1}[REDACTED]${3}").into_owned();
    value = BEARER.replace_all(&value, "Bearer [REDACTED]").into_owned();
    value = KEY.replace_all(&value, "[REDACTED]").into_owned();
    if value.len() > 4096 {
        let mut end = 4096;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.push_str("...[truncated]");
    }
    value
}
