# `pkg/executor/split.rs`

## 文件定位

本文件属于 `astersql-executor` crate；`pkg/executor/Cargo.toml` 将库入口设为 `lib.rs`，而 `pkg/executor/lib.rs:231-232` 以 `pub mod split` 公开本模块。它承载两组相关能力：一组是 `SPLIT TABLE` / `SPLIT TABLE ... INDEX ...` 的切分键生成、Region 切分与 scatter 等待；另一组是加载 Region、补齐统计与 scatter 状态、把 TiKV 键格式化为可读字符串的查询辅助逻辑（`pkg/executor/split.rs:178-425,459-728`）。

在完整 SQL 链路中，规划结果会在 `pkg/executor/builder.rs:2443` 进入 `buildSplitRegion`，该函数按 `SplitRegionPlanData::has_index` 选择 `ExecutorKind::SplitIndexRegion` 或 `ExecutorKind::SplitTableRegion`，再交给通用依赖工厂构造（`pkg/executor/builder.rs:3852-3865`）。需要注意：仓库生产 Rust 代码中未找到 `SplitRuntime` 的实现，也未找到 `SplitIndexRegionExec` / `SplitTableRegionExec` 的直接实例化；因此本文件当前提供的是完整、可测试的泛型算法边界，但不能仅凭本文件证明 builder 已把真实 TiKV/PD runtime 接到这些泛型执行器上。

## 核心职责

- `SplitIndexRegionExec` 与 `SplitTableRegionExec` 把语句参数展开到逻辑表或选定分区的物理 ID，支持显式值列表和 `lower` / `upper` / `num` 两种切分点来源（`getSplitIdxKeys`、`getSplitTableKeys`）。
- `Open` 预计算切分键并清空一次执行状态；`Next` 保证切分只执行一次，并输出“切出的 Region 数”和“完成 scatter 的比例”两列。
- `waitScatterRegionFinish` 在共享总超时预算内逐个等待 Region scatter；上下文结束后仍以 50ms 短退避检查剩余 Region，使结果能统计已经完成的后续 Region。
- `getPhysicalTableRegions` / `getPhysicalIndexRegions` 负责查询键范围内的 Region。前者合并记录范围和所有 public 索引范围，用调用方提供的 `HashSet<u64>` 跨范围去重。
- `getRegionMeta`、`getRegionInfo`、`checkRegionsStatus` 和 `regionKeyDecoder` 把原始 Region 描述转换为展示所需的 leader/store、统计、scatter 状态和可读起止键。
- `SplitRuntime` 隔离元数据、tablecodec、chunk、上下文以及 TiKV/PD I/O；本文件本身不持有具体客户端，也不实现真实存储访问。

## 主要符号

- `checkScatterRegionFinishBackOff: i32 = 50`：上下文已结束后检查单个 scatter 的短超时，单位由 runtime 契约定义为毫秒。
- `splitRegionResult { splitRegions, finishScatterNum }`：一次语句的计数结果；最终第二列由二者相除得到比例。
- `RegionDescriptor`、`RegionStatistics`、`regionMeta`：分别表示存储返回的 Region 身份/键范围、可选统计数据，以及面向展示的聚合结果。`regionMeta.physicalID` 保留该 Region 本次是从哪个物理表范围发现的。
- `SplitRuntime`：模块唯一的外部能力端口。关联类型抽象 `Context`、`Chunk`、`Datum`、`HandleColumns` 与 `Error`；方法覆盖分区/表/索引元数据、键编码、范围插值、Region split/scatter、Region 加载与统计、前缀生成以及告警。
- `SplitIndexRegionExec<R>`：索引切分执行器。公开方法包括 `Open`、`Next`、`splitIndexRegion` 及按值/范围生成键的各级方法。
- `SplitTableRegionExec<R>`：记录 handle 切分执行器，与索引执行器结构对称，额外保存 `handleCols`。
- `selectedPhysicalIDs`：内部公共分区解析器；非分区表返回逻辑表 ID，未指定分区时返回所有分区，显式名称按语句顺序、忽略 ASCII 大小写匹配，未知名称报错。
- `waitScatterRegionFinish`、`appendSplitRegionResultToChunk`、`isCtxDone`：切分执行阶段的共享辅助函数。
- `getPhysicalTableRegions`、`getPhysicalIndexRegions`、`checkRegionsStatus`、`getRegionMeta`、`getRegionInfo`：Region 查询和元数据组装管线。
- `decodeRegionsKey` 与 `regionKeyDecoder::decodeRegionKey`：批量/单键格式化入口；私有的 `decode_comparable_i64` 恢复 TiDB comparable int 被翻转的符号位，`hex` 提供不可结构化部分的稳定十六进制回退。

