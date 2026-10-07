# `pkg/executor/join/hash_join_v1.rs`

## 文件定位

本文件属于 Cargo crate `astersql-executor-join`；同目录 `Cargo.toml` 把 `lib.rs` 设为 crate 根，`lib.rs` 再通过 `pub mod hash_join_v1` 公开本模块。它实现内存模型下的 Hash Join v1 执行器，并在同一模块中提供简化的相关子查询 `NestedLoopApplyExec`。上游通常经 `hash_join_test_util.rs::build_hash_join_v1_exec` 或 `HashJoinV1Exec::{new,new_full_outer}` 组装实例，下游依赖 `hash_join_base.rs` 的共享构建状态与 worker 防护、`hash_table_v1.rs` 的哈希行容器、`joiner.rs` 的连接语义以及 `row_table_builder.rs` 的行/chunk 数据模型。

文件第 13 行起到实际 `use crate::hash_join_base` 之前是一大段注释化的 Go 迁移草稿，不参与编译；当前有效 Rust 实现从约第 1450 行的导入区开始。草稿保存了 Go goroutine、channel、failpoint 和完整 NAAJ 分支的迁移线索，但本文描述“当前行为”时只以编译态 Rust 符号为准。

`Cargo.toml` 的普通依赖只有 `astersql-executor-internal-exec` 与 `astersql-util-execdetails`；本文件直接使用后者的 `HashStateRuntimeStats`、`RuntimeStatsColl`。大量历史 Join 依赖被声明在 Windows target 下，但本文件本身没有条件编译项。仓库中 `pkg/executor` 与 `pkg/executor/join` 均没有 `doc.go`，模块边界由 `lib.rs` 和附近源码给出。

## 核心职责

1. `HashJoinCtxV1` 保存连接类型、两侧键列、NAAJ 开关、build 是否为保留侧、声明并发度和输出批大小，并在构造时检查基本不变量。
2. `BuildWorkerV1` 展平 build chunks，构造 `HashRowContainer`，按内存限额触发容器 spill；`HashJoinV1Exec::open` 负责登记 build 成功或失败。
3. `ProbeSideTupleFetcherV1` 顺序提供 probe chunks；`ProbeWorkerV1` 对每行查哈希桶，调用 `Joiner` 解释 Inner/Outer/Semi/Anti/NAAJ 语义并标记已使用的 build 行。
4. `HashJoinV1Exec` 驱动 `Created -> Open -> Exhausted/Closed` 生命周期，把完整结果缓存后按 `max_chunk_size` 分批返回，同时处理 outer-build 尾扫、Full Outer 两侧未匹配行、运行时统计和显式关闭。
5. `NestedLoopApplyExec` 按 outer 行调用 `InnerBuilder`，以关联列编码为缓存键复用 inner 结果，并用同一个 `Joiner` 处理匹配与未匹配输出。
6. `JoinRuntimeStats`、`CacheInfo` 及辅助函数承载 Apply 缓存统计、Hash 统计、Full Outer 单侧过滤、NAAJ NULL 分类与关联键编码。

当前 Rust 的 `concurrency` 参与校验、统计以及哈希表的并发模式选择，但活动 `produce_all` 路径只创建一个 probe worker 并顺序遍历 chunk；它不是 Go 版多 goroutine/channel 调度的等价实现。

## 主要符号

