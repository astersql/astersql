# `pkg/executor/show_affinity.rs`

## 文件定位

本文件属于 `astersql-executor` crate；`pkg/executor/Cargo.toml` 将 crate 根指定为 `lib.rs`，而 `pkg/executor/lib.rs` 以 `pub mod show_affinity` 导出本模块。它承载 `SHOW AFFINITY` 的 Rust 侧核心数据模型与行生成算法：从运行时取得带亲和属性的表，按表级或分区级展开亲和组，批量读取组状态，再生成八列展示行。

当前接线状态必须与算法本身区分。仓库全局 Rust 搜索显示，生产代码尚未实现 `ShowAffinityRuntime`，也没有调用 `fetchShowAffinity`；直接调用者仅见 `pkg/executor/show_affinity_test.rs`。Rust 的统一 SHOW 执行器 `pkg/executor/show.rs` 虽定义 `ShowStmtType::Affinity`，但在 `fetchAll` 中把它交给通用的 `fetchOperation(ShowOperation::Affinity)`，没有转入本文件。因此，本文件是可测试的移植核心和适配边界，并非当前 Rust 生产 SHOW 主链中已经接通的实现。

## 核心职责

- `fetchShowAffinity` 实现与 Go `(*ShowExec).fetchShowAffinity` 对齐的纯编排：应用精确表名及 LIKE 过滤、按亲和级别生成组 ID、一次取得所有组状态、编码展示行。
- `ShowAffinityRuntime` 把 InfoSchema、DDL 组 ID、PD/亲和管理器、SHOW 行缓冲等环境能力抽象为 trait，使算法不依赖具体会话或网络客户端。
- `AffinityLevel`、`AffinitySchemaTables`、`AffinityTable`、`AffinityPartition` 描述算法所需的最小元数据投影；`AffinityState` 描述亲和组状态快照；`ShowAffinityValue` 表示输出单元格及原生 `NULL`。
- 本文件不负责 SQL 解析、列定义、WHERE 结果后过滤、InfoSchema 扫描或 PD HTTP 调用的具体实现；这些能力应由生产适配器及 SHOW 主链提供。

## 主要符号

- `pub enum AffinityLevel { Table, Partition, Other }`：决定一张表生成一个表级组，还是每个分区生成一个组；`Other` 被显式忽略，防止未知级别被误当成已支持语义。
- `pub struct AffinityPartition { id, name }`：分区 ID 用于构造组 ID，原始名称写入结果第三列。
- `pub struct AffinityTable`：保存表 ID、展示名、小写匹配名、可选亲和级别和分区列表。`level == None` 表示未配置亲和，直接跳过。
- `pub struct AffinitySchemaTables`：把数据库展示名与该库的候选表组合起来。
- `pub struct AffinityState`：包含 leader Store ID、voter Store ID 列表、阶段、Region 总数及满足亲和约束的 Region 数。
- `pub enum ShowAffinityValue`：当前仅有 `String`、`U64`、`Null`，对应本查询所需的字符串、无符号数和 SQL NULL。
- `pub trait ShowAffinityRuntime`：公开的环境契约。关联类型 `Context` 和 `Error` 保留具体上下文及错误类型；七个方法分别提供过滤器、候选表、两类组 ID、状态读取与结果写出。
- `struct TablePartitionInfo`：私有中间记录，将数据库、表、可选分区与组 ID 固化，隔开“元数据展开”和“状态编码”两个阶段。
- `pub fn fetchShowAffinity<R: ShowAffinityRuntime>(...) -> Result<(), R::Error>`：唯一算法入口。函数名保留 Go 风格，文件用 `#![allow(non_snake_case)]` 允许该命名。

## 执行流程

