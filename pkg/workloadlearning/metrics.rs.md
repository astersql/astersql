# [`pkg/workloadlearning/metrics.rs`](metrics.rs)

## 文件定位

`metrics.rs` 属于 `astersql-workloadlearning` crate，是工作负载学习链路共享的数据模型层，而不是主动执行分析的入口。crate 根 `pkg/workloadlearning/lib.rs` 以 `mod metrics` 装配该模块并通过 `pub use metrics::*` 对外再导出 `CIStr` 与 `TableReadCostMetrics`；同 crate 的 `handle.rs` 负责产生、累加、归一化并序列化这些指标，`cache.rs` 负责从存储反序列化并缓存它们。

该文件对应 Go 的 `pkg/workloadlearning/metrics.go`。Rust 版本在五个业务字段之外增加了 serde 兼容逻辑，使模型可以跨 `WorkloadStore` 的 JSON 字符串边界保存和恢复。`pkg/workloadlearning/Cargo.toml` 表明此 crate 直接依赖带 derive 功能的 `serde` 和 `serde_json`，且移植来源包为 `pkg/workloadlearning`。

## 核心职责

- `CIStr` 同时保存标识符的原始形式 `O` 与小写形式 `L`，供展示和不区分大小写的表定位分别使用。
- `TableReadCostMetrics` 承载一张表在统计窗口内的累计扫描时间、累计内存用量、读取频率和归一化读取代价，同时保留数据库名与表名。
- `CIStr::deserialize` 兼容三种历史/边界 JSON 表示：`{"O": ..., "L": ...}` 对象、单个字符串和 `null`。
- 私有模块 `duration_nanos` 固定 `Duration` 的 JSON 表示为纳秒整数，避免依赖 `std::time::Duration` 的默认表示。

本文件只定义模型、构造和序列化规则；按表 ID 聚合、代价计算、持久化、缓存加锁均由相邻模块承担。

## 主要符号

- `pub struct CIStr { pub O: String, pub L: String }`：大小写不敏感标识符。派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq` 和 `Serialize`，反序列化则由手写实现控制兼容格式。
- `pub fn CIStr::new(value: impl Into<String>) -> CIStr`：保留输入到 `O`，用 `String::to_lowercase` 计算 `L`。它使用 Unicode 小写转换，而不是仅 ASCII 转换；`metrics_test.rs::cistr_new_uses_unicode_lowercase_like_go` 以 `ÄBC -> äbc` 固化该行为。
- `impl Deserialize for CIStr`：先反序列化到内部无标签枚举 `Representation`。对象形式原样接收 `O`、`L`，缺失字段各自默认为空字符串；字符串形式调用 `CIStr::new` 自动计算 `L`；`null` 返回两个字段均为空的默认值。
- `pub struct TableReadCostMetrics`：派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Serialize`、`Deserialize`。字段分别为 `DbName: CIStr`、`TableName: CIStr`、`TableScanTime: Duration`、`TableMemUsage: i64`、`ReadFrequency: i64`、`TableReadCost: f64`。
- `duration_nanos::serialize`：将 `Duration::as_nanos()` 截断上限到 `u64::MAX` 后输出 JSON 无符号整数。
- `duration_nanos::deserialize`：只接受可反序列化为 `u64` 的纳秒值，再调用 `Duration::from_nanos`。

公开 API 是两个结构体、它们的公开字段以及 `CIStr::new`；`Representation` 和 `duration_nanos` 都是文件内部实现细节。

## 执行流程

典型生产流程如下：

1. `handle.rs::extractScanAndMemoryFromBinaryPlan` 将二进制计划 JSON 解析为算子树，`extractMetricsFromOperatorTree` 识别读算子，私有 `metric` 函数用 `CIStr::new` 创建库表名并生成尚未带频率和归一化代价的 `TableReadCostMetrics`。
2. `handle.rs::AccumulateMetricsGroupByTableID` 使用 `DbName.L` 和 `TableName.L` 查询表 ID，将扫描时间与内存按语句频率放大，并累计 `ReadFrequency`。
3. `Handle::analyzeBasedOnStatementStats` 汇总所有表的扫描时间和内存；每张表的 `TableReadCost` 被设置为“扫描时间占比 + 内存占比”。两项分母都非零时，按 Go 文件注释，其理论范围为 0 到 2；任一总量为零时相应项贡献 0。
4. `Handle::SaveTableReadCostMetrics` 调用 `serde_json::to_string`。此时 `CIStr` 输出含 `O`/`L` 的对象，`TableScanTime` 通过 `duration_nanos::serialize` 输出纳秒整数，然后 `WorkloadStore::save_metrics` 按版本保存。
5. `cache.rs::WLCacheWorker::UpdateTableReadCostCache` 从存储加载 JSON，并用 `serde_json::from_str::<TableReadCostMetrics>` 恢复模型；成功的条目进入按表 ID 索引的内存缓存。`GetTableReadCostMetrics` 对外返回时只复制四个数值指标，库表名通过 `Default` 清空，以匹配 Go 的查询投影。

