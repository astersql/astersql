# `pkg/executor/check_table_index.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs` 以 `pub mod check_table_index` 公开该模块，`pkg/executor/Cargo.toml` 将库入口设为 `lib.rs`。文件直接依赖 `astersql-errors` 提供共享错误，以及 `astersql-util-chunk` 提供执行器 `Next` 接口使用的空结果批；其余表、索引、会话和查询能力都抽象成本文件内 trait，因而核心算法可以脱离具体存储实现测试。

它承载 `ADMIN CHECK TABLE` / `ADMIN CHECK INDEX` 的两套一致性检查算法：`CheckTableExec` 是逐索引回表并补做表侧检查的慢路径，`FastCheckTableExec` 是通过内部 SQL 的全局 checksum、递归分桶和最终逐行比对定位差异的快路径。两者都是“检查型”执行器，`Next` 不向传入的 `chunk::Chunk` 追加结果行，只以成功或错误表示检查结果。

当前 Rust 生产接线需要谨慎理解：`pkg/executor/builder.rs` 的 `buildCheckTable` 只根据 `fast_check_enabled()` 和 `indexes_support_fast_check()` 选择通用的 `ExecutorKind::FastCheckTable` 或 `ExecutorKind::CheckTable`，仓库内除本模块及其独立测试外，没有搜索到 `CheckTableExec` / `FastCheckTableExec` 的直接构造点。因此本文件提供了可注入运行时的算法实现，但“通用 kind 到本文件具体类型”的完整生产适配在现有直接证据中未验证，不能仅凭 Go 版本接线推定 Rust 已经走到这里。

## 核心职责

- 慢路径 `CheckTableExec`：打开所有 `IndexLookUpExecutor` 来源，先调用 `CheckTableRuntime::CheckIndicesCount` 比较表和普通二级索引计数，再根据计数方向检查缺失/悬空项；计数相等时最多用 3 个 scoped worker 消费各索引来源。多值索引还必须调用 `CheckRecordAndIndex` 从表侧复核。
- 快路径 `FastCheckTableExec`：临时允许不可见索引，最多并发 3 个 `checkIndexWorker`；每个 worker 获取独立系统会话，继承用户会话的资源/超时变量并可设置 snapshot，然后对单个索引执行 checksum 检查。
- 差异定位：先比较表侧与索引侧的 `(bit_xor(checksum), count)`；不一致时以 handle checksum 为分桶键，最多细化 9 轮或直到候选桶不超过 100 行，再拉取候选行并按 handle 归并比较。
- 安全边界：验证索引侧 SQL 的执行计划确实使用二级索引，转义 schema/table/column/index 标识符，恢复临时会话状态，把 worker panic 与锁中毒、字段类型、越界和算术溢出转成 `SharedError`。

## 主要符号

- 元数据和值对象：`IndexColumn`、`IndexInfo`、`TableMeta` 描述算法需要的最小索引/表信息；`SessionVars` 是快路径要覆盖并恢复的会话变量快照；`SqlValue`、`QueryRow`、`RecordData` 表示内部 SQL 结果和最终不一致记录；`groupByChecksum` 保存一个桶的编号、checksum 和计数。
- 慢路径接口：`IndexLookUpExecutor::{Open, Close, NextBatch, Index}` 封装索引回表来源；`CheckTableRuntime` 提供 base 打开、chunk 容量、计数比较、表侧复核和失败日志；`IndexCountComparison` 精确表达相等、索引更多或表更多及相关索引偏移。
- `CheckTableExec::{new, Open, Next, Close}` 是慢路径生命周期；`checkTableIndexHandle` 按不区分大小写的索引名覆盖分区产生的同名来源；`checkIndexHandle` 消费来源直到空批或取消；`checkTableRecord` 遍历逻辑表或所有物理分区；`index_at` / `source_index` 负责有检查的元数据访问；`handlePanic` 转换 worker panic。
- 快路径接口：`FastCheckSession` 抽象变量、SQL 执行和查询；`FastCheckRuntime` 抽象 base 打开、不可见索引开关、snapshot、系统会话池、分桶参数、记录解码、不一致报告和桶差异日志。
- `FastCheckTableExec::{new, Open, Next, createWorker}` 管理快路径任务；`InvisibleIndexGuard::drop` 保证退出时关闭不可见索引；`checkIndexWorker::{initSessCtx, quickPassGlobalChecksum, HandleTask, checkIndex, Close}` 完成单索引检查及会话归还。
- 辅助函数：`backupFastCheckSysSessionVars`、`applyFastCheckSysSessionVars`、`fastCheckSysSessionVarsBackup::restoreTo` 管理会话变量；`queryToRow`、`getCheckSum`、`getGlobalCheckSum` 读取查询结果；`verifyIndexSideQuery` 检查计划；`TableName`、`ColumnName`、`escapeName` 生成安全标识符；`firstDifferentBucket`、`logBucketDifferences`、`report_row_mismatch` 定位并报告差异；`panicError` 转换快路径 panic。

