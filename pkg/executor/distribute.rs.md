# `pkg/executor/distribute.rs`

## 文件定位

[`distribute.rs`](./distribute.rs) 属于 `astersql-executor` crate，并由 [`lib.rs`](./lib.rs) 的 `pub mod distribute` 公开。它是 `DISTRIBUTE TABLE` 与 `CANCEL DISTRIBUTION JOB` 在 Rust 侧的执行逻辑模型：把表或分区转换成物理键范围，向 PD 的 `balance-range-scheduler` 提交配置，再尝试取得调度作业 ID；取消路径则按 ID 撤销作业。它不处理 `SHOW DISTRIBUTION JOBS` 或 `SHOW TABLE ... DISTRIBUTIONS`。

[`Cargo.toml`](./Cargo.toml) 将 crate 根设为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/executor"` 指向 Go 对照包。本文件唯一直接使用的 workspace crate 是清单中声明的 `astersql-util-chunk`；PD client、表编码、上下文和错误类型均由 `DistributionBackend` 的实现注入。

当前接线必须分层描述：Rust 模块已经公开，算法也由 [`distribute_table_test.rs`](./distribute_table_test.rs) 独立测试；但仓库内 Rust 精确搜索没有发现测试之外的 `DistributeTableExec`、`CancelDistributionJobExec` 或 `DistributionBackend` 构造/实现。完整 SQL 生产入口仍可在 Go [`builder.go`](./builder.go) 中看到。因此本文件是已实现的可注入执行逻辑，现有证据不能证明它已经接入 Rust SQL 构建主链。

## 核心职责

1. `getKeyRanges` 将非分区表 ID、全部分区 ID 或用户选定的分区 ID 排序，并把相邻物理 ID 合并成尽可能少的 `DistributionKeyRange`。
2. `Open` 预计算键范围，并按分区名的小写形式排序，使相同分区集合生成稳定的作业别名。
3. `distributeTable` 组装 `alias`、`engine`、`rule`、可选 `timeout` 及逗号分隔的起止键，交给 `DistributionBackend::create_scheduler_config`。
4. `Next` 保证提交至多一次，并容忍调度微服务对 PD 配置更新的短暂滞后：最多查询三次作业列表，查询间等待 500ms；找到匹配作业后输出最大的活动 job ID。
5. `CancelDistributionJobExec::Next` 保证一个执行器实例至多发送一次取消请求。
6. `DistributionBackend` 把键编码、缺失分区错误、PD 配置、作业查询、可取消等待和取消操作收敛成可替换边界，使核心状态机不依赖具体网络 client。

## 主要符号

- `schedulerName: &str = "balance-range-scheduler"`：创建、查询和取消操作共享的 PD 调度器名称；保留 Go 风格名称是移植兼容选择。
- `DistributionTable { database_name, table_name, table_id, partitions }`：执行器所需的最小表元数据。`partitions` 是分区名到物理表 ID 的有序映射；空映射表示按非分区表处理。
- `DistributionKeyRange { start_key, end_key }`：已编码、可直接提交给调度器的半开键范围文本。编码和转义的正确性由 backend 负责。
- `SchedulerJob { alias, engine, rule, status, job_id }`：查询结果的最小作业快照。`job_id` 使用 `f64`，对应 Go 从无类型 JSON 解码后使用 `float64` 的协议。
- `DistributionBackend`：包含关联类型 `Context`、`Error`，以及 `key_range`、`missing_partition`、`create_scheduler_config`、`scheduler_jobs`、`wait_or_cancel`、`cancel_scheduler_job` 六个方法。
- `DistributeTableExec<B>`：持有 backend、表元数据、分区选择、规则、引擎、超时、一次性 `done` 标志和 `Open` 生成的 `key_ranges`。
- `DistributeTableExec::{Open, Next}`：外部生命周期入口。其余公开方法 `getSchedulerJob`、`distributeTable`、`getAlias`、`getKeyRanges` 是可单独验证的流程步骤，但生产调用应维持 `Open` 先于 `Next`。
- `CancelDistributionJobExec<B> { backend, job_id, done }`：取消执行器；其 `Next` 不产生结果块，只返回取消结果。

文件没有条件编译项，也没有在生产源内嵌测试。`#![allow(non_snake_case)]` 用于保留 `Open`、`Next`、`getKeyRanges` 等 Go 风格符号名。

## 执行流程

分布流程从 `Open` 开始。`getKeyRanges` 先选择物理 ID：无分区元数据时只取 `table_id`；有分区但没有显式分区名时取全部分区 ID；有显式名称时以大小写不敏感方式逐项查找，任一名称不存在便通过 `missing_partition` 返回错误。随后 ID 升序排列，相邻的 `n, n+1, ...` 被合并，并以每段首尾 ID 调用 `key_range(first, last)`。`Open` 保存结果，再按 `to_lowercase()` 对 `partition_names` 排序。

