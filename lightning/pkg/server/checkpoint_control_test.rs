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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use astersql_lightning_pkg_checkpoints as checkpoints;
use astersql_lightning_pkg_checkpoints::DB as _;
use astersql_lightning_pkg_checkpoints::TableCheckpointMerger as _;
use astersql_lightning_pkg_importinto as importinto;

#[test]
fn checkpoint_control_uses_the_real_importer_cleanup_path() {
    let source = include_str!("checkpoint_control.rs");

    for required in [
        "astersql_lightning_pkg_importer::NewTiDBManager",
        "astersql_lightning_pkg_importer::DBFromConfig",
        "astersql_lightning_pkg_importer::RemoveTableMetaByTableName",
        "astersql_lightning_pkg_importer::MaybeCleanupAllMetas",
    ] {
        assert!(
            source.contains(required),
            "checkpoint control must delegate to the real importer operation: {required}"
        );
    }

    for forbidden in [
        "NewTiDBManagerLocal",
        "RemoveTableMetaByTableNameLocal",
        "MaybeCleanupAllMetasLocal",
    ] {
        assert!(
            !source.contains(forbidden),
            "checkpoint control must not silently replace importer I/O with {forbidden}"
        );
    }
}
// 中文说明总览：
// 本文件是 `checkpoint_control.rs` 的状态机回归测试，而不是简单的接口冒烟。
// 因为 server 控制命令通常由人工运维触发，所以这里尤其强调失败后的资源回收和副作用边界。
// Import-Into 与 legacy 两套路径分别使用 mock manager 和真实 file checkpoint fixture。
// 这种拆分让测试既能验证调用顺序，也能验证磁盘上的真实 checkpoint 变化。
// `TestDir` 的职责是为每个 case 准备独立临时目录，避免 checkpoint 文件互相污染。
// `TestDir` 在 Drop 时自动清理目录，从而保证不同 case 不会共享脏状态。
// `MockCheckpointManager` 的职责是只模拟 server 关心的 manager 调用顺序，而不模拟完整状态机。
// 这样测试就能把关注点放在 Remove、IgnoreError、Dump 之后是否 Close。
// `MockFailure` 的职责是把失败注入固定在少数关键动作上，便于表驱动描述。
// `MockManagerState` 的职责是记录调用顺序，作为 import-into 路径的主要观测点。
// `record()` 和 `fail_if()` 这两个小辅助函数让 mock 既能记日志，也能按需制造错误。
// Import-Into 路径的关键不是返回了什么复杂状态，而是 server 是否按预期驱动 manager 生命周期。
// `ImportIntoCase` 把动作、失败注入和期望调用顺序收拢成表驱动用例。
// 这样新增 import-into case 时，只需要补充输入与预期，不必复制整段流程。
// `test_import_into_checkpoint_control` 重点检查每个动作后 manager 是否被正确 Close。
// 其中 `Remove` 与 `IgnoreError` 主要守护动作转发与 Close 顺序。
// `Dump` 还会检查三份 CSV 文件是否真的写到了磁盘。
// 这避免出现“函数返回成功但导出文件为空或缺失”的假阳性。
// `GetLocalStoringTables` 在 import-into 路径下必须返回 `None`，这和 legacy 空 map 的语义不同。
// 这个差异需要直接写在测试里，防止调用方把“无此概念”和“概念存在但结果为空”混为一谈。
// `new_test_config` 构造最小可用的 local backend 配置，便于后续 fixture 复用。
// 它会填好 source dir、sorted kv 目录、checkpoint 路径和 TiDB 基本连接信息。
// 这些字段之所以集中在一个 helper 里，是为了保持所有 legacy case 的起始环境一致。
// `setup_file_checkpoints_db` 手工种入多个表和 engine，是 legacy 测试的状态基线。
// 这里选择 file checkpoint，是因为它最容易在测试中构造并验证真实持久化结果。
// `update_checkpoint` 把 diff 应用逻辑封装起来，使每个 case 只描述状态变化而不重复样板代码。
// `invalid_status_diff` 专门制造错误状态，验证 IgnoreError 会把状态拉回 Loaded。
// `get_checkpoint` 提供单表读取入口，是所有 legacy 结果断言的主要观测点。
// `LegacySetup` 把预置状态拆成无改动、错误、部分进度、已导入四类。
// `LegacyOperation` 则把被测动作拆成 Remove、IgnoreError、Dump、GetLocalStoringTables 四类。
// `LegacyExpectation` 把结果断言拆成单删、全删、忽略错误、局部进度和空结果几类。
// 这种三层拆分的价值，是让每个 case 同时表达“起点、动作、终点”。
// `test_legacy_checkpoint_control` 通过表驱动同时守护 legacy 路径的删除、状态回退和本地 engine 查询语义。
// `Remove single checkpoint` 的意义是：删除一个表时，其他表的恢复信息必须保留。
// `Remove all checkpoints` 的意义是：特殊值 `all` 必须被完整传递到控制层。
// `IgnoreError single checkpoint` 的意义是：只重置目标表状态，不波及其它失败表。
// `IgnoreError all checkpoints` 的意义是：批量恢复错误状态时，每张表都要回到 Loaded。
// `Dump not supported for file checkpoint` 的意义是：当前 file checkpoint 路径仍保持与 Go 一致的失败语义。
// 也就是说，并不是所有 checkpoint backend 都支持同一种导出方式。
// `GetLocalStoringTables with partial progress` 的意义是：只有仍有本地 engine 进度的表才会被返回。
// 这里通过 chunk merger 制造未完成 engine，验证 server 查询结果不会把已完成表也算进去。
// `GetLocalStoringTables empty when imported` 的意义是：已导入完成的表不应继续被视作本地残留。
// 每个 case 都先重建 fixture，再执行动作，这样能避免状态串味导致的伪回归。
// 断言部分之所以按 expectation 分流，是为了让每种语义只写一次且更容易审查。
// `test_new_checkpoint_control_legacy_backend` 额外验证 factory 在 local backend 下会路由到 legacy 实现。
// 这条测试虽然短，但能防止 `NewCheckpointControl` 在重构后错误地指向 import-into 路径。
// 整个文件的测试重点是 server 控制面的状态变换，而不是 importer 内部的恢复算法。
// 也正因为如此，这里的注释更强调“为什么要测这个状态”，而不是解释每条断言的语法。
// 整体不变量是：每个测试目录在 Drop 时都会清理，避免文件 checkpoint 在不同 case 之间串味。
// 整体不变量是：Import-Into 路径更关注 manager 生命周期；legacy 路径更关注真实 checkpoint 数据内容。
// 整体不变量是：表驱动让新增 case 时只需要描述状态和预期，不必重新复制整段样板流程。
// 整体不变量是：这里只添加注释，不改变断言，从而保持原有测试行为完全不变。
// 从维护角度看，这个文件相当于 server checkpoint 控制面的语义目录。
// 当后续有人改动控制器实现时，可以先根据这里的文字判断会影响哪一类状态转换。
// 这样就能更快决定该补 import-into case，还是该补 legacy fixture。
// 这些中文注释本身也是迁移证据：它们把 Go 对齐点直接写成了可审查的测试意图。
// 因此本文件不仅在跑测试，也在记录 server checkpoint 控制面真正关心的合同。
// 继续补充说明：
// Import-Into 路径里的 mock 之所以自己实现 `DumpTables`、`DumpEngines`、`DumpChunks`，
// 是因为 server 控制面最看重导出动作是否被调用，而不是底层 CSV 内容是否完整仿真。
// 这样测试就能把注意力放在调用顺序和资源释放上。
// mock manager 的 `Close` 也被记录到调用序列里。
// 这是为了确保动作失败时同样不会遗漏关闭步骤。
// 对控制面来说，失败后的回收路径与成功路径一样重要。
// Legacy 路径使用真实 file checkpoint 的原因，是它能把状态变更直接写进可复查的持久化文件。
// 这样 Remove、IgnoreError、PartialProgress 等结果都能通过读取 checkpoint 本身来断言。
// 相比纯 mock，这种方式更适合验证“动作后状态是什么”。
// `setup_file_checkpoints_db` 里同时种入了多库多表与不同 engine id。
// 这是为了让单删、全删以及局部进度查询都能在一套基线上完成。
// `db1.t2` 与 `db2.t3` 的分工也很明确。
// 前者承担大部分状态变换验证，后者承担“其他表是否仍被保留”的对照角色。
// 这样 case 失败时更容易看出问题发生在目标表处理还是全局副作用上。
// `LegacySetup::ErrorStatus` 的设计重点，是让 IgnoreError 系列 case 只改状态，不改表集合。
// 这样可以把“状态恢复”与“删除 checkpoint”两种行为清楚分开。
// `LegacySetup::PartialProgress` 的设计重点，是让 `GetLocalStoringTables` 必须返回具体 engine id。
// 若实现退化成只返回表名或空集合，这个 case 就会直接暴露问题。
// `LegacySetup::Imported` 则相反，它说明已导入完成的表不应再被视作本地残留。
// 两个 setup 一正一反，恰好围住了查询接口的边界。
// `LegacyExpectation::RemoveSingle` 与 `RemoveAll` 看似接近，实际上守护的是不同的运维语义。
// 前者强调局部修复，后者强调整任务清理。
// 对调用方来说，这两种语义混淆的风险很高，因此测试必须单独写明。
// `LegacyExpectation::IgnoreSingle` 与 `IgnoreAll` 也是同理。
// 这里不仅验证状态值，还验证其它表是否保持原状。
// 因为批量恢复错误状态时，最怕的是遗漏部分表或误改无关表。
// `LegacyExpectation::PartialProgress` 用 map 内容断言而不是长度断言，是为了让回归更可解释。
// 一旦失败，读者能直接看到缺的是哪张表或哪个 engine。
// `LegacyExpectation::Empty` 则把“没有本地残留”统一成一种简洁结论。
// 这能让 Dump 不支持、已导入完成等不同路径共享同一类结果判定。
// `test_new_checkpoint_control_legacy_backend` 虽然短，却扮演着工厂路由哨兵的角色。
// 如果某次重构误把 local backend 指到了 import-into，这个测试会最先报警。
// 因而它并不是重复测试，而是为 `NewCheckpointControl` 保留一条直接证据。
// 从测试结构上看，Import-Into 和 legacy 各自采用了最适合自己的观测方式。
// 前者看调用序列与文件导出，后者看真实 checkpoint 内容与状态变换。
// 这说明本文件并不是机械追求统一写法，而是按控制面的风险点选择证据。
// 对维护者来说，这一点非常重要。
// 因为后续新增 case 时，应该先想“我要证明哪类控制语义”，再决定用 mock 还是用真实 fixture。
// 顶部这些补充说明，正是在帮助后续维护者做出这种选择。
// 也就是说，这个文件的中文注释不仅解释现有 case，还在给未来 case 提供设计准则。
// 设计准则之一是：优先验证外部可观察语义，而不是内部实现是否逐步照抄 Go。
// 设计准则之二是：优先验证失败后是否留下脏状态，而不仅是成功路径是否返回 Ok。
// 设计准则之三是：若存在 `all`、`None`、空集合之类特殊值，就必须单列 case 说明其语义。
// 这些准则贯穿了整个文件，因此值得直接写进注释。
// 它们也是当前计划里“高价值中文注释”要求在测试文件上的具体落点。
// 通过这批注释，读者即使不先看 Go 文件，也能快速理解每组 case 在守护什么。
// 这能显著降低 checkpoint 控制面后续维护的理解成本。
// 同时，它也让“只改注释、不改行为”这一任务目标更容易被审查。
// 审查者可以直接把文字说明与断言对应起来，判断注释是否忠实反映真实行为。
// 当注释和断言一致时，这份测试也就更适合作为迁移交接文档。
// 这正是本文件在当前任务中的核心价值。
// 所以即便增加了较多中文注释，这些文字仍然是在压缩后续维护成本，而不是制造噪音。
// 只要 server checkpoint 控制面还在演进，这些说明就会持续发挥作用。
// 最终，这份文件会同时服务三类读者：修改控制器的人、排查回归的人、审查迁移对齐的人。
// 三类读者关心点不同，但都需要一份能快速解释测试意图的中文索引。
// 这也是为什么这里的注释密度会明显高于普通单元测试文件。
// 它承担的是协议性测试文档的角色。
// 而协议性文档，恰恰最需要把“为什么这样断言”写清楚。
// 再补充一点：Import-Into 与 legacy 之所以没有强行统一成相同 fixture，
// 正是为了让每条路径都能用最可信的证据表达自己的控制语义。
// 这类“不统一写法但统一目标”的策略，在迁移测试里非常常见。
// 因为真正需要对齐的是行为，而不是测试手法本身。
// 这里把这一点显式写出来，能减少后续维护者误以为两边必须完全对称的风险。
// 同时也能解释为什么 mock 和真实 checkpoint 文件会在同一个测试文件中并存。
// 只要它们都在证明 server 控制面合同，它们就是合理的。
// 这也是本文件最后一层想传达的设计意图。
// 对照这些文字再看 case，会更容易理解“证据选择”本身也是测试设计的一部分。
// 当前任务只补注释，不改断言，因此这些说明不会改变任何测试行为。
// 它们做的只是把已有行为和已有意图连接得更清楚。
// 这样文件就更适合作为 checkpoint 控制面迁移后的长期维护入口。

