// Copyright 2026 AsterSQL.

// SQL 解析器（parser）crate 的库入口。
//
// 将词法分析（Lexer/Scanner）、Yacc 生成的语法分析、AST（抽象语法树）、
// SQL digester（归一化摘要）、Hint 解析以及 MySQL 协议相关常量与错误类型
// 聚合为对外可复用的模块树；具体实现通过 `include!` 与子 crate 再导出拼装。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

extern crate self as parser;

/// 解析器错误类型再导出（terror 共享错误体系）。
pub mod errors {
    pub use parser_terror::errors::*;
    /// 与 Go 侧 `terror.Error` 对应的共享错误别名。
    pub type Error = SharedError;
}

/// MySQL 兼容层：字符集、SQL Mode、错误码与权限类型。
pub mod mysql {
    pub use parser_mysql::charset::{DefaultCharset, DefaultCollationName, IsUTF8Charset};
    pub use parser_mysql::r#const::{
        DefaultDecimal, DefaultSQLMode, ErrTextLength, GetSQLMode, ModeANSIQuotes,
        ModeHighNotPrecedence, ModeIgnoreSpace, ModeNoBackslashEscapes, ModePipesAsConcat,
        ModeStrictTransTables, SQLMode,
    };
    pub use parser_mysql::errcode::*;
    pub use parser_mysql::privs::{AllPriv, GrantPriv, PrivilegeType};
}

/// Terror（分级错误）模块再导出。
pub mod terror {
    pub use parser_terror::parser::terror::*;
}

/// 字符集与排序规则（collation）工具再导出。
pub mod charset {
    pub use parser_charset::*;
}

/// 认证相关身份标识（用户/角色）再导出。
pub mod auth {
    pub use parser_auth::parser::auth::auth::{RoleIdentity, UserIdentity};
}

/// 类型系统相关错误常量再导出。
pub mod types {
    pub use parser_types::types::{
        ErrDataOutOfRange, ErrIllegalValueForType, ErrTruncatedWrongValue,
    };
}

/// TiDB 特性开关相关定义。
#[path = "tidb/features.rs"]
pub mod tidbfeature;
/// 通用工具（如字符串转义）。
#[path = "util/escape.rs"]
pub mod util;

use parser_test_driver;

#[cfg(test)]
#[path = "parsergen/grammar.rs"]
mod parsergen_grammar;

/// 核心解析实现：词法、语法、字面量 token 常量与测试挂载点。
pub mod parser_impl {
    use super::{auth, charset, errors, mysql, terror, tidbfeature, types, util};
    use crate::parser_test_driver;
    use parser_ast as ast;

