# `pkg/executor/memtable_reader.rs`

## 文件定位

本文说明对应源码 [`memtable_reader.rs`](./memtable_reader.rs)。该文件属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs` 以 `pub mod memtable_reader` 暴露该模块，`pkg/executor/Cargo.toml` 则声明该 crate 对应 Go 包 `pkg/executor`。它移植了 Go 文件 `pkg/executor/memtable_reader.go` 中集群类内存表的执行和取数算法，覆盖 `cluster_config`、`cluster_load`、`cluster_systeminfo`、`cluster_hardware`、`cluster_log`、热点 Region 历史以及 `tikv_region_peers`。

这里的“内存表”不是把用户表数据常驻内存，而是把集群发现、HTTP/gRPC、PD 和 TiKV 元数据即时转换成 SQL 行的虚拟表。Rust 文件通过 `memTableRuntime` trait 隔离会话权限、事务、网络和存储访问；文件自身实现筛选、并发汇集、批量归并和行编码。

当前接线状态需要特别注意：仓库搜索只找到测试中的 `memTableRuntime` 实现，也没有发现生产 Rust 代码构造 `MemTableReaderExec` 或各 Retriever。因而本文件是公开、可测试的移植模块，但不能仅据此断言 Rust SQL 执行主链已经使用它；线上 Go 主链仍由同路径 Go 实现提供完整会话和网络接线。

## 核心职责

- `MemTableReaderExec` 提供 `Open`、`Next`、`Close` 生命周期，把统一执行器接口委托给具体 Retriever，并为五类巡检表实现会话级快照缓存。
- `memTableRetriever` 枚举统一分派五种实现：集群配置、节点负载/系统/硬件信息、集群日志、热点历史和 Region Peer。
- `memTableRuntime` 定义所有外部边界，包括权限、集群节点发现、HTTP/日志流/PD 查询、TiKV 存储检查、Region 到表索引的映射、时间格式化、告警和运行时统计注册。
- `clusterConfigRetriever` 与 `clusterServerInfoRetriever` 执行一次性、按节点过滤的并发采集。
- `clusterLogRetriever` 和 `hotRegionsHistoryRetriver` 使用最小堆归并多个远端有序结果，并分别以 `clusterLogBatchSize` 和 `hotRegionsHistoryBatchSize`（均为 256）限制单批输出。
- `tikvRegionPeersRetriever` 根据 Store/Region 条件去重 Region、筛选 Peer，并按 `DOWN > PENDING > NORMAL` 标注状态。

## 主要符号

- `MemTableError(String)` / `MemTableResult<T>`：本模块统一错误载体和结果别名。错误保留可读文本，但不保存结构化错误码或源错误链。
- `datum`、`datumRow`、`datumRowSet`：本地行模型，支持字符串、有符号/无符号整数、浮点、时间戳字符串和 `NULL`。
- `memTableRuntime`：关键适配 trait。其实现者必须提供事务探测与激活、`CONFIG`/`PROCESS` 权限判断、节点和 PD 发现、远端抓取、TiKV Region 查询、告警及统计注册。
- `MemTableReaderExec { tableName, retriever, cacheRetrieved, inspectionTableCache, runtime }`：顶层执行对象。`runtime` 同时保存在各具体 Retriever 中，构造方必须保证它们语义一致。
- `tableSnapshot { rows, error }`：巡检缓存项，同时缓存成功行或失败错误，保证同一轮巡检重复读取观察到相同结果。
- `clusterTableExtractor`、`clusterLogTableExtractor`、`hotRegionsHistoryTableExtractor`、`tikvRegionPeersExtractor`：承接计划侧下推条件；`skipRequest` 是共同的短路标志。
- `clusterConfigRetriever::retrieve` / `fetchClusterConfig`：检查 `CONFIG`，按节点类型拼接配置 URL，隐藏敏感配置项，按键排序，并按原节点顺序合并结果。
- `clusterServerInfoRetriever::retrieve`：Load/System 要求 `PROCESS`，Hardware 要求 `CONFIG`；并发拉取后按节点顺序扁平化成功行。
- `parseFailpointServerInfo`：解析 `type,address,status;...`；Rust 版对少于三个字段返回错误，而 Go 版直接按下标读取。
- `logResponseHeap` / `clusterLogRetriever`：按日志时间、再按节点类型维护最小堆；每个堆项持有一个流及当前消息缓冲。
- `hotRegionsResponseHeap` / `hotRegionsHistoryRetriver`：按 `updateTime`、再按 `hotDegree` 维护最小堆，并借助 `hot_region_table_mappings` 将一条热点记录展开为零到多条库表/索引行。
- `tikvRegionPeersRetriever`：实现 Store/Region 组合过滤、Region 去重和 Peer 状态编码。

## 执行流程

1. 构造方选择一个 `memTableRetriever` 变体，并注入实现了 `memTableRuntime` 的共享 `Arc`。本文件没有生产构造器；该步骤目前只在相关独立 Rust 测试中可见。
2. `MemTableReaderExec::Open` 先调用 `executor_open`。随后调用 `probe_transaction`；仅在返回 `Ok(true)` 时调用 `activate_transaction`。探测错误被 `unwrap_or(false)` 当作“无有效事务”，这一点与激活错误会直接向上传播不同。
3. `Next` 将表名转为小写。若存在 `inspectionTableCache` 且表名属于五个可缓存集群表，首次读取会复用已有 `tableSnapshot` 或调用 `memTableRetriever::read` 生成并写入快照；之后 `cacheRetrieved` 令后续调用返回空行集。非缓存路径直接调用 `read`。
4. `memTableRetriever::read` 按枚举变体分派。配置、节点信息和 Region Peer 在一次成功或开始尝试后把 `retrieved` 置为真；日志和热点历史用 `retrieving` 标记初始化、用 `isDrained` 标记流已耗尽。
5. 配置与节点信息路径使用 `thread::scope` 对节点并发。配置路径保存原始节点下标，过滤 `hidden`、按配置键排序，再按节点下标稳定合并；单节点失败转为 warning。节点信息路径同样按下标排序，但只收集成功结果。
6. 日志路径先验证 `PROCESS`、起止时间以及“至少一个模式/级别/实例/节点类型”约束，再为每个有效节点打开流。首次从各流取一批非空消息压入 `logResponseHeap`，随后不断弹出全局最早消息；某流缓冲耗尽时再取下一批，最多返回 256 行。
7. 热点路径先验证 `PROCESS` 和有界时间窗，再对“每个 PD × 每种热点类型”并发查询。成功且非空的响应进入 `hotRegionsResponseHeap`；每次弹出最早/热度最小的记录，经 TiKV 存储检查和 schema 映射展开，最多在循环条件检查时以 256 为批量目标。
8. Region Peer 路径先确保 TiKV 存储可用。无过滤时取全部 Region；仅 Store 过滤时按 Store 查询并以 Region ID 去重；带 Region ID 时从已按 Store 获取的映射中取交集，或在未指定 Store 时逐 ID 查询。最后逐 Peer 输出 leader、learner、状态和 down 秒数。
9. `Close` 先将 Retriever 的可选统计注册到 runtime，再调用 `shutdown`。只有日志 Retriever 持有可取消远端资源；其 `close` 对取消句柄 `take` 后调用 `cancel`，保证重复关闭不会重复取消。

## 数据与状态

- 顶层状态：`cacheRetrieved` 是当前执行器是否已经消费巡检快照的游标；`inspectionTableCache` 是按小写表名索引的可选共享快照。缓存同时保存错误，所以失败不会因重复读取而变成成功或触发第二次远端访问。
- 一次性 Retriever：`retrieved` 防止配置、节点信息和 Region Peer 重复扫描。当前实现通常在真正远端调用前置位，因此调用失败后再次调用也不会重试。
- 流式 Retriever：`retrieving` 区分是否已经创建远端请求，`isDrained` 表示堆已空。初始化失败会设置 `isDrained = true`，后续读取返回空集。
- 日志堆项 `logStreamResult` 同时拥有节点地址、类型、当前批消息和 `Box<dyn logStream>`。堆不复制流；弹出、补充、再压回完成所有权迁移。
- 热点堆项 `hotRegionsResult` 保存一个 PD 响应。每弹出一次就从其 `historyHotRegion` 头部移除一项，余项再入堆。
- Region Peer 使用 `BTreeMap`/`BTreeSet` 做确定性去重和状态查找。down 映射优先于 pending 集合；leader 通过与 `region.leader.id` 比较得出。
- `datum::Timestamp` 仍是字符串包装；具体时区和格式完全取决于 runtime 的 `format_timestamp`，不是本文件自行完成。

## 依赖与调用关系

RustCodeGraph 对文件给出的关键内部边包括：`MemTableReaderExec::Next -> memTableRetriever::read -> 各 Retriever::retrieve`，`clusterLogRetriever::retrieve -> initialize/startRetrieving/logResponseHeap::{Push,Pop}`，`hotRegionsHistoryRetriver::retrieve -> initialize/startRetrieving/hotRegionsResponseHeap::{Push,Pop}/getHotRegionRowWithSchemaInfo`，以及 `tikvRegionPeersRetriever::retrieve -> packTiKVRegionPeersRows`。

向下依赖被刻意压缩为标准库和 trait：`std::thread::scope` 提供有界线程并发，`Arc` 共享 runtime/cancellation，`BTreeMap`/`BTreeSet` 提供映射、集合和稳定顺序。HTTP、gRPC、PD client、session context、TiKV helper 等 Go 版直接依赖，在 Rust 中都隐藏在 `memTableRuntime`、`logStream` 和 `cancellation` 后面；因此 `Cargo.toml` 没有为本文件声明专属网络依赖或 feature gate。

向上接线方面，`pkg/executor/lib.rs` 公开模块并在测试配置下以 `#[path = "memtable_reader_test.rs"]` 装载独立测试。仓库内对 `MemTableReaderExec`、`memTableRuntime` 和具体 Retriever 的 Rust 引用只出现在本文件及 `memtable_reader_test.rs`、`cluster_table_test.rs`、`hot_regions_history_table_test.rs`、`tikv_regions_peers_table_test.rs` 等测试中；未找到生产 runtime 实现或 builder 构造边。因此当前可验证的是模块内部算法和测试入口，而不是 Rust SQL 请求到本执行器的完整调用链。