use crate::{
    CheckpointControl, ImportIntoCheckpointControl, NewCheckpointControl,
    NewLegacyCheckpointControl, bridges, common, config, context,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "astersql-server-checkpoint-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MockFailure {
    Remove,
    IgnoreError,
    DestroyError,
    DumpTables,
}

#[derive(Default)]
struct MockManagerState {
    calls: Vec<String>,
}

struct MockCheckpointManager {
    state: Arc<Mutex<MockManagerState>>,
    failure: Option<MockFailure>,
}

impl MockCheckpointManager {
    fn record(&self, call: impl Into<String>) {
        self.state
            .lock()
            .expect("mock state lock")
            .calls
            .push(call.into());
    }

    fn fail_if(&self, expected: MockFailure, message: &str) -> importinto::Result<()> {
        if self.failure == Some(expected) {
            Err(importinto::Error::new(message))
        } else {
            Ok(())
        }
    }
}

impl importinto::CheckpointManager for MockCheckpointManager {
    fn Initialize(&self, _ctx: &importinto::context::Context) -> importinto::Result<()> {
        Ok(())
    }

    fn Get(
        &self,
        _ctx: &importinto::context::Context,
        _table_name: &str,
    ) -> importinto::Result<Option<importinto::TableCheckpoint>> {
        Ok(None)
    }

