// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// `CIStr` / `NewCIStr` 的基础行为测试。
//
// 对齐 Go：构造后 `O`/`L`/`Display` 关系，以及 JSON 往返（字符串输入与对象输出）。

use crate::model::{CIStr, NewCIStr};

/// 构造后原始串进 O、小写进 L，Display 输出原始写法。
#[test]
fn test_t() {
    let abc = NewCIStr("aBC");
    assert_eq!(
        (&abc.O, &abc.L, abc.to_string()),
        (&"aBC".into(), &"abc".into(), "aBC".into())
    );
}

/// JSON：字符串反序列化自动生成 L；再序列化为 `{"O","L"}` 对象并可 round-trip。
#[test]
fn test_unmarshal_cistr() {
    let ci: CIStr = serde_json::from_str(r#""aaBB""#).unwrap();
    assert_eq!((&ci.O, &ci.L), (&"aaBB".into(), &"aabb".into()));
    let encoded = serde_json::to_string(&ci).unwrap();
    assert_eq!(encoded, r#"{"O":"aaBB","L":"aabb"}"#);
    assert_eq!(serde_json::from_str::<CIStr>(&encoded).unwrap(), ci);
}