    /// 本模块使用的错误别名。
    pub type Error = errors::Error;
    /// Hint / 配置等场景使用的简易值类型。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Value {
        Null,
        String(String),
    }
    /// 解析错误别名，便于与 Go `ParserError` 对齐。
    pub type ParserError = errors::Error;
    /// 由消息构造解析错误。
    #[allow(non_snake_case)]
    pub fn ParserError(message: String) -> ParserError {
        errors::New(message)
    }

    pub use lexer_support::{ParseHint, Pos};
    use lexer_support::{
        Scanner, hintParser, isInCorrectIdentifierName, newHintParser, yyhintSymType,
    };

    // 嵌入纯 Rust parsergen 产物与 Parser 主逻辑。
    include!("yy_parser.rs");
    #[cfg(test)]
    mod yy_parser_test {
        include!("yy_parser_test.rs");
    }
    include!("generated/main_tables.rs");
    include!("parser.rs");

    /// 整型字面量 token 的 i32 形式（供外部比较）。
    pub const intLit: i32 = token::intLit as i32;
    /// 十进制字面量 token。
    pub const decLit: i32 = token::decLit as i32;
    /// 浮点字面量 token。
    pub const floatLit: i32 = token::floatLit as i32;
    /// 十六进制字面量 token。
    pub const hexLit: i32 = token::hexLit as i32;
    /// 位串字面量 token。
    pub const bitLit: i32 = token::bitLit as i32;
    /// 非法/无效 token。
    pub const invalid: i32 = token::invalid as i32;

    impl Default for Parser {
        fn default() -> Self {
            *New()
        }
    }

    /// 词法与 Hint 解析支撑：token 表、关键字、lexer、misc 规则树。
    mod lexer_support {
        use super::*;
        include!("generated/lexer_tokens.rs");
        include!("generated/hint_tables.rs");
        include!("hintparser.rs");
        include!("hintparserimpl.rs");
        include!("keywords.rs");
        include!("lexer.rs");
        include!("misc.rs");

        #[cfg(test)]
        mod lexer_5_aster_unit_test {
            use super::*;
            include!("lexer_5_aster_unit_test.rs");
        }

        #[cfg(test)]
        mod misc_2_aster_unit_test {
            use super::*;
            include!("misc_2_aster_unit_test.rs");
        }

        #[cfg(test)]
        mod parser_manifest_aster_unit_test {
            use super::*;
            include!("parser_manifest_aster_unit_test.rs");
        }

        #[cfg(test)]
        mod parsergen_baseline_aster_unit_test {
            use super::*;
            include!("parsergen_baseline_aster_unit_test.rs");
        }
    }

    #[cfg(test)]
    mod parser_3_aster_unit_test {
        use super::*;
        include!("parser_3_aster_unit_test.rs");
    }

    #[cfg(test)]
    mod yy_parser_4_aster_unit_test {
        use super::*;
        include!("yy_parser_4_aster_unit_test.rs");
    }

    #[cfg(test)]
    mod parser_semantic_support_test {
        use super::*;
        include!("parser_semantic_support_test.rs");
    }

    #[cfg(test)]
    mod parser_runtime_aster_unit_test {
        use super::*;
        include!("parser_runtime_aster_unit_test.rs");
    }
}

/// AST（抽象语法树）节点与还原接口再导出。
pub mod ast {
    pub use parser_ast::*;
}

/// SQL digester：将 SQL 归一化为可比较的摘要指纹。
pub mod digester_impl {
    mod digester {
        include!("digester.rs");
    }
    mod keywords {
        include!("keywords.rs");
    }
    pub use digester::*;
    pub use keywords::*;
}
pub use digester_impl::*;

/// 代码生成/辅助生成逻辑。
pub mod generate {
    include!("generate.rs");
}

pub use parser_impl::*;

#[cfg(test)]
#[path = "digester_1_aster_unit_test.rs"]
mod digester_aster_unit_test;

#[cfg(test)]
#[path = "parser_actions/dml_test.rs"]
mod dml_actions_test;

#[cfg(test)]
#[path = "parser_actions/misc_test.rs"]
mod misc_actions_test;

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "consistent_test.rs"]
mod consistent_test;
#[cfg(test)]
#[path = "digester_test.rs"]
mod digester_test;
#[cfg(test)]
#[path = "grant_role_aster_unit_test.rs"]
mod grant_role_aster_unit_test;
#[cfg(test)]
#[path = "hintparser_test.rs"]
mod hintparser_test;
#[cfg(test)]
#[path = "keywords_test.rs"]
mod keywords_test;
#[cfg(test)]
#[path = "lateral_test.rs"]
mod lateral_test;
#[cfg(test)]
#[path = "lexer_test.rs"]
mod lexer_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "parser_contract_cases.rs"]
mod parser_contract_cases;
#[cfg(test)]
#[path = "parser_contract_cases_test.rs"]
mod parser_contract_cases_test;
#[cfg(test)]
#[path = "parser_generated_sources_aster_unit_test.rs"]
mod parser_generated_sources_aster_unit_test;
#[cfg(test)]
#[path = "parser_no_legacy_dependency_aster_unit_test.rs"]
mod parser_no_legacy_dependency_aster_unit_test;
#[cfg(test)]
#[path = "parser_rust_inputs_aster_unit_test.rs"]
mod parser_rust_inputs_aster_unit_test;
#[cfg(test)]
#[path = "parser_test.rs"]
mod parser_test;
#[cfg(test)]
#[path = "reserved_words_test.rs"]
mod reserved_words_test;