- `ExecutorState::{Created,Open,Exhausted,Closed}`：执行器可见状态。`next` 可从 `Created` 隐式打开；耗尽返回默认空 `HashJoinWorkerResult` 并转为 `Exhausted`；关闭后 `next` 报错。
- `HashJoinCtxV1` / `validate()`：核心配置。要求两侧键数相同且非空，`concurrency`、`max_chunk_size` 为正；NAAJ 仅允许 `AntiSemi`/`AntiLeftOuterSemi`；Full Outer 禁止旧式 `build_side_is_outer`。
- `ProbeSideTupleFetcherV1::new(chunks)`：把预置 probe chunks 包装为 `ProbeSideTupleFetcherBase`，取数、取消检查和回收行为在基类中。
- `ProbeWorkerV1::{new,join_probe_row,join_full_outer_probe_row}`：单行 probe 核心。普通路径选择正常桶或 NA 候选，交给 `Joiner`，并标记实际匹配 build 行；Full Outer 路径先执行 probe 单侧过滤，再用一对方向相反的 outer joiner 分别生成匹配结果和 probe 未匹配结果。
- `BuildWorkerV1::{new,build}`：在 `BuildWorkerBase::run_guarded` 中获取 build 行、填充 `HashRowContainer`、检查内存并可能 `spill()`；panic 由基类转换为字符串错误。
- `HashJoinV1Exec::{new,new_full_outer}`：普通构造器要求 context 与 joiner 类型一致；Full Outer 构造器要求 context 为 `FullOuter`，且两个 joiner 必须是互为相反方向的 `LeftOuter`/`RightOuter`。
- `HashJoinV1Exec::{with_runtime_stats,SetMemoryLimit,IsSpillTriggered,DiskBytes,state}`：分别接入 statement 统计、配置 spill 阈值以及暴露 spill、磁盘字节和生命周期状态。两个 PascalCase 方法延续 Go 命名。
- `HashJoinV1Exec::{open,next,execute_all,close}`：公开生命周期 API；`produce_all` 是私有的完整 probe 和尾扫实现。
- `CorrelatedKey`、`InnerBuilder`、`CacheInfo`、`NestedLoopApplyExec`：Apply 模型。`InnerBuilder` 要求闭包 `Send + Sync`；缓存值为完整 inner 行向量。
- `JoinRuntimeStats::merge`：累加缓存命中/未命中与哈希统计，缓存 entries 取最大值，`has_hash_stat` 做逻辑或。
- `passes_filter`：只有所有谓词都返回 `Some(true)` 才通过；`false` 与 SQL NULL 都被视为单侧过滤拒绝。
- `classify_naaj` / `naaj_has_null`：依据 probe 键和候选 build 行是否含 NULL 得到四种 `NaajType`，再决定未匹配输出是否携带 NULL 语义。
- `encode_correlated_key`：把关联列的 `Debug` 文本依次写入字节并以 `0xff` 分隔；越界返回描述性错误。

## 执行流程

普通 Hash Join 的完整流程如下：

1. `new` 先调用 `HashJoinCtxV1::validate`，再核对 `Joiner::join_type`，保存两侧 chunks，状态设为 `Created`。
2. `open` 对共享 `HashJoinContextBase` 执行 `reset`，清空输出和游标，构造 `BuildWorkerV1`。Full Outer 会先用 `full_outer_build_filter` 分出可入表行和被拒绝但仍需作为未匹配行保留的 build 行。
3. `BuildWorkerV1::build` 展平输入，创建以 `build_key_indices` 为键的 `HashRowContainer`，写入行并计算内存字节；超过可选阈值时容器 spill。成功后执行器保存 table、记录 hash-state 行数并 `finish_build`，失败则 `fail(error)` 并原样返回。
4. 首次 `next` 在尚无结果时调用 `produce_all`。该函数先 `wait_for_build_side`，再让 fetcher 逐块取 probe 数据；每块在 `ProbeWorkerBase::run_guarded` 中逐行调用 `join_probe_row`，之后清空并回收 chunk。
5. 普通 probe 使用 `get_matched_rows_by_indices`；NAAJ 使用 `get_na_rows_by_indices`。若 build 是 outer，`try_to_match_outers` 返回逐 build 行状态，仅将 `Matched` 指针标记 used；否则 `try_to_match_inners` 生成结果，未匹配时以 `result.has_null || naaj_has_null(naaj)` 调用 `on_miss_match`。
6. probe 完毕后，Left/Right Outer 的 outer-build 模式扫描 `table.unmatched_rows()` 补发 build 未匹配行。Full Outer 则用 build joiner 补发入表但未命中的 build 行，并补发 build 单侧过滤拒绝行；probe 过滤拒绝行已在逐行 probe 时由 probe joiner 输出。
7. `next` 从内存中的 `output` 切出不超过 `max_chunk_size` 的结果；`execute_all` 循环调用它直到空批。`close` 注册可选 hash-state 统计、取消共享上下文、关闭并释放 table、清空输出并置 `Closed`。

Apply 流程是：`open` 清游标、输出、缓存和计数；`next(required_rows)` 逐个 outer 行生成关联键，命中缓存则克隆 inner rows，否则调用 `InnerBuilder` 并缓存结果；随后 `Joiner::try_to_match_inners` 写入共享输出缓冲，完全未匹配时调用 `on_miss_match`，最后 drain 至多 `required_rows` 行。缓存跨多个 `next` 批次保留，但再次 `open` 会丢弃。