## 执行流程

慢路径从 `CheckTableExec::Open` 开始：先调用 `OpenBase`，再依次打开每个索引来源，最后清除退出标志和 `done`。第一次 `Next` 先把 `done` 置真，并在任何计数或扫描前拒绝 `condition.is_some()` 的部分索引；多值索引和列存索引不进入普通计数名称列表。`CheckIndicesCount` 相等时，单来源先直接消费一次；随后任务偏移进入 `VecDeque`，最多 3 个 scoped worker 调用 `checkIndexHandle`，多值索引完成索引侧消费后再调用 `checkTableRecord`。计数不相等时，`checkIndex=true`（`ADMIN CHECK INDEX` 兼容路径）立即报计数错误；否则索引更多就按同名来源消费，表更多就从表侧检查对应索引。首个 worker 错误或 panic 通过 channel 返回并让其他 worker 停止。后续 `Next` 为空操作；`Close` 设置退出标志并关闭全部来源，保留第一个关闭错误。

快路径从 `FastCheckTableExec::Open` 打开 base 并重置 `done`。第一次 `Next` 开启不可见索引，建立带 `Drop` 恢复的 guard，把每个索引偏移放入共享队列，并启动 `min(index_count, 3)` 个 scoped worker。每个 worker 的 `HandleTask` 验证 `BucketSize >= 2`，从池中取得系统会话，调用 `initSessCtx` 备份/应用会话变量并 best-effort 设置 snapshot，然后进入 `checkIndex`；无论成功失败，都恢复变量、best-effort 清除 snapshot 并归还会话。

`checkIndex` 选择 common handle、整数主键或 `_tidb_rowid`，把隐藏虚拟生成列替换为生成表达式，并把部分索引条件拼入两侧 SQL。事务 `begin` 后先由 `quickPassGlobalChecksum` 比较整表 checksum 与计数；相等且没有 `ForceBucketedCheck` 时立即成功。不相等时，每轮分别强制 table scan 与目标 index scan，对 handle checksum 构造筛选式和分组式，读取并排序桶结果、记录全部桶差异，再以 `firstDifferentBucket` 选择下一层偏移和模数。循环最多执行 9 轮（`1..10`），若已经缩到不超过 100 行或当前轮重新一致则停止。

仍有差异时，`checkIndex` 分别按 handle 排序读取目标桶的表侧与索引侧明细。`report_row_mismatch` 调用运行时解码、再次按编码 handle 排序，并以双游标归并：同 handle 但 checksum/值不同、只在表侧、只在索引侧三种情况分别传入相应的 `Option<RecordData>` 给 `ReportInconsistency`。任一报告返回错误都会终止当前任务并取消同一执行器的其他 worker。

## 数据与状态

