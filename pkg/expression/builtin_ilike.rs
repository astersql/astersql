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

// ILIKE 标量内置函数与相关表达式错误类型。
//
// 对应 Go `builtin_ilike.go`：大小写不敏感的 LIKE 匹配。仅当 pattern 与
// escape 均为常量时缓存编译后的通配模式；克隆故意清空运行时缓存，避免跨会话共享。

use std::sync::{Arc, RwLock};

use crate::collate;
use collate_dependency::WildcardPattern;
use stringutil_dependency as stringutil;
use thiserror::Error;

/// Errors shared by the information and ILIKE builtins in this file group.
///
/// 本文件组（信息类与 ILIKE）共用的表达式错误枚举。
#[derive(Debug, Error)]
pub enum ExpressionError {
    #[error("escape should be const")]
    EscapeMustBeConstant,
    #[error("missing session variable when evaluating {0}")]
    MissingSession(&'static str),
    #[error("access denied; {0} privilege is required")]
    AccessDenied(String),
    #[error("table access denied for {user}@{host} on {table}")]
    TableAccessDenied {
        user: String,
        host: String,
        table: String,
    },
    #[error("sequence access denied: {operation} on {sequence} for {user}@{host}")]
    SequenceAccessDenied {
        operation: &'static str,
        sequence: String,
        user: String,
        host: String,
    },
    #[error("incorrect arguments: {0}")]
    InvalidArgument(String),
    #[error("operation cancelled: {0}")]
    Cancelled(String),
    #[error("{0}")]
    External(String),
}

/// 已编译通配模式的缓存项：源 pattern、escape 字节与可复用匹配器。
struct CachedPattern {
    source: String,
    escape: u8,
    pattern: Arc<dyn WildcardPattern>,
}

/// Executable ILIKE signature.
///
/// As in Go, only a pattern and escape known to be constant for the current
/// evaluation context are cached. Cloning deliberately starts with an empty
/// runtime cache so expressions can safely be shared across sessions.
///
/// 可执行的 ILIKE 签名：排序规则、常量标志与 pattern 缓存。
pub struct IlikeSig {
    collation: String,
    pattern_is_constant: bool,
    escape_is_constant: bool,
    pattern_cache: RwLock<Option<CachedPattern>>,
}

impl Clone for IlikeSig {
    /// 克隆时不复制运行时缓存，强制新上下文重新编译。
    fn clone(&self) -> Self {
        Self::new(
            self.collation.clone(),
            self.pattern_is_constant,
            self.escape_is_constant,
        )
    }
}

impl IlikeSig {
    /// 构造签名；`pattern_is_constant`/`escape_is_constant` 决定是否可缓存。
    pub fn new(
        collation: impl Into<String>,
        pattern_is_constant: bool,
        escape_is_constant: bool,
    ) -> Self {
        Self {
            collation: collation.into(),
            pattern_is_constant,
            escape_is_constant,
            pattern_cache: RwLock::new(None),
        }
    }

    /// 运行时 pattern 缓存是否已填充。
    pub fn cache_initialized(&self) -> bool {
        self.pattern_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    /// Scalar ILIKE evaluation with Go-compatible NULL propagation and ASCII
    /// case folding. Non-ASCII code points retain their original case.
    ///
    /// 标量求值：任一参数为 NULL 则返回 NULL；仅 ASCII 做大小写折叠。
    pub fn eval_int(
        &self,
        value: Option<&str>,
        pattern: Option<&str>,
        escape: Option<i64>,
    ) -> Result<Option<i64>, ExpressionError> {
        // NULL 传播：三参任一缺失即返回 SQL NULL。
        let (Some(value), Some(pattern), Some(escape)) = (value, pattern, escape) else {
            return Ok(None);
        };
        Ok(Some(i64::from(self.matches(
            value,
            pattern,
            escape,
            self.pattern_is_constant && self.escape_is_constant,
        ))))
    }

    /// 规范化后编译/取缓存并执行通配匹配；`cacheable` 控制是否写入缓存。
    pub(crate) fn matches(&self, value: &str, pattern: &str, escape: i64, cacheable: bool) -> bool {
        let (value, pattern, escape) = normalize_ilike(value, pattern, escape);
        let compiled = if cacheable {
            self.cached_pattern(&pattern, escape)
        } else {
            compile_pattern(&self.collation, &pattern, escape)
        };
        compiled.DoMatch(&value)
    }

    /// 命中相同 source/escape 的缓存则复用，否则编译并写回。
    fn cached_pattern(&self, source: &str, escape: u8) -> Arc<dyn WildcardPattern> {
        {
            let cache = self
                .pattern_cache
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(cached) = cache.as_ref()
                && cached.source == source
                && cached.escape == escape
            {
                return Arc::clone(&cached.pattern);
            }
        }

        let compiled = compile_pattern(&self.collation, source, escape);
        let mut cache = self
            .pattern_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cache = Some(CachedPattern {
            source: source.to_owned(),
            escape,
            pattern: Arc::clone(&compiled),
        });
        compiled
    }
}

/// 按排序规则取二进制排序器并编译通配 pattern。
fn compile_pattern(collation: &str, source: &str, escape: u8) -> Arc<dyn WildcardPattern> {
    let mut pattern = collate::ConvertAndGetBinCollator(collation).Pattern();
    pattern.Compile(source, escape);
    Arc::from(pattern)
}

/// 对 value/pattern 做 ASCII 小写折叠；若 escape 本身是字母则保护该字节不被折叠。
pub(crate) fn normalize_ilike(value: &str, pattern: &str, escape: i64) -> (String, String, u8) {
    let mut value = value.as_bytes().to_vec();
    let mut pattern = pattern.as_bytes().to_vec();
    let mut actual_escape = escape as u8;

    stringutil::string_util::LowerOneString(&mut value);
    // escape 为 ASCII 字母时，折叠 pattern 时排除该 escape 字节（Go 规则）。
    if stringutil::string_util::IsUpperASCII(actual_escape)
        || stringutil::string_util::IsLowerASCII(actual_escape)
    {
        actual_escape =
            stringutil::string_util::LowerOneStringExcludeEscapeChar(&mut pattern, actual_escape);
    } else {
        stringutil::string_util::LowerOneString(&mut pattern);
    }

    // Inputs originated as UTF-8 strings. ASCII-only mutation cannot invalidate
    // them, so these conversions are infallible.
    // 输入本为 UTF-8；仅改 ASCII 字节不会破坏编码，故 from_utf8 必然成功。
    (
        String::from_utf8(value).expect("ASCII folding preserves UTF-8"),
        String::from_utf8(pattern).expect("ASCII folding preserves UTF-8"),
        actual_escape,
    )
}
