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

// 验证键断言写入的优先级与错误处理语义。
//
// 通过可观测写入次数的内存缓冲，覆盖已有断言不可覆盖、空断言可替换，
// 以及查询失败时不得产生写入等关键约束。

use crate::assertion::{AssertionBuffer, AssertionOp, KeyFlags, set_assertion};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 模拟缓冲查询时可区分的“键不存在”与存储故障。
enum BufferError {
    NotFound,
    Storage,
}

#[derive(Default)]
/// 最小化的断言缓冲测试替身，并记录实际发生的更新次数。
struct MockBuffer {
    flags: HashMap<Vec<u8>, KeyFlags>,
    failure: Option<BufferError>,
    updates: usize,
}

impl AssertionBuffer for MockBuffer {
    type Error = BufferError;

    fn get_flags(&self, key: &[u8]) -> Result<KeyFlags, Self::Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.flags.get(key).copied().ok_or(BufferError::NotFound)
    }

    fn is_not_found(error: &Self::Error) -> bool {
        *error == BufferError::NotFound
    }

    fn update_assertion_flags(&mut self, key: &[u8], assertion: AssertionOp) {
        self.flags.insert(
            key.to_vec(),
            KeyFlags {
                assertion: (assertion != AssertionOp::None).then_some(assertion),
            },
        );
        self.updates += 1;
    }
}

#[test]
// 首次写入的有效断言具有优先级，后续任何断言（包括 None）都不能覆盖它。
fn first_non_none_assertion_is_immutable() {
    for initial in [
        AssertionOp::Exist,
        AssertionOp::NotExist,
        AssertionOp::Unknown,
    ] {
        let mut buffer = MockBuffer::default();
        set_assertion(&mut buffer, b"k", initial).unwrap();
        for replacement in [
            AssertionOp::Exist,
            AssertionOp::NotExist,
            AssertionOp::Unknown,
            AssertionOp::None,
        ] {
            set_assertion(&mut buffer, b"k", replacement).unwrap();
            assert_eq!(buffer.flags[&b"k"[..]].assertion, Some(initial));
        }
        assert_eq!(buffer.updates, 1);
    }
}

#[test]
// None 只表示尚未建立有效断言，因此后续有效断言仍需写入。
fn none_assertion_can_be_replaced_later() {
    let mut buffer = MockBuffer::default();

    set_assertion(&mut buffer, b"k", AssertionOp::None).unwrap();
    assert_eq!(buffer.flags[&b"k"[..]].assertion, None);
    set_assertion(&mut buffer, b"k", AssertionOp::Exist).unwrap();

    assert_eq!(buffer.flags[&b"k"[..]].assertion, Some(AssertionOp::Exist));
    assert_eq!(buffer.updates, 2);
}

#[test]
// 键已有标志但没有断言时，应复用该键并初始化断言。
fn existing_key_without_assertion_is_initialized() {
    let mut buffer = MockBuffer::default();
    buffer
        .flags
        .insert(b"k".to_vec(), KeyFlags { assertion: None });

    set_assertion(&mut buffer, b"k", AssertionOp::NotExist).unwrap();

    assert_eq!(
        buffer.flags[&b"k"[..]].assertion,
        Some(AssertionOp::NotExist)
    );
}

#[test]
// 只有“键不存在”可进入初始化路径；其它查询错误必须原样返回且禁止写入。
fn non_not_found_lookup_error_is_propagated_without_update() {
    let mut buffer = MockBuffer {
        failure: Some(BufferError::Storage),
        ..Default::default()
    };

    assert_eq!(
        set_assertion(&mut buffer, b"k", AssertionOp::Exist),
        Err(BufferError::Storage)
    );
    assert_eq!(buffer.updates, 0);
}