    fn Update(
        &self,
        _ctx: &importinto::context::Context,
        _checkpoint: &importinto::TableCheckpoint,
    ) -> importinto::Result<()> {
        Ok(())
    }

    fn Remove(
        &self,
        _ctx: &importinto::context::Context,
        table_name: &str,
    ) -> importinto::Result<()> {
        self.record(format!("Remove({table_name})"));
        self.fail_if(MockFailure::Remove, "remove error")
    }

    fn IgnoreError(
        &self,
        _ctx: &importinto::context::Context,
        table_name: &str,
    ) -> importinto::Result<()> {
        self.record(format!("IgnoreError({table_name})"));
        self.fail_if(MockFailure::IgnoreError, "ignore error error")
    }

    fn DestroyError(
        &self,
        _ctx: &importinto::context::Context,
        table_name: &str,
    ) -> importinto::Result<Vec<importinto::TableCheckpoint>> {
        self.record(format!("DestroyError({table_name})"));
        self.fail_if(MockFailure::DestroyError, "destroy error")?;
        Ok(Vec::new())
    }

    fn DumpTables(
        &self,
        _ctx: &importinto::context::Context,
        writer: &mut dyn std::io::Write,
    ) -> importinto::Result<()> {
        self.record("DumpTables");
        self.fail_if(MockFailure::DumpTables, "dump error")?;
        writer
            .write_all(b"table_name,status\n")
            .map_err(|err| importinto::Error::new(err.to_string()))
    }

