# `pkg/util/execdetails/execdetails.rs`

## 文件定位

本文件是 SQL 执行明细的聚合与展示实现，对应源码为 [`execdetails.rs`](./execdetails.rs)。源码由 `pkg/util/execdetails/internal/group1/lib.rs` 中的 `execdetails_impl` 通过 `include!("../../execdetails.rs")` 纳入 `astersql-util-execdetails-group1`，再由根 crate `pkg/util/execdetails/lib.rs` 的 `execdetails` 模块统一再导出。根 `Cargo.toml` 将该 group1 子 crate、RU v2 子 crate和 util 子 crate组合为 `astersql-util-execdetails`；本文件本身没有条件编译项，测试装配位于上述两个 `lib.rs`。

它处在“分布式请求返回明细 -> 语句级聚合 -> 慢日志、语句摘要和结构化日志”的中间层。已接线的 Rust 路径包括：`pkg/distsql/select_result.rs::record_cop_evidence` 调用 `MergeCopExecDetails`/`MergeReadPoolTaskDetails`；`pkg/sessionctx/stmtctx/stmtctx.rs::GetExecDetails` 读取快照；`pkg/sessionctx/variable/slow_log.rs` 消费 `CopTasksDetails` 和 IA 远读统计；`pkg/util/util.rs` 把 `ExecDetails::ToZapFields` 转成日志字段。

## 核心职责

- 用 `ExecDetails`、`CopExecDetails` 表达一次语句累积的 coprocessor、扫描、提交、加锁、读池和请求次数信息。
- 用 `SyncExecDetails` 在多个并发返回的 Region/cop 任务之间串行合并明细，并维护 `P90Summary`。
- 从原始样本生成 `CopTasksDetails`（平均、P90、最大值、最大值地址、退避分组）和较精简的 `CopTasksSummary`。
- 用 `ExecDetails::String`、两个 `ToZapFields` 和 `TaskTimeStats::String` 输出稳定、零值省略、与 Go 字段名及顺序兼容的观测数据。
- 通过 `GetIARemoteReadSegmentStats` 从 `util::ScanDetail` 抽取 IA remote read 的段数、字节数和等待耗时。
- 保存 `StmtExecDetails` 的 SQL 响应写回耗时及懒初始化 RU v2 指标。该类型的方法在当前 Rust 中不是公开 API，主要保持 Go 包内语义。

## 主要符号

- `ExecDetails`：语句级总容器。`CopExecDetails` 在 Go 中是匿名嵌入，Rust 改为具名字段；提交、普通/共享 lock-keys、read-pool 都用 `Option` 表达 Go 指针的可空性。
- `CopExecDetails`：单个 cop 任务的 `ScanDetail`、处理/等待耗时、callee 地址，以及按退避类型记录的 sleep 总时长与次数。
- `P90Summary` / `P90BackoffSummary`：分别维护所有 cop 任务和每种退避类型的样本及累计值；`Reset` 清空状态，`Merge` 每次增加一个 cop 样本。
- `MaxDetailsNumsForOneQuery = 1000`：Go 百分位实现切换到 t-digest 前保留的原始样本上限。本文件声明该兼容常量，但 group1 当前实际使用的 `Percentile` 定义在 `internal/group1/lib.rs`，其 `Vec` 实现尚未引用此上限。
- `StmtExecDetailKey` / `StmtExecDetails`：对应 Go context key 和语句局部明细；`ensureRUV2Metrics` 懒建指标，`getRUV2Metrics` 不触发初始化，`setRUV2Metrics` 替换可选指标。
- `IARemoteReadSegmentStats` / `GetIARemoteReadSegmentStats`：对扫描明细的 IA 三字段做值快照；输入为 `None` 时返回全零值。
- `ExecDetails::{String, ToZapFields}`：前者形成慢日志风格文本，后者形成 zap 风格键值；`push_commit_details` 和 `push_commit_zap_fields` 封装提交分支。
- `SyncExecDetails`：唯一共享可变入口，内部 `sync::Mutex<SyncExecDetailsInner>` 同时保护 `ExecDetails` 和 `P90Summary`。
- `MergeExecDetails`、`MergeCopExecDetails`、`MergeScanDetail`、`MergeReadPoolTaskDetails`、`MergeLockKeysExecDetails`、`MergeSharedLockKeysExecDetails`：按来源分别合并，避免独立 scan/read-pool 更新错误增加 cop 请求数。
- `CopTasksDetails` / `CopTasksSummary` / `TaskTimeStats`：对外的详细、精简和单维时间统计结果。
- `seconds`、`format_go_string_slice`：内部格式辅助；后者刻意输出 Go `%v` 的字符串切片样式，如 `[backoff1 backoff2]`。

