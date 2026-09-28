// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `OriginError` 单元测试。
//
// 覆盖 nil、单层叶子错误与多层 `source` 包装，断言返回最深层根因。

use std::error::Error;
use std::fmt;

use crate::errors::OriginError;

#[derive(Debug)]
/// 无 source 的叶子错误，用作根因断言目标。
struct LeafError(&'static str);

impl fmt::Display for LeafError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for LeafError {}

#[derive(Debug)]
/// 带 source 的包装错误，用于构造多层错误链。
struct WrappedError {
    message: &'static str,
    source: Box<dyn Error + 'static>,
}

impl fmt::Display for WrappedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl Error for WrappedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[test]
/// 验证 None / 叶子 / 一层包装 / 两层包装均落到最深叶子。
fn test_origin_error() {
    assert!(OriginError(None).is_none());

    let err1 = LeafError("err1");
    assert!(std::ptr::eq(
        OriginError(Some(&err1 as &(dyn Error + 'static))).unwrap(),
        &err1 as &(dyn Error + 'static)
    ));

    let err2 = WrappedError {
        message: "trace1",
        source: Box::new(LeafError("err1")),
    };
    assert_eq!(
        OriginError(Some(&err2 as &(dyn Error + 'static)))
            .unwrap()
            .to_string(),
        "err1"
    );

    let err3 = WrappedError {
        message: "trace2",
        source: Box::new(WrappedError {
            message: "trace1",
            source: Box::new(LeafError("err1")),
        }),
    };
    assert_eq!(
        OriginError(Some(&err3 as &(dyn Error + 'static)))
            .unwrap()
            .to_string(),
        "err1"
    );
}
