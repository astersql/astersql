# `pkg/executor/lockstats/lock_stats_executor.rs`

## 文件定位

本文件是 `astersql-executor-lockstats` 子 crate 中的 LOCK STATS 实现。crate 入口 `pkg/executor/lockstats/lib.rs` 以 `pub mod lock_stats_executor` 导出本模块，并把独立测试文件 `lock_stats_executor_test.rs` 挂到测试构建中；`pkg/executor/lockstats/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/executor/lockstats`。文件的直接业务目标是把 SQL 计划携带的表/分区名称解析为统计子系统需要的物理 ID 和显示名，再请求统计句柄写入锁定元数据，使后续自动统计更新能够识别这些锁。

当前 Rust 实现是自包含的移植边界：`LockExec`、`Runtime`、`InfoSchema`、`StatsHandle` 以及元数据 DTO 都在本文件定义，源码只直接依赖 `std`。工作区根 `Cargo.toml` 和 `pkg/executor/Cargo.toml` 已声明 `astersql-executor-lockstats`，但对 Rust 源码的精确引用搜索只发现本 crate 的模块、测试以及 `unlock_stats_executor.rs` 的复用，未发现 Rust executor builder 构造 `LockExec`。因此，可以确认本地执行逻辑存在，不能据此宣称它已经接入 Rust SQL 请求主链。Go 主链的对应入口是 `pkg/executor/builder.go::buildLockStats`。

## 核心职责

1. `LockExec::Next` 校验运行时统计句柄和输入表列表，选择分区级或整表级锁定路径，并把统计句柄返回的非空提示追加为语句告警。
2. `LockExec::onlyLockPartitions` 固化分流不变量：只有“恰好一张表且显式列出至少一个分区”才调用 `StatsHandle::LockPartitions`；其他非空输入均调用 `StatsHandle::LockTables`。
3. `populatePartitionIDAndNames` 通过 `InfoSchema` 把用户指定的分区名解析成表 ID 及“分区物理 ID到小写显示名”的映射，并拒绝空分区列表、非分区表和未知分区。
4. `populateTableAndPartitionIDs` 为整表模式构造“表 ID 到 `StatsLockTable`”的映射；分区表会同时展开全部物理分区，非分区表保留空的 `PartitionInfo`。
5. `genFullPartitionName` 统一生成统计锁提示所用的分区全名。相邻的 `unlock_stats_executor.rs` 直接复用两个 `populate*` 函数以及 `Runtime`、`TableName`、`Error`，所以这些公开项同时是锁定和解锁路径的共享边界。

## 主要符号

- `Error(String)` / `Result<T>`：本模块的轻量错误边界。`Error` 实现 `Display` 和 `std::error::Error`，但不附加错误类别、调用栈或源错误链。
- `CIStr { O, L }`：模仿 Go `ast.CIStr` 的标识符。`CIStr::new` 保留原始拼写到 `O`，并用 `to_lowercase` 生成 `L`。
- `TableName`：执行计划输入，含 `Schema`、`Name` 和 `PartitionNames`。公开字段沿用 Go 命名。
- `PartitionDefinition`、`TableMeta`：供 `InfoSchema` 返回的最小表元数据；`TableMeta::Partitions == None` 表示非分区表。
- `InfoSchema::TableByName(schema, table)`：名称到 `TableMeta` 的查询接口。调用者传入 `Schema.L` 与 `Name.L`。
- `StatsLockTable { FullName, PartitionInfo }`：整表锁请求值；`PartitionInfo` 的键是物理分区 ID，值是完整显示名。
- `StatsHandle`：统计锁存储边界。本文件调用 `LockPartitions`、`LockTables`；`RemoveLockedPartitions`、`RemoveLockedTables` 是为相邻解锁执行器共享的同一 trait 能力。
- `Runtime`：提供可选统计句柄、当前 `InfoSchema` 和告警追加能力。`Send + Sync` 约束允许实现持有线程安全的共享服务。
- `LockExec { runtime, Tables }`：公开执行器。`Open`、`Close` 当前是无状态空操作；`Next` 承担一次完整副作用。
- `populatePartitionIDAndNames`、`populateTableAndPartitionIDs`、`genFullPartitionName`：公开解析/格式化辅助函数；前两个也由 `unlock_stats_executor.rs` 调用。