## 数据与状态

`CIStr` 的不变量仅由 `CIStr::new` 和字符串形式反序列化保证：`L` 是 `O` 的 Unicode 小写形式。公开字段可被调用者直接改写，而对象形式 JSON 也会原样接受两个字段，因此类型自身不保证任意实例始终满足 `L == O.to_lowercase()`；依赖小写查找的调用方应优先使用构造函数，不能假设外部对象输入已经规范化。

`TableReadCostMetrics::default()` 会生成空库表名、零时长、三个数值零值。生产聚合以表 ID 作为外部 map 键；该 ID 不存放在结构体内。`TableScanTime` 是 `Duration`，JSON 单位固定为纳秒；`TableMemUsage` 的语义单位是字节；`ReadFrequency` 是统计记录频率之和；`TableReadCost` 是计算结果而非原始计数。结构体不缓存总扫描时间、总内存或统计窗口，这些上下文由 `Handle` 和存储版本管理。

## 依赖与调用关系

- 上游构造者：`handle.rs::metric` 创建基础指标；`AccumulateMetricsGroupByTableID` 更新累计量；`Handle::analyzeBasedOnStatementStats` 写入归一化代价。
- 上游持久化者：`Handle::SaveTableReadCostMetrics` 依赖本文件的 `Serialize` 实现生成 JSON。
- 上游消费者：`cache.rs::UpdateTableReadCostCache` 依赖本文件的 `Deserialize` 实现恢复 JSON；`GetTableReadCostMetrics` 返回数值字段的克隆投影。
- 模块边界：`lib.rs` 公开再导出本文件符号，并在 `#[cfg(test)]` 下将 `metrics_test.rs` 作为独立测试模块装配，符合源文件与测试分离要求。
- 下游库：本文件直接使用 `serde::{Serialize, Deserialize, Deserializer}` 与标准库 `Duration`；序列化实际调用由相邻模块通过 `serde_json` 发起。

RustCodeGraph 对文件的索引显示 15 个符号，并将其关联到 `handle.rs` 等使用位置；精确源码核验确认当前生产 Rust 引用集中在同 crate 的 `handle.rs` 和 `cache.rs`。图索引列出的其他同名使用文件可能来自宽泛符号关联，不能据此认定它们直接调用本模块。

## 错误处理与边界

`CIStr::deserialize` 将底层 serde 错误直接作为 `D::Error` 返回；对象、字符串和 `null` 之外的 JSON 类型会失败。对象缺少 `O` 或 `L` 不会失败，而是将缺失字段置空，也不会重算或校验 `L`。这提供兼容性，但意味着畸形或不一致对象能够进入系统。

`duration_nanos::serialize` 对超过 `u64::MAX` 纳秒的 `Duration` 采用饱和截断，因此极端大时长往返后会变为 `u64::MAX` 纳秒而不是原值。反序列化只接收 `u64`，负数、浮点数、字符串以及超过 `u64` 的整数都会报错。错误在 `SaveTableReadCostMetrics` 中被转换为字符串并中止保存准备；缓存加载路径则在 `UpdateTableReadCostCache` 中静默跳过单条解析失败的记录。

本结构体允许负的 `TableMemUsage`、负的 `ReadFrequency`、非有限或超出 0 到 2 的 `TableReadCost`，自身不做业务校验。正常生产路径通过相邻聚合逻辑约束含义，但直接构造或外部 JSON 输入不受此约束。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、文件句柄或网络资源；两个数据结构都是拥有所有字符串和数值的普通值类型，克隆不会共享可变状态。serde 适配只在调用栈上创建临时枚举或数值。

并发所有权位于 `cache.rs::WLCacheWorker`：`TableReadCostMetrics` 集合受 `RwLock<TableReadCostCache>` 保护，刷新时整体替换快照，查询时在读锁内复制出一个新值。持久化生命周期位于 `handle.rs` 和 `WorkloadStore`：结构体先在分析阶段聚合，再序列化为带版本的行，之后可由缓存反序列化。修改本文件的 wire format 会同时影响已有存储行和刷新路径，应按持久化兼容变更处理。

## 与 Go 版本的对应关系