## 数据与状态

`HashJoinV1Exec` 拥有输入 chunks、可选哈希表、完整输出向量与游标。当前设计会在第一次取结果时先物化全部 join 输出，内存占用不仅包括 build table，还包括所有结果；`max_chunk_size` 只限制每次返回量，不限制物化峰值。`open` 可在 `Open`、`Exhausted` 或 `Closed` 后重新构建，但若当前已是 `Open` 则直接成功，不重置。

`HashRowContainer` 保存 chunks、哈希表、NAAJ NULL 桶、used 位图、内存/磁盘字节及 spilled 标志。`IsSpillTriggered` 同时检查共享 context 与 table；实际 build 回调直接让 table spill，而 `BuildWorkerBase` 还会设置共享 spilled 标志。`DiskBytes` 在 table 不存在时返回零。

Full Outer 有两套 `Joiner`：build joiner 负责匹配组合及 build 未匹配布局，probe joiner负责 probe 未匹配布局。`full_outer_rejected_build_rows` 单独保留未通过 build 单侧过滤的行，防止它们进哈希表后伪造匹配，又确保最终仍按 outer 语义输出。

`hash_state_stats` 每次 `open` 在接入统计集合时新建；build 成功后按已接受 build chunks 行数累加。`close` 用 `take()` 只注册一次。`HashJoinRuntimeStats` 的 build/probe 时长当前以 `+=` 累计，重复 open 不清零，因此它描述执行器实例累计耗时，而不是严格的单轮快照。

Apply 缓存键是关联值 `Debug` 表示加分隔字节，并非 Go 的类型感知 SQL codec。缓存拥有 inner rows 的克隆；命中时还会再 clone 一次。`CacheInfo.entries` 每次 `next` 结束更新为当前 map 大小。`CorrelatedKey` 只是公开别名，实际 cache 字段直接写成 `HashMap<Vec<u8>, Vec<Row>>`。

## 依赖与调用关系

- 装配入口：`pkg/executor/join/lib.rs` 公开本模块，并只在 `cfg(test)` 下装配 `hash_join_v1_test.rs`；`full_outer_join_test.rs` 也直接导入本文件的两个主要类型。
- 直接上游：`hash_join_test_util.rs::build_hash_join_v1_exec` 组装 context、joiner 和 inputs；`full_outer_join_test.rs::full_outer_executor` 调用 `new_full_outer`；`pkg/executor/join_pkg_test.rs`、`pkg/executor/benchmark_test.rs` 和 jointest 也直接构造/运行 v1。源码搜索没有发现生产 builder 把当前 Rust `HashJoinV1Exec` 接入 SQL executor 树，因此不能把这些测试/基准入口描述成完整服务器生产接线。
- 共享运行时：`hash_join_base.rs` 提供 `HashJoinContextBase`、build/probe worker base、fetcher base 和 `HashJoinWorkerResult`。RustCodeGraph 明确给出 `produce_all -> join_probe_row` 调用边，`next -> produce_all` 调用边，以及 `join_probe_row -> join_full_outer_probe_row/classify_naaj/naaj_has_null` 调用边。
- 哈希存储：`hash_table_v1.rs::HashRowContainer` 承担建表、正常/NA 查找、used 标记、未匹配扫描与 spill 资源。
- 结果语义：`joiner.rs::Joiner` 负责条件求值、行拼接、半连接短路和未匹配填充值；本文件负责选择调用模式和维护 build 行是否使用。
- 统计：`hash_join_stats.rs` 提供执行耗时结构；`astersql-util-execdetails` 提供 `HashStateRuntimeStats` 和注册集合。
- Go 对照：同路径 `hash_join_v1.go` 是类型和总体流程来源；`hash_join_base.go`、`hash_table_v1.go`、`joiner.go` 提供其协作契约。

## 错误处理与边界

配置错误在构造阶段以 `Result<_, String>` 返回：键数不等或为空、并发/chunk 为零、NAAJ 类型不合法、Full Outer 配置不合法、context/joiner 类型不一致都会被拒绝。Full Outer 还会拒绝缺失 joiner、同方向 joiner或非 Left/Right Outer 的组合。