本文件没有 trait、enum 或类型别名；公开 API 主要是上述 `pub struct`、常量和方法，两个提交格式化 helper、两个 locked merge helper及格式辅助函数是内部实现。

## 执行流程

1. DistSQL 收到 cop 响应后构造 `CopExecDetails`。`record_cop_evidence` 将它传给 `SyncExecDetails::MergeCopExecDetails`，并独立合并 read-pool 明细。
2. `MergeCopExecDetails` 对空输入直接返回；否则取得互斥锁，累计 `CopTime`、`BackoffTime` 和 `RequestCount`，再合并 scan/time 数据，并把处理、等待及退避样本交给 `P90Summary::Merge`。
3. `P90Summary::Merge` 为每个任务各加入一个 process/wait 样本；只遍历 `backoffTimes` 中出现的类型，缺失的 `backoffSleep` 按零耗时处理，同时累计该类型出现于多少请求、总 sleep 和总退避次数。
4. 提交、普通锁和共享锁沿各自入口合并到底层 client 明细的 `Merge`；首次值直接安装。`MergeScanDetail` 与 `MergeReadPoolTaskDetails` 是补充信息入口，不增加 `RequestCount`，后者还过滤空聚合并在首次写入时 clone。
5. `CopTasksDetails` 在锁内根据任务数计算 process/wait 的总计、平均、P90、最大值和最大值地址，再按退避类型产生同类统计；`CopTasksSummary` 只保留任务数、总时间和最大时间/地址。零任务均返回 `None`。
6. 输出阶段，`ExecDetails::String` 按固定顺序仅追加非零字段；提交字段在持有 `CommitDetails.Mu` 时读取退避和慢 RPC，释放锁后再原子读取 resolve-lock 与 prewrite-region。`ToZapFields` 使用对应的小写或兼容键生成结构化字段。
7. 慢日志通过 `GetIARemoteReadSegmentStats` 写出非零 IA 指标，并通过 `CopTasksDetails` 写 cop 汇总；通用日志路径从 statement context 取 `GetExecDetails` 快照后调用 `ToZapFields`。

## 数据与状态

聚合状态分成两组并由同一把锁保持一致：`execDetails` 保存可加总的真实计数/耗时和可选 client 明细，`detailsSummary` 保存用于 P90/最大值的样本。`RequestCount` 只由 `MergeCopExecDetails` 增加；直接合并 scan 或 read-pool 不代表新增 cop 响应。`Reset` 同时恢复两组状态，防止旧百分位样本与新累计值混用。

时间统一使用 `std::time::Duration` 的兼容再导出。提交明细中的 `CommitBackoffTime`、`ResolveLockTime` 以纳秒整数存储，格式化时转换为 `Duration`；`PrewriteRegionNum` 通过原子读取取得。百分位样本是 `DurationWithAddr`，因此最大值能保留对应 store 地址。`GetExecDetails` 返回 clone 快照：外部修改不会改回聚合器，但 clone 后其内容也不再受聚合器锁保护。

字段名常量是日志兼容协议的一部分，例如 `Cop_time`、`Process_keys`、`Slowest_prewrite_rpc_detail` 和三个 `IA_remote_read_*` 名称；改名会影响慢日志解析、规则匹配或下游测试。`LockKeysDuration` 当前仅作为数据字段存在，本文件的格式化和合并流程没有更新或输出它。

## 依赖与调用关系

上游生产调用主要来自 `pkg/distsql/select_result.rs`（cop/read-pool 证据）、`pkg/session/runtime/*`（提交、加锁、共享锁、read-pool）以及 statement context。读取端包括 `pkg/sessionctx/variable/slow_log.rs`、`pkg/util/stmtsummary/*`、`pkg/executor/show_slow_queries.rs` 和 `pkg/util/util.rs`。

