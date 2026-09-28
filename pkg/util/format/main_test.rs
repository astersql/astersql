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

// format 包测试入口替代。
//
// Go 的 `TestMain` 安装进程级测试夹具与 goroutine 泄漏检查；本模块无依赖且
// Rust 测试框架无对等全局 setup，故改为验证 Formatter 内嵌 `Write` 转发行为。

use std::io::Write;
use std::sync::{Arc, Mutex};

use super::format::IndentFormatter;

// Go's TestMain only installs process-wide Go test setup and goroutine leak
// checks. Rust's test harness has no matching global setup requirement for this
// dependency-free module, so this file exercises the Formatter's embedded
// io::Write behavior instead.
/// 直接 `write` 应转发到底层 Writer，不经过 Format 状态机。
#[test]
fn test_formatter_forwards_direct_writes() {
    /// 测试用内存 Writer。
    #[derive(Clone)]
    struct Writer(Arc<Mutex<Vec<u8>>>);

    impl Write for Writer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let output = Arc::new(Mutex::new(Vec::new()));
    let mut formatter = IndentFormatter(Writer(output.clone()), "\t");
    assert_eq!(formatter.write(b"raw bytes").unwrap(), 9);
    assert_eq!(*output.lock().unwrap(), b"raw bytes");
}
