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

// driver_error 核心转换用例：ResultUndetermined 与内存缓冲超限错误消息。
//
// 对应 Go `TestConvertError` / `TestMemBufferOversizeError`：验证经 Trace/WithStack/Wrap
// 包装后仍能映射到 `terror.ErrResultUndetermined`，以及 Txn/Entry/KeyTooLarge 保留 Go 侧文案。

use crate::{TiKvError, ToTiDBErr, errors, terror};

// Go's TestMain installs common Go test state and checks goroutine leaks. This
// native Rust test binary neither starts that Go state nor owns Go goroutines,
// so Cargo's standard test harness is the corresponding boundary here.

// TestConvertError: every derived form converts to ErrResultUndetermined.
/// 验证 ResultUndetermined 经多种包装后均等于 ErrResultUndetermined。
#[test]
fn test_convert_error() {
    // 四种包装形态：恒等、Trace、WithStack、Wrap（对齐 Go 错误链）。
    let wrap_funcs: [fn(errors::SharedError) -> errors::SharedError; 4] = [
        |error| error,
        |error| errors::Trace(Some(error)).expect("Trace preserves a non-nil error"),
        |error| errors::WithStack(Some(error)).expect("WithStack preserves a non-nil error"),
        |error| errors::Wrap(Some(error), "dummy").expect("Wrap preserves a non-nil error"),
    ];

    // All derived versions converts to `terror.ErrResultUndetermined`.
    for wrap in wrap_funcs {
        let source = wrap(errors::SharedError::new(TiKvError::ResultUndetermined));
        let tidb_error = ToTiDBErr(Some(source)).expect("non-nil errors stay non-nil");
        assert!(
            terror::ErrResultUndetermined.Equal(Some(&tidb_error)),
            "expected {}, got {tidb_error}",
            *terror::ErrResultUndetermined
        );
    }
}

// TestMemBufferOversizeError: converted errors retain the Go client messages.
/// 验证事务/条目/键过大错误转换后仍包含 Go 客户端原始消息片段。
#[test]
fn test_mem_buffer_oversize_error() {
    let cases = [
        (
            TiKvError::TxnTooLarge { size: 100 },
            "Transaction is too large, size: 100",
        ),
        (
            TiKvError::EntryTooLarge {
                limit: 10,
                size: 20,
            },
            "entry too large, the max entry size is 10, the size of data is 20",
        ),
        (
            TiKvError::KeyTooLarge {
                key_size: u16::MAX as u64 + 1,
            },
            "key is too large, the size of given key is 65536",
        ),
    ];

    for (source, expected) in cases {
        let tidb_error =
            ToTiDBErr(Some(errors::SharedError::new(source))).expect("non-nil errors stay non-nil");
        assert!(
            tidb_error.to_string().contains(expected),
            "{tidb_error:?} does not contain {expected:?}"
        );
    }
}