下游类型和行为由 group1 包装层提供：`util::{ScanDetail, TimeDetail, CommitDetails, LockKeysDetails, PoolTaskDetails}` 完成实际字段合并；`Percentile<DurationWithAddr>` 完成样本排序、P90 和最大值；`sync::Mutex` 包装标准互斥锁；`atomic` 以 Relaxed 顺序读取 client 兼容原子字段；`zap` 提供本 crate 的日志字段表示。`RUV2Metrics` 也由 group1 兼容层注入。本文件直接只导入 `HashMap`，其余名称来自 include 所在模块的 `use crate::*`。

`pkg/util/execdetails/Cargo.toml` 的根库入口是 `lib.rs`，直接依赖 group1、ruv2、util 三个本地子 crate，并声明 `protobuf = 2.8.0`、`tdigest = 1.0.0`；后两者不在本文件直接调用，百分位的实际实现位置必须以 group1 当前代码为准。

## 错误处理与边界

本文件没有 `Result` 返回或可恢复错误通道。可缺失的输入以 `Option` 表示：空 cop、scan、read-pool、commit/lock 明细按各入口语义忽略或保持空值；TiFlash 当前可能不提供 `ScanDetail`，`mergeScanDetailLocked` 明确跳过它。输出只展示正数/非零耗时，默认 `ExecDetails::String` 是空字符串，零任务的 cop 统计是 `None` 或空字段列表。

`CopTasksDetails` 将正的 `i32` 任务/请求数用 `u32::try_from(...).expect(...)` 转成除数，并假定每个计数都有对应 percentile 样本；若内部不变量被破坏，会 panic。`sync::Mutex::Lock` 对 poisoned mutex 也会 panic。正常入口通过 `Merge` 同步增加计数和样本，因此这些断言是内部一致性检查，不是面向调用者的错误恢复机制。

时间加法、`RequestCount` 和退避次数没有显式溢出处理。`format_go_string_slice` 只为普通退避类型字符串复现 Go 格式；若元素自身包含空格或括号，表示会有歧义，但这与当前兼容目标一致。

## 并发与资源生命周期

`SyncExecDetails` 的所有读写聚合操作都在 `mu` 下进行，锁同时覆盖总计和百分位样本，避免读到跨版本组合。耗时较大的 percentile 排序和 backoff map 构造也在 `CopTasksDetails` 的临界区内；扩展时应注意任务很多时的锁持有时间。

提交明细具有第二层锁和原子字段：格式化时先锁 `CommitDetails.Mu`，复制/格式化其 backoff 与慢 RPC 状态，显式 `drop(mu)` 后读取 resolve-lock 和 region 原子值。不要在持有 `SyncExecDetails.mu` 的新代码里以相反顺序获取这些锁，否则可能形成锁顺序风险。

