// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Validation and TiDB-side execution guards for the full-text builtins.
//
// 全文检索（FTS）内置函数的校验与 TiDB 侧执行守卫。
// 负责 `FTS_MATCH_WORD` / `MATCH ... AGAINST` 的参数形状检查；
// 真正匹配计算依赖全文索引，索引外求值一律报错（NULL AGAINST 除外）。

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
/// AGAINST 侧表达式形态：常量字符串、NULL、非字符串常量或非常量。
pub enum FtsAgainst {
    String(String),
    Null,
    NonStringConstant,
    NonConstant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// MATCH 列表中的列参数形态。
pub enum MatchArgument {
    StringColumn,
    NonStringColumn,
    NonColumn,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// MySQL 全文检索修饰符：自然语言、布尔、查询扩展等。
pub enum FulltextSearchModifier {
    #[default]
    NaturalLanguage,
    Boolean,
    QueryExpansion,
    NaturalLanguageWithQueryExpansion,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
/// FTS 构建或求值阶段的错误。
pub enum FtsError {
    #[error("incorrect parameter count")]
    IncorrectParameterCount,
    #[error("FTS_MATCH_WORD() is only supported in starter deployment mode")]
    StarterOnly,
    #[error("match against a non-constant string")]
    NonConstantAgainst,
    #[error("match against a non-string constant")]
    NonStringAgainst,
    #[error("not matching a column")]
    NonColumn,
    #[error("Doesn't support match search on a non-string column without fulltext index")]
    NonStringColumn,
    #[error("cannot use 'FTS_MATCH_WORD()' outside of fulltext index")]
    MatchWordOutsideIndex,
    #[error("cannot use 'MATCH ... AGAINST' outside of fulltext index")]
    MatchAgainstOutsideIndex,
    #[error("unexpected builtin signature for FTS_MATCH_AGAINST")]
    UnexpectedSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// `FTS_MATCH_WORD` 签名：仅 starter 部署可用，且须在全文索引内求值。
pub struct FtsMatchWordSig {
    against: String,
    columns: usize,
    fts_function_used: bool,
}

impl FtsMatchWordSig {
    /// 是否标记使用了 FTS 函数（规划器侧标记）。
    pub fn fts_function_used(&self) -> bool {
        self.fts_function_used
    }

    /// AGAINST 常量查询串。
    pub fn against(&self) -> &str {
        &self.against
    }

    /// MATCH 列个数。
    pub fn column_count(&self) -> usize {
        self.columns
    }

    /// 索引外求值：始终报 MatchWordOutsideIndex。
    pub fn eval_real(&self) -> Result<Option<f64>, FtsError> {
        Err(FtsError::MatchWordOutsideIndex)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// MySQL `MATCH ... AGAINST` 签名。
pub struct FtsMysqlMatchAgainstSig {
    against: Option<String>,
    columns: usize,
    modifier: FulltextSearchModifier,
}

impl FtsMysqlMatchAgainstSig {
    /// 设置全文检索修饰符。
    pub fn set_modifier(&mut self, modifier: FulltextSearchModifier) {
        self.modifier = modifier;
    }

    /// 当前修饰符。
    pub fn modifier(&self) -> FulltextSearchModifier {
        self.modifier
    }

    /// MATCH 列个数。
    pub fn column_count(&self) -> usize {
        self.columns
    }

    /// AGAINST 为 NULL 时返回 NULL；否则索引外求值报错。
    pub fn eval_real(&self) -> Result<Option<f64>, FtsError> {
        if self.against.is_none() {
            Ok(None)
        } else {
            Err(FtsError::MatchAgainstOutsideIndex)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// FTS 签名枚举：TiDB 扩展 MATCH_WORD 或 MySQL MATCH AGAINST。
pub enum FtsSignature {
    MatchWord(FtsMatchWordSig),
    Mysql(FtsMysqlMatchAgainstSig),
}

impl FtsSignature {
    /// 仅 MySQL 签名有修饰符。
    pub fn mysql_modifier(&self) -> Option<FulltextSearchModifier> {
        match self {
            Self::Mysql(signature) => Some(signature.modifier()),
            Self::MatchWord(_) => None,
        }
    }
}

/// 校验 MATCH 列：必须是列；可选要求字符串列。
fn validate_columns(columns: &[MatchArgument], require_string: bool) -> Result<(), FtsError> {
    for column in columns {
        match column {
            MatchArgument::NonColumn => return Err(FtsError::NonColumn),
            MatchArgument::NonStringColumn if require_string => {
                return Err(FtsError::NonStringColumn);
            }
            MatchArgument::StringColumn | MatchArgument::NonStringColumn => {}
        }
    }
    Ok(())
}

/// 构建 `FTS_MATCH_WORD`：要求 starter 部署且 AGAINST 为常量字符串。
pub fn build_match_word(
    starter_deployment: bool,
    against: FtsAgainst,
    columns: &[MatchArgument],
) -> Result<FtsMatchWordSig, FtsError> {
    // Go registers FTS_MATCH_WORD with minArgs == maxArgs == 2 and verifyArgs runs first.
    if columns.len() != 1 {
        return Err(FtsError::IncorrectParameterCount);
    }
    if !starter_deployment {
        return Err(FtsError::StarterOnly);
    }
    let against = match against {
        FtsAgainst::String(value) => value,
        FtsAgainst::Null | FtsAgainst::NonStringConstant | FtsAgainst::NonConstant => {
            return Err(FtsError::NonConstantAgainst);
        }
    };
    // Go 的 FTS_MATCH_WORD 只要求参数是列；执行类型由 protobuf 签名固定，此处不拒收。
    // The Go FTS_MATCH_WORD path only requires Column arguments. Their execution
    // type is fixed by the protobuf signature rather than rejected here.
    validate_columns(columns, false)?;
    Ok(FtsMatchWordSig {
        against,
        columns: columns.len(),
        fts_function_used: true,
    })
}

/// 构建 MySQL `MATCH ... AGAINST`：AGAINST 须为字符串常量或 NULL，列须为字符串列。
pub fn build_mysql_match_against(
    against: FtsAgainst,
    columns: &[MatchArgument],
) -> Result<FtsMysqlMatchAgainstSig, FtsError> {
    // Go registers MATCH ... AGAINST with a minimum of two total arguments.
    if columns.is_empty() {
        return Err(FtsError::IncorrectParameterCount);
    }
    let against = match against {
        FtsAgainst::String(value) => Some(value),
        FtsAgainst::Null => None,
        FtsAgainst::NonStringConstant => return Err(FtsError::NonStringAgainst),
        FtsAgainst::NonConstant => return Err(FtsError::NonConstantAgainst),
    };
    validate_columns(columns, true)?;
    Ok(FtsMysqlMatchAgainstSig {
        against,
        columns: columns.len(),
        modifier: FulltextSearchModifier::default(),
    })
}

/// 为 MySQL 签名设置修饰符；对 MATCH_WORD 签名返回 UnexpectedSignature。
pub fn set_mysql_match_against_modifier(
    signature: &mut FtsSignature,
    modifier: FulltextSearchModifier,
) -> Result<(), FtsError> {
    match signature {
        FtsSignature::Mysql(signature) => {
            signature.set_modifier(modifier);
            Ok(())
        }
        FtsSignature::MatchWord(_) => Err(FtsError::UnexpectedSignature),
    }
}
