# `br/pkg/restore/log_client/ssts.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-log-client` library crate；crate 入口 `br/pkg/restore/log_client/lib.rs` 以 `#[path = "ssts.rs"] pub mod ssts` 挂载模块，并通过 `pub use ssts::*` 把这里的公开常量、trait 和结构体提升到 crate 根。`br/pkg/restore/log_client/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `br/pkg/restore/log_client`，直接语义基线是同目录的 `ssts.go`。

它位于日志恢复（PITR）的“备份元数据/文件迭代器”与“分裂、过滤、导入编排”之间：`LogFileManager::GetCompactionIter` 把压缩日志备份产物包装成 `CompactedSSTs`，`LogFileManager::GetIngestedSSTs` 把直接复制的 SST 逐文件包装成 `CopiedSST`；下游通过统一的 `dyn SSTs` 接口读取表 ID 和文件集合，而不必区分两种来源（`log_file_manager.rs:61,439-456`）。本文件只定义集合模型与访问契约，不负责读取对象存储、生成 rewrite rule、切分 region 或发起 ingest。

## 核心职责

1. 用 `SSTs` trait 统一表示“一组待恢复 SST”，暴露类别、物理表 ID、文件快照及替换文件集合的能力。
2. 用 `CompactedSSTs` 适配 `LogFileSubcompaction`：一个对象可包含多个 `SstOutputs`，表 ID 直接来自 `Meta.TableId`。
3. 用 `CopiedSST` 适配单个直接复制/ingest 的 `File`：从起止键解码物理表 ID，以 `AtomicI64` 缓存结果，并携带可选的跨阶段表 ID 重写元数据。
4. 用 `RewrittenSSTs` 与 `SSTs::as_rewritten` 提供对象安全的可选扩展，使下游能区分 SST 的物理内容表（`TableID`）与过滤/规则查找时应视作的逻辑表（`RewrittenTo`）。
5. 保留 Go 的可观察契约：类型标记分别为 1/2；跨表键范围、空文件上取表 ID及给单文件对象设置多个文件均为编程错误并 panic。

## 主要符号

- `CompactedSSTsType: i32 = 1`、`CopiedSSTsType: i32 = 2`：稳定的来源分类值，由各自的 `SSTs::Type` 返回。新增消费方应比较常量，不应散落字面量。
- `trait RewrittenSSTs { fn RewrittenTo(&self) -> i64; }`：只描述过滤/查规则所用的逻辑表 ID。当前唯一实现者是 `CopiedSST`。
- `trait SSTs: fmt::Display`：对象安全的集合接口。`GetSSTs` 返回拥有所有权的 `Vec<File>`（Rust 实现会 clone），`SetSSTs` 用新集合替换内部选择结果；默认 `as_rewritten` 返回 `None`。
- `CompactedSSTs { pub inner: LogFileSubcompaction }` 与 `CompactedSSTs::new`：保存完整 subcompaction。`Display` 输出 `CompactedSSTs: {Meta}`；`TableID` 读 `inner.Meta.TableId`；文件读写映射到 `inner.SstOutputs`。
- `CopiedSST { pub File: Option<File>, pub Rewritten: RewrittenTableID, cachedTableID: AtomicI64 }` 与 `CopiedSST::new`：公开文件槽和重写元数据，私有原子缓存初始为 0。该类型刻意不实现 `Clone`，避免不明确的缓存复制语义。
- `CopiedSST` 的 `SSTs` 实现：`Type` 返回 2；`TableID` 验证起止键同表并缓存；`GetSSTs` 把 `Some(file)` 映射为单元素向量、`None` 映射为空向量；`SetSSTs` 只接受 0 或 1 个文件；`as_rewritten` 返回 `Some(self)`。
- `CopiedSST` 的 `RewrittenSSTs` 实现：`Rewritten.Upstream > 0` 时返回该值，否则回退到物理 `TableID()`。`Downstream` 字段由本类型携带但不参与这里的决策。

## 执行流程

压缩 SST 的主流程如下：

