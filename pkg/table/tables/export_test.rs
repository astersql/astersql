// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! 验证 `tables` 的断言写入入口可供外部风格测试使用。
//!
//! 本模块以最小缓冲区替身隔离真实事务，只锁定 `set_assertion` 的公开可见性与写回行为，
//! 对齐 Go `export_test.go` 为包外测试暴露内部入口的用途。

use crate::assertion::{AssertionBuffer, AssertionOp, KeyFlags, set_assertion};

/// 仅保存一个断言的测试缓冲区，用于满足 `AssertionBuffer` 所需的最小接口。
struct ExportBuffer(Option<AssertionOp>);

impl AssertionBuffer for ExportBuffer {
    type Error = ();

    fn get_flags(&self, _key: &[u8]) -> Result<KeyFlags, Self::Error> {
        Ok(KeyFlags { assertion: self.0 })
    }

    fn is_not_found(_error: &Self::Error) -> bool {
        false
    }

    fn update_assertion_flags(&mut self, _key: &[u8], assertion: AssertionOp) {
        self.0 = Some(assertion);
    }
}

#[test]
fn set_assertion_is_available_to_external_style_tests() {
    // 从空断言开始，确认公开入口会经过 trait 接口把新断言写回缓冲区。
    let mut buffer = ExportBuffer(None);
    set_assertion(&mut buffer, b"k", AssertionOp::Unknown).unwrap();
    assert_eq!(buffer.0, Some(AssertionOp::Unknown));
}