文件中没有常量、宏、条件编译项或泛型实现。所有 trait 和业务 DTO 都是公开项；真正只在 `LockExec` 内使用的分流方法 `onlyLockPartitions` 也被声明为 `pub`，这是当前 API 事实而非外部接线证据。

## 执行流程

`LockExec::Open` 不分配资源，直接返回成功。随后一次 `LockExec::Next` 按以下顺序执行：

1. 调用 `Runtime::StatsHandle`。若返回 `None`，立即返回 `"Lock Stats: handle is nil"`，不会查询 `InfoSchema`。
2. 检查 `Tables`。空列表返回 `"Lock Stats: table should not empty"`，不会发起锁定请求。
3. 获取一次 `Runtime::InfoSchema`，并调用 `onlyLockPartitions` 决定分支。
4. 分区分支只读取 `Tables[0]`：`populatePartitionIDAndNames` 先查表元数据，再逐一大小写不敏感匹配分区定义，最后调用 `StatsHandle::LockPartitions(table_id, "schema.table", partitions)`。
5. 整表分支遍历所有输入：`populateTableAndPartitionIDs` 逐表查询元数据，为每张分区表枚举全部分区并用 `genFullPartitionName` 生成显示名，然后调用一次 `StatsHandle::LockTables`。
6. 两个句柄方法都返回 `Result<String>`。错误通过 `?` 原样终止；成功且字符串非空时，`Runtime::AppendWarning(Error(message))` 追加告警。空字符串不产生告警。
7. 返回 `Ok(())`。`Close` 同样不释放资源，直接成功。

分区解析中，查询名与元数据的 `PartitionDefinition::Name.L` 都在比较时再次调用 `to_lowercase`。这对应 Go `tables.FindPartitionByName` 的大小写不敏感行为，即使某个 `CIStr.L` 由外部构造且尚未正规化，仍能匹配。映射中保存的名称则是请求侧 `name.L.clone()`，不是元数据侧名称。

## 数据与状态

`LockExec` 仅持有两个长期字段：共享的 `Arc<dyn Runtime>` 和拥有所有权的 `Vec<TableName>`。`Open`、`Next`、`Close` 不写入执行器字段，因此本文件没有游标、批次状态或“已执行”标志；如果上层重复调用 `Next`，代码会重复发起锁定请求，幂等性及重复告警行为取决于 `StatsHandle` 实现，本文件没有保证。

分区模式临时创建 `HashMap<i64, String>`，容量按用户分区名数量预留。重复指定同一物理分区会覆盖同一 ID 的旧值，因此传给句柄的是去重后的映射。整表模式按输入表数量预留外层映射；重复表 ID 同样由后项覆盖前项。每个分区表的内层映射按元数据定义数量预留并包含全部分区，而 `TableName.PartitionNames` 在整表分支中不参与筛选。

名称规范遵循当前代码：表全名和分区全名使用 `Schema.L`、`Name.L`；分区锁映射值使用请求分区的 `L`；错误中的未知分区使用请求的原始拼写 `O`。`CIStr::new` 使用 Rust Unicode 小写转换，这与 Go 的 Unicode 大小写折叠并非所有字符上严格等价，当前测试只证明了 ASCII 混合大小写场景。

## 依赖与调用关系

内部调用边经 RustCodeGraph 核对为：

- `LockExec::Next -> LockExec::onlyLockPartitions`。
- 分区分支：`LockExec::Next -> populatePartitionIDAndNames -> InfoSchema::TableByName`，随后 `LockExec::Next -> StatsHandle::LockPartitions`。
- 整表分支：`LockExec::Next -> populateTableAndPartitionIDs -> InfoSchema::TableByName`，且 `populateTableAndPartitionIDs -> genFullPartitionName`，随后 `LockExec::Next -> StatsHandle::LockTables`。
- 告警边：`LockExec::Next -> Runtime::AppendWarning`。
- 共享消费者：`unlock_stats_executor.rs::UnlockExec::Next` 调用 `populatePartitionIDAndNames` 或 `populateTableAndPartitionIDs`，并使用同一 `Runtime` / `StatsHandle` 边界执行移除操作。