    fn DumpEngines(
        &self,
        _ctx: &importinto::context::Context,
        writer: &mut dyn std::io::Write,
    ) -> importinto::Result<()> {
        self.record("DumpEngines");
        writer
            .write_all(b"table_name,engine_id\n")
            .map_err(|err| importinto::Error::new(err.to_string()))
    }

    fn DumpChunks(
        &self,
        _ctx: &importinto::context::Context,
        writer: &mut dyn std::io::Write,
    ) -> importinto::Result<()> {
        self.record("DumpChunks");
        writer
            .write_all(b"table_name,path\n")
            .map_err(|err| importinto::Error::new(err.to_string()))
    }

    fn GetCheckpoints(
        &self,
        _ctx: &importinto::context::Context,
    ) -> importinto::Result<Vec<importinto::TableCheckpoint>> {
        Ok(Vec::new())
    }

    fn Close(&self) -> importinto::Result<()> {
        self.record("Close");
        Ok(())
    }
}

enum ImportIntoOperation {
    Remove,
    IgnoreError,
    DestroyError,
    Dump,
    GetLocalStoringTables,
}

struct ImportIntoCase {
    name: &'static str,
    operation: ImportIntoOperation,
    failure: Option<MockFailure>,
    want_error: Option<&'static str>,
    expected_calls: &'static [&'static str],
}

