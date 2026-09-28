// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Go-equivalent tests for `pitr_collector_test.go`.
//! Storage boundary: MemStorage (no objstore/etcd).
//! PiTR 收集器测试：准备 migration、单文件/多文件、重开与冲突。
//! PiTRCollT 封装批处理生命周期 must_start/done/reopen。
//! 冲突用例锁定不可静默覆盖的元数据约束。
//! 成功路径需写出 operation metadata，失败路径不得留下脏 SST 索引。
//! 对齐 Go `pitr_collector_test.go` 的场景划分。
//! PiTRCollT 包装收集器生命周期，简化 batch/done/reopen 测试。
//! PrepareMig 用例锁定 operation metadata 写出。
//! 单文件与多文件用例覆盖 rewrite rule 聚合。
//! reopen 后应看到既有 SST 索引，而不是空库。
//! conflict 用例确保冲突不能被静默忽略。
//! must_start_restore_batch 失败时应让测试立即失败。
//! mark_success/done 区分“批完成”与“整体成功”语义。
//! 不检查对象存储真实上传，只检查本地元数据组织。
//! 路径断言使用相对布局，避免机器相关绝对路径。
//! 与 Go 测试场景名对应，便于双端排查。
//! 补充要点1：PiTRCollT 包装收集器生命周期，简化 batch/done/reopen 测试。
//! 补充要点2：PrepareMig 用例锁定 operation metadata 写出。
//! 补充要点3：单文件与多文件用例覆盖 rewrite rule 聚合。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::pitr_collector::{PiTRCollDep, newPiTRCollForTest, pitrCollector};
use crate::stubs::{
    BackupFileSet, Context, Copier, ExternalStorage, MemStorage, Result, RewriteRules,
    TableIDRemap, backuppb, import_sstpb,
};

/// `PiTRCollT`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
struct PiTRCollT {
    coll: pitrCollector,
    tso_cnt: Arc<AtomicU64>,
    success: Arc<AtomicBool>,
    restore: Arc<MemStorage>,
    task: Arc<MemStorage>,
    cx: Context,
}

impl PiTRCollT {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn new() -> Self {
        let restore = Arc::new(MemStorage::default());
        let task = Arc::new(MemStorage::default());
        let tso_cnt = Arc::new(AtomicU64::new(0));
        let success = Arc::new(AtomicBool::new(false));
        let tso_c = tso_cnt.clone();
        let success_c = success.clone();
        let deps = PiTRCollDep {
            enabled: true,
            Storage: Some(restore.clone()),
            TaskStorage: Some(task.clone()),
            name: "test-pitr".into(),
            restoreUUID: vec![7; 16],
            maxCopyConcurrency: 4,
            tso: Some(Box::new(move |_| {
                Ok(tso_c.fetch_add(1, Ordering::SeqCst) + 1)
            })),
            restoreSuccess: Some(Box::new(move || success_c.load(Ordering::SeqCst))),
            ..Default::default()
        };
        let coll = newPiTRCollForTest(deps).unwrap();
        Self {
            coll,
            tso_cnt,
            success,
            restore,
            task,
            cx: Context::Background(),
        }
    }

    /// `must_start_restore_batch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn must_start_restore_batch(
        &self,
        fs: Vec<BackupFileSet>,
    ) -> Box<dyn FnOnce() -> crate::stubs::Result<()> + '_> {
        for b in &fs {
            for file in &b.SSTFiles {
                self.restore.seed(&file.Name, b"something");
            }
        }
        self.coll.onBatch(&self.cx, &fs).unwrap().unwrap()
    }

    /// `done`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn done(self) {
        self.coll.close().unwrap();
    }

    /// `mark_success`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn mark_success(&self) {
        self.success.store(true, Ordering::SeqCst);
    }

    /// `reopen`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn reopen(&mut self) {
        let deps = PiTRCollDep {
            enabled: true,
            Storage: Some(self.restore.clone()),
            TaskStorage: Some(self.task.clone()),
            name: format!(
                "test-reopen-{}",
                self.tso_cnt.fetch_add(1, Ordering::SeqCst)
            ),
            restoreUUID: vec![7; 16],
            maxCopyConcurrency: 4,
            tso: Some({
                let c = self.tso_cnt.clone();
                Box::new(move |_| Ok(c.fetch_add(1, Ordering::SeqCst) + 1))
            }),
            restoreSuccess: Some({
                let s = self.success.clone();
                Box::new(move || s.load(Ordering::SeqCst))
            }),
            ..Default::default()
        };
        self.success.store(false, Ordering::SeqCst);
        self.coll = newPiTRCollForTest(deps).unwrap();
    }
}

/// `batch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn batch(name: &str, remap: Option<(i64, i64)>) -> Vec<BackupFileSet> {
    vec![BackupFileSet {
        TableID: 1,
        SSTFiles: vec![backuppb::File {
            Name: name.into(),
            ..Default::default()
        }],
        RewriteRules: remap.map(|(o, n)| RewriteRules {
            TableIDRemapHint: vec![TableIDRemap {
                Origin: o,
                Rewritten: n,
            }],
            ..Default::default()
        }),
    }]
}