## 执行流程

索引和表执行器共享同一生命周期。第一步，`Open` 调用 `getSplit*Keys`：若 `valueLists` 非空，就对每个目标物理 ID 编码每行显式值；否则把上下界和 Region 数交给 runtime 的范围切分算法。索引值列表路径还先调用 `index_start_and_boundary_keys`，与 Go 的 `regionsplit.GetSplitIdxPhysicalStartAndOtherIdxKeys` 对齐。任何编码、范围校验或分区解析错误都会从 `Open` 返回。

第二步，第一次 `Next` 先重置输出 chunk，再把 `done` 置为 `true`，调用 `splitIndexRegion` 或 `splitTableRegion`。执行器把预计算键、上下文和逻辑表 ID交给 `SplitRuntime::split_regions`；成功返回的 Region ID 数写入 `splitRegions`。split I/O 失败会调用 `warn_split_failed` 并按空 ID 列表继续，这是与 Go `split.go:93-120,259-288` 一致的“告警而非语句失败”语义。

第三步，只有 Region ID 非空且 `wait_split_region_finish()` 为真时才等待 scatter。`waitScatterRegionFinish` 从语句开始时刻扣减总超时；若上下文已经结束，则不立即退出，而是对每个剩余 ID 使用 50ms 短检查。单个等待失败只告警，成功数累加。最后 `appendSplitRegionResultToChunk` 写入总数与 `finishScatterNum / splitRegions`；后续 `Next` 只清空 chunk 并返回。

Region 查询路径从记录或索引键范围调用 `load_regions`，通过 `getRegionMeta` 以 Region ID 去重、复制 leader/store/physical ID、可选加载统计并解码起止键。表级查询随后遍历所有 public 索引并合并结果，最后 `checkRegionsStatus` 为每个保留 Region 查询 scatter 状态；索引级查询只处理一个索引范围。

## 数据与状态

两个执行器的输入状态包括分区名、上下界、目标 Region 数和值列表；表执行器还持有 handle 列描述。`splitIdxKeys` / `splitKeys` 是 `Open` 产生、`Next` 消费的缓存，`done` 是一次性游标状态，`splitRegionResult` 是当前执行结果。`Open` 会把后三者重置，因此复用实例时必须先重新打开。

`selectedPhysicalIDs` 保持显式分区名的输入顺序，也不会主动去重重复名称；若 runtime 返回分区列表而调用方未指定名称，则保持 runtime 的分区顺序。切分键顺序因此由“物理 ID 顺序 × 每个物理 ID 的值/范围算法”共同决定。

Region 展示数据的去重状态不藏在模块全局变量中，而由调用方传入 `HashSet<u64>`。这允许调用方在记录范围、多个索引范围乃至多个调用之间共享去重集合；同一 Region 横跨键范围时只保留第一次遇到的 `physicalID` 和解码上下文。`region_statistics` 返回 `None` 时统计字段维持默认零值，而不是报缺失错误。

## 依赖与调用关系

上游静态入口是 `Plan::SplitRegion -> ExecutorBuilder::buildSplitRegion`（`pkg/executor/builder.rs:2443,3853`），但该 builder 只选择 `ExecutorKind` 并调用依赖工厂。对仓库生产 `.rs` 文件的检索未找到 `impl SplitRuntime` 或两个泛型执行器的实例化，所以从 builder 到本文件具体类型的生产调用边目前未验证；测试通过 mock runtime 直接调用本文件。