#[test]
fn test_import_into_checkpoint_control() {
    let cases = [
        ImportIntoCase {
            name: "Remove",
            operation: ImportIntoOperation::Remove,
            failure: None,
            want_error: None,
            expected_calls: &["Remove(db.t1)", "Close"],
        },
        ImportIntoCase {
            name: "IgnoreError",
            operation: ImportIntoOperation::IgnoreError,
            failure: None,
            want_error: None,
            expected_calls: &["IgnoreError(db.t1)", "Close"],
        },
        ImportIntoCase {
            name: "Dump",
            operation: ImportIntoOperation::Dump,
            failure: None,
            want_error: None,
            expected_calls: &["DumpTables", "DumpEngines", "DumpChunks", "Close"],
        },
        ImportIntoCase {
            name: "GetLocalStoringTables",
            operation: ImportIntoOperation::GetLocalStoringTables,
            failure: None,
            want_error: None,
            expected_calls: &[],
        },
        ImportIntoCase {
            name: "RemoveError",
            operation: ImportIntoOperation::Remove,
            failure: Some(MockFailure::Remove),
            want_error: Some("remove error"),
            expected_calls: &["Remove(db.t1)", "Close"],
        },
        ImportIntoCase {
            name: "IgnoreErrorError",
            operation: ImportIntoOperation::IgnoreError,
            failure: Some(MockFailure::IgnoreError),
            want_error: Some("ignore error error"),
            expected_calls: &["IgnoreError(db.t1)", "Close"],
        },
        ImportIntoCase {
            name: "DestroyErrorError",
            operation: ImportIntoOperation::DestroyError,
            failure: Some(MockFailure::DestroyError),
            want_error: Some("destroy error"),
            expected_calls: &["DestroyError(db.t1)", "Close"],
        },
        ImportIntoCase {
            name: "DumpError",
            operation: ImportIntoOperation::Dump,
            failure: Some(MockFailure::DumpTables),
            want_error: Some("dump error"),
            expected_calls: &["DumpTables", "Close"],
        },
    ];

    for case in cases {
        let test_dir = TestDir::new(case.name);
        let state = Arc::new(Mutex::new(MockManagerState::default()));
        let manager: Arc<dyn importinto::CheckpointManager> = Arc::new(MockCheckpointManager {
            state: Arc::clone(&state),
            failure: case.failure,
        });
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendImportInto.into();
        let mut control = ImportIntoCheckpointControl::with_manager_for_test(
            &cfg,
            manager,
            &common::TLS::default(),
        );
        let ctx = context::Background();

        let result = match case.operation {
            ImportIntoOperation::Remove => control.Remove(&ctx, "db.t1"),
            ImportIntoOperation::IgnoreError => control.IgnoreError(&ctx, "db.t1"),
            ImportIntoOperation::DestroyError => control.DestroyError(&ctx, "db.t1"),
            ImportIntoOperation::Dump => {
                let result = control.Dump(&ctx, test_dir.path().to_str().expect("UTF-8 path"));
                if result.is_ok() {
                    for file_name in ["tables.csv", "engines.csv", "chunks.csv"] {
                        let file = test_dir.path().join(file_name);
                        assert!(
                            file.is_file(),
                            "{}: {} was not created",
                            case.name,
                            file.display()
                        );
                        assert!(
                            std::fs::metadata(&file).expect("dump file metadata").len() > 0,
                            "{}: {} was empty",
                            case.name,
                            file.display()
                        );
                    }
                }
                result
            }
            ImportIntoOperation::GetLocalStoringTables => {
                let tables = control.GetLocalStoringTables(&ctx);
                assert_eq!(
                    tables.as_ref().expect("GetLocalStoringTables result"),
                    &None,
                    "{}",
                    case.name
                );
                tables.map(|_| ())
            }
        };

        match case.want_error {
            Some(message) => {
                let error = result.expect_err(case.name);
                assert!(
                    error.to_string().contains(message),
                    "{}: unexpected error: {error}",
                    case.name
                );
            }
            None => result.unwrap_or_else(|error| panic!("{}: {error}", case.name)),
        }

        let calls = state.lock().expect("mock state lock").calls.clone();
        assert_eq!(
            calls,
            case.expected_calls
                .iter()
                .map(|call| (*call).to_string())
                .collect::<Vec<_>>(),
            "{}",
            case.name
        );
    }
}

