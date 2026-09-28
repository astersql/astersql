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

// terror 迁移期单元测试。
//
// 覆盖错误注册、合成（Synthesize）、未注册类别行为，以及 ErrorEqual 根因比较，
// 确保 Rust 路径与 Go terror 语义一致。Synthesize 可在注册冻结后构造外部错误且不写入码表。

use crate::parser::terror::{
    ClassKV, ClassParser, ErrClass, ErrClassToMySQLCodes, ErrCode, ErrorEqual, ErrorNotEqual,
    GetErrClass, ToSQLError,
};

/// 已注册错误应保留类别、错误码，并能正确转换为 MySQL SQLError。
#[test]
fn registered_errors_keep_class_code_and_mysql_conversion() {
    let class = ErrClass(21_601);
    assert_eq!(class.String(), "21601");

    #[allow(deprecated)]
    let error = ClassKV.New(ErrCode(1062), "duplicate entry");
    let shared = crate::errors::SharedError::new((*error).clone());
    assert!(ClassKV.EqualClass(Some(&shared)));
    assert_eq!(GetErrClass(&error), ClassKV);
    assert!(
        ErrClassToMySQLCodes
            .read()
            .expect("error code registry poisoned")[&ClassKV]
            .contains_key(&ErrCode(1062))
    );

    let sql_error = ToSQLError(&error);
    assert_eq!(sql_error.Code, 1062);
    assert_eq!(sql_error.Message, "duplicate entry");
}

/// Synthesize 构造的错误不写入 ErrClassToMySQLCodes，ToSQLError 回退为 ErrUnknown。
#[test]
fn synthesized_errors_do_not_register_codes() {
    let code = ErrCode(21_602);
    let error = ClassParser.Synthesize(code, "remote parser error");

    assert_eq!(error.RFCCode(), "parser:21602");
    assert!(
        !ErrClassToMySQLCodes
            .read()
            .expect("error code registry poisoned")
            .get(&ClassParser)
            .is_some_and(|codes| codes.contains_key(&code))
    );
    assert_eq!(
        ToSQLError(&error).Code,
        crate::parser::mysql::errcode::ErrUnknown
    );
}

/// 未注册类别 Synthesize 时 RFC 前缀为空，与 Go 直接索引空描述一致。
#[test]
fn unregistered_class_synthesis_matches_go_empty_description() {
    let error = ErrClass(21_603).Synthesize(ErrCode(7), "external error");

    // terror.go indexes errClass2Desc directly here. A missing key therefore
    // contributes the empty string instead of ErrClass.String's numeric fallback.
    assert_eq!(error.RFCCode(), ":7");
    assert_eq!(GetErrClass(&error), ErrClass(-1));
}

/// 未注册类别走 New 仍会登记码表，但 EqualClass/类别反查按未注册语义处理。
#[test]
fn unregistered_class_constructor_matches_go_empty_description() {
    let class = ErrClass(21_604);
    let code = ErrCode(8);
    #[allow(deprecated)]
    let error = class.New(code, "local error");

    assert_eq!(error.RFCCode(), ":8");
    assert_eq!(GetErrClass(&error), ErrClass(-1));
    let shared = crate::errors::SharedError::new((*error).clone());
    assert!(!class.EqualClass(Some(&shared)));
    assert_eq!(
        ToSQLError(&error).Code,
        crate::parser::mysql::errcode::ErrUnknown
    );
    assert!(
        ErrClassToMySQLCodes
            .read()
            .expect("error code registry poisoned")[&class]
            .contains_key(&code)
    );
}

/// ErrorEqual 比较根 cause 或错误文本；双方均为 None 视为相等。
#[test]
fn error_equality_uses_root_cause_or_error_text() {
    let first = crate::errors::New("same error");
    let traced = crate::errors::Trace(Some(first.clone())).expect("trace error");
    let same_text = crate::errors::New("same error");

    assert!(ErrorEqual(Some(&first), Some(&traced)));
    assert!(ErrorEqual(Some(&traced), Some(&same_text)));
    assert!(ErrorEqual(None, None));
    assert!(ErrorNotEqual(Some(&first), None));
}