## 错误处理与边界

- 权限是硬错误：配置/Hardware 缺 `CONFIG`、Load/System/日志/热点缺 `PROCESS` 会立即失败。
- 日志和热点历史必须有非零起止时间；日志还拒绝没有模式、级别、实例和节点类型条件的全表扫描，防止诊断接口过载。
- `skipRequest`、`retrieved`、`isDrained` 都返回空结果而不是错误，表示计划已判空或数据已经消费完。
- 配置、日志、热点的部分节点失败通常调用 `append_warning` 后继续返回其他节点数据；缺少 `statusAddr` 也按 warning 跳过。线程 panic 在配置/日志/热点路径会转换成错误或 warning 文本。
- 节点信息 Retriever 对工作线程 panic 和 `fetch_server_info` 错误直接丢弃，只收集 `Ok`；该行为与其他路径的 warning 策略不同，扩展时不能假定所有部分失败均可见。
- `Open` 吞掉 `probe_transaction` 的错误；这是源码事实，调用方若要求事务探测错误可见，需要修改该符号并补回归测试。
- 热点输出一条远端记录可能映射到多条 schema 行，所以循环以进入迭代前的 `rows.len() < 256` 为条件，并不能保证最终行数严格不超过 256。
- Region Peer 的 `u64` ID 在若干筛选结构中以 `as i64` 转换；超出 `i64::MAX` 时会发生补码转换。当前代码和测试没有证明这类极值输入的兼容性。
- `parseFailpointServerInfo` 接受三个以上字段并忽略多余字段，拒绝少于三个字段；测试覆盖了多节点和短行错误。