`CheckTableExec::done` 和 `FastCheckTableExec::done` 保证检查至多执行一次；空来源/空索引列表会直接返回。慢路径的 `exitCh: AtomicBool` 是 `Close` 与 worker 间的协作取消标志，`srcs` 中每个 trait object 各自置于 `Arc<Mutex<_>>` 中，任务队列也是 `Mutex<VecDeque<usize>>`。`IndexInfo.id` 用于把来源重新关联到原始索引，名称匹配采用 ASCII 不区分大小写；`TableMeta.partition_ids` 为空表示逻辑表自身就是物理表，否则每个物理 ID 都要检查。

快路径复制 `TableMeta`、`IndexInfo` 和用户 `SessionVars` 给 worker，不共享可变系统会话。`fastCheckSysSessionVarsBackup` 覆盖并恢复不可见索引、查询内存额度、DistSQL/执行器并发度、最大执行时间和 TiKV 读超时。`InvisibleIndexGuard` 负责执行器级开关恢复，系统会话级变量由 `HandleTask` 显式恢复。

分桶状态由 `rows_to_check: i64`、`offset: u64`、`modulus: u64`、`checked_once` 和 `mismatch` 构成。桶列表按 `bucket` 排序；checksum 的 SQL `NULL` 规范化为 0，空全局结果规范化为 `(0, 0)`。差异行以编码后的 `handle: Vec<u8>` 排序，避免把复合 common handle 当作单个整数比较。

## 依赖与调用关系

RustCodeGraph 对本文件报告的模块级使用者包括 `pkg/executor/distsql.rs`、`pkg/executor/join/index_lookup_join.rs`、`pkg/executor/test/planreplayer/plan_replayer_test.rs` 和 `pkg/session/runtime/admin.rs`，但逐符号搜索没有给出它们直接构造本文件执行器的可靠调用边；这些“used by”文件级边不能替代具体接线证据。可以确认的上游选择点是 `pkg/executor/builder.rs::buildCheckTable`，它只产出通用 executor kind。

文件内部慢路径的主调用链是 `CheckTableExec::Next -> CheckIndicesCount -> checkIndexHandle/checkTableIndexHandle/checkTableRecord -> IndexLookUpExecutor::NextBatch/CheckRecordAndIndex`；快路径是 `FastCheckTableExec::Next -> createWorker -> checkIndexWorker::HandleTask -> initSessCtx -> checkIndex -> quickPassGlobalChecksum/getCheckSum/queryToRow -> report_row_mismatch`。调用运行时的 `DecodeRecord` 与 `ReportInconsistency` 是算法到真实表编码、索引编码、存储和错误类型的边界。

Cargo 层面，本文件实际使用 `astersql-errors` 和 `astersql-util-chunk`，二者均在 `pkg/executor/Cargo.toml` 的 `[dependencies]` 中声明。`nextgen` feature 只转发给 `astersql-dxf-importinto/nextgen`，本文件没有 `cfg` 条件或 feature 分支。

## 错误处理与边界

- 部分索引在慢路径 `Next` 的第一轮元数据遍历中无条件报错，且发生在跳过多值/列存索引、计数或读取来源之前；`pkg/executor/check_table_index_test.rs::slow_checker_rejects_partial_indexes_before_skipping_special_indexes` 对三个索引类别和两种 `check_index` 值都固定了该顺序。
- `index_at`、`QueryRow::value/Get*`、`HandleTask` 的任务偏移以及 `BucketSize < 2` 都返回明确错误。无符号值转 `i64` 会检查溢出；分桶 `offset` 加法和 `modulus` 乘法也检查溢出。
- 所有 `Mutex` 加锁都把 poison 转为带上下文的错误；worker 使用 `catch_unwind`，慢路径 `handlePanic` 和快路径 `panicError` 不让 panic 越过线程边界。
- `verifyIndexSideQuery` 要求计划出现 `IndexFullScan`、`IndexRangeScan`，或 access object 含 `", index:"` 的 `PointGet`/`BatchPointGet`，同时拒绝任何 `TableFullScan`。查询或字段类型解析失败也视为验证失败，随后 `checkIndex` 返回错误。
- snapshot 设置和清除遵循 Go 的诊断性 best-effort 语义，不覆盖索引检查结果；独立 Rust 测试 `snapshot_setup_failure_is_best_effort_like_go` 验证设置失败仍成功返回。相比之下，系统会话获取、`begin`、checksum 查询、解码和不一致报告失败都会向上传播。
- 快路径只抽取第一个差异桶继续细化；最多 9 轮后进入行级检查。checksum 属于快速筛选而不是唯一正确性证据：只有 checksum/计数完全相等才快速通过，已定位的差异最终由具体行比较和运行时报错确认。