1. `LogFileManager::GetCompactionIter` 从 migration-aware compaction 迭代器取得 `LogFileSubcompaction`，调用 `CompactedSSTs::new` 后擦除为 `Box<dyn SSTs + Send>`。
2. 下游以 `TableID()` 取得 `Meta.TableId`，以 `GetSSTs()` 取得 `SstOutputs` 的克隆。
3. `CompactedFileSplitStrategy::ShouldSkip` 根据规则与 checkpoint 过滤文件；若仅部分文件已完成，则调用 `SetSSTs` 把剩余文件写回同一 subcompaction。
4. `CompactedFileSplitStrategy::Accumulate` 遍历剩余文件，按表汇总键范围、KV 数和大小；后续导入阶段继续消费该集合。

直接复制 SST 的主流程如下：

1. `LogFileManager::GetIngestedSSTs` 展平每个 `IngestedSSTs` 文件组，为每个 `File` 创建一个 `CopiedSST`，并复制组级 `RewrittenTableID`。
2. 首次调用 `TableID()` 时读取 `File.StartKey` 与 `File.EndKey`，分别调用 `tablecodec::DecodeTableID`。两端 ID 必须相同；相同则以 `SeqCst` 写入缓存并返回。
3. 查找过滤规则时，`CompactedFileSplitStrategy::hasRule/inspect` 优先通过 `as_rewritten().RewrittenTo()` 使用逻辑目标表；累计时若逻辑 ID 与物理 ID 不同，还会构造从物理表到逻辑表的边界 rewrite rule。
4. `LogClient::rewriteRulesFor` 在导入规则准备阶段再次比较 `RewrittenTo()` 与 `TableID()`；不同时克隆规则，并用 `RewriteSourceTableID(rewritten, physical)` 把查到的逻辑规则改成适配 SST 物理键的规则，失败则返回带上下文的错误。

## 数据与状态

`CompactedSSTs` 的可变状态全部位于 `inner`：`Meta` 提供组级表 ID，`SstOutputs: Vec<File>` 是允许被过滤后替换的当前文件集合。`GetSSTs` 返回 clone，因此调用方修改返回向量不会影响对象；只有 `SetSSTs(&mut self, ...)` 会提交选择结果。

`CopiedSST` 保持“至多一个文件”的不变量。`File: None` 表示该 SST 已被过滤清空，而不是一个可继续解析表 ID的合法文件；`Rewritten.Upstream` 是过滤/查规则视角的逻辑表 ID，`TableID()` 则始终表示文件键中编码的物理表 ID。`Rewritten.Downstream` 在本文件中未读取，不能据此推断这里会校验或应用 downstream 映射。

`cachedTableID` 使用 0 作为“尚未缓存”哨兵。非零表 ID 首次解析后固定；即使随后 `SetSSTs` 替换或清空 `File`，缓存也不会失效，`ssts_test.rs::copied_sst_matches_go_rewrite_and_cached_table_contract` 明确锁定了这一 Go 兼容行为。若真实表 ID 为 0，每次调用都会重新解码，因为 0 无法与未初始化区分；这是当前实现边界，不应描述为一次性缓存。

## 依赖与调用关系

直接依赖均可从 `ssts.rs` 的 import 验证：标准库 `fmt` 提供 trait 对象的显示约束，`AtomicI64/Ordering` 提供共享引用下的缓存写入；`crate::stubs::backuppb::{File, LogFileSubcompaction, RewrittenTableID}` 是当前 crate 的 protobuf 兼容模型；`crate::stubs::tablecodec::DecodeTableID` 解析 TiDB 表键；外部 crate `hex` 仅用于跨表 panic 中输出原始边界键。`Cargo.toml` 显式声明 `hex = "0.4"`，而 backuppb/tablecodec 来自本 crate 的本地 `stubs.rs`，不是本文件直接链接 kvproto/tablecodec crate。

已核对的主要上游是：`log_file_manager.rs::GetCompactionIter` 构造 `CompactedSSTs`，`GetIngestedSSTs` 构造 `CopiedSST`，`CountExtraSSTTotalKVs` 通过 `GetSSTs` 统计文件 KV 数。主要下游消费方是 `compacted_file_strategy.rs::{inspect, Accumulate, ShouldSkip, hasRule}` 和 `client.rs::rewriteRulesFor`。crate 根的 `pub use ssts::*` 还允许同 crate 及上层依赖从根路径使用这些公开类型。