第一次 `DistributeTableExec::Next` 先清空输出 `Chunk`，检查并设置 `done`，然后调用 `distributeTable`。后者以 `getAlias` 生成 `库.表.` 或 `库.表.partition(p0,p1,...)`，加入引擎和规则；`timeout` 非空才加入配置。各范围的 `start_key` 和 `end_key` 分别按原顺序以逗号连接，最后调用 `create_scheduler_config(ctx, schedulerName, input)`。

配置创建成功后，`Next` 最多进行三轮查询。`getSchedulerJob` 调用 `scheduler_jobs`，只保留 alias、engine、rule 同时匹配且 `status != "finished"` 的作业，再取最大的 `job_id`。查询失败与未匹配都表现为 `(false, -1.0)`，前两次失败后各调用一次 `wait_or_cancel(..., 500ms)`；第三次仍未找到时正常返回空结果。找到 ID 时将其转换为 `u64`，追加到输出块第 0 列。后续 `Next` 仍会先 `Reset` 块，但因 `done` 为真不会重复提交。

取消流程更短：第一次 `CancelDistributionJobExec::Next` 设置 `done`，然后调用 `cancel_scheduler_job(ctx, schedulerName, job_id)`；后续调用直接成功返回，不再访问 backend。

## 数据与状态

两个执行器都通过公开的 `done: bool` 表示一次性状态。重要不变量是：`done` 在外部操作之前被置为 `true`，所以创建配置或取消请求即使返回错误，同一实例的下一次 `Next` 也不会重试。这与 Go 文件的顺序一致，但 runtime adapter 不能假设“错误后再次调用同一执行器”会重发请求。

`Open` 只重算 `key_ranges` 和排序 `partition_names`，不会复位 `done`。复用执行器实例时，调用者必须自行建立新的生命周期或显式初始化状态；当前 API 没有构造函数替调用者保证这一点。直接跳过 `Open` 调用 `Next` 会提交实例中现有的 `key_ranges`，包括可能为空或陈旧的值。

`DistributionTable.partitions` 使用 `BTreeMap`，但 `getKeyRanges` 仍会按 ID 排序，因此范围顺序由物理 ID 而非分区名称决定。连续 ID 合并能减少调度参数段数；不连续 ID 保持多个对应的起止键。显式分区名不会去重，同一名称重复出现会产生重复物理 ID 和重复范围，调用方或语法/计划层应避免重复选择。

`SchedulerJob.job_id` 的 `f64` 形式延续 Go JSON 协议；本文件未检查 NaN、无穷、负数、非整数或超出 `u64` 范围的值。正常协议要求 backend 只提供可精确表示的非负整数 ID。`getSchedulerJob` 用 `-1.0` 作为“未找到”哨兵，只接受大于该值的候选。

## 依赖与调用关系

- 模块装配：[`lib.rs`](./lib.rs) 通过 `pub mod distribute` 暴露生产模块，并在 `#[cfg(test)]` 下以 `#[path = "distribute_table_test.rs"]` 装配独立 Rust 测试。
- 内部调用链：`Open -> getKeyRanges -> DistributionBackend::{missing_partition,key_range}`；`Next -> distributeTable -> getAlias/create_scheduler_config`；随后 `Next -> getSchedulerJob -> scheduler_jobs/getAlias`，必要时调用 `wait_or_cancel`；取消路径 `CancelDistributionJobExec::Next -> cancel_scheduler_job`。
- 数据输出：`DistributeTableExec::Next` 直接依赖 `astersql_util_chunk::Chunk::{Reset, AppendUint64}`。`CancelDistributionJobExec::Next` 没有 chunk 参数，这是 Rust 边界相对 Go executor 接口的简化。
- Go 生产上游：[`builder.go`](./builder.go) 的 `buildDistributeTable` 构造 Go `DistributeTableExec`，另一个构建分支构造 Go `CancelDistributionJobExec`。这些是 SQL 语句到该行为的架构对照，不是 Rust 类型已被构造的证据。
- Rust 上游现状：RustCodeGraph 对精确类型只列出 [`distribute_table_test.rs`](./distribute_table_test.rs) 的导入与实例化；仓库内排除目标文件和测试后的精确 Rust 搜索没有命中。这意味着 backend 的真实 PD/表编码适配器和 Rust builder 接线均未验证。
- Cargo 边界：[`Cargo.toml`](./Cargo.toml) 声明 `astersql-util-chunk`；本文件其余直接依赖均来自标准库。crate 清单中的其他 executor 依赖不能当成本文件已调用相应子系统的证据。