1. `fetchShowAffinity` 读取 `runtime.field_filter()`；`None` 归一为空字符串，同时取得 `tables_with_affinity()` 返回的候选库表。
2. 遍历库和表。没有亲和级别的表直接跳过；非空精确过滤要求 `lowercase_name` 完全相等；无论精确过滤是否存在，随后仍调用 `field_pattern_matches`。这一顺序由 Rust 测试 `exact_filter_does_not_bypass_like_pattern` 固定。
3. 对 `AffinityLevel::Table` 生成一条 `TablePartitionInfo`，其分区名为 `None`，组 ID 来自 `table_group_id(table.id)`。
4. 对 `AffinityLevel::Partition` 的每个分区各生成一条记录，组 ID 来自 `partition_group_id(table.id, partition.id)`；空分区列表自然产生零行。`Other` 不产生记录。
5. 元数据全部展开后，函数只调用一次 `all_group_states(context)`。若调用失败，`?` 立即返回，尚未进入写行循环，因此不会输出半成品行。
6. 对每个中间记录按组 ID 查状态。存在状态时，非零 leader 编码为 `U64`，voter 按原顺序转十进制并以逗号连接；`pending`、`preparing`、`stable` 映射为首字母大写，未知阶段原样透传；两个计数编码为 `U64`。
7. 组状态缺失时，五个状态相关字段全部为 `Null`。最后按数据库、表、分区、leader、voters、status、Region 数、亲和 Region 数的固定顺序调用 `append_row`。

## 数据与状态

算法只在栈上维护过滤字符串、候选表所有权、`infos: Vec<TablePartitionInfo>` 和一次性的 `HashMap<String, AffinityState>` 快照，不保存跨调用状态。`Vec::with_capacity(tables.len())` 只按库级结果数预分配；分区展开可能触发扩容，但不影响语义。

表级行的分区列为 `Null`，分区级行为分区名称。`leader_store_id == 0` 和空 voter 列表分别表示未知/尚无对应 Store，均显示为 `Null`，而不是数值零或空字符串。只要组状态存在，两个计数即使为零也显示 `U64(0)`；只有组状态整体缺失时才显示 `Null`。组状态快照按 `group_id` 查找，重复组 ID 会让多行共享同一快照，但本函数不去重或校验 ID 唯一性。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::HashMap`；本文件没有直接引用 `astersql-domain-affinity`、`astersql-ddl` 或 InfoSchema crate。`pkg/executor/Cargo.toml` 声明了这些执行器级依赖，但具体调用被刻意留在 `ShowAffinityRuntime` 的未来生产实现中。

下游抽象调用边为：`fetchShowAffinity` → `field_filter` / `tables_with_affinity` / `field_pattern_matches` → `table_group_id` 或 `partition_group_id` → `all_group_states` → `append_row`。RustCodeGraph 将这些 trait 方法和本地入口识别为本文件的主要调用关系；独立测试通过 `AffinityRuntime` 实现全部边界。

上游方面，`pkg/executor/lib.rs` 只负责模块导出及测试模块注册。当前 Rust 直接调用者均在 `pkg/executor/show_affinity_test.rs`，没有生产调用边。作为对照，Go 的 `pkg/executor/show.go` 会在 `ast.ShowAffinity` 分支调用 `e.fetchShowAffinity(ctx)`，其实现直接连接 Domain/InfoSchema、DDL 组 ID 与 affinity 管理器。

## 错误处理与边界

唯一可失败操作是 `ShowAffinityRuntime::all_group_states`；错误类型完全由运行时决定，函数不包装、不转换，直接通过 `Result<(), R::Error>` 传播。`group_state_error_is_propagated_without_rows` 验证错误原样返回且结果行为空。

过滤语义依赖调用者预先提供的小写 `field_filter` 与 `AffinityTable::lowercase_name`；函数自身不做大小写折叠。LIKE 的具体通配、排序规则和转义语义也全部属于 `field_pattern_matches` 的实现责任。候选表快照中的 `level: None`、未知级别和空分区集合都被安全跳过。

状态映射不验证 Region 计数间关系，也不排序或去重 voter；这是对上游状态快照的忠实展示。未知 phase 会原样显示，以便兼容 PD 新阶段。函数也不排序结果，输出顺序等于候选库表及分区输入顺序。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或网络连接。`runtime` 和 `context` 均以独占可变引用借入，保证一次调用期间不能由同一 Rust 所有权路径并发修改；实际运行时若内部共享资源，线程安全与取消语义由其实现负责。

状态读取是一次批量快照，随后仅在本地 `HashMap` 上查找；没有逐组网络往返。所有输入投影和状态快照在函数返回时释放，写出的行由 `append_row` 的实现接管。若状态读取报错，局部中间记录随栈展开释放，且不会调用 `append_row`。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/show_affinity.go`。Rust 保留了相同的两段式结构：先从带亲和属性的表构造 `tablePartitionInfo`，再调用一次全量状态查询并生成八列。表级/分区级组 ID、双重过滤、零 leader、空 voters、三种已知 phase、未知 phase 透传以及组缺失时五列原生 NULL 均保持一致。