本文件直接依赖标准库的 `HashSet`、`Duration` 和 `Instant`，其余全部下游能力由 `SplitRuntime` 注入：键生成映射到 tablecodec/regionsplit 语义，`split_regions` 与 `wait_scatter_region_finish` 映射到 `kv::SplittableStore`，Region 加载与统计映射到 TiKV region cache 和 PD HTTP 信息。crate 边界由 `pkg/executor/Cargo.toml` 的 `astersql-executor` 包声明确认；本文件没有条件编译项，模块单元测试通过 `pkg/executor/lib.rs:572-574` 的独立 `split_test.rs` 接入。

测试侧主要调用关系包括：`pkg/executor/split_test.rs` 直接验证 `regionKeyDecoder` 和 `selectedPhysicalIDs`；`pkg/executor/test/splittest/split_table_test.rs` 以 `MockPdRuntime` 实现 `SplitRuntime`，驱动 `Open -> Next -> split_regions/wait -> getPhysicalTableRegions/getRegionMeta` 的完整内存链路。

## 错误处理与边界

- 键生成错误、未知分区、Region 加载失败、统计查询失败和 scatter 状态查询失败都通过 `R::Error` 返回；两个执行器要求 `R::Error: From<String>`，以承接 `selectedPhysicalIDs` 的未知分区错误。
- `split_regions` 失败与单 Region scatter 等待失败被有意降级为告警；前者输出零个 split，后者只降低完成比例。扩展时不能无意把这两类错误改为硬失败，否则会偏离 Go 行为。
- `wait_split_timeout().saturating_sub(elapsed)` 避免 Rust duration 下溢，毫秒值再截到 `i32::MAX`。超时为零仍会调用 runtime，由 runtime 解释零毫秒语义。
- `appendSplitRegionResultToChunk` 仅在成功数和总数都大于零时做除法，避免除零；它不强制 `finishScatterNum <= splitRegions`，该不变量依赖等待循环每个 ID 至多计数一次。
- `regionKeyDecoder` 按“当前索引前缀、记录前缀、表前缀、裸 `t`、纯十六进制”的优先级解释键。记录后缀恰为 8 字节时按 comparable i64 解码；无符号主键只改变显示转换。长度不足 8 字节时安全回退十六进制。表级 `_i` 分支只有在至少 8 字节 index ID 可解码时才切片 `encoded_index[8..]`。
- 与 Go 不同，Rust 的 `getPhysical*Regions` 要求调用方总是传入有效 `HashSet`，没有 `nil` 时自动创建集合的分支。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁或事务。两个执行器通过 `&mut self` 串行推进；`done` 只保证单个实例在顺序调用下恰好执行一次，并不是跨线程同步原语。`SplitRuntime` 也未声明 `Send` / `Sync` 约束。