## 并发与资源生命周期

两条路径都使用 `thread::scope`，所以 `Next` 返回前所有 worker 已结束，不存在借用数据逃逸。并发度硬编码为最多 3；共享任务队列通过 `Mutex<VecDeque<_>>` 串行取任务，`AtomicBool` 用 Acquire/Release 传播失败或取消。错误 channel 在主线程丢弃最后一个 sender 后以 `try_recv` 读取首个已送达错误；慢路径还会在实际读取错误前调用 `LogIndexCheckFailure`。

慢路径的来源生命周期是 `Open` 全部打开、worker 在互斥锁下批量读取、`Close` 全部关闭；关闭过程即使前一个来源失败也继续关闭其余来源，只返回首错。调用方必须保证成功 `Open` 后最终执行 `Close`，否则具体 `IndexLookUpExecutor` 持有的下游资源无法由本文件代为释放。

快路径每个索引任务单独获取并归还系统会话。`HandleTask` 对 `initSessCtx` 失败有专门分支，会先归还会话；初始化成功后则无论 `checkIndex` 结果如何都会恢复变量、清 snapshot 并归还。执行器级不可见索引开关由 RAII guard 覆盖正常返回、错误和 scoped worker panic。`FastCheckSession::Query` 的 record-set 关闭职责属于 trait 的具体实现；本文件只消费已经物化的 `Vec<QueryRow>`。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/check_table_index.go`。Rust 保留了主要结构与命名：`CheckTableExec`、`FastCheckTableExec`、`checkIndexWorker`、会话变量备份/恢复、全局 checksum 快速通过、最多 3 worker、最多 10 层限制意图、100 行细查阈值、SQL 标识符转义、执行计划验证和表/索引明细报告。Go 的 `builder.go::buildCheckTable` 会直接构造两种执行器；Rust `builder.rs` 目前只选择通用 kind，这是最重要的接线差异。

Rust 用 `CheckTableRuntime` / `FastCheckRuntime` / `FastCheckSession` trait 取代 Go 对 TiDB session、KV、table、workerpool 和 consistency reporter 的直接依赖，并用标准 scoped thread、`Mutex<VecDeque<_>>` 与 `mpsc` 取代 Go 的 channel/operator workerpool。Rust 的错误文本是普通 `astersql_errors::SharedError`；Go 的部分索引错误使用具备 MySQL 错误码的 `errCheckPartialIndexWithoutFastCheck`，因此对外错误码兼容性不能由本文件单独证明。

快路径细节也有可见差异：Go 在 `getCheckSum` 中为并发内部 SQL 初始化独立 `ExecDetails` 并负责关闭 record set，Rust 把查询/资源管理下放到 `FastCheckSession::Query`；Go 通过 failpoint 强制分桶，Rust 通过 `FastCheckRuntime::ForceBucketedCheck` 注入；Rust 明确把 checksum 为 `NULL` 归零，并对偏移/模数溢出报错。Go 的行比对包含 `lastTableRecord` 回看逻辑，Rust `report_row_mismatch` 使用标准双游标排序归并，语义目标相同但实现并非逐句翻译。

Go 回归测试 `pkg/executor/test/admintest/admin_test.go` 证明了多值索引固定走慢路径、部分索引在无可用快路径时报错、分区表损坏可检测、全局 checksum 相等时跳过分桶、不一致时进入分桶、PointGet/BatchPointGet 二级索引计划可接受、并发检查以及用户会话变量传播。它们是算法意图证据，不等同于 Rust 生产接线或 Rust 集成测试已通过。

## 扩展指南

- 新增慢路径索引类别或计数策略时，先改 `IndexInfo`、`CheckTableExec::Next` 和必要的 `CheckTableRuntime` 方法，并在独立的 `pkg/executor/check_table_index_test.rs` 增加顺序、计数方向、分区及取消回归；不要把测试内嵌进生产文件。
- 新增快路径可传播的会话变量时，必须同步修改 `SessionVars`、`fastCheckSysSessionVarsBackup`、`backupFastCheckSysSessionVars`、`restoreTo`、`applyFastCheckSysSessionVars` 以及系统会话测试，确保正常、错误和 panic 路径都恢复原值。
- 修改 SQL 生成时，应集中复用 `TableName` / `ColumnName` / `escapeName`，同步检查普通列、隐藏虚拟生成列、common handle、整数 handle、`_tidb_rowid`、部分索引条件和不可见索引；索引侧 SQL 仍须先经 `verifyIndexSideQuery`，否则可能误用表扫并掩盖不一致。
- 修改分桶算法时，保持全局 checksum 的快速通过、checksum 与 count 双重比较、桶排序、最大轮数、候选行阈值和 checked arithmetic；增加空结果、缺桶、同桶不同计数、偏移/模数溢出及强制分桶的独立 Rust 测试。
- 修改行级报告时，应围绕 `FastCheckRuntime::DecodeRecord` / `ReportInconsistency` 和 `report_row_mismatch` 扩展，覆盖同 handle 值不同、仅表侧、仅索引侧、复合 handle 和重复/排序边界，并核对 Go `consistency.Reporter` 的编码与脱敏语义。
- 若要让本实现进入完整 Rust SQL 主链，需要先找到或新增 `ExecutorKind::{CheckTable, FastCheckTable}` 到具体执行器及 runtime adapter 的构造点，并用 Rust 端 SQL 集成测试证明；不能只修改本算法文件后宣称已接线。兼容风险主要是 MySQL 错误码、snapshot 与部分索引语义，性能风险主要是固定 3 并发、内部 SQL 物化、分桶轮数和候选行内存。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件；`node --file pkg/executor/check_table_index.rs --offset 1 --limit 2000` 核对了 1,168 行完整源码及文件级使用者；`explore "check_table_index.rs CheckTableIndexExecutor CheckIndexRange"` 给出了本文件的内部调用流与主要符号。对具体生产构造点的符号查询未得到可靠直接边，因此文档将其标为未验证而非推断。
- Rust 源与边界：`pkg/executor/check_table_index.rs`；模块入口 `pkg/executor/lib.rs`；通用构建选择 `pkg/executor/builder.rs::buildCheckTable`；crate 声明和直接依赖 `pkg/executor/Cargo.toml`。
- Rust 独立测试：`pkg/executor/check_table_index_test.rs`，覆盖 snapshot 设置失败仍继续，以及慢路径在计数/扫描前拒绝部分索引；测试由 `pkg/executor/lib.rs` 的 `#[cfg(test)] mod check_table_index_test` 独立装配。
- Go 对照：`pkg/executor/check_table_index.go` 和 `pkg/executor/builder.go::buildCheckTable`；相关回归证据来自 `pkg/executor/test/admintest/admin_test.go` 中多值/部分索引、分区损坏、快速通过与强制分桶、PointGet/BatchPointGet、并发及会话变量传播用例。
- 本任务是纯文档分析，按计划不运行 Cargo，也不把 Go 测试结果当作 Rust 运行证据。结构验收要求目标文件存在且恰含本页 11 个固定二级标题；最终交付前另行执行该命令并报告退出码。
