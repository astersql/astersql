// Copyright 2026 AsterSQL.

// Normalize 错误身份（identity）、相等性、Wrap 与 JSON 序列化测试。
//
// 校验原型上的 Code/ID/RFCCode/消息模板、`Equal`/`Is`/`ErrorEqual` 判定、
// Wrap 因果链与 `source`，以及当前 schema 与遗留 class schema 的 serde 往返。

use std::error::Error as StdError;

use astersql_errors::{
    Error, ErrorEqual, ErrorNotEqual, MySQLErrorCode, New, Normalize, RFCCodeText, SharedError,
};

/// 对照 Go：Normalize 身份字段、相等比较、Wrap 因果，以及 JSON 编解码兼容。
#[test]
fn normalize_identity_wrap_and_json_match_go() {
    // DDL 表不可用类错误原型：RFC 码 + MySQL 错误码 8210。
    let prototype = Normalize(
        "table %s is unavailable",
        &[RFCCodeText("ddl:TableUnavailable"), MySQLErrorCode(8210)],
    );

    assert_eq!(prototype.Code(), 8210);
    assert_eq!(prototype.ID(), "ddl:TableUnavailable");
    assert_eq!(prototype.RFCCode(), "ddl:TableUnavailable");
    assert_eq!(prototype.MessageTemplate(), "table %s is unavailable");
    assert!(prototype.Args().is_empty());
    assert_eq!(prototype.GetMsg(), "table %s is unavailable");
    assert_eq!(prototype.GetSelfMsg(), prototype.GetMsg());
    assert_eq!(prototype.Location(), ("", 0));
    assert_eq!(
        prototype.to_string(),
        "[ddl:TableUnavailable]table %s is unavailable"
    );

    // 相等性以 RFC ID 为准，而非消息文本或单独的 MySQL code。
    let same_id = Normalize(
        "a different message",
        &[RFCCodeText("ddl:TableUnavailable")],
    );
    let different_id = Normalize("table %s is unavailable", &[MySQLErrorCode(8210)]);
    let same_shared = SharedError::new(same_id.clone());
    let different_shared = SharedError::new(different_id);
    assert!(prototype.Equal(Some(&same_shared)));
    assert!(!prototype.NotEqual(Some(&same_shared)));
    assert!(!prototype.Equal(Some(&different_shared)));
    assert!(prototype.Is(&same_id));
    assert!(ErrorEqual(
        Some(&same_shared),
        Some(&SharedError::new(same_id))
    ));
    assert!(ErrorNotEqual(Some(&same_shared), Some(&different_shared)));

    let cause = New("disk unavailable");
    let wrapped = prototype.Wrap(Some(cause.clone())).expect("non-nil cause");
    assert_eq!(wrapped.ID(), prototype.ID());
    assert_eq!(
        wrapped.to_string(),
        "[ddl:TableUnavailable]table %s is unavailable: disk unavailable"
    );
    assert!(wrapped.Unwrap().is_some_and(|inner| inner.ptr_eq(&cause)));
    assert!(wrapped.Cause().is_some_and(|inner| inner.ptr_eq(&cause)));
    assert_eq!(
        StdError::source(&wrapped)
            .map(ToString::to_string)
            .as_deref(),
        Some("disk unavailable")
    );
    assert!(prototype.Wrap(None).is_none());

    // 当前 JSON schema：class/code/message/rfccode；反序列化后无 cause。
    let json = serde_json::to_value(&wrapped).expect("serialize normalize error");
    assert_eq!(
        json,
        serde_json::json!({
            "class": 2,
            "code": 8210,
            "message": "table %s is unavailable",
            "rfccode": "ddl:TableUnavailable"
        })
    );
    let decoded: Error = serde_json::from_value(json).expect("deserialize current schema");
    assert_eq!(decoded, prototype);
    assert!(decoded.Unwrap().is_none());

    // 遗留 class schema：空 rfccode 时 ID 由 class 名与 code 合成。
    let legacy: Error = serde_json::from_value(serde_json::json!({
        "class": 5,
        "code": 17,
        "message": "legacy executor error",
        "rfccode": ""
    }))
    .expect("deserialize legacy class schema");
    assert_eq!(legacy.Code(), 17);
    assert_eq!(legacy.ID(), "executor:17");
    assert_eq!(legacy.to_string(), "[executor:17]legacy executor error");
}