RustCodeGraph 的 `explore` 结果确认 Rust 侧调用边包括：`RewrittenTo` 被 `client.rs::rewriteRulesFor`、`compacted_file_strategy.rs` 及 `ssts_test.rs/parity_test.rs` 使用；`TableID` 被 `client.rs::rewriteRulesFor`、checkpoint/flow-control 相关路径和测试使用；`GetSSTs` 被 `log_file_manager.rs::CountExtraSSTTotalKVs`、策略及测试使用。精确 `callers` 查询对同名 trait 方法/实现方法未返回边，因此本文用索引的 `explore` 流图与对应源码位置交叉验证，而没有把空查询解释为“无调用者”。

## 错误处理与边界

- `CopiedSST::TableID` 在 `File == None` 时通过 `expect("CopiedSST.TableID called without a file")` panic。调用方必须在清空前取得并保留所需表 ID，或避免对已过滤空对象调用该方法。
- 起止键经 `DecodeTableID` 得到不同表 ID 时立即 panic，并在消息中包含两个 ID 及十六进制起止键。当前不支持横跨相邻表的单个 SST，不能静默选取一端。
- `CopiedSST::SetSSTs` 接受空向量（清空）或单元素向量（替换）；两个及以上文件会以 Go 对齐文案 panic。多文件集合必须使用 `CompactedSSTs` 或在上层拆成多个 `CopiedSST`。
- `DecodeTableID` 本身返回 `i64` 而非 `Result`；本层没有额外验证键长度或前缀合法性，解析语义由 `stubs::tablecodec` 决定。
- `CompactedSSTs` 不验证 `SstOutputs` 各文件是否确实属于 `Meta.TableId`，也不限制 `SetSSTs` 必须是原集合的子集；“只传入原 `GetSSTs` 子集”是与 Go 接口注释一致的调用方契约。
- `RewrittenTo` 仅把严格大于 0 的 `Upstream` 当作有效重写 ID；0 和负数都会回退到 `TableID()`。
- 本文件的失败均为不变量破坏导致的 panic，没有可恢复的 `Result`。可恢复的规则映射失败发生在下游 `LogClient::rewriteRulesFor`，由其返回 `Err`。

## 并发与资源生命周期

两个结构体都不持有任务、通道、锁、文件句柄、网络连接或显式析构资源，生命周期由装箱它们的迭代器/调用方所有权控制。`GetSSTs` clone `File`，避免向外借出内部向量；`SetSSTs` 需要独占 `&mut self`，负责原地提交过滤结果。

`CopiedSST::TableID(&self)` 会写缓存，因此使用 `AtomicI64` 而不是普通字段；`SeqCst` load/store 让并发读者观察统一的缓存值，并使 `CopiedSST` 能满足 `LogFileManager` 使用的 `Box<dyn SSTs + Send>`。这里不是 compare-and-swap：两个首次调用者可能同时解码同一不可变文件并重复 store 相同 ID，但不会产生不同的合法结果。与此同时，`SetSSTs(&mut self)` 不重置缓存；并发替换文件也被 Rust 的独占借用规则禁止。trait 对象只要求 `Send` 的使用点没有要求 `Sync`，因此不能仅凭原子缓存宣称整个恢复管线支持跨线程共享同一个 `dyn SSTs`。

## 与 Go 版本的对应关系

Rust 的 `RewrittenSSTs`/`SSTs`、`CompactedSSTs`、`CopiedSST`、两个类型常量以及各方法逐项对应 `br/pkg/restore/log_client/ssts.go`。关键一致性包括：压缩集合从 meta 取表 ID；复制集合从起止键取物理表 ID；`Upstream > 0` 优先作为逻辑目标；缓存不因 `SetSSTs` 失效；0/1/多文件分支及跨表范围的 panic 策略均保留。

语言适配差异如下：Go 的 `*backuppb.File`/nil 在 Rust 中是 `Option<File>`，`[]*backuppb.File` 在 Rust 中是拥有所有权的 `Vec<File>`；Go 嵌入 `*LogFileSubcompaction`，Rust 用命名字段 `inner`；Go 的 `fmt.Stringer` 对应 Rust 的 `fmt::Display`；Go 用类型断言识别 `RewrittenSSTs`，Rust 通过 `SSTs::as_rewritten` 显式返回可选 trait 对象；Go 的 `atomic.Int64` 对应 Rust `AtomicI64`。Rust 的 `Display` 在空 `CopiedSST` 时稳定输出 `<nil>`，而非解引用文件。