build 和 probe worker 都通过基类 `run_guarded` 把 unwind panic 转成字符串错误并登记共享失败；普通闭包错误用 `?` 传播。build 失败会调用 `HashJoinContextBase::fail`，使等待 probe 的路径可观察到同一失败。取消由共享 context 在 fetch/wait 边界返回错误；当前逐行循环不会在每行检查取消。

哈希查询、谓词、Joiner 和 InnerBuilder 错误均原样向上返回。`produce_all` 若 table 缺失报 `"hash table is not built"`；Full Outer 缺失对应 joiner时报专门错误；Apply 关联列越界会包含具体 index。`next` 在 `Closed` 状态报 `"hash join is closed"`，耗尽则返回空批而不是错误。

`passes_filter` 把谓词 NULL 当作未通过，符合单侧过滤的保留侧处理要求。NAAJ 分类只检查 `probe_key_indices` 在 probe 和候选 build 行中的同一下标；若两侧 key 下标布局不同，build 行 NULL 分类可能与 `build_key_indices` 不一致，这是当前实现事实和扩展时需重点验证的兼容风险。

`encode_correlated_key` 使用调试字符串，不具备 SQL collation、时区、十进制规范化或无碰撞编码的正式保证；分隔符也没有转义协议。增加复杂 `Value` 类型或把 Apply 接入生产数据前，必须以类型化编码替换或证明唯一性。

## 并发与资源生命周期

`HashJoinContextBase` 内部用 `Arc<(Mutex<_>, Condvar)>` 共享 build 完成、失败、取消和 spill 状态，clone 后的 worker 观察同一状态。`BuildWorkerBase`/`ProbeWorkerBase` 提供 panic 边界；`RuntimeStatsColl` 由 `Arc<Mutex<_>>` 保护，`close` 注册时锁中毒会 panic。

虽然 `HashJoinCtxV1.concurrency > 1` 会让 `HashRowContainer` 选择并发模式，当前活动 v1 实现并没有按该数值创建多个 Rust 线程：build、fetch 和 probe 都在调用线程串行执行。Go 注释草稿中的 worker wait group、channel、原子 finished 和 per-worker joiner 不属于当前编译态资源模型。

资源生命周期为：构造时拥有输入；`open` 创建 table；第一次 `next` 物化 output；耗尽后 table 仍保留，直到再次 `open` 覆盖或 `close` 调用 `HashRowContainer::close` 并置空。`close` 同时 `cancel` context、清空 output，并注册尚未注册的 hash-state stats。类型没有 `Drop` 实现；调用者若要求及时释放 table/spill 资源，应显式 `close`。

Apply 的 `InnerBuilder: Send + Sync` 允许安全封装可共享闭包，但 `NestedLoopApplyExec` 自身以 `&mut self` 串行推进，缓存和输出没有内部锁。`close` 清空 outer rows、output 和 cache；再次使用需重新提供 outer 数据，当前 `open` 不恢复被 `close` 清掉的输入。

## 与 Go 版本的对应关系

Go `HashJoinCtxV1`、`ProbeSideTupleFetcherV1`、`ProbeWorkerV1`、`BuildWorkerV1`、`HashJoinV1Exec` 与 `NestedLoopApplyExec` 都能在 Rust 中找到同名或等价类型；总体意图仍是 build 哈希表、probe、按 Joiner 语义输出，以及按关联键缓存 Apply 内表结果。Rust 的注释草稿还保留了 Go 原方法形状，便于逐段核对。

主要语义对齐点包括：build/probe 两阶段、outer-build 的 used 标记与尾扫、Full Outer 两侧保留及单侧过滤、NAAJ 对条件 NULL 的区分、Apply 缓存跨 `Next` 复用、重复 open 重置缓存，以及 close 时注册统计。Rust 独立测试直接固定 inner/left outer 多匹配、outer-build 未匹配行、重复 open/close、hash-state 行数和 Apply 缓存计数。

当前差异也很显著：Go 从真实子执行器增量拉取 chunk，以 goroutine、channel、多个 probe worker 和 wait group 并行流水；Rust接收预置 `Vec<Chunk>`，顺序 build/probe 并一次性物化全部输出。Go 的内存/磁盘 tracker、OOM action、required rows 反压、failpoint 和 panic channel 协调更完整；Rust只保留可选字节阈值与简化 spill 标志。Go Apply 支持 outer/inner filter、真实 executor 生命周期、类型化 codec、可选缓存及内存 tracker；Rust始终启用内存 HashMap 缓存，并以闭包代替 inner executor。