## 并发与资源生命周期

配置、节点信息和热点请求使用 `thread::scope`，所以子线程不能逃逸函数作用域，函数返回前必定 join；共享 runtime 必须满足 `Send + Sync`。结果先携带原节点下标再排序，避免并发完成顺序改变配置和节点信息输出。热点结果由堆按业务时间重新排序，而不是按线程完成顺序。

日志 Retriever 的长期资源由 `Box<dyn logStream>` 和共享 `Arc<dyn cancellation>` 表示。`startRetrieving` 创建一次 cancellation 并克隆给每个流；`close` 取得并清空句柄后取消，`MemTableReaderExec::Close` 是保证该清理发生的外层入口。与 Go 版后台 goroutine/通道不同，Rust runtime 把“下一批消息”封装为同步的 `next_messages` 调用；是否真正异步、是否阻塞线程、连接何时释放均取决于尚未存在的生产 runtime 实现。

其他 Retriever 组合 `dummyCloser`，关闭无副作用且没有运行时统计。文件中没有显式锁；测试 runtime 用 `Mutex` 记录请求，但这是测试适配器的选择，不是生产契约。巡检缓存作为 `MemTableReaderExec` 的可变字段使用，本文件本身不支持同一执行器实例的并发 `Next`。

## 与 Go 版本的对应关系

Rust 的类型和函数名大体逐一对应 `pkg/executor/memtable_reader.go`：两个批量常量、`MemTableReaderExec`、五种 Retriever、两种最小堆、热点 DTO 和 Region Peer 打包逻辑都保留了 Go 命名（包括 `hotRegionsHistoryRetriver` 的拼写）。权限、过滤、稳定排序、日志/热点多路归并、巡检缓存和关闭取消的主要语义也保持一致。

主要差异如下：