外部边界方面，`pkg/executor/lockstats/lib.rs` 导出本模块；`pkg/executor/lockstats/Cargo.toml` 声明 crate 名、Go 包映射和仅在 Windows 目标下列出的上游 crate 依赖。当前文件本身没有引用这些外部 crate，而是使用本地 trait/DTO。`pkg/executor/Cargo.toml` 与工作区根 `Cargo.toml` 声明了此 crate，但精确 Rust 搜索未发现 `LockExec` 的外部构造者，因此 Rust 端应用主链状态为“未发现接线”，不是“已验证接入”。Go 端则由 `pkg/executor/builder.go::buildLockStats` 从 `plannercore.LockStats.Tables` 构造 `lockstats.LockExec`，通用 executor 生命周期再调用其 `Open/Next/Close`。

## 错误处理与边界

以下错误会短路并阻止后续句柄调用：缺少统计句柄、执行器表列表为空、分区列表为空、`InfoSchema::TableByName` 失败、目标表没有分区元数据、请求分区在元数据中不存在，以及任一 `StatsHandle` 方法失败。解析多个表或分区时没有回滚：解析阶段只构建内存映射，尚无锁副作用；映射全部成功后才调用一次句柄，所以不会由本文件造成部分表已锁、部分表未锁。句柄内部是否原子不在此 trait 合同中表达。

`populatePartitionIDAndNames` 明确拒绝空分区列表；通常该条件由 `onlyLockPartitions` 保证，但函数是公开且被解锁路径复用，因此保留独立防御。它还拒绝 `TableMeta::Partitions == None`。`populateTableAndPartitionIDs` 则允许非分区表并为其生成空 `PartitionInfo`。

代码没有显式检查空 schema/table 名、重复表、重复分区、分区定义的重复 ID，或 `TableMeta.ID` 冲突；`HashMap::insert` 对重复键采用后写覆盖。未找到 Rust 测试直接覆盖 `LockExec::Next`、句柄错误传播、告警、非分区表走分区模式、未知分区、多表中途查询失败或重复 ID，因此这些行为主要由源码控制流证明，测试保障有限。

## 并发与资源生命周期

`Runtime`、`InfoSchema`、`StatsHandle` 都要求 `Send + Sync`，执行器通过 `Arc<dyn Runtime>` 共享运行时；这使依赖具备跨线程共享的类型条件，但 `LockExec::Next` 本身是 `&mut self` 的同步方法，没有启动线程、异步任务、通道或后台工作。所有临时 `HashMap` 都在单次调用栈内创建，并以共享引用传给句柄；方法返回后由 Rust 自动释放。

`InfoSchema` 的 `Arc` 仅在本次 `Next` 中持有。`Open`/`Close` 不改变引用计数之外的业务资源，也不管理锁的生命周期；“统计锁”是由 `StatsHandle` 持久化或维护的外部状态，执行器关闭不会自动解锁，解除锁定由 `UnlockExec` 路径负责。

调用 `AppendWarning` 发生在锁定句柄已成功返回之后。如果运行时实现需要内部同步，责任属于 `Runtime` 实现；接口返回 `()`，本文件无法观察或回滚告警追加失败。代码也没有取消令牌、超时或重试逻辑，这些策略若存在只能位于 `InfoSchema` / `StatsHandle` 的具体实现或上层执行框架。

## 与 Go 版本的对应关系

Rust `LockExec::Next` 基本逐分支对应 `pkg/executor/lockstats/lock_stats_executor.go::(*LockExec).Next`：先从 domain/runtime 获取统计句柄，拒绝 nil/空表，再选择 `LockPartitions` 或 `LockTables`，最后把非空返回消息追加为 warning。`onlyLockPartitions` 的条件完全一致；`populatePartitionIDAndNames`、`populateTableAndPartitionIDs` 和 `genFullPartitionName` 的输出形状与名称格式也保持一致。

主要结构差异如下：

- Go `LockExec` 嵌入 `exec.BaseExecutor` 并有编译期 `exec.Executor` 断言；Rust `LockExec` 使用本地 `Runtime` trait，未实现仓库通用 executor trait，也未发现 Rust builder 构造引用。
- Go 从 `domain.GetDomain(e.Ctx())` 获取 `StatsHandle` / `InfoSchema`，并通过 `StmtCtx.AppendWarning` 写告警；Rust 把三者抽象为可注入的 `Runtime`，便于隔离依赖，但当前测试尚未构造运行时验证 `Next`。
- Go 使用真实 `ast.TableName`、`infoschema.InfoSchema`、`types.StatsLockTable` 和 `tables.FindPartitionByName`；Rust 定义最小本地 DTO/trait。Cargo 中列出了相应 crate，但本源文件尚未使用它们。
- Go 的 `FindPartitionByName` 返回其标准未知分区错误；Rust 自行生成 `unknown partition '<原始名>'`。错误文本与错误类型不保证完全一致。
- Go 的 `Open(context.Context)` / `Next(context.Context, *chunk.Chunk)` 带上下文和输出 chunk；Rust 方法无上下文和 chunk。两者的 LOCK STATS 都不产生结果行。