RustCodeGraph 能可靠解析文件内部调用边，但对两个同名 `Next` 产生了互相调用的名称级伪边；本文没有采用该伪边，而是以两个方法体和具体 backend 调用核对真实关系。

## 错误处理与边界

`Open` 只有选定分区不存在时返回 backend 生成的错误；大小写不敏感匹配使用 Unicode `to_lowercase`。如果 `partitions` 中存在仅大小写不同的重复名称，迭代中第一个匹配项胜出；正常表元数据应保证名称唯一。

配置创建错误由 `distributeTable` 和 `Next` 原样传播。作业列表查询错误则被 `getSchedulerJob` 有意降级成“未找到”，让 `Next` 重试；三轮后仍失败不会向调用者暴露查询错误，也不会输出 job ID。等待期间的取消或超时通过 `wait_or_cancel` 返回的错误向上传播。与 Go 版本相比，Rust 抽象不在此处记录查询/解码失败日志，日志责任若有需要应由 backend 承担。

作业匹配同时要求 alias、engine、rule 相等，并排除状态文本精确等于 `finished` 的记录；其他未知状态都会被当作活动作业。多个匹配项取数值最大的 ID，依赖 PD 保证作业字段合法。找到 ID 后的 `as u64` 转换没有显式协议校验，backend 不应把任意外部浮点值未经验证地传入核心逻辑。

取消错误原样传播，包括 Go 测试中模拟的 `job not found`。由于 `done` 已先置位，取消失败后本实例不会再次尝试。创建配置也具有同样的一次性失败语义。

参数合法性（例如 engine 只能是 TiKV/TiFlash、rule 与 engine 的组合约束、timeout 格式）不在本文件校验。Go [`distribute_table_test.go`](./distribute_table_test.go) 证明这些错误在 planner/更上游产生；新增 Rust 接线时不能误把 backend trait 当作完整的用户输入验证层。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。所有 backend 调用都是同步可变借用，`&mut self` 与 `&mut Context` 使同一执行器调用在类型层面串行；`DistributionBackend` 也没有 `Send`/`Sync` 要求。是否持有网络连接、定时器或请求上下文资源完全由 backend 实现决定。

等待生命周期由 `wait_or_cancel` 抽象：`Next` 本身只传入两次至多 500ms 的等待请求。实现必须在上下文取消时尽快返回错误；若简单阻塞满时长，会偏离 Go `select` 同时监听 timer 与 `ctx.Done()` 的语义。查询首轮成功时不等待，第二轮成功时只等待一次，三轮均失败时总计等待两次。

键范围和作业列表均完整保存在内存中。`getSchedulerJob` 消耗 backend 返回的 `Vec<SchedulerJob>`；`distributeTable` 为所有键分别建立临时引用向量并连接字符串。通常范围数因连续 ID 合并而较小，但大量不连续分区会线性增加内存与调度配置长度。

执行器没有显式 `Close`/`Drop` 行为。backend 若持有连接或其他资源，应由其自身 RAII 或外层执行框架管理；不能期待此文件在成功、错误或取消后执行清理回调。

## 与 Go 版本的对应关系

直接对照文件是 [`distribute.go`](./distribute.go)。两版保持了相同的调度器名称、两种执行器、`Open`/`Next` 一次性流程、分区 ID 排序与连续范围合并、稳定 alias、配置字段、三次查询、两次 500ms 等待、活动作业筛选、最大 job ID 以及取消语义。

Rust 用 `DistributionBackend` 替代 Go 对 `infosync`、`tablecodec`、`codec`、`tables`、InfoSchema 和 `context.Context` 的直接调用。`DistributionTable` 提前物化数据库名和分区映射；Go 则在执行时从 `TableInfo`/InfoSchema 查数据库名，并用 `tables.FindPartitionByName` 解析分区。Rust 的 `key_range` 必须在 adapter 中重建 Go 的 table prefix 编码、`codec.EncodeBytes` 与 URL/UTF-8 转义协议，当前测试 backend 的 `table-N` 只是确定性替身。

Rust `scheduler_jobs` 直接返回类型化 `SchedulerJob`；Go `getSchedulerJob` 自行把 `any` 解码成列表和 map，并记录空配置、类型错误及请求错误。Rust 因而把反序列化与日志边界移到 backend。两版都把查询失败当作暂时不可见并重试，也都在三次未找到后无 job ID 地成功返回。

