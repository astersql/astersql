# 任务 40: 提交 59bfe3daab 严格对齐 Rust

批次：【批次 40】；依赖批次：批次 39（顺序与共享资源依赖，不把 Git 父子顺序视为业务依赖）

状态：已完成，待回归

目的：COMPARE MATERIALIZED VIEW 真实计划/执行、SEM/权限、输出与 DECIMAL 值相等；不吞并后续列助手。

来源：Go commit `59bfe3daabe4af4088631bb5094d0a67f204f613`；父提交 `1c0793ea5765e203868fbed4c0840541cabb46d2`；标题：executor, planner: support materialized view comparison (#72472)

预计会话范围：用户要求一来源 commit 一任务，共 15 个来源路径；大提交在本任务内按行为分段，跨上下文维护阶段证据，不拆来源或吞并其他提交。

## 提交对齐范围硬约束

只负责本来源 diff、不可缺少的最小局部接线和原测试意图。先核对现有 Rust，等价实现只取证；每项生产修改必须映射到本任务具体 Go hunk 或不可缺少的局部接线依据。禁止递归补建完整子系统、通用基础设施、其他 commit；无关缺口仅列范围外建议，不阻塞已被证明的本增量。不得用桩、空样例、编译通过、零测试或删减逻辑代替完成。保留条件顺序、错误、数据形状、状态、事务、并发、副作用和原测试分支。后续替代必须记录完整 SHA、源码去向与保留行为，不恢复已撤销逻辑。

保留 PostgreSQL 的 SqlDialect::PostgreSQL、$n 参数、Parse/Bind/Execute/Sync、portal 生命周期、SQLSTATE、OID/二进制编码、search_path、事务失败状态和类型语义。Go 的 MySQL 专用规则只在既有方言边界内对齐，不能覆盖 PostgreSQL 的 NULL 顺序、强制转换、标识符或系统变量逻辑。

外部 Rust 依赖在独立上游仓库移植、提交并打 tag；本仓库只统一引用已发布 Git tag，不得 vendor/third_party/复制源码/[patch] 本地覆盖。清理本任务临时副本。Rust 测试放独立文件；保留 PingCAP Apache License，真正可用后添加 // Copyright 2026 AsterSQL.。不得覆盖、暂存或回滚其他未提交工作，尤其 PostgreSQL 当前功能任务。

## 文件

Git 核验的全部来源变更路径（R 行保留旧/新路径，含构建、生成、文档及测试）：

    M	docs/agents/README.md
    D	docs/agents/executor/fullouter_join_dev_note.md
    M	pkg/executor/BUILD.bazel
    M	pkg/executor/builder.go
    M	pkg/executor/internal/util/touched_rows.go
    A	pkg/executor/mview_compare.go
    M	pkg/executor/mviewdeltamergeagg/BUILD.bazel
    M	pkg/executor/mviewdeltamergeagg/exec_test.go
    M	pkg/executor/test/materializedview/BUILD.bazel
    A	pkg/executor/test/materializedview/materialized_view_compare_test.go
    M	pkg/planner/core/common_plans.go
    M	pkg/planner/core/planbuilder.go
    M	pkg/planner/core/preprocess.go
    A	tests/integrationtest/r/executor/mview_compare.result
    A	tests/integrationtest/t/executor/mview_compare.test

Rust 候选不等于必须修改；缺失路径明确为执行期计划新增：

- `pkg/executor/builder.rs`：M，现有。依据：pkg/executor/builder.go。
- `pkg/executor/mviewdeltamergeagg_test.rs`：排除，现有独立测试，优先扩展。依据：新增 port_59bfe3daab_ 前缀的本提交行为回归；优先扩展已有测试。
- `pkg/executor/internal/util/migration_aster_unit_test.rs`：排除，现有独立测试，优先扩展。依据：新增 port_59bfe3daab_ 前缀的本提交行为回归；优先扩展已有测试。
- `pkg/executor/internal/util/lib.rs`：排除，现有。依据：仅在新增模块或测试未挂载时做必要装配；现有测试已挂载则不变，AST 根含业务语义需同步说明。
- `pkg/executor/internal/util/touched_rows.rs`：A，计划新增或复用前序任务建立的同路径。依据：pkg/executor/internal/util/touched_rows.go。
- `pkg/executor/lib.rs`：排除，现有。依据：仅在新增模块或测试未挂载时做必要装配；现有测试已挂载则不变，AST 根含业务语义需同步说明。
- `pkg/executor/mview_compare.rs`：A，计划新增或复用前序任务建立的同路径。依据：pkg/executor/mview_compare.go。
- `pkg/planner/core/common_plans_test.rs`：排除，现有独立测试，优先扩展。依据：新增 port_59bfe3daab_ 前缀的本提交行为回归；优先扩展已有测试。
- `pkg/planner/core/common_plans.rs`：M，现有。依据：pkg/planner/core/common_plans.go。
- `pkg/planner/core/lib.rs`：M（含业务逻辑，修改才同步说明），现有。依据：仅在新增模块或测试未挂载时做必要装配；现有测试已挂载则不变，AST 根含业务语义需同步说明。
- `pkg/planner/core/planbuilder.rs`：M，现有。依据：pkg/planner/core/planbuilder.go。
- `pkg/planner/core/preprocess.rs`：M，现有。依据：pkg/planner/core/preprocess.go。

Cargo 所有者（只在证明必要时改依赖）：

- `pkg/executor/Cargo.toml`，package `astersql-executor`，已核实存在。
- `pkg/executor/internal/util/Cargo.toml`，package `astersql-executor-internal-util`，已核实存在。
- `pkg/planner/core/Cargo.toml`，package `astersql-planner-core`，已核实存在。
- `Cargo.toml`、`Cargo.lock` 为共享元数据候选；`docs/docs-manifest.md` 仅按下列净变化维护。

## 上下文

COMPARE MATERIALIZED VIEW 真实计划/执行、SEM/权限、输出与 DECIMAL 值相等；不吞并后续列助手。 按串行批次保护共享写入、格式化/lint 和提交窗口，不因 Cargo 分槽而并行。

后续 `fba22fa599b70bb8e688c8ca4fb6c287fd1a4d53` 在 touched_rows.go 恢复扩展列助手。本提交保留 COMPARE 与 DECIMAL 值相等，不吞并后续 float/bit/string/enum/set 变化。

## Go 变更定位清单

所有 hunk 都需在覆盖记录中有结论；行号属于本来源快照。新增函数列出补充入口，不以文件名替代行为阅读。

### `docs/agents/README.md`

    @@ -14 +13,0 @@ This directory is the single top-level home for agent-facing documentation.

### `docs/agents/executor/fullouter_join_dev_note.md`

    @@ -1,240 +0,0 @@

### `pkg/executor/BUILD.bazel`

    @@ -55,0 +56 @@ go_library(

### `pkg/executor/builder.go`

    @@ -222,0 +223,2 @@ func (b *executorBuilder) build(p base.Plan) exec.Executor {
    @@ -6714,0 +6717,7 @@ func (b *executorBuilder) buildWorkloadRepoCreate(_ *plannercore.WorkloadRepoCre

### `pkg/executor/internal/util/touched_rows.go`

    @@ -27,2 +27,2 @@ import (
    @@ -58,0 +59,4 @@ func MarkTouchedRowsByColumn(

### `pkg/executor/mview_compare.go`

    @@ -0,0 +1,996 @@

### `pkg/executor/mviewdeltamergeagg/BUILD.bazel`

    @@ -46 +46 @@ go_test(

### `pkg/executor/mviewdeltamergeagg/exec_test.go`

    @@ -1952,0 +1953,51 @@ func TestMarkUpdateTouchedRowsByColumnStringCollation(t *testing.T) {

### `pkg/executor/test/materializedview/BUILD.bazel`

    @@ -7,0 +8 @@ go_test(
    @@ -16 +17 @@ go_test(
    @@ -31,0 +33 @@ go_test(

### `pkg/executor/test/materializedview/materialized_view_compare_test.go`

    @@ -0,0 +1,456 @@

### `pkg/planner/core/common_plans.go`

    @@ -1980,0 +1981,7 @@ type AdminShowBDRRole struct {

### `pkg/planner/core/planbuilder.go`

    @@ -664,0 +665,2 @@ func (b *PlanBuilder) Build(ctx context.Context, node *resolve.NodeW) (base.Plan
    @@ -6844,0 +6847,57 @@ func (b *PlanBuilder) checkSEMStmt(stmt ast.Node) error {

### `pkg/planner/core/preprocess.go`

    @@ -349,0 +350,3 @@ func (p *preprocessor) Enter(in ast.Node) bool {
    @@ -688 +691 @@ func (p *preprocessor) Leave(in ast.Node) bool {

### `tests/integrationtest/r/executor/mview_compare.result`

    @@ -0,0 +1,102 @@

### `tests/integrationtest/t/executor/mview_compare.test`

    @@ -0,0 +1,124 @@

新增/修改声明入口：`Close`、`Next`、`Open`、`TestCompareMaterializedView`、`TestMarkUpdateTouchedRowsByColumnDecimalValueComparison`、`buildCompareMaterializedView`、`buildCompareMaterializedViewDiffLayout`、`buildCompareMaterializedViewJoinSchema`、`buildCompareMaterializedViewKeyTypes`、`buildCompareMaterializedViewNullEQFlags`、`buildCompareMaterializedViewOrdinalSlice`、`buildCompareMaterializedViewOutputCreateTableSQL`、`buildCompareMaterializedViewRecordSetSchema`、`buildCompareMaterializedViewSelectSQL`、`checkCompareMaterializedViewOutputTableNotExists`、`chooseCompareMaterializedViewDifferTypeColumnName`、`classifyCompareMaterializedViewRowDiff`、`cleanupCompareMaterializedViewOutputTable`、`commit`、`ensureTxn`、`flushBatch`、`formatCompareMaterializedViewSummary`、`newCompareMaterializedViewHashJoinExec`、`nextCompareTimestamp`、`openCompareMaterializedViewOutputWriter`、`openCompareMaterializedViewQueryExec`、`openCompareMaterializedViewSnapshotSession`、`precheckCompareMaterializedViewCurrentState`、`prepareCompareMaterializedViewRowIdxes`、`prepareCompareMaterializedViewSnapshotSession`、`readCompareMaterializedViewLastSuccessReadTSO`、`resolveCompareMaterializedViewOutputWriterBatchBytes`、`resolveCompareMaterializedViewSnapshotTS`、`resolveCompareMaterializedViewTarget`、`rollback`、`setCompareMaterializedViewSnapshot`、`testCompareMaterializedViewDropsOutputTableOnWriteFailure`、`testCompareMaterializedViewEmptyInputs`、`testCompareMaterializedViewNullableGroupKeyMinMax`、`testCompareMaterializedViewOutputTimestampUsesDefinitionTimeZone`、`testCompareMaterializedViewOutputWriterBatchesByTxnSize`、`testCompareMaterializedViewPrivilegeCheckBeforeSnapshotResolution`、`testCompareMaterializedViewPrivilegeSkeleton`、`testCompareMaterializedViewRejectsReadTSOAfterSnapshot`、`testCompareMaterializedViewSummaryAndOutput`、`testCompareMaterializedViewUsesDefinitionTimeZone`、`testCompareMaterializedViewUsesSnapshotMetadataAfterOutOfPlaceCutover`、`writeRow`。

## Rust 说明文档同步清单

| Rust 源文件 | 动作 | 同目录说明动作 | manifest 动作 | 分类依据 |
| --- | --- | --- | --- | --- |
| pkg/executor/builder.rs | 不变 | 不变 | 不变 | 复用已有 typed compiler；SQL 分派的必要局部接线在 session/runtime/dispatch.rs |
| pkg/executor/mviewdeltamergeagg_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| pkg/executor/internal/util/migration_aster_unit_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| pkg/executor/internal/util/lib.rs | 排除 | 不创建/不更新 | 不变 | 仅模块导出/测试挂载的骨架 |
| pkg/executor/internal/util/touched_rows.rs | 不变（不新建） | 不创建 | 不变 | DECIMAL 复用 Datum Compare，新的 CompareLayout 调用数值相等；无 internal-util 生产差异 |
| pkg/executor/lib.rs | 排除 | 不创建/不更新 | 不变 | 仅模块导出/测试挂载的骨架 |
| pkg/executor/mview_compare.rs | A | A（新增缺失说明） `pkg/executor/mview_compare.rs.md` | 新增唯一链接，净 +1 | 实际业务逻辑，仅实际改动时同步；等价则记不变 |
| pkg/planner/core/common_plans_test.rs | 排除 | 不创建/不更新 | 不变 | 独立测试 |
| pkg/planner/core/common_plans.rs | M | M `pkg/planner/core/common_plans.rs.md` | 已有条目保持分组/排序/勾选，净 0 | 实际业务逻辑，仅实际改动时同步；等价则记不变 |
| pkg/planner/core/lib.rs | 不变 | 不变 | 不变 | 已有 common_plans 重新导出无需修改 |
| pkg/planner/core/planbuilder.rs | M | M `pkg/planner/core/planbuilder.rs.md` | 已有条目保持分组/排序/勾选，净 0 | 实际业务逻辑，仅实际改动时同步；等价则记不变 |
| pkg/planner/core/preprocess.rs | M | M `pkg/planner/core/preprocess.rs.md` | 已有条目保持分组/排序/勾选，净 0 | 实际业务逻辑，仅实际改动时同步；等价则记不变 |

实际新增局部接线候选：

| Rust 源文件 | 动作 | 同目录说明动作 | manifest 动作 | 分类依据 |
| --- | --- | --- | --- | --- |
| pkg/session/runtime/mview_compare.rs | A | A | 唯一新增链接，净 +1 | Go mview_compare.go 的真实 session/snapshot/transaction 接线 |
| pkg/session/runtime/mview_source.rs | M | M | 净0 | 复用 typed 字面量编码与指定 snapshot schema 的 definition/BuildCompleteDiffSource |
| pkg/session/runtime/dispatch.rs | M | M | 净0 | Go builder.go 的 canonical SQL分派对应 |
| pkg/session/runtime.rs | M | 排除 | 不变 | 纯模块/独立测试挂载 |
| pkg/session/runtime/mview_compare_test.rs | A | 排除 | 不变 | 独立真实SQL回归 |
| pkg/session/runtime/mview_refresh_test.rs | M | 排除 | 不变 | 仅4个既有fixture helper的pub(super)可见性 |

每个候选恰好一行；实际不变改记不变且不制造说明 diff。已有 manifest 条目但说明缺失只补说明不加计数；说明已存在但无条目只补唯一条目。若本提交确需 D/R，先补具体旧/新路径及 hunk 依据：D 同删说明/链接并净减一，R 同步说明/链接并保留勾选和总数；不扩大到其他来源。章节至少包含：文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南；明确已接线、仅测试可见及 Go 独有。 `.rs.md` 不需要“## 验证依据”章节，文档完整性检查不要求该章节；任务的测试与验证记录要求仍保留。

## Cargo 共享槽位规则

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 测试计划

COMPARE MATERIALIZED VIEW 真实计划/执行、SEM/权限、输出与 DECIMAL 值相等；不吞并后续列助手。

原 Go 测试入口（必须逐个读输入、输出、错误与副作用，保留全部原测试意图）：

- `pkg/executor/mviewdeltamergeagg/exec_test.go`：`TestMarkUpdateTouchedRowsByColumnDecimalValueComparison`。
- `pkg/executor/test/materializedview/materialized_view_compare_test.go`：`TestCompareMaterializedView`。
- SQL 契约 `tests/integrationtest/r/executor/mview_compare.result`：来源变化的输入与预期输出逐项映射真实 Rust 执行入口。
- SQL 契约 `tests/integrationtest/t/executor/mview_compare.test`：来源变化的输入与预期输出逐项映射真实 Rust 执行入口。

独立 Rust 测试安排（`port_59bfe3daab_` 是明确计划新增前缀，未建测试时不能以零测试通过）：

- `pkg/executor/mviewdeltamergeagg_test.rs`：扩展该文件，新增 `port_59bfe3daab_` 前缀用例覆盖 `astersql-executor` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `pkg/executor/Cargo.toml` 的 lib 测试入口。
- `pkg/executor/internal/util/migration_aster_unit_test.rs`：扩展该文件，新增 `port_59bfe3daab_` 前缀用例覆盖 `astersql-executor-internal-util` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `pkg/executor/internal/util/Cargo.toml` 的 lib 测试入口。
- `pkg/planner/core/common_plans_test.rs`：扩展该文件，新增 `port_59bfe3daab_` 前缀用例覆盖 `astersql-planner-core` 所有本提交生产差异；已有等价测试可复用并将其精确名称/命令写入覆盖记录，不为无生产影响 crate 强造逻辑。确认测试挂入 `pkg/planner/core/Cargo.toml` 的 lib 测试入口。

红灯：先写调用真实入口的独立用例，用下列同一命令取得修复前行为断言失败；新增 API 可先最小接线，编译失败不算红灯。绿灯：修复后重跑，所有断言通过且有效测试数大于 0。已等价时取当前绿灯与源码证据，不人为回退。只 mock 外部 PD/S3/网络/时钟边界，真实序列化、事务、回滚不能换成桩。

## 步骤

1. 读 plan.md、本任务、当前仓库规则和最近 Rust 实现/测试，核对来源命令、未提交修改和 PostgreSQL 基线。
2. 对每个 hunk 标注已有等价/需修复/后续替代/Go 专用，并建立独立回归的红灯或当前绿灯。
3. 仅实现本增量及必要局部接线；每项生产修改填写具体 Go diff 依据，保留 PostgreSQL 分支。
4. Rust 改动先 cargo fmt --all，再聚焦验证/Ready，逐项同步合格说明及 manifest 净变化。
5. 自审差异和覆盖记录；满足门槛后删除本任务文件并独立提交。中断保留证据，清理本任务锁和进程。

## 验证

命令从仓库根执行。确切来源命令：

    git show -s --format=fuller 59bfe3daabe4af4088631bb5094d0a67f204f613
    git diff --name-status -M 1c0793ea5765e203868fbed4c0840541cabb46d2 59bfe3daabe4af4088631bb5094d0a67f204f613
    git diff --no-ext-diff --unified=40 1c0793ea5765e203868fbed4c0840541cabb46d2 59bfe3daabe4af4088631bb5094d0a67f204f613

取得槽位后，计划新增前缀的聚焦红/绿命令（若复用现有等价测试，在本任务记精确替代过滤条件与证据；不得全包/零测试替代）：

    cargo test --manifest-path pkg/executor/Cargo.toml -p astersql-executor --lib port_59bfe3daab_ -- --test-threads=1
    cargo test --manifest-path pkg/executor/internal/util/Cargo.toml -p astersql-executor-internal-util --lib port_59bfe3daab_ -- --test-threads=1
    cargo test --manifest-path pkg/planner/core/Cargo.toml -p astersql-planner-core --lib port_59bfe3daab_ -- --test-threads=1

代码 Ready：

    cargo fmt --all
    cargo fmt --all -- --check
    make lint
    git diff --check

生产接线/依赖变化再验证实际 NextGen 二进制（不代替行为测试）：

    cargo build --manifest-path cmd/tidb-server/Cargo.toml -p astersql-cmd-tidb-server --bin astersql-cmd-tidb-server --features nextgen

PostgreSQL 保护：相关共享路径修改前后运行并比较基线；纯 Go 栈格式/注释且 Rust 不变可以明确记录不适用。不能借已有失败隐藏新回归。

    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_query_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_extended_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_transaction_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_types_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_session_test:: -- --test-threads=1
    cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib pg_client_integration_test::pg_core_libpq_insert_readback -- --test-threads=1

客户端依赖 libpq 和 Python，缺失/失败如实报告；不得 ignored 或协议替身代替。类型/时间/函数变化另补相应现有 pg_* 测试边界，不混入其他任务未提交功能。

Go 对照按 docs/agents/testing-flow.md 决定 failpoint、记录和 RealTiKV。执行期新增/移动 Go、改 import/测试/依赖/Bazel 元数据或出现工具链错误，构建前先 make bazel_prepare。failpoint 必须启用并清理；集成记录用 run-tests.sh -r；RealTiKV 后台 playground，成功/失败/中断清理进程和本任务数据。禁止默认 make bazel_lint_changed。

文档检查：逐个实际 A/M/R 说明执行 test -f，验证上述全部章节；核查 docs/docs-manifest.md 的本任务链接存在、条目唯一、分组排序和勾选未扰动、总数按净量变化。D/R 还要确认旧说明/旧链接不残留。范围外缺失不补齐；文档验证不替代行为验证。

成功信号：检查退出码 0、相关测试数 > 0、无新失败；所有 hunk 有实际入口/断言或有据不适用结论；接线变化通过 NextGen 编译；文档同步一致。报告槽位、绝对 CARGO_TARGET_DIR、命令/退出码/数量、未验证项，以及正确性/兼容性/性能风险。


文档精确校验命令（只验证本任务实际修改的候选；不为范围外既有缺失扩展工作）：

    python3 - <<'DOC_CHECK'
    from pathlib import Path
    import subprocess, re
    candidates = ['pkg/executor/builder.rs', 'pkg/executor/internal/util/touched_rows.rs', 'pkg/executor/mview_compare.rs', 'pkg/planner/core/common_plans.rs', 'pkg/planner/core/lib.rs', 'pkg/planner/core/planbuilder.rs', 'pkg/planner/core/preprocess.rs', 'pkg/session/runtime/mview_compare.rs', 'pkg/session/runtime/dispatch.rs', 'pkg/session/runtime/mview_source.rs']
    modified = set(subprocess.check_output(['git','diff','--name-only','HEAD'], text=True).splitlines())
    modified.update(subprocess.check_output(['git','ls-files','--others','--exclude-standard'], text=True).splitlines())
    index = Path('docs/docs-manifest.md').read_text()
    old = subprocess.check_output(['git','show','HEAD:docs/docs-manifest.md'], text=True)
    links = re.findall(r'\]\(([^)]+)\)', index)
    oldlinks = re.findall(r'\]\(([^)]+)\)', old)
    checked = 0
    for source in candidates:
        if source not in modified:
            continue
        doc = Path(source + '.md')
        link = '../' + str(doc)
        if not Path(source).exists():
            assert not doc.exists() and link not in links, source
            continue
        assert doc.is_file(), doc
        body = doc.read_text()
        for heading in ['文件定位','核心职责','主要符号','执行流程','数据与状态','依赖与调用关系','错误处理与边界','并发与资源生命周期','与 Go 版本的对应关系','扩展指南']:
            assert re.search(r'^##+ .*' + re.escape(heading), body, re.M), (doc, heading)
        assert links.count(link) == 1, (doc, '清单链接不唯一或缺失')
        checked += 1
    before = int(re.search(r'计划为 (\d+) 个', old)[1])
    after = int(re.search(r'计划为 (\d+) 个', index)[1])
    oldset = {x for x in oldlinks if x.endswith('.rs.md')}
    newset = {x for x in links if x.endswith('.rs.md')}
    assert after - before == len(newset - oldset) - len(oldset - newset), '清单计数净变化不一致'
    print('本任务已检查说明:', checked, '；无源码变化时 0 仅表示不适用，不是行为测试证据')
    DOC_CHECK

同时人工审查 `git diff -- docs/docs-manifest.md` 的分组排序、原勾选状态，以及实际重命名的旧路径清理；若执行时新增候选，先更新上面的具体候选列表。

## 完成

真实行为测试、逐项覆盖证据、适用 Ready、PostgreSQL 保护和 Rust 说明同步齐全后才标记 `已完成`。Rust 已等价时取得当前绿灯，不人为回退。Go 既有回归失败按 AGENTS.md 记录后可不阻塞；此豁免不适用于 Rust 行为未证明、零测试或未接线。范围外待办不继续阻塞已对齐的本提交。

完成后必须删除本编号任务文件，将本任务代码、独立测试、.rs.md、docs/docs-manifest.md、必要 manifest/lock 与任务文件删除整理成一个独立 Git commit。使用 ./skills/git-commit/SKILL.md，逐路径暂存，执行 git diff --cached --check 和 git diff --cached 核对没有其他任务或用户修改，再提交；Git commit 是必须收尾。即使无需 Rust 修改，也提交任务文件删除。删除前将覆盖证据摘要放入提交正文以保留记录。不修改只读 plan.md，不自行 push、amend 或 rebase。阻塞则在本任务记录 `已阻塞` 与具体原因，不提交、不删除。

## Go 到 Rust 覆盖记录

按每个 hunk 补齐以下记录；同一路径多个 hunk 要逐个展开，不以初始未开始冒充证据。

| Go 行为/差异入口 | Rust 真实入口候选 | 已有/修复/后续完整 SHA | 输入输出、命令、退出码、数量 |
| --- | --- | --- | --- |
| docs/agents/README.md @@ -14 +13,0 | 无运行行为 | Go 专用资料目录列表删除 | 来源diff已核对；无Rust改动 |
| docs/agents/executor/fullouter_join_dev_note.md @@ -1,240 +0,0 | 无运行行为 | Go 专用开发说明删除 | 不恢复删除的开发笔记；无Rust改动 |
| pkg/executor/BUILD.bazel @@ -55,0 +56 | executor/lib.rs、session/runtime.rs | 新增Rust模块挂载 | 对应Go新增文件srcs；Cargo依赖不变 |
| pkg/executor/builder.go @@ -222,0 +223,2 | session/runtime/dispatch.rs CompareStmt分派 | 已修复，必要canonical局部接线 | session 8例真实SQL入口；退出0 |
| pkg/executor/builder.go @@ -6714,0 +6717,7 | session/runtime/mview_compare.rs execute_compare_sql | 已修复；builder.rs已有typed compiler复用不变 | 真实typed full outer join；session 8例退出0 |
| pkg/executor/internal/util/touched_rows.go @@ -27,2 +27,2 | executor/mview_compare.rs payload_equal | 注释意图：DECIMAL数值、字符串二进制 | 不吞并后续float/bit/string助手 |
| pkg/executor/internal/util/touched_rows.go @@ -58,0 +59,4 | executor/mview_compare.rs payload_equal；types/datum.rs Equals | 已复用Decimal Compare，新增分类入口显式忽略decimal内部padding | executor port_59bfe3daab_：2 passed，退出0；8对原输入bitmap=0,0,0,0,1,0,1,1 |
| pkg/executor/mview_compare.go @@ -0,0 +1,996 | executor/mview_compare.rs + session/runtime/mview_compare.rs | 已修复：当前/历史权限、快照schema/watermark、两TS查询、typed join、差异分类、摘要/输出、分批事务与失败清理 | session 8 passed + executor 2 passed，退出0；最终复验已通过 |
| pkg/executor/mviewdeltamergeagg/BUILD.bazel @@ -46 +46 | 已有mviewdeltamergeagg_test挂载 | Go shard_count 33->34 | Rust2个聚焦测试，非零；无Go/Bazel改动 |
| pkg/executor/mviewdeltamergeagg/exec_test.go @@ -1952,0 +1953,51 | mviewdeltamergeagg_test.rs decimal_value_comparison_preserves_null_and_padding | 已覆盖原compact/padded双向、同值、异值、NULL双向 | executor 2 passed，bitmap精确断言 |
| pkg/executor/test/materializedview/BUILD.bazel @@ -7,0 +8 | session/runtime.rs测试挂载 | Go新测试文件srcs对应独立Rust测试 | session 8 passed |
| pkg/executor/test/materializedview/BUILD.bazel @@ -16 +17 | 同上 | Go shard_count 47->48，无Rust运行语义 | 不修改Bazel |
| pkg/executor/test/materializedview/BUILD.bazel @@ -31,0 +33 | 同上 | Go sessionctx测试依赖；复用既有Rust session | 不新增依赖 |
| pkg/executor/test/materializedview/materialized_view_compare_test.go @@ -0,0 +1,456 | session/runtime/mview_compare_test.rs 8个独立用例 | 原11子项按行为合并而不删分支：权限/先检查、摘要输出、历史cutover、批字节/清理、定义时区/输出时间、nullable minmax、empty、future TSO | 8 passed，0 failed/ignored；精确摘要断言已补；真实事务/DDL/KV |
| pkg/planner/core/common_plans.go @@ -1980,0 +1981,7 | common_plans.rs CompareMaterializedView | 已修复，持有AST与schema | planner port_59bfe3daab_：1 passed，退出0 |
| pkg/planner/core/planbuilder.go @@ -664,0 +665,2 | planbuilder.rs Statement/BuiltPlan/Build | 已修复，分派到buildCompareMaterializedView | planner1 + session8，退出0 |
| pkg/planner/core/planbuilder.go @@ -6844,0 +6847,57 | planbuilder.rs buildCompareMaterializedView；既有dispatch SEM | 已修复，数据库/权限顺序和有无结果schema；SEM复用既有IsRestrictedSQL分类 | planner1真实builder + session权限矩阵，退出0 |
| pkg/planner/core/preprocess.go @@ -349,0 +350,3 | preprocess.rs Enter；session target当前/历史元数据分离 | 已修复；compatibility visitor仅测试可见，canonical实际目标解析独立于历史读 | planner1 + session历史cutover/当前权限，退出0 |
| pkg/planner/core/preprocess.go @@ -688 +691 | preprocess.rs Leave | 已修复，离开时清除flag | planner Enter/Leave通过，退出0 |
| tests/integrationtest/t/executor/mview_compare.test @@ -0,0 +1,124 | mview_compare_test.rs真实SQL | 无差异、水位后写入、FAST/OOP、输出列名/无索引、权限和目标已存在、wrong object意图 | Rust实际入口覆盖；FAST/OOP无差异及1347普通表错误补强复验通过；Go录制未运行 |
| tests/integrationtest/r/executor/mview_compare.result @@ -0,0 +1,102 | 同上；common_plans_test.rs结果schema | 精确摘要/差异3类、结果列名与输出DDL属性、权限错误文本 | session8 + planner1，退出0；无Go golden改动 |

## 范围外建议

不规划本提交以外的实现。发现历史缺口/其他提交/通用基础设施问题，只写具体路径、证据与建议归属；不扩写、不擅自回滚既有工作。

## Progress（进度）

- [x] 核对来源、已有实现与 PostgreSQL 基线。
- [x] 补齐每个增量的红绿/等价/替代覆盖。
- [x] 完成适用 Ready、文档同步与自审。
- [x] 独立 Git commit 收尾；按最新授权保留编号文件供既有 PostgreSQL 失败统一回归。

## Surprises & Discoveries（发现）

规划未运行行为测试；缺失候选是计划新增，不代表已实现或已接线。Ready skill 路径缺失和 Bazel 不可用需执行时复查。

## Decision Log（决策）

2026-10-30：一来源一任务，严格局部对齐；串行保护共享源码、说明、manifest、fmt/lint 和 Git index；独立回归按来源前缀聚焦，不能减少原测试分支。

## Outcomes & Retrospective（结果）

已完成来源增量、真实行为与说明同步，并准备独立提交。有效聚焦12个全部通过，Ready适用fmt/check、lint、NextGen、文档/manifest与diff检查通过。六组PG共294通过/45失败，失败名称及规范化错误正文与修改前完全一致；依2026-11-01最新授权标记“已完成，待回归”并保留本编号文件，不因范围外回归扩大来源。


## 2026-11-01 执行前置核查（已阻塞）

- 已读取 `skills/do-task-plans/SKILL.md`、只读 `plan.md`、本任务、`skills/rustcodegraph/SKILL.md` 和 `PLANS.md` 开头的执行要求。未修改 plan.md。
- 依赖批次 39 已有独立提交 `af58c5f14a`，正文记录来源 `1c0793ea5765e203868fbed4c0840541cabb46d2`、聚焦验证和 Ready 通过，以及 PostgreSQL 既有失败集合；未据此声称本任务通过。
- `git show -s --format=fuller 59bfe3daabe4af4088631bb5094d0a67f204f613` 与 `git diff --name-status -M 1c0793ea5765e203868fbed4c0840541cabb46d2 59bfe3daabe4af4088631bb5094d0a67f204f613` 均退出 0，来源 15 个路径与计划一致。完整 unified=40 差异尚未逐 hunk 阅读，覆盖表仍为未取证。
- RustCodeGraph `status` 退出 0，已有索引；`explore` / `query` / `node` 显示 COMPARE 有 Rust AST/解析，但查询未找到 Rust 计划或执行器。`rg -n 'compare.*materialized|CompareMaterialized|materialized.*compare' pkg --glob '*.rs'` 的命中仅见 parser 与 SEM 分类。`pkg/executor/mview_compare.rs`、`pkg/executor/internal/util/touched_rows.rs` 均不存在（`ls` 退出 1）；缺失生产实现属于本任务待完成内容，不是等价结论，也不单独作为范围外阻塞。
- 阻塞：AGENTS.md 强制交付使用 `.agents/skills/tidb-verify-profile` 的 Ready 流程，但 `.agents` 目录不存在；`ls .agents/skills/tidb-verify-profile/SKILL.md` 退出 1。追加搜索 `rg --files --hidden skills .github-bak /Users/xiangmin/.codex/skills 2>/dev/null | rg 'tidb-verify-profile|testing-flow.md$|agents-review-guide.md$'` 无结果；`docs/agents/testing-flow.md` 与 `docs/agents/agents-review-guide.md` 本身存在，未读取验证流程内容。需恢复所要求的 Ready skill 或明确替代流程，不能自称适用 Ready 达标。
- 按执行技能“遇到阻碍（缺少依赖、测试失败、指令不明确）”停止，尚未生产修改、测试修改、fmt、lint、Cargo、NextGen 编译或 PostgreSQL 基线验证。有效行为测试数量 0（未运行，非通过）。未领取 Cargo 槽位；CARGO_TARGET_DIR 未设置，无本任务锁或子进程需要释放。
- 工作区已有其他任务的源码/说明/任务删除，包括 `pkg/planner/core/planbuilder.rs`（核查时 +45）及其测试（+65）；未覆盖、暂存或回滚。Git index 核查为空。只修改本编号文件记录阻塞，不删除、不提交；全部实现与交付门槛仍待完成。
- 恢复后：读取 Ready skill 和测试流程，重新核查共享文件状态；逐 hunk 阅读来源、原 Go 测试与 SQL 契约，领取共享槽位后建立真实行为证据，执行最小局部实现、保护回归、说明同步及独立提交。

2026-11-01 恢复：用户明确授权以任务文件与 AGENTS.md 的 Ready 命令替代缺失 skill；缺失不再阻塞。继续真实行为实现与全部适用门槛。


## 2026-11-01 恢复实施证据（进行中）

- 读完来源 mview_compare.go、materialized_view_compare_test.go 和 SQL t/r 契约，已核对 builder、touched_rows、common_plans、planbuilder、preprocess、BUILD/文档差异。来源完整差异保存在 `/tmp/task40-source.diff`，仅作会话外临时证据，不提交。
- 修改前 PG 槽位 2，CARGO_TARGET_DIR=`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`。六组结果/退出码依序为 query 115/10/101、extended 157/34/101、transaction 2/0/0、types 16/1/101、session 3/0/0、libpq 1/0/0；日志 `/tmp/task40-baseline-pg*.log`。槽位已释放。
- 红灯槽位 1，同仓库绝对 `target/rust-slot-1`；`cargo fmt --all` 0，`cargo test --manifest-path pkg/session/Cargo.toml -p astersql-session --lib port_59bfe3daab_ -- --test-threads=1` 101，0 passed/1 failed，真实 SQL 报 statement requires the full planner/executor session ABI；不是编译失败。红灯时测试在 mview_refresh_test，后来移动到独立 mview_compare_test。
- 复用已有 typed HashJoin、snapshot compiler、SessionInitGuard 和真实事务。新增执行与会话文件，不补建新 join/事务/刷新子系统。touched_rows.rs 不新增：现有 Datum Decimal Compare 是可复用入口，新 CompareLayout 显式对 DECIMAL 做数值比较；独立 executor 测试保留原 8 对 compact/padded/NULL 输入。internal-util 无生产差异，不运行前缀零测试。
- 初版实现后真实会话绿灯：同一 session 命令 0，8 passed/0 failed/0 ignored（`/tmp/task40-green.log`）。包括 summary/3类输出/水位、权限顺序与权限组合、nullable MIN/MAX/空输入/差异列名碰撞、cutover历史表ID与索引隔离、未来水位拒绝、失败批次清理、定义时区过滤、TIMESTAMP输出、900行大文本/1MiB限制。executor与planner聚焦仍在运行，尚未完成最终 Ready。
- 中途一次 cutover 测试等待 DDL worker，检查已有同类测试后补 EnableMockDDLForTest；仅终止本任务测试进程，随后完整8例绿灯。独立测试复用同目录既有真实 fixture；没有 mock 序列化、事务或回滚。
- 生产映射：Go builder.go 两 hunk -> session dispatch 的 CompareStmt 分派与会话入口；builder.rs 复用不变。Go common_plans 新 hunk -> CompareMaterializedView AST/schema；Go planbuilder 两 hunk -> Statement/BuiltPlan分派和权限/schema builder；Go preprocess 两 hunk -> Enter/Leave flag，兼容visitor仅测试可见，canonical入口以当前/历史目标分开解析落地。Go mview_compare.go 新文件 -> executor/mview_compare.rs的typed分类/命名/批次上限与session/runtime/mview_compare.rs的真实快照/权限/查询/输出事务/清理，mview_source.rs只扩大已有datum_literal可见性。每一生产变化都在此映射内。
- 原 Go BUILD.bazel 的 1+1+3 hunks 是 Go 源/测试清单、shard_count与sessionctx测试依赖；Rust新增模块通过已有lib/runtime挂载，无Cargo依赖变化，不更改Go/Bazel。docs/agents/README.md 单hunk和fullouter_join_dev_note.md整文件删除只去掉开发资料，无Rust运行逻辑，不恢复删除文档。
- 原 exec_test.go DECIMAL单hunk由executor 8对输入测试映射；materialized_view_compare_test.go的11个子测试意图分布在8个会话测试（批量、失败清理、时区成功/失败复用同执行路径）；SQL t/r契约的无差异、刷新水位、列名冲突、无索引、权限/输出存在错误已映射，FAST后无差异仍需补入聚焦测试。未运行Go对照或Go集成录制。
- Rust说明同步：builder.rs、internal-util/lib.rs、touched_rows.rs、planner/core/lib.rs均不变；executor/lib.rs与session/runtime.rs仅模块挂载；新增2个生产说明executor/mview_compare.rs.md和session/runtime/mview_compare.rs.md，manifest净+2；更新common_plans/planbuilder/preprocess/dispatch/mview_source对应说明。`python3 /tmp/task40-doccheck.py`退出0，7个说明、10章节及唯一链接/净计数检查通过。

2026-11-01 Ready进度：executor聚焦2 passed、planner聚焦1 passed，均退出0；`make lint`退出0。最终fmt/check、NextGen与六组PG保护仍在进行，未作最终完成声明。

2026-11-01 自审局部收敛：新增definition_with_schema复用BuildCompleteDiffSource的布局校验；REFRESH原入口仍使用当前schema，COMPARE使用历史schema。该助手提取是本提交复用已有生产逻辑不可缺少的局部接线。源码修改后首先在动态槽位2执行cargo fmt --all（0），再复验session8与NextGen；PG受保护入口未增加PostgreSQL分支，原先query组115/10，后续组将使用最新源码编译。


## 最终验证与交付记录（2026-11-01）

验证 profile：Ready。缺失 `.agents/skills/tidb-verify-profile` 已按用户最新授权，以 AGENTS.md 与本任务明确命令替代，不新增技能或基础设施。最后一轮仅增加原SQL契约的1347错误断言，fmt先行；重跑8个会话测试及fmt-check均退出0。来源之外的PostgreSQL失败不扩写修复。

| 确切验证命令（仓库根目录） | 退出码 | 有效测试数/结果 |
| --- | --- | --- |
| `cargo fmt --all` | 0 | 格式化；非行为证据 |
| `cargo fmt --all -- --check` | 0 | 最后一轮退出0 |
| `cargo test --manifest-path pkg/session/Cargo.toml -p astersql-session --lib port_59bfe3daab_ -- --test-threads=1` | 0 | 8 passed，0 failed/ignored；最终含精确摘要、FAST/OOP、1347错误、1MiB/900行输出、时区、真实失败清理 |
| `cargo test --manifest-path pkg/executor/Cargo.toml -p astersql-executor --lib port_59bfe3daab_ -- --test-threads=1` | 0 | 2 passed，0 failed/ignored；原DECIMAL8对输入与差异分类/命名/批上限 |
| `cargo test --manifest-path pkg/planner/core/Cargo.toml -p astersql-planner-core --lib port_59bfe3daab_ -- --test-threads=1` | 0 | 1 passed，0 failed/ignored；数据库、权限、schema及preprocess进入/离开 |
| `cargo test --manifest-path pkg/util/sem/v2/Cargo.toml -p astersql-util-sem-v2 --lib port_af11710fe4_materialized_real_parser_sem_commands -- --test-threads=1` | 0 | 1 passed，0 failed/ignored；复用前序真实解析/SEM映射 |
| `cargo build --manifest-path cmd/tidb-server/Cargo.toml -p astersql-cmd-tidb-server --bin astersql-cmd-tidb-server --features nextgen` | 0 | 实际NextGen二进制；不作为行为测试替代 |
| `make lint` | 0 | 适用Ready lint |
| `git diff --check` | 0 | 全工作区通过 |
| `git diff --cached --check` | 0 | 本任务stage通过 |
| `python3 /tmp/task40-doccheck.py` | 0 | 7个生产说明、10章节、唯一链接，manifest净+2；脚本为本任务上述精确检查的候选收窄版本 |

PostgreSQL保护确切命令均为 `cargo test --manifest-path pkg/server/Cargo.toml -p astersql-server --lib <FILTER> -- --test-threads=1`，下表给出实际 FILTER。前/后各运行一轮，比较每一失败名称；移除进程线程ID与括号内ms耗时后，45个失败的错误正文也逐组完全相同，无新增、消失或错误内容变化。

| FILTER | 修改前 passed/failed | 修改后 passed/failed | 前/后退出码 |
| --- | --- | --- | --- |
| `pg_query_test::` | 115/10 | 115/10 | 101/101 |
| `pg_extended_test::` | 157/34 | 157/34 | 101/101 |
| `pg_transaction_test::` | 2/0 | 2/0 | 0/0 |
| `pg_types_test::` | 16/1 | 16/1 | 101/101 |
| `pg_session_test::` | 3/0 | 3/0 | 0/0 |
| `pg_client_integration_test::pg_core_libpq_insert_readback` | 1/0 | 1/0 | 0/0 |

记录文件：`/tmp/task40-baseline-pg*.log`、`/tmp/task40-ready-pg*.log`、`/tmp/task40-red.log`、`/tmp/task40-last-session.log`、`/tmp/task40-executor.log`、`/tmp/task40-planner.log`、`/tmp/task40-sem.log`、`/tmp/task40-nextgen-final.log`、`/tmp/task40-lint.log`。临时证据不纳入提交。

Cargo槽位实际动态领取：基线2；红/绿与Ready主阶段1；最后snapshot-helper复验/NextGen阶段2；SEM阶段3。绝对目录分别为 `/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`、`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2`、`/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-3`。每阶段设置对应CARGO_TARGET_DIR，记录会话和PID；阶段结束或本任务中断后仅释放自己的owner/lock，保留编译缓存。最终无本任务锁或后台子进程。

自审与隔离：只逐路径暂存本来源生产/独立测试/说明/manifest/本任务记录。共享planbuilder.rs、dispatch.rs及说明、mview_refresh_test.rs按hunk暂存；排除其他任务的SHOW实现、权限/分区/PG等工作。plan.md保持只读。无Cargo依赖变化，无外部临时依赖副本、vendor、本地patch或tag变动；Go与Bazel文件未改，本任务未触发bazel_prepare。

风险：正确性由原Go行为的真实入口回归与快照/事务/清理证明；兼容性仍有45个已证明不变的范围外PG既有失败，需统一回归；性能未做基准测试，复用typed full outer join的build侧内存模型和canonical逐行SQL写入桥。macOS NextGen链接有既有大型`__eh_frame` compact unwind警告，构建退出0。未本地验证：Go对照/录制、RealTiKV、性能/并发压力；未进行昂贵全包扫描或bazel_lint_changed。

最终决策：2026-11-01用户要求“实现与聚焦验证已完成、仅剩无关回归失败时按契约标记已完成，待回归”，优先于最初任务四状态/删除约定。故本次保留编号文件记录统一回归证据，代码、测试与说明独立提交，不标已阻塞，不吞并其他来源，不push/amend/rebase。