Go NAAJ 对 null bucket、same-key bucket、all bucket 有细分的快速返回顺序；当前 Rust把候选获取和四态分类收敛到 `HashRowContainer`、`classify_naaj` 与 `Joiner`。扩展时应以 Go 具体分支和 Rust 测试共同核对，不能仅因类型名相同就假定所有 SQL 三值逻辑、性能短路和错误时序完全等价。

## 扩展指南

- 新增连接类型或配置字段时，先更新 `HashJoinCtxV1::validate` 和构造器一致性检查，再检查 `ProbeWorkerV1::join_probe_row`、outer-build 尾扫与 `Joiner`。同步独立测试 `hash_join_v1_test.rs`，Full Outer 则同步 `full_outer_join_test.rs`。
- 修改 build/probe 键布局时，同时检查 `HashRowContainer` 查询、`classify_naaj` 的 build 下标选择和 NAAJ NULL 测试；不要只改正常等值查找。
- 扩展 Full Outer 时，保持“两套 opposite outer joiner + 两侧单表过滤 + 仅真实匹配标 used”不变量；谓词为 false/NULL、同键多行和 build/probe 任一侧被过滤都应有回归。
- 若实现真实并行，不能只循环创建多个 `ProbeWorkerV1`：需明确 table 写后只读边界、used 标记同步、结果顺序/回收、取消与 panic 唤醒、每 worker Joiner 状态以及统计聚合。性能基准应覆盖锁竞争和输出背压。
- 若改变物化策略，应保留 `next` 的 chunk 上限、跨批同一 outer 行的匹配完整性、错误优先级与 close 释放契约；相关 helper `execute_hash_join_exec` 和 jointest 可作为调用协议依据。
- Apply 扩展应优先替换 `encode_correlated_key` 为 SQL 类型感知编码，并定义缓存是否可禁用、大小/淘汰、内存计费和 inner 错误后的条目一致性。回归仍放在独立 `hash_join_v1_test.rs` 或同目录独立测试文件，不要把 Rust 测试嵌入生产源文件。
- 新增运行时统计时，检查 `open` 重置还是累计、`close` 是否恰好注册一次、重复 open/close 的计数含义，并同步 `hash_join_stats.rs` 及对应独立测试。

兼容风险集中在 SQL NULL/NAAJ、outer 行布局、列投影和 Full Outer 过滤；正确性风险集中在 used 标记与关联键碰撞；性能风险集中在 build/probe/input 克隆、Apply 缓存克隆和全量输出物化。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/join` 确认目标、Go 对照与测试均已索引。
- RustCodeGraph `node --file pkg/executor/join/hash_join_v1.rs`：读取目标文件并确认它被 `pkg/executor/benchmark_test.rs`、`pkg/executor/join/hash_join_v1_test.rs` 使用；`node HashJoinV1Exec`、`node NestedLoopApplyExec` 对照 Rust/Go 类型；`node produce_all`、`node join_probe_row` 提供 `next -> produce_all -> join_probe_row` 及其下游调用边。精确 `callers/callees` 命令在 30 秒内没有返回结果，因此其他入口以源码引用搜索补证。
- 已读生产/装配路径：`pkg/executor/join/hash_join_v1.rs`、`hash_join_base.rs`、`hash_table_v1.rs` 的类型节点、`joiner.rs` 的类型节点、`hash_join_test_util.rs`、`lib.rs`、`Cargo.toml`；`pkg/executor` 和 `pkg/executor/join` 无 `doc.go`。
- Go 对照：`pkg/executor/join/hash_join_v1.go` 的 context/worker/执行器、`Next`、build、Apply 和统计段；Rust 文件头部注释草稿仅作为迁移线索，没有当作活动实现证据。
- 独立 Rust 测试：`hash_join_v1_test.rs` 覆盖 inner/left outer、outer-build、重复 open/close、hash-state 统计与 Apply 缓存；`full_outer_join_test.rs` 覆盖双侧未匹配、other condition、两侧过滤和 spill；`hash_join_test_util.rs` 明确 open/next/close 调用协议。源码引用还显示 `join_pkg_test.rs`、`benchmark_test.rs` 和 `test/jointest/hashjoin/hash_join_test.rs` 的直接用例。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付以任务指定的固定 11 个二级标题结构检查、链接/事实人工复核及 Git diff 范围检查作为验证。