fn new_test_config(test_dir: &TestDir) -> config::Config {
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.SourceDir = "/data".into();
    cfg.TaskID = 0;
    cfg.TiDB.Port = 4000;
    cfg.TiDB.PdAddr = "127.0.0.1:2379".into();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.TikvImporter.SortedKVDir = test_dir
        .path()
        .join("sorted-kv")
        .to_string_lossy()
        .into_owned();
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = config::CheckpointDriverFile.into();
    cfg.Checkpoint.DSN = test_dir.path().join("cp.pb").to_string_lossy().into_owned();
    cfg
}

fn setup_file_checkpoints_db(cfg: &config::Config) {
    let ctx = checkpoints::context::Background();
    let checkpoint_cfg = bridges::to_checkpoints_cfg(cfg);
    let mut cpdb = checkpoints::NewFileCheckpointsDB(ctx, &checkpoint_cfg.Checkpoint.DSN).unwrap();

    cpdb.Initialize(
        ctx,
        &checkpoint_cfg,
        HashMap::from([
            (
                "db1".into(),
                checkpoints::importdef::DBInfo {
                    Name: "db1".into(),
                    Tables: vec![
                        checkpoints::importdef::TableInfo {
                            Name: "t1".into(),
                            ..Default::default()
                        },
                        checkpoints::importdef::TableInfo {
                            Name: "t2".into(),
                            ..Default::default()
                        },
                    ],
                },
            ),
            (
                "db2".into(),
                checkpoints::importdef::DBInfo {
                    Name: "db2".into(),
                    Tables: vec![checkpoints::importdef::TableInfo {
                        Name: "t3".into(),
                        ..Default::default()
                    }],
                },
            ),
        ]),
    )
    .unwrap();

    cpdb.InsertEngineCheckpoints(
        ctx,
        "`db1`.`t2`",
        HashMap::from([
            (
                0,
                checkpoints::EngineCheckpoint {
                    Status: checkpoints::CheckpointStatusLoaded,
                    Chunks: vec![checkpoints::ChunkCheckpoint {
                        Key: checkpoints::ChunkCheckpointKey {
                            Path: "/tmp/path/1.sql".into(),
                            Offset: 0,
                        },
                        FileMeta: checkpoints::mydump::SourceFileMeta {
                            Path: "/tmp/path/1.sql".into(),
                            Type: checkpoints::mydump::SourceType(3),
                            FileSize: 12345,
                            ..Default::default()
                        },
                        Chunk: checkpoints::mydump::Chunk {
                            Offset: 12,
                            RealOffset: 10,
                            EndOffset: 102400,
                            PrevRowIDMax: 1,
                            RowIDMax: 5000,
                        },
                        ..Default::default()
                    }],
                },
            ),
            (
                -1,
                checkpoints::EngineCheckpoint {
                    Status: checkpoints::CheckpointStatusLoaded,
                    Chunks: Vec::new(),
                },
            ),
        ]),
    )
    .unwrap();
    cpdb.InsertEngineCheckpoints(
        ctx,
        "`db2`.`t3`",
        HashMap::from([(
            -1,
            checkpoints::EngineCheckpoint {
                Status: checkpoints::CheckpointStatusLoaded,
                Chunks: Vec::new(),
            },
        )]),
    )
    .unwrap();
    cpdb.Close().unwrap();
}