- Go 的 `memTableRetriever` 是接口，Rust 用闭合枚举分派；新增 Retriever 必须同时修改枚举以及 `read`、`shutdown`、`runtime_stats` 三处分支。
- Go 直接依赖 session context、HTTP/gRPC、PD/TiKV helper；Rust 把这些动作全部委托给 `memTableRuntime`。这提高了可测试性，但生产等价性取决于未来 runtime 适配器。
- Go `Next` 把 datum 行拷贝进 `chunk.Chunk`；Rust `Next` 直接返回 `datumRowSet`，说明它尚未接入 Go 等价的 Rust executor/chunk ABI。
- Go 日志使用 goroutine 和 channel 持续接收流，Rust 使用 `logStream::next_messages` 的拉取接口和作用域线程打开流。
- Go 的 failpoint 解析假定字段完整；Rust 返回 `MemTableError`，防止短输入越界。
- Rust 配置 URL 逻辑与当前 Go 一致，覆盖 PD、TiKV、TiDB、TiFlash、TiCDC、TiProxy、TSO 和 Scheduling；隐藏项过滤及键排序也对应 Go 行为。
- Go 是当前完整生产实现；Rust 只有公开模块和测试接线。文档中的“支持”均指本文件已表达相应算法，不代表生产 Rust 服务路径已启用。

## 扩展指南

- 新增一类内存表时，先增加具体 Retriever 和所需 extractor/DTO，再扩展 `memTableRetriever` 的枚举及 `read`、`shutdown`、`runtime_stats` 全部分派；若表可用于巡检快照，还要同步 `isInspectionCacheableTable`。
- 新增外部 I/O 能力应优先扩展 `memTableRuntime` 或专用子 trait，并同步所有测试 runtime 实现。不要在算法中硬编码真实 HTTP/PD client，否则会破坏当前边界和确定性测试方式。
- 修改日志或热点归并规则时，同步验证 `Less` 的严格顺序、空消息不入堆、流补批和 256 行批量边界；对应独立测试位置是 `cluster_table_test.rs`、`memtable_reader_test.rs` 和 `hot_regions_history_table_test.rs`。
- 修改 Region 筛选或状态优先级时，同步 `tikv_regions_peers_table_test.rs`，尤其覆盖无过滤、仅 Store、仅 Region、Store+Region 交集、重复 Region、leader/pending/down 组合。
- 修改权限、时间窗、warning 或错误传播时必须与 `memtable_reader.go` 逐分支对照；Rust 测试应继续放在独立 `*_test.rs` 文件，不要内嵌到生产源文件。
- 若要完成生产接线，需要新增真实 `memTableRuntime` 实现、在 Rust executor builder 构造 `MemTableReaderExec`、适配 chunk/会话上下文，并验证取消和连接释放；这些工作不属于当前纯文档任务，也不能用现有测试 fixture 代替。
- 性能风险集中在每次节点/PD 扇出的 OS 线程数量、`Vec::remove(0)` 的线性搬移、热点一对多展开突破目标批量以及全量 Region 扫描；扩展规模前应建立对应基准或限流策略。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；通过 `explore "MemTableReaderExec"`、`query MemTableReaderExec/memTableRetriever/clusterConfigRetriever` 以及 `node --file pkg/executor/memtable_reader.rs` 分段读取全部 1,325 行。
- 调用边查询：对 `MemTableReaderExec.Next`、`clusterLogRetriever.retrieve`、`hotRegionsHistoryRetriver.retrieve`、`tikvRegionPeersRetriever.retrieve` 执行 `callees`；图在重名时优先解析到 Go 符号，因此 Rust 内部精确边同时由文件级索引源码核对。`callers` 未给出 Rust 生产构造点，与仓库 `rg` 搜索结果一致。
- crate 与模块边界：读取 `pkg/executor/Cargo.toml`，并核对 `pkg/executor/lib.rs:146` 的公开模块声明和 `pkg/executor/lib.rs:539-540` 的独立测试模块声明。
- Go 对照：通过 RustCodeGraph 分段读取 `pkg/executor/memtable_reader.go` 全部相关实现，重点核对 `MemTableReaderExec`、配置/节点信息、日志、热点历史和 Region Peer 流程。
- Rust 测试：读取完整 `pkg/executor/memtable_reader_test.rs`；同时核对 `pkg/executor/cluster_table_test.rs`、`pkg/executor/hot_regions_history_table_test.rs`、`pkg/executor/tikv_regions_peers_table_test.rs` 的测试入口与 runtime fixture。它们分别覆盖 failpoint 解析、ILIKE 日志过滤及关闭、日志堆、热点权限/时间窗/扇出/归并/warning、Region 查询组合和 Peer 状态。
- Go 测试：核对 `pkg/executor/memtable_reader_test.go` 中 `TestTiDBClusterConfig`、`TestTiDBClusterLog` 和 `TestTiDBClusterLogError` 等回归入口。它是 Go 生产路径的行为证据，不等同于 Rust 生产接线证据。
- 人工范围检查：仓库搜索未发现生产 Rust `impl memTableRuntime` 或 `MemTableReaderExec` 构造点；因此本文明确标注当前迁移边界，没有把测试适配器描述为线上实现。