`metrics.go::TableReadCostMetrics` 与 Rust 结构体字段一一对应：Go 的 `ast.CIStr` 对应本地 `CIStr`，`time.Duration` 对应 `std::time::Duration`，三个数值字段类型与含义保持一致。Go 注释明确扫描时间和内存均按读取频率加权，读取频率为同表记录频率之和，最终代价为扫描占比与内存占比之和；这些计算实际由 Rust `handle.rs` 完成，而本文件保存结果。

主要差异是 Rust 为跨存储边界显式派生 serde，并定义纳秒 wire format；Go 当前结构体注释仍有“添加 JSON tag”的 TODO。Rust 本地 `CIStr` 的字符串/`null` 反序列化属于兼容扩展，不是 Go 结构定义中可见的 API。Rust `CIStr::new` 的 Unicode 小写语义由 `metrics_test.rs` 两个测试验证，以贴近 Go `ast.NewCIStr`；但对象形式 `O`/`L` 的缺省和不一致行为、`null`、`Duration` 饱和序列化尚无本文件专门测试证明。

Go 的 `handle_test.go` 与 `cache_test.go` 进一步验证指标的累加、保存、读取和空缓存行为；Rust 对应证据位于独立的 `handle_test.rs`、`cache_test.rs`。Rust 测试还验证缓存查询会清空库表名，仅暴露成本字段。

## 扩展指南

- 新增指标字段时，应先在 `TableReadCostMetrics` 定义其单位和零值语义，再同步 `handle.rs` 的提取、频率加权、聚合、归一化与保存逻辑，以及 `cache.rs::GetTableReadCostMetrics` 的投影；同时检查旧 JSON 缺少字段时是否需要 `#[serde(default)]`，否则历史记录会无法加载。
- 修改字段名或 JSON 表示属于持久化协议变更。应保留旧格式反序列化路径，补充 JSON 往返与旧记录兼容测试，避免缓存刷新静默丢弃旧行。
- 扩展 `CIStr` 时，应保持 `O` 用于原样展示、`L` 用于查找的约定；如要强化不变量，需要决定是否修正旧对象中的空/错误 `L`，并以 Go `ast.CIStr` 行为作为兼容基准。
- 修改时长编码时，应明确单位、溢出策略和历史数据迁移。至少覆盖 0、普通纳秒、`u64::MAX` 边界、错误 JSON 类型及大于 `u64::MAX` 纳秒的饱和行为。
- 测试必须继续放在独立文件：构造/serde 边界放入 `pkg/workloadlearning/metrics_test.rs`；聚合和归一化放入 `handle_test.rs`；存储恢复、坏行和查询投影放入 `cache_test.rs`。需要同步核对 Go 的 `metrics.go`、`handle_test.go`、`cache_test.go`，不能为了测试通过而弱化 Go 已有语义。

主要兼容风险是持久化 JSON 不兼容，正确性风险是 `O`/`L` 失配或单位误解导致表解析和成本计算错误，性能风险则来自新增大字段在每次 clone、序列化和缓存快照替换中的复制开销。

## 验证依据

- 目标源码：`pkg/workloadlearning/metrics.rs`，核对 `CIStr`、手写 `Deserialize`、`TableReadCostMetrics` 与 `duration_nanos` 的完整实现。
- RustCodeGraph：`status` 确认索引可用；`files --filter pkg/workloadlearning` 确认模块文件集合；`node --file pkg/workloadlearning/metrics.rs` 读取 83 行源码和 15 个索引符号；`query TableReadCostMetrics --kind struct` 定位 Go/Rust 定义及 `handle.rs`、`cache.rs` 相关入口；`node --file` 核对 `handle.rs` 的分析、聚合、保存调用链和 `cache.rs` 的恢复、查询调用链。`callers`/`callees` 命令未返回可用边，因此调用关系以索引文件上下文和精确源码引用交叉确认。
- crate 与装配：`pkg/workloadlearning/Cargo.toml`、`pkg/workloadlearning/lib.rs`。
- Go 对照：`pkg/workloadlearning/metrics.go`；相关行为测试为 `pkg/workloadlearning/handle_test.go`、`pkg/workloadlearning/cache_test.go`。
- Rust 测试：`pkg/workloadlearning/metrics_test.rs` 验证 Unicode 小写与旧字符串 JSON；`handle_test.rs` 验证保存、按频率累计和归一化；`cache_test.rs` 验证序列化记录恢复、缓存读取投影与空缓存。
- 人工复核结论：该文件存在的原因是为工作负载学习的“提取—聚合—持久化—缓存”链提供稳定共享模型和 JSON 边界；运行路径与安全扩展点均能由上述真实符号定位。未运行 Cargo，符合本纯文档任务约束。