相关 Go 测试没有单独的 `ssts_test.go`；Go 侧 `client_test.go` 构造 `CompactedSSTs` 覆盖上层恢复场景。Rust 侧把直接契约回归独立放在 `ssts_test.rs`，并在 `parity_test.rs::go_rust_public_contract_matches`、`compacted_file_strategy_test.rs::rewritten_ssts_match_rules_by_logical_target_and_accumulate_there` 中覆盖跨组件接线。

## 扩展指南

- 新增第三类 SST 集合时，应实现 `fmt::Display + SSTs`，分配不冲突的类型常量，并在 `log_file_manager.rs` 的对应产生端构造它；若它有逻辑/物理表 ID 分离语义，再实现 `RewrittenSSTs` 并覆写 `as_rewritten`。同步更新独立的 `ssts_test.rs` 与跨模块 `parity_test.rs`，必要时扩展 `compacted_file_strategy_test.rs`。
- 修改 `CopiedSST::TableID` 时必须保持“物理键表 ID”语义；不要把 `Rewritten.Upstream` 混入该返回值，否则 `client.rs::rewriteRulesFor` 的 source-ID 转换会反向。任何改动都应覆盖空文件、同表、跨表、Upstream 回退和表 ID 0。
- 若要在替换 `File` 时使缓存失效，需要先确认是否故意打破 Go 当前行为，并同步调整 `SetSSTs`、缓存并发方案、Go 对照实现以及 `copied_sst_matches_go_rewrite_and_cached_table_contract`；简单地 store 0 可能让清空后调用转为 panic。
- 若要支持一个 `CopiedSST` 持有多个文件，不能只放宽 `SetSSTs`：还需重新设计字段、`TableID` 的跨文件一致性、`Display`、`GetSSTs`、上游构造和 checkpoint 部分过滤语义。
- 若把本地 `stubs::backuppb` 替换为真实 protobuf 类型，应检查 `File` clone 成本、字段命名、`LogFileSubcompactionMeta` 的显示格式与 `Send` 能力，并保持 Cargo 依赖可复现；不要在本文件偷偷引入本地 `[patch]` 或 vendor 副本。
- 性能敏感改动应特别评估 `GetSSTs` 的全量 clone 与 `SeqCst` 开销。若改成借用切片，会改变 trait 对象和调用方可变过滤接口，必须联动所有消费者而非局部替换。

## 验证依据

- 目标实现：`br/pkg/restore/log_client/ssts.rs`。RustCodeGraph `node --file ... --symbols-only` 识别 2 个常量、2 个 trait、2 个结构体及其构造、显示和 trait 方法；源码人工复核了全部 169 行逻辑。
- 索引证据：`rustcodegraph status` 报告索引包含 11,467 个文件、7,032 个 Rust 文件；`query CompactedSSTs/CopiedSST/SSTs/RewrittenTo` 定位到本文件与 Go 对照；`explore "... CompactedSSTs CopiedSST SSTs RewrittenSSTs callers callees"` 给出 `client.rs`、`log_file_manager.rs`、`compacted_file_strategy.rs` 和各测试的调用关系。对重名方法运行的精确 `callers` 无结果，已以 `explore` 和源码搜索补证。
- crate/模块证据：`br/pkg/restore/log_client/Cargo.toml`、`br/pkg/restore/log_client/lib.rs`；前者确认 crate、Go 包映射和 `hex` 依赖，后者确认模块挂载、公开再导出及 `ssts_test.rs` 的独立测试挂载。
- Go 对照：`br/pkg/restore/log_client/ssts.go`；上层 Go 接线参考 `client.go`、`log_file_manager.go`、`compacted_file_strategy.go` 和 `client_test.go`。
- Rust 调用方：`br/pkg/restore/log_client/log_file_manager.rs::{SSTIter, GetCompactionIter, GetIngestedSSTs, CountExtraSSTTotalKVs}`、`compacted_file_strategy.rs::{inspect, Accumulate, ShouldSkip, hasRule}`、`client.rs::rewriteRulesFor`。
- 测试证据：`ssts_test.rs` 覆盖空文件 panic、压缩集合读写、重写优先/回退、缓存保留、跨表 panic 和多文件 panic；`compacted_file_strategy_test.rs` 覆盖逻辑目标表 70 与物理表 7 的策略接线；`parity_test.rs` 覆盖 Go/Rust 公开契约。本任务是纯文档分析，按计划不运行 Cargo。