测试对应上，`lock_stats_executor_test.rs::populate_partition_id_and_names` 和 `populate_table_and_partition_ids` 移植了两个 Go 测试的核心成功路径及空输入错误；Rust 额外测试 `populate_partition_id_and_names_matches_case_insensitively`，锁定 Go `EqualFold` 语义。Rust 测试还明确验证非分区表得到空映射，但没有覆盖 Go builder/Executor 接线。

## 扩展指南

若新增 LOCK STATS 行为，应按边界选择接入点：分流条件修改 `LockExec::onlyLockPartitions`；名称到 ID 的规则修改对应 `populate*` 函数；持久化或提示策略扩展 `StatsHandle` / `Runtime`；分区显示格式修改 `genFullPartitionName`。由于解锁执行器复用解析函数和 trait，修改这些共享项时必须同时审查 `unlock_stats_executor.rs` 的行为，避免锁定与解锁解析出不同物理对象。

安全扩展时应保持三个不变量：所有元数据解析成功后才调用一次锁句柄；非空句柄消息只作为 warning 而非错误；单表显式分区与整表/多表路径不能混用。若要真正接入 Rust SQL 主链，还需要在 Rust executor builder 和通用 executor trait 处做最小接线，并用真实统计句柄替换或适配本地抽象；这属于本文件当前事实之外的后续工作，不能仅通过修改 Cargo 依赖宣称完成。

测试必须放在独立的 `pkg/executor/lockstats/lock_stats_executor_test.rs`，不要内嵌到生产文件。扩展解析规则时补充成功、未知分区、非分区表、重复输入和元数据错误用例；扩展 `Next` 时应增加 mock `Runtime` / `StatsHandle`，验证分支调用次数、参数、错误短路、空消息与非空 warning。若改变与 Go 共有的语义，应同步对照 `lock_stats_executor_test.go`，并明确记录有意差异。性能上应关注大批表/分区时的元数据查询次数、Unicode 小写分配和映射容量；兼容性上应关注错误文本、名称大小写以及重复键覆盖规则。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 Rust、Go 与测试文件均已索引。
- RustCodeGraph `files --filter pkg/executor/lockstats`：确认模块包含 `lib.rs`、锁定/解锁 Rust 与 Go 实现及两种语言的独立测试。
- RustCodeGraph `node --file pkg/executor/lockstats/lock_stats_executor.rs --offset 1 --limit 420`：读取并核对目标文件全部 224 行、公开符号与控制流。
- RustCodeGraph 对 `LockExec`、`populatePartitionIDAndNames`、`populateTableAndPartitionIDs`、`genFullPartitionName`、`onlyLockPartitions` 的精确查询，以及围绕目标文件的 `explore`：确认 Rust/Go 对应符号、`Next` 的主要调用边、`populateTableAndPartitionIDs -> genFullPartitionName` 和独立 Rust 测试调用边。通用方法名存在索引重名噪声，故未据其推断外部接线。
- 已读源码/配置：`pkg/executor/lockstats/lock_stats_executor.rs`、`pkg/executor/lockstats/Cargo.toml`、`pkg/executor/lockstats/lib.rs`、`pkg/executor/lockstats/unlock_stats_executor.rs`、工作区根 `Cargo.toml`、`pkg/executor/Cargo.toml`。
- 已读对照/测试：`pkg/executor/lockstats/lock_stats_executor.go`、`pkg/executor/lockstats/lock_stats_executor_test.rs`、`pkg/executor/lockstats/lock_stats_executor_test.go`，以及 Go 构造入口 `pkg/executor/builder.go::buildLockStats`。目标包不存在 `doc.go`。
- 精确文本引用搜索：Rust 外部引用仅发现模块入口、独立测试和解锁实现复用；Go 引用确认 builder 构造 `lockstats.LockExec`。因此本文把 Rust 主链接线标为“未发现”，没有把 Cargo 声明当作运行时接线证据。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前另执行任务指定的 11 章节结构命令，并人工复核本文可回答文件存在原因、执行方式与安全扩展点。