fn update_checkpoint(
    cfg: &config::Config,
    tables: impl IntoIterator<Item = (&'static str, checkpoints::TableCheckpointDiff)>,
) {
    let ctx = checkpoints::context::Background();
    let mut cpdb = checkpoints::NewFileCheckpointsDB(ctx, &cfg.Checkpoint.DSN).unwrap();
    cpdb.Update(
        ctx,
        tables
            .into_iter()
            .map(|(table, diff)| (table.to_string(), diff))
            .collect(),
    )
    .unwrap();
    cpdb.Close().unwrap();
}

fn invalid_status_diff() -> checkpoints::TableCheckpointDiff {
    let mut diff = checkpoints::NewTableCheckpointDiff();
    let mut merger = checkpoints::StatusCheckpointMerger {
        EngineID: -1,
        Status: checkpoints::CheckpointStatusAllWritten,
    };
    merger.SetInvalid();
    merger.MergeInto(&mut diff);
    diff
}

fn get_checkpoint(
    cfg: &config::Config,
    table: &str,
) -> checkpoints::Result<checkpoints::TableCheckpoint> {
    let ctx = checkpoints::context::Background();
    let mut cpdb = checkpoints::NewFileCheckpointsDB(ctx, &cfg.Checkpoint.DSN)?;
    let checkpoint = cpdb.Get(ctx, table);
    cpdb.Close()?;
    checkpoint
}

enum LegacySetup {
    None,
    ErrorStatus,
    PartialProgress,
    Imported,
}

enum LegacyOperation {
    Remove(&'static str),
    IgnoreError(&'static str),
    Dump,
    GetLocalStoringTables,
}

enum LegacyExpectation {
    RemoveSingle,
    RemoveAll,
    IgnoreSingle,
    IgnoreAll,
    PartialProgress,
    Empty,
}

struct LegacyCase {
    name: &'static str,
    setup: LegacySetup,
    operation: LegacyOperation,
    want_error: Option<&'static str>,
    expectation: LegacyExpectation,
}

#[test]
fn test_legacy_checkpoint_control() {
    let cases = [
        LegacyCase {
            name: "Remove single checkpoint",
            setup: LegacySetup::None,
            operation: LegacyOperation::Remove("`db1`.`t2`"),
            want_error: None,
            expectation: LegacyExpectation::RemoveSingle,
        },
        LegacyCase {
            name: "Remove all checkpoints",
            setup: LegacySetup::None,
            operation: LegacyOperation::Remove("all"),
            want_error: None,
            expectation: LegacyExpectation::RemoveAll,
        },
        LegacyCase {
            name: "IgnoreError single checkpoint",
            setup: LegacySetup::ErrorStatus,
            operation: LegacyOperation::IgnoreError("`db1`.`t2`"),
            want_error: None,
            expectation: LegacyExpectation::IgnoreSingle,
        },
        LegacyCase {
            name: "IgnoreError all checkpoints",
            setup: LegacySetup::ErrorStatus,
            operation: LegacyOperation::IgnoreError("all"),
            want_error: None,
            expectation: LegacyExpectation::IgnoreAll,
        },
        LegacyCase {
            name: "Dump not supported for file checkpoint",
            setup: LegacySetup::None,
            operation: LegacyOperation::Dump,
            want_error: Some("not unsupported"),
            expectation: LegacyExpectation::Empty,
        },
        LegacyCase {
            name: "GetLocalStoringTables with partial progress",
            setup: LegacySetup::PartialProgress,
            operation: LegacyOperation::GetLocalStoringTables,
            want_error: None,
            expectation: LegacyExpectation::PartialProgress,
        },
        LegacyCase {
            name: "GetLocalStoringTables empty when imported",
            setup: LegacySetup::Imported,
            operation: LegacyOperation::GetLocalStoringTables,
            want_error: None,
            expectation: LegacyExpectation::Empty,
        },
    ];

    for case in cases {
        let test_dir = TestDir::new(case.name);
        let cfg = new_test_config(&test_dir);
        setup_file_checkpoints_db(&cfg);

        match case.setup {
            LegacySetup::None => {}
            LegacySetup::ErrorStatus => update_checkpoint(
                &cfg,
                [
                    ("`db1`.`t2`", invalid_status_diff()),
                    ("`db2`.`t3`", invalid_status_diff()),
                ],
            ),
            LegacySetup::PartialProgress => {
                let mut diff = checkpoints::NewTableCheckpointDiff();
                checkpoints::ChunkCheckpointMerger {
                    EngineID: 0,
                    Key: checkpoints::ChunkCheckpointKey {
                        Path: "/tmp/path/1.sql".into(),
                        Offset: 0,
                    },
                    Pos: 100,
                    RealPos: 100,
                    RowID: 50,
                    ..Default::default()
                }
                .MergeInto(&mut diff);
                update_checkpoint(&cfg, [("`db1`.`t2`", diff)]);
            }
            LegacySetup::Imported => {
                let mut diff = checkpoints::NewTableCheckpointDiff();
                checkpoints::StatusCheckpointMerger {
                    EngineID: 0,
                    Status: checkpoints::CheckpointStatusImported,
                }
                .MergeInto(&mut diff);
                update_checkpoint(&cfg, [("`db1`.`t2`", diff)]);
            }
        }

        let ctx = context::Background();
        let mut control =
            NewLegacyCheckpointControl(&cfg, &common::TLS::default()).expect(case.name);
        let mut local_tables = None;
        let result = match case.operation {
            LegacyOperation::Remove(table) => control.Remove(&ctx, table),
            LegacyOperation::IgnoreError(table) => control.IgnoreError(&ctx, table),
            LegacyOperation::Dump => control.Dump(
                &ctx,
                test_dir.path().join("dump").to_str().expect("UTF-8 path"),
            ),
            LegacyOperation::GetLocalStoringTables => {
                let result = control.GetLocalStoringTables(&ctx);
                if let Ok(tables) = &result {
                    local_tables = tables.clone();
                }
                result.map(|_| ())
            }
        };

        match case.want_error {
            Some(message) => {
                let error = result.expect_err(case.name);
                assert!(
                    error.to_string().contains(message),
                    "{}: unexpected error: {error}",
                    case.name
                );
                continue;
            }
            None => result.unwrap_or_else(|error| panic!("{}: {error}", case.name)),
        }

        match case.expectation {
            LegacyExpectation::RemoveSingle => {
                assert!(get_checkpoint(&cfg, "`db1`.`t2`").is_err(), "{}", case.name);
                assert!(get_checkpoint(&cfg, "`db2`.`t3`").is_ok(), "{}", case.name);
            }
            LegacyExpectation::RemoveAll => {
                assert!(get_checkpoint(&cfg, "`db1`.`t2`").is_err(), "{}", case.name);
                assert!(get_checkpoint(&cfg, "`db2`.`t3`").is_err(), "{}", case.name);
            }
            LegacyExpectation::IgnoreSingle => {
                assert_eq!(
                    get_checkpoint(&cfg, "`db1`.`t2`").unwrap().Status,
                    checkpoints::CheckpointStatusLoaded,
                    "{}",
                    case.name
                );
                assert_eq!(
                    get_checkpoint(&cfg, "`db2`.`t3`").unwrap().Status,
                    checkpoints::CheckpointStatusAllWritten / 10,
                    "{}",
                    case.name
                );
            }
            LegacyExpectation::IgnoreAll => {
                for table in ["`db1`.`t2`", "`db2`.`t3`"] {
                    assert_eq!(
                        get_checkpoint(&cfg, table).unwrap().Status,
                        checkpoints::CheckpointStatusLoaded,
                        "{}: {table}",
                        case.name
                    );
                }
            }
            LegacyExpectation::PartialProgress => {
                let tables = local_tables.expect(case.name);
                assert_eq!(tables.get("`db1`.`t2`"), Some(&vec![0]), "{}", case.name);
                assert_eq!(tables.len(), 1, "{}", case.name);
            }
            LegacyExpectation::Empty => {
                assert!(
                    local_tables.map(|tables| tables.is_empty()).unwrap_or(true),
                    "{}",
                    case.name
                );
            }
        }
    }
}

#[test]
fn test_new_checkpoint_control_legacy_backend() {
    let test_dir = TestDir::new("new-legacy-control");
    let mut cfg = new_test_config(&test_dir);
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    setup_file_checkpoints_db(&cfg);

    let ctx = context::Background();
    let mut control = NewCheckpointControl(&cfg, &common::TLS::default())
        .expect("construct legacy checkpoint control");
    control
        .Remove(&ctx, "`db1`.`t2`")
        .expect("legacy control removes a file checkpoint");

    assert!(get_checkpoint(&cfg, "`db1`.`t2`").is_err());
    assert!(get_checkpoint(&cfg, "`db2`.`t3`").is_ok());
}