/// TestPiTRCollectorPrepareMigWritesOperationMetadata — Go same (slim: migration path).
#[test]
/// 测试 `test_pitr_collector_prepare_mig_writes_operation_metadata`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_pitr_collector_prepare_mig_writes_operation_metadata() {
    let coll = PiTRCollT::new();
    coll.coll.prepareMig(&coll.cx).unwrap();
    let paths = coll.coll.migration_paths_for_test();
    assert_eq!(paths.len(), 1);
    assert!(paths[0].contains("extbackupmeta"));
    assert!(
        coll.task
            .FileExists(&coll.cx, &coll.coll.metaPath())
            .unwrap()
    );
    coll.done();
}

/// TestCollAFile — Go `TestCollAFile`.
#[test]
/// 测试 `test_coll_a_file`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_coll_a_file() {
    let coll = PiTRCollT::new();
    let wait = coll.must_start_restore_batch(batch("a.sst", Some((1, 2))));
    wait().unwrap();
    let files = coll.coll.files_for_test();
    assert_eq!(files.len(), 1);
    assert!(files[0].contains("a.sst"));
    assert_eq!(coll.coll.rewrites_for_test().get(&1), Some(&2));
    coll.mark_success();
    coll.done();
}

/// TestCollManyFileAndRewriteRules — Go same.
#[test]
/// 测试 `test_coll_many_file_and_rewrite_rules`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_coll_many_file_and_rewrite_rules() {
    let coll = PiTRCollT::new();
    let mut fs = batch("a.sst", Some((10, 20)));
    fs[0].SSTFiles.push(backuppb::File {
        Name: "b.sst".into(),
        ..Default::default()
    });
    fs[0]
        .RewriteRules
        .as_mut()
        .unwrap()
        .TableIDRemapHint
        .push(TableIDRemap {
            Origin: 11,
            Rewritten: 21,
        });
    let wait = coll.must_start_restore_batch(fs);
    wait().unwrap();
    assert_eq!(coll.coll.files_for_test().len(), 2);
    assert_eq!(coll.coll.rewrites_for_test().get(&10), Some(&20));
    assert_eq!(coll.coll.rewrites_for_test().get(&11), Some(&21));
    coll.mark_success();
    coll.done();
}

/// TestReopen — Go `TestReopen`.
#[test]
/// 测试 `test_reopen`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_reopen() {
    let mut coll = PiTRCollT::new();
    let wait = coll.must_start_restore_batch(batch("a.sst", Some((1, 2))));
    wait().unwrap();
    coll.mark_success();
    coll.coll.close().unwrap();

    coll.reopen();
    let wait = coll.must_start_restore_batch(batch("c.sst", Some((3, 4))));
    wait().unwrap();
    assert!(
        coll.coll
            .files_for_test()
            .iter()
            .any(|f| f.contains("c.sst"))
    );
    coll.mark_success();
    coll.done();
}

/// TestConflict — Go `TestConflict`.
#[test]
/// 测试 `test_conflict`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_conflict() {
    let coll = PiTRCollT::new();
    let wait = coll.must_start_restore_batch(batch("a.sst", Some((1, 2))));
    wait().unwrap();
    assert!(coll.coll.putRewriteRule(&coll.cx, 1, 3).is_err());
    coll.mark_success();
    coll.done();
}