本文件不创建线程、异步任务、通道或事务，也不拥有外部 I/O 资源。生命周期主要是 statement context 持有聚合器、多个响应逐步合并、消费者 clone/构建摘要，最后由 context 一同释放；`Reset` 支持复用同一个聚合器。`StmtExecDetails` 的 RU 指标按需 clone 内嵌 storage，目前不是共享锁保护对象，调用者须按语句局部所有权使用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/execdetails/execdetails.go`，总体结构、字段、合并顺序、零值过滤、P90 取法和日志键均按 Go 移植。主要语言映射是：Go 指针对应 `Option`，匿名嵌入的 `CopExecDetails` 对应 Rust 具名字段，Go mutex 的 `defer Unlock` 对应 guard 生命周期，Go 原子 load 对应兼容层 Relaxed load，返回 nil 摘要对应 `None`。

已验证的有意差异包括：Rust `GetExecDetails` 做深层 `Clone`，比 Go 的浅结构体副本更隔离；Rust 无 nil receiver，所以 `StmtExecDetails` 的 nil receiver 防御由调用侧承担；Rust `P90Summary::Merge` 不用“map 是否为 nil”判断初始化，避免默认空 `HashMap` 导致误清除已预置的 process/wait 样本，该行为由 `test_p90_merge_preserves_samples_with_initialized_empty_backoff_map` 固定。

当前迁移仍有一个应如实保留的差异：Go `util.go::Percentile` 在原始样本达到 `MaxDetailsNumsForOneQuery` 后使用 t-digest，而本文件实际链接的 `internal/group1/lib.rs::Percentile` 仍以无上限 `Vec` 保存全部样本；虽然 Cargo 声明了 `tdigest` 且另一个 `pkg/util/execdetails/util.rs` 有切换实现，本文件经 group1 include 时没有使用后者。因此不能声称该聚合路径已经具备 Go 的样本上限或同等内存界限。

## 扩展指南

- 新增执行明细字段时，先确定它属于单任务 `CopExecDetails`、语句总计 `ExecDetails` 还是 percentile 摘要；同步修改正确的 merge 入口、`Reset`、文本/zap 输出，并保持与 Go 字段顺序及零值规则一致。
- 新增独立证据源时，不要复用 `MergeCopExecDetails`，除非它确实代表一个新 cop 请求；scan/read-pool 的现有独立入口证明请求计数与附加明细必须解耦。
- 新增 percentile 维度时，必须在一次锁持有期间同时更新累计值、样本和计数，并为零样本、单样本、多个地址、P90 边界及 reset 后状态补测试。
- 修改提交输出时维持锁顺序：只在需要时持有 `CommitDetails.Mu`，离开后再访问原子字段；同时核对 `String` 与 `ToZapFields`，避免两个观测面漂移。
- 百分位内存边界若要真正对齐 Go，应修改本文件实际使用的 `internal/group1/lib.rs::Percentile` 或调整接线，而不是只改未被该 include 路径使用的另一份实现；需要验证超过 1000 个样本时的精度、最大地址和内存行为。
- 测试应继续放在独立文件。核心同步补 `pkg/util/execdetails/execdetails_1_aster_unit_test.rs`，集成/Go 对照补 `pkg/util/execdetails/execdetails_test.rs`，并核对 `pkg/util/execdetails/execdetails_test.go`；不要把测试嵌入本生产文件。

兼容风险集中在日志键/顺序、浮点秒格式、Go 的 nil/浅拷贝语义和 P90 选择；性能风险集中在锁内排序、clone 大型明细以及当前无界样本向量。

## 验证依据

- RustCodeGraph 状态：索引覆盖 11,467 个文件；`files --filter pkg/util/execdetails` 确认目标 Rust、Go 和测试文件均在索引中。
- RustCodeGraph 精确查询：`MergeCopExecDetails` 定位到本文件第 714 行，`CopTasksDetails` 定位到第 822 行，两个 `ToZapFields` 定位到第 513/1016 行，`GetIARemoteReadSegmentStats` 同时定位 Rust 第 242 行和 Go 第 224 行，`P90Summary` 同时定位 Rust/Go 定义。图的 `callers`/`callees` 对这些 Rust 方法返回空边，因此调用点按技能降级规则用 `rg` 核对，未把空图边解释为“无人调用”。
- 读取的生产与装配证据：`pkg/util/execdetails/execdetails.rs`、`pkg/util/execdetails/internal/group1/lib.rs`、`pkg/util/execdetails/lib.rs`、`pkg/util/execdetails/Cargo.toml`、`pkg/distsql/select_result.rs`、`pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/sessionctx/variable/slow_log.rs`、`pkg/util/util.rs`、`pkg/util/stmtsummary/statement_summary.rs`。
- Go 对照：`pkg/util/execdetails/execdetails.go` 和百分位实现 `pkg/util/execdetails/util.go`。它们证实字段/流程映射以及 1000 样本后转 t-digest 的 Go 行为。
- 独立测试：`pkg/util/execdetails/execdetails_test.rs` 覆盖 String/zap、read-pool/scan 独立合并、IA 空值和 P90 保样本；`pkg/util/execdetails/execdetails_1_aster_unit_test.rs` 覆盖 Go 顺序、零值过滤、cop 平均/P90/最大地址及退避统计；`pkg/util/execdetails/execdetails_test.go` 提供原始 Go 输出和 IA 行为基线。
- 本任务是只读行为分析加单一 Markdown 产物，按计划不运行 Cargo；最终结构检查要求本文恰有十一个规定的二级标题。