实现形式存在两类差异。第一，Go 方法直接依赖 `ShowExec`、Domain/InfoSchema、DDL 和 affinity 包；Rust 把这些依赖拆为 `ShowAffinityRuntime`，使用自有数据投影和单元格枚举。第二，Go 已由 `pkg/executor/show.go` 的 SHOW 分发调用，且 `pkg/executor/show_affinity_test.go` 通过 SQL/TestKit 验证端到端行为；Rust 当前只有算法级测试，尚无生产 runtime 适配和从 `ShowExec` 到本入口的调用边。

Go 测试还覆盖 SQL 侧 WHERE/LIKE 后过滤、真实列数和 PD mock。Rust 本文件只负责它能观察到的表名精确过滤与 LIKE 谓词调用；更广的 WHERE 条件应由统一 SHOW 层处理，不能据此声称本函数单独实现了完整 SQL 过滤。

## 扩展指南

- 接通生产链时，应在执行器层实现 `ShowAffinityRuntime`，把 InfoSchema 的亲和表投影为本文件类型，把 DDL 表/分区组 ID 和 affinity 全量状态查询接入，并将 `ShowAffinityValue` 转换到统一 SHOW 行类型；同时在 `pkg/executor/show.rs` 的 `Affinity` 分支明确调用本入口。必须新增独立 Rust 集成测试验证 SQL 分发，不能把测试嵌入本源文件。
- 新增亲和级别时，应扩展 `AffinityLevel` 及 `fetchShowAffinity` 的匹配分支，并同步 `pkg/executor/show_affinity_test.rs`；不要让 `Other` 静默代表一个已有业务级别。
- 新增或调整展示列时，应同步 `AffinityState`、`ShowAffinityValue`、`append_row` 的固定列序、统一 SHOW 列定义及 Go 对照。列序或 NULL 语义变化属于客户端兼容风险。
- 改变过滤行为时，应同时核对 `exact_filter_does_not_bypass_like_pattern` 和 Go 的 Extractor/排序规则语义，特别注意调用者负责的小写规范化。
- 若 PD 提供真正的按 ID 批量查询，可扩展 runtime 接口接收 `infos` 中的组 ID，减少全量扫描；需评估重复 ID、返回缺项、请求大小及错误原子性，并保持“取状态失败不输出任何行”的不变量。
- 性能关注点主要是克隆表/库名称、分区展开规模、全量状态 `HashMap` 的内存和 voter 字符串分配。优化时不得改变输入顺序和 voter 顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/executor/show_affinity.rs --offset 1 --limit 500` 完整读取 213 行，并识别本文件被测试和若干模块使用；`query fetchShowAffinity` 定位 Rust/Go 两个同名入口；针对 `fetchShowAffinity`、`field_filter`、`field_pattern_matches`、`tables_with_affinity`、组 ID、状态读取和写行的查询核对了本地调用关系。
- Rust 源与装配：`pkg/executor/show_affinity.rs`、`pkg/executor/lib.rs`、`pkg/executor/show.rs`、`pkg/executor/Cargo.toml`。全局 Rust 搜索确认除 `pkg/executor/show_affinity_test.rs` 外没有 `fetchShowAffinity` 调用或 `ShowAffinityRuntime` 实现。
- Rust 独立测试：`pkg/executor/show_affinity_test.rs` 覆盖精确过滤仍叠加 LIKE、分区展开、组缺失 NULL、已知和未知 phase、错误无半行及完整八列映射。
- Go 对照：`pkg/executor/show_affinity.go`、`pkg/executor/show.go`、`pkg/executor/show_affinity_test.go`，用于核对生产调用链、InfoSchema/DDL/PD 边界、SQL 过滤与 NULL/列序语义。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证使用任务指定的十一章节结构命令，并人工复核所有“已接线/已支持”表述均有上述源码或搜索证据。