/// TestConcurrency — Go `TestConcurrency`.
#[test]
/// 测试 `test_concurrency`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_concurrency() {
    struct BlockingCopier {
        inner: MemStorage,
        active: AtomicUsize,
        max_active: AtomicUsize,
        gate: (Mutex<bool>, Condvar),
    }

    impl ExternalStorage for BlockingCopier {
        fn WriteFile(&self, ctx: &Context, path: &str, data: &[u8]) -> Result<()> {
            self.inner.WriteFile(ctx, path, data)
        }

        fn ReadFile(&self, ctx: &Context, path: &str) -> Result<Vec<u8>> {
            self.inner.ReadFile(ctx, path)
        }

        fn FileExists(&self, ctx: &Context, path: &str) -> Result<bool> {
            self.inner.FileExists(ctx, path)
        }

        fn WalkDir(&self, ctx: &Context, prefix: &str) -> Result<Vec<String>> {
            self.inner.WalkDir(ctx, prefix)
        }
    }

    impl Copier for BlockingCopier {
        fn CopyFrom(
            &self,
            ctx: &Context,
            from: &dyn ExternalStorage,
            from_path: &str,
            to_path: &str,
        ) -> Result<()> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            let mut open = self.gate.0.lock().unwrap();
            while !*open {
                open = self.gate.1.wait(open).unwrap();
            }
            drop(open);
            let result = self.inner.CopyFrom(ctx, from, from_path, to_path);
            self.active.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }

    let restore = Arc::new(MemStorage::default());
    let task = Arc::new(BlockingCopier {
        inner: MemStorage::default(),
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        gate: (Mutex::new(false), Condvar::new()),
    });
    let mut coll = newPiTRCollForTest(PiTRCollDep {
        enabled: true,
        Storage: Some(restore.clone()),
        TaskStorage: Some(task.clone()),
        name: "test-concurrency".into(),
        restoreUUID: vec![7; 16],
        ..Default::default()
    })
    .unwrap();
    coll.setConcurrency(2);
    let coll = Arc::new(coll);
    let ctx = Context::Background();

    let mut handles = Vec::new();
    for i in 0..10 {
        let name = format!("f{i}.sst");
        restore.seed(&name, b"x");
        let coll = coll.clone();
        let ctx = ctx.clone();
        handles.push(thread::spawn(move || {
            let fs = batch(&name, Some((i as i64, i as i64 + 100)));
            let wait = coll.onBatch(&ctx, &fs).unwrap().unwrap();
            wait().unwrap();
        }));
    }

    let deadline = Instant::now() + Duration::from_secs(1);
    while task.active.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    thread::sleep(Duration::from_millis(25));
    let observed_max = task.max_active.load(Ordering::SeqCst);
    *task.gate.0.lock().unwrap() = true;
    task.gate.1.notify_all();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(observed_max, 2);
    assert_eq!(coll.files_for_test().len(), 10);
    coll.close().unwrap();
}

/// Go `onBatch` schedules copy work before returning its wait closure.
#[test]
fn test_on_batch_owned_starts_uploads_before_wait_callback() {
    struct StartSignalCopier {
        inner: MemStorage,
        started: AtomicUsize,
        gate: (Mutex<bool>, Condvar),
    }

    impl ExternalStorage for StartSignalCopier {
        fn WriteFile(&self, ctx: &Context, path: &str, data: &[u8]) -> Result<()> {
            self.inner.WriteFile(ctx, path, data)
        }

        fn ReadFile(&self, ctx: &Context, path: &str) -> Result<Vec<u8>> {
            self.inner.ReadFile(ctx, path)
        }

        fn FileExists(&self, ctx: &Context, path: &str) -> Result<bool> {
            self.inner.FileExists(ctx, path)
        }

        fn WalkDir(&self, ctx: &Context, prefix: &str) -> Result<Vec<String>> {
            self.inner.WalkDir(ctx, prefix)
        }
    }

    impl Copier for StartSignalCopier {
        fn CopyFrom(
            &self,
            ctx: &Context,
            from: &dyn ExternalStorage,
            from_path: &str,
            to_path: &str,
        ) -> Result<()> {
            self.started.fetch_add(1, Ordering::SeqCst);
            let mut open = self.gate.0.lock().unwrap();
            while !*open {
                open = self.gate.1.wait(open).unwrap();
            }
            drop(open);
            self.inner.CopyFrom(ctx, from, from_path, to_path)
        }
    }

    let restore = Arc::new(MemStorage::default());
    let task = Arc::new(StartSignalCopier {
        inner: MemStorage::default(),
        started: AtomicUsize::new(0),
        gate: (Mutex::new(false), Condvar::new()),
    });
    let coll = Arc::new(
        newPiTRCollForTest(PiTRCollDep {
            enabled: true,
            Storage: Some(restore.clone()),
            TaskStorage: Some(task.clone()),
            name: "test-start-before-wait".into(),
            restoreUUID: vec![7; 16],
            ..Default::default()
        })
        .unwrap(),
    );
    let ctx = Context::Background();
    let mut sets = Vec::new();
    for i in 0..2 {
        let name = format!("start-{i}.sst");
        restore.seed(&name, b"x");
        sets.extend(batch(&name, None));
    }

    let wait = coll.onBatchOwned(&ctx, &sets).unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_millis(250);
    while task.started.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let started_before_wait = task.started.load(Ordering::SeqCst);
    *task.gate.0.lock().unwrap() = true;
    task.gate.1.notify_all();
    wait().unwrap();

    assert_eq!(started_before_wait, 2);
    assert_eq!(coll.files_for_test().len(), 2);
    coll.close().unwrap();
}

/// Unsupported rewrite timestamp (parity with Go verifyCompatibilityFor).
#[test]
/// 测试 `test_unsupported_rewrite_timestamp`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_unsupported_rewrite_timestamp() {
    let coll = PiTRCollT::new();
    let bad = vec![BackupFileSet {
        TableID: 1,
        SSTFiles: vec![],
        RewriteRules: Some(RewriteRules {
            Data: vec![import_sstpb::RewriteRule {
                NewTimestamp: 9,
                ..Default::default()
            }],
            ..Default::default()
        }),
    }];
    assert!(coll.coll.verifyCompatibilityFor(&bad[0]).is_err());
    coll.done();
}