Rust `partition_names.sort_by_key(|name| name.to_lowercase())` 与 Go 按 `ast.CIStr.L` 排序意图相同，但 Unicode 规范化细节没有跨版本证明。Rust 取消 `Next` 不接收 chunk；Go 为满足统一 `exec.Executor` 接口保留未使用的 chunk 参数。Rust 类型当前也未实现与 Go `BaseExecutor` 等价的统一 executor trait。

Rust 独立测试覆盖物理 ID 合并、缺失分区、大小写不敏感选择与 alias 排序、查询错误/不可见后的重试、过滤 finished/错误 alias、选择最大活动 ID、只提交一次及取消错误/只执行一次。Go [`distribute_table_test.go`](./distribute_table_test.go) 还提供 SQL 层证据：真实建表与分区语法、乱序分区、timeout、planner 参数错误以及取消不存在/存在作业；这些用例不能替代尚未存在的 Rust SQL 集成接线验证。

## 扩展指南

- 接入真实 Rust SQL 主链时，应新增或定位 `DistributionBackend` 的生产 adapter，并在 Rust builder 中把计划节点构造成这两个执行器。adapter 必须准确实现 Go 的 table key 编码/转义、PD scheduler API、类型化 job 解码、日志和上下文取消；随后增加独立 SQL/构建器测试，不能仅凭本文件单测宣称生产可用。
- 增加配置字段时集中修改 `distributeTable`，同步 [`distribute_table_test.rs`](./distribute_table_test.rs) 对完整 `BTreeMap` 的断言，并核对 Go `distributeTable` 和 PD API。兼容风险包括字段拼写、空值是否省略、逗号协议和 scheduler 版本。
- 修改分区选择或键范围合并时，以 `getKeyRanges` 为入口；覆盖非分区表、全部分区、大小写、乱序、不连续/连续 ID、缺失名及重复名。不要把测试放回生产 `.rs`，继续使用独立测试文件。
- 修改 alias 时同时检查 `Open` 的排序、`getAlias` 与 `getSchedulerJob` 的精确匹配，因为 alias 既是提交字段也是回查关联键。格式变化可能导致旧/新节点互相看不到作业，是滚动升级兼容风险。
- 修改重试策略时保持“配置创建只一次、查询可重试、等待可取消”的分层；增加零等待、首轮/末轮成功、查询连续错误和等待取消测试。更长或更密集轮询会增加 PD/微服务压力和 SQL 延迟。
- 若要支持错误后重试或执行器复用，需要显式重新设计 `done` 状态机和幂等协议。仅把 `done = true` 移到成功之后可能让未知结果的网络请求重复提交或重复取消。
- 若保留 `f64` job ID，应在 backend 解码边界验证有限、非负、整数且不超过 `u64`；若改成整数类型，要同步确认 PD JSON 协议与 Go 兼容性。

## 验证依据

- RustCodeGraph 索引：`status` 报告 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/distribute.rs` 确认目标文件已索引并含 20 个符号；`node --file ... --offset 1 --limit 500` 读取了全部 275 行。
- RustCodeGraph 符号与调用：查询/读取了 `DistributeTableExec`、`CancelDistributionJobExec`、`Next`、`getKeyRanges`、`getSchedulerJob`、`distributeTable`；确认 `getKeyRanges <- Open`、`getSchedulerJob <- Next`、`distributeTable <- Next` 及各 backend 下游边。精确类型的上游只出现独立 Rust 测试；同名 `Next` 的名称级伪边未作为证据。
- Rust 源与装配：完整读取 [`distribute.rs`](./distribute.rs)；读取 [`lib.rs`](./lib.rs) 的生产模块公开与 `#[cfg(test)]` 测试装配；读取 [`Cargo.toml`](./Cargo.toml) 的 package、lib、feature、porting metadata 和 `astersql-util-chunk` 依赖声明。目标包不存在 `doc.go`，因此没有可读的包级 Go 契约文件。
- Rust 独立测试：完整读取 [`distribute_table_test.rs`](./distribute_table_test.rs)，核对元数据、范围合并、分区错误、alias、配置、查询重试/过滤、最大 ID、一次性提交及取消传播。
- Go 对照与测试：完整读取 [`distribute.go`](./distribute.go)；用精确搜索核对 [`builder.go`](./builder.go) 的两个构造点；读取 [`distribute_table_test.go`](./distribute_table_test.go) 中 `TestDistributeTable` 和 `TestCancelDistributionJob`，并区分同文件其他 SHOW 类测试与本 Rust 文件职责。
- 人工复核：本文明确标出 Rust 已实现、独立测试覆盖和未验证生产接线三种状态，并回答文件存在原因、两条执行路径、状态/错误/资源边界及安全扩展位置。本任务按计划不运行 Cargo。