外部资源生命周期由 runtime 管理。执行器只借用 `Context` 与 `Chunk`，没有保存它们；Region ID、键和元数据以拥有所有权的 `Vec` 在调用间传递。scatter 总预算从进入 `split*Region` 时的 `Instant` 开始，所有 Region 顺序共享同一预算。与 Go 版本的 `context.WithTimeout` 不同，Rust 模块自身不创建或取消超时上下文，runtime 必须让 `context_done`、`wait_split_timeout` 和 `wait_scatter_region_finish` 协同实现等价生命周期。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/split.go`。Rust 保留了 Go 的两种执行器、值列表/边界两类键生成、分区展开、一次性 `Next`、split 失败只告警、可选等待 scatter、两列结果、Region 去重、状态/统计填充和键解码结构。主要算法对应关系是：

- Go `SplitIndexRegionExec` / `SplitTableRegionExec` 对应同名 Rust 泛型结构；Go 的 TiDB 具体字段被收敛到 `SplitRuntime`。
- Go `tables.FindPartitionByName` 的循环被统一为 `selectedPhysicalIDs`；Rust 测试证明忽略 ASCII 大小写并保持语句顺序，但错误文本是 Rust 自有字符串。
- Go `regionsplit.GetSplitIndexKeys` / `GetSplitTableKeys`、index key 和 record key 编码被委托给 runtime，使本文件不复制这些算法。
- Go 的 `kv.SplittableStore` 类型断言、session variables、日志器、region cache 和 PD client 被 runtime 方法取代。Rust 没有在本文件内表达“不支持 SplittableStore 时静默返回”的具体分支；具体实现必须自行保持该兼容行为。
- Go `getRegionInfo` 在存储不是 `kv.EtcdBackend`、无 PD 地址时返回原数据；Rust 将这一选择下放给 `region_statistics -> Result<Option<_>>`，其中 `None` 表示没有统计。
- Go 在表 split 上为 context 标注 `kv.InternalTxnDDL`，Rust 本文件未直接设置此标记；是否等价由未来生产 runtime/context 接线决定，当前未验证。

独立 Rust 单元测试 `pkg/executor/split_test.rs` 覆盖记录/索引/无符号 handle 解码、分区顺序和未知分区错误。更完整的 `pkg/executor/test/splittest/split_table_test.rs` 覆盖上下界和值列表表切分、索引切分、跨记录/索引范围去重、无符号 handle、统计/状态以及显式分区过滤，并在注释和断言中对照 `pkg/executor/split_test.go` 的期望。

## 扩展指南

接入真实生产执行链时，最关键的新增点是提供 `SplitRuntime` 实现，并证明依赖工厂构造的 `ExecutorKind::{SplitIndexRegion,SplitTableRegion}` 实际驱动本文件的 `Open` / `Next`。实现必须复用现有 tablecodec/regionsplit、TiKV region cache、PD 和 session context 能力，而不是在本文件复制一套简化协议；同时核对表 split 的 `InternalTxnDDL` context 标记、非 splittable store 的静默兼容以及超时 context 的创建/取消。

增加新的切分方式时，应先扩展 `getSplitIdxKeys` / `getSplitTableKeys` 的输入分派，再把具体编码能力放到 `SplitRuntime` 或既有 regionsplit crate；必须保持每个分区独立生成键和执行顺序。改变分区选择应修改 `selectedPhysicalIDs`，并同步 `pkg/executor/split_test.rs` 的顺序、大小写、未知名称与重复名称测试。

改变 scatter 语义应集中修改 `waitScatterRegionFinish`，同步验证总超时、已取消 context、部分成功、告警参数与比例计算。改变 Region 展示应同步检查 `getPhysicalTableRegions`、`getRegionMeta` 和 `regionKeyDecoder`，特别关注跨索引去重、无符号 handle、非整型/common handle、未知前缀与缺失 PD 统计。

测试逻辑应继续放在独立文件：窄单元测试放 `pkg/executor/split_test.rs`，端到端 mock Region 行为放 `pkg/executor/test/splittest/split_table_test.rs`；不要把测试内嵌回 `split.rs`。兼容性风险主要来自键编码/显示文本与 Go 不一致，正确性风险来自重复或遗漏物理分区和 Region，性能风险来自对每个 Region 串行进行统计及 scatter 状态 RPC。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`query SplitTableRegion`、`query SplitIndexRegion` 和 `query SplitRegion` 定位到 Rust/Go 实现、planner 与 builder。对限定符号执行 `explore`、`callers`、`callees` 未返回可用边，因此调用接线又以源码检索核验，并明确保留“生产 runtime 未接线”的限制。
- 生产源码：完整阅读 `pkg/executor/split.rs`；读取 `pkg/executor/lib.rs:231-232,572-574` 的模块/测试声明和 `pkg/executor/builder.rs:2443,3852-3865` 的规划入口。
- crate 配置：读取 `pkg/executor/Cargo.toml`，确认包名 `astersql-executor`、`lib.rs` 入口、`nextgen` feature 以及 `pkg/executor` 的 Go 移植元数据；`split.rs` 自身不受 feature 条件控制。
- Go 对照：读取 `pkg/executor/split.go` 的执行器、scatter、结果输出、Region 加载/去重、统计和键解码实现，并检索 `pkg/executor/split_test.go` 的 `TestSplitIndex`、`TestSplitTable`、步长边界与 clustered index 场景。
- Rust 测试：完整阅读 `pkg/executor/split_test.rs`；阅读 `pkg/executor/test/splittest/split_table_test.rs:823-1019` 的表/索引、值列表、去重和分区过滤回归，并检索该文件其余覆盖点。
- 静态接线检查：在生产 Rust 文件中检索 `impl SplitRuntime`、两个执行器实例化以及 `getPhysical*Regions` 调用，除本文件外无匹配；因此没有声称真实 PD/TiKV 生产接线已完成。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付结构检查要求本文恰有十一个固定二级标题。
