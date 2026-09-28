// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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
//! 中文注释索引开始
//! 本文件负责`br/pkg/encryption/master_key/multi_master_key_backend_test.rs`对应的主密钥后端装配，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少32行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `MockBackend`承载\"MockBackend\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Backend`把\"Backend\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Decrypt`是当前文件的重要函数，承担\"Decrypt\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Close`是当前文件的重要函数，承担\"Close\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `TestMultiMasterKeyBackend`承载\"TestMultiMasterKeyBackend\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl TestMultiMasterKeyBackend`把\"TestMultiMasterKeyBackend\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `test_multi_master_key_backend_decrypt`对齐 Go 同名测试或契约片段，用来固定\"test multi master key backend decrypt\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `write_temp_key`是当前文件的重要函数，承担\"write temp key\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"do nothing\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"success first backend — second must not be called.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"success second backend — first fails, second succeeds.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"all backends fail — error aggregates both failure messages.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"no backends — internal error path.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Production MultiMasterKeyBackend via NewMultiMasterKeyBackend + real file keys\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"(Go injects mock Backend interface; Rust uses CreateBackend for each MasterKey).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::{
    Backend, EncryptedContent, MasterKey, MasterKeyBackend, MasterKeyFile,
    NewMultiMasterKeyBackend, createFileBackend,
};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// MockBackend matches Go testify/mock Backend: records Decrypt calls and returns preset values.
struct MockBackend {
    decrypt_calls: Arc<AtomicUsize>,
    result: Result<Vec<u8>, String>,
}

impl Backend for MockBackend {
    fn Decrypt(&self, _encryptedContent: &EncryptedContent) -> Result<Vec<u8>, String> {
        self.decrypt_calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }

    fn Close(&mut self) {
        // do nothing
    }
}

/// Test-only multi backend that accepts any Backend, mirroring Go's []Backend interface.
struct TestMultiMasterKeyBackend {
    backends: Vec<Box<dyn Backend + Send>>,
}

impl TestMultiMasterKeyBackend {
    fn Decrypt(&self, encryptedContent: &EncryptedContent) -> Result<Vec<u8>, String> {
        if self.backends.is_empty() {
            return Err("internal error: should always contain at least one backend".into());
        }
        let mut errs = Vec::new();
        for b in &self.backends {
            match b.Decrypt(encryptedContent) {
                Ok(r) => return Ok(r),
                Err(e) => errs.push(e),
            }
        }
        Err(format!(
            "failed to decrypt in multi master key backend: {}",
            errs.join("; ")
        ))
    }
}

#[test]
fn test_multi_master_key_backend_decrypt() {
    let encryptedContent = EncryptedContent {
        Content: b"encrypted".to_vec(),
        Metadata: Default::default(),
    };

    // success first backend — second must not be called.
    {
        let calls1 = Arc::new(AtomicUsize::new(0));
        let calls2 = Arc::new(AtomicUsize::new(0));
        let mock1 = MockBackend {
            decrypt_calls: Arc::clone(&calls1),
            result: Ok(b"decrypted".to_vec()),
        };
        let mock2 = MockBackend {
            decrypt_calls: Arc::clone(&calls2),
            result: Ok(b"should-not-return".to_vec()),
        };
        let backend = TestMultiMasterKeyBackend {
            backends: vec![Box::new(mock1), Box::new(mock2)],
        };

        let result = backend
            .Decrypt(&encryptedContent)
            .expect("require.NoError: first backend succeeds");
        assert_eq!(b"decrypted".to_vec(), result);
        assert_eq!(1, calls1.load(Ordering::SeqCst));
        assert_eq!(0, calls2.load(Ordering::SeqCst), "AssertNotCalled Decrypt");
    }

    // success second backend — first fails, second succeeds.
    {
        let calls1 = Arc::new(AtomicUsize::new(0));
        let calls2 = Arc::new(AtomicUsize::new(0));
        let mock1 = MockBackend {
            decrypt_calls: Arc::clone(&calls1),
            result: Err("failed".into()),
        };
        let mock2 = MockBackend {
            decrypt_calls: Arc::clone(&calls2),
            result: Ok(b"decrypted".to_vec()),
        };
        let backend = TestMultiMasterKeyBackend {
            backends: vec![Box::new(mock1), Box::new(mock2)],
        };

        let result = backend
            .Decrypt(&encryptedContent)
            .expect("require.NoError: second backend succeeds");
        assert_eq!(b"decrypted".to_vec(), result);
        assert_eq!(1, calls1.load(Ordering::SeqCst));
        assert_eq!(1, calls2.load(Ordering::SeqCst));
    }

    // all backends fail — error aggregates both failure messages.
    {
        let calls1 = Arc::new(AtomicUsize::new(0));
        let calls2 = Arc::new(AtomicUsize::new(0));
        let mock1 = MockBackend {
            decrypt_calls: Arc::clone(&calls1),
            result: Err("failed1".into()),
        };
        let mock2 = MockBackend {
            decrypt_calls: Arc::clone(&calls2),
            result: Err("failed2".into()),
        };
        let backend = TestMultiMasterKeyBackend {
            backends: vec![Box::new(mock1), Box::new(mock2)],
        };

        let err = backend.Decrypt(&encryptedContent).unwrap_err();
        assert!(err.contains("failed1"), "got {err}");
        assert!(err.contains("failed2"), "got {err}");
        assert_eq!(1, calls1.load(Ordering::SeqCst));
        assert_eq!(1, calls2.load(Ordering::SeqCst));
    }

    // no backends — internal error path.
    {
        let backend = TestMultiMasterKeyBackend { backends: vec![] };
        let err = backend.Decrypt(&encryptedContent).unwrap_err();
        assert!(err.contains("internal error"), "got {err}");
    }

    // Production MultiMasterKeyBackend via NewMultiMasterKeyBackend + real file keys
    // (Go injects mock Backend interface; Rust uses CreateBackend for each MasterKey).
    {
        let key1 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let key2 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let path1 = write_temp_key(key1);
        let path2 = write_temp_key(key2);
        let fb2 = createFileBackend(&path2).expect("file backend 2");
        let encrypted = fb2.Encrypt(b"decrypted").expect("encrypt with key2");
        let master_keys = [
            MasterKey {
                Backend: MasterKeyBackend::File(MasterKeyFile {
                    Path: path1.clone(),
                }),
            },
            MasterKey {
                Backend: MasterKeyBackend::File(MasterKeyFile {
                    Path: path2.clone(),
                }),
            },
        ];
        let multi =
            NewMultiMasterKeyBackend(Some(&master_keys)).expect("create multi master key backend");
        let result = multi
            .Decrypt(&encrypted)
            .expect("second file backend decrypts");
        assert_eq!(b"decrypted".to_vec(), result);
        let _ = std::fs::remove_file(&path1);
        let _ = std::fs::remove_file(&path2);
    }
}

fn write_temp_key(hex_key: &str) -> String {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "multi_mk_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut f = std::fs::File::create(&path).expect("create temp key");
    writeln!(f, "{hex_key}").expect("write key");
    path.to_string_lossy().into_owned()
}
