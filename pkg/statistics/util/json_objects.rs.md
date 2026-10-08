# `pkg/statistics/util/json_objects.rs`

## 文件定位

本文件属于 Cargo crate `astersql-statistics-util`；crate 入口 `pkg/statistics/util/lib.rs` 通过 `pub mod json_objects` 声明模块，并用 `pub use json_objects::*` 再导出全部公开对象。它定义统计信息导入、导出边界使用的数据形状，而不负责从内存统计构造对象、JSON 编解码、gzip 分块或数据库持久化。

Rust 接口层已经消费这里的 `JSONTable`：`pkg/statistics/handle/types/interfaces.rs` 将其再导出，并在 `StatsReadWriter`、`PartitionStatisticLoadTask` 和 `PersistFunc` 的签名中用作统计导入、导出及分区任务载荷。当前完整的 Go 运行链位于 `pkg/statistics/handle/storage/json.go` 和 `stats_read_writer.go`；Rust 侧另有 `pkg/statistics/handle/storage/json.rs::JsonTable` 这一不同的数据模型和编码实现，不能与本文件的 `JSONTable` 混为一体。

## 核心职责

- `TiDBGlobalStats` 约定分区表 `Partitions` 映射中全局统计的保留键为 `"global"`。
- `JSONTable` 聚合列、索引、分区、谓词列以及表级计数、版本和历史标记，保留与 Go `statsutil.JSONTable` 相同的字段语义。
- `JSONTable::Sort` 只规范化 `PredicateColumns` 的顺序，以列 ID 升序消除由 map 遍历引入的非确定性，主要服务稳定比较和测试。
- `JSONColumn` 保存一个列或索引的 protobuf 统计载荷与辅助元数据；`TotalMemoryUsage` 汇总三个可选 protobuf 消息的编码尺寸。
- `JSONPredicateColumn` 保存谓词列 ID 及最近使用、最近分析时间的可空字符串表示。

本文件没有实现 JSON wire format。虽然名称和字段与 Go JSON DTO 对齐，结构体当前没有 `serde::Serialize`/`Deserialize` 派生，也没有字段 rename 标注。

## 主要符号

- `pub const TiDBGlobalStats: &str = "global"`：分区统计容器中的全局项键；Go 对照消费点见 `pkg/statistics/handle/storage/stats_read_writer.go`。
- `pub struct JSONTable`：
  - `Columns`、`Indices`：以名称为键，值为独占的 `Box<JSONColumn>`；
  - `Partitions`：以分区名为键递归持有 `Box<JSONTable>`，因此可以表达分区表及全局项；
  - `DatabaseName`、`TableName`：对象所属库表；
  - `PredicateColumns`：谓词列用量信息列表；
  - `Count`、`ModifyCount`、`Version`：表统计的行数、修改数和版本；
  - `IsHistoricalStats`：标记对象是否来自历史统计。
- `pub fn JSONTable::Sort(&mut self)`：原地按 `JSONPredicateColumn::ID` 升序排序；空列表自然保持为空，重复 ID 允许存在且不会报错。
- `pub struct JSONColumn`：包含 `Option<Box<tipb::Histogram>>`、`Option<Box<tipb::CmSketch>>`、`Option<Box<tipb::FmSketch>>`，以及可空 `StatsVer`、空值数、总列大小、最后更新版本和相关系数。
- `pub fn JSONColumn::TotalMemoryUsage(&self) -> i64`：对存在的三种 protobuf 消息调用 `protobuf::Message::compute_size()`，转换为 `i64` 后求和；其他标量字段和 Rust 容器开销不计入。
- `pub struct JSONPredicateColumn`：`LastUsedAt`、`LastAnalyzedAt` 均为 `Option<String>`，`ID` 为 `i64`。

文件级 `#![allow(non_snake_case, non_upper_case_globals)]` 是为了保留 Go 风格的公开名称和兼容字段形状，而不是推荐新的 Rust API 继续采用该命名方式。

## 执行流程

1. 上游构造 `JSONColumn`，把直方图、CM Sketch、FM Sketch 的 tipb protobuf 和版本、计数等元数据放入对应字段。Go 的直接证据是 `dumpJSONCol`；Rust 本文件只提供容器。
2. 上游构造 `JSONTable`，按列名和索引名填充两个 map；分区表再按分区名递归填充 `Partitions`，全局统计使用 `TiDBGlobalStats` 键。
3. 谓词列来源通常是 map，遍历顺序不稳定。需要确定性输出或比较时，调用 `JSONTable::Sort`，它只重排 `PredicateColumns`，不会递归排序分区，也不会排序 `HashMap`。
4. 构造列/索引统计期间可调用 `JSONColumn::TotalMemoryUsage`，得到三个 protobuf 消息编码尺寸之和。Go `GenJSONTableFromStats` 将该结果交给内存 tracker；Rust 目标文件不持有 tracker，也不执行配额判断。
5. 后续编码、压缩、落盘或反向加载由外层 storage/read-writer 实现完成；这些步骤的错误不会在本文件内产生或处理。

## 数据与状态

所有状态都由调用者拥有并显式传入；本文件没有全局可变状态。`JSONTable` 通过 `Box` 形成树状所有权：父表独占各列/索引对象和子分区表，`Sort` 需要 `&mut self`，因此排序期间由 Rust 借用规则保证独占修改 `PredicateColumns`。

`HashMap` 的键顺序未定义，`Sort` 仅为谓词列向量建立 `ID` 升序不变量。时间字段使用字符串而非时间类型，本文件不解析、校验时区或格式。`StatsVer: Option<i64>` 的 `None` 保留“旧版 JSON 不含统计版本”的区别，不能简单等同于数值零。protobuf 字段的 `None` 表示对应统计载荷缺失；`TotalMemoryUsage` 对缺失项贡献零。

## 依赖与调用关系

直接下游依赖只有：

- `protobuf::Message`：为 tipb 消息提供 `compute_size`；导入为 `_`，仅使 trait 方法可用。
- `tipb`：提供 `Histogram`、`CmSketch` 和 `FmSketch` protobuf 类型。
- Rust 标准库：`HashMap`、`String`、`Vec`、`Box` 和切片排序。

`pkg/statistics/util/Cargo.toml` 明确声明 `protobuf = "=2.8.0"`，并从固定 revision 的 `pingcap/tipb` 启用 `protobuf-codec` feature；crate 没有声明 `serde`。RustCodeGraph 对该文件的索引列出 6 个符号，并报告被 `pkg/statistics/handle/types/interfaces.rs` 及若干 planner/executor 测试文件使用。原始 Rust 搜索进一步确认：`interfaces.rs` 将 `JSONTable` 用于 `StatsReadWriter` 的导入导出接口；`pkg/planner/core/casetest/cbotest/cbo_test.rs` 从 JSON fixture 手工构造 `JSONTable`/`JSONColumn`。

Go 主链的直接对应关系是 `storage.GenJSONTableFromStats` 构造对象、`JSONTableToBlocks`/`BlocksToJSONTable` 进行 JSON 与 gzip 分块转换、`statsReadWriter` 组织普通表/分区/全局统计的导出和加载。它们用于解释设计来源，不代表这些 Go 函数调用 Rust 实现。

## 错误处理与边界

本文件的两个方法都不返回 `Result`，也不主动产生业务错误：

- `Sort` 对空向量、负 ID 和重复 ID 均可工作；它不检查 ID 是否真实存在于表定义中，也不去重。
- `TotalMemoryUsage` 对三个载荷逐项判空后计数；它不包括 `JSONColumn` 自身、堆分配、map/string/vector 容量或 JSON/gzip 后大小，所以只能视为与 Go `Size()` 口径对齐的 protobuf 载荷估算，不能视为进程实际驻留内存。
- 每个 `compute_size()` 返回值先转为 `i64` 再累加。常规 protobuf 消息尺寸远小于 `i64::MAX`；代码没有显式溢出检查。
- 数据结构没有构造器或校验器，调用者可以创建字段彼此不一致的对象；名称有效性、版本兼容、表元数据匹配和编码错误必须由外层处理。
- 当前缺少 serde 实现，因此直接要求 `serde_json` 编解码这些结构会在编译期缺少 trait 实现；扩展时不能仅凭类型名假定该能力已存在。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或文件/网络资源。对象生命周期完全由拥有它们的 `JSONTable`、`JSONColumn` 及调用栈决定；`Box` 在所有者释放时递归释放内容，`Option` 控制 protobuf 载荷是否存在。

这些结构当前未显式实现或派生 `Clone`、`Send`、`Sync`；是否可在线程间传递由所有字段的自动 trait 推导决定。接口层 `PartitionStatisticLoadTask` 可通过 `std::sync::mpsc::Receiver` 进入并发加载流程，但并发调度和错误传播定义在 `pkg/statistics/handle/types/interfaces.rs` 及其实现中，不在本文件内。修改字段类型时必须重新评估自动 `Send`/`Sync` 性质和任务载荷成本。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/util/json_objects.go`：常量、三个结构体、字段语义、`Sort` 和 `TotalMemoryUsage` 均为逐项移植。主要对应如下：

- Go `map[string]*T` 对应 Rust `HashMap<String, Box<T>>`；Go `nil` 指针载荷对应 Rust `Option<Box<T>>`。
- Go `*int64 StatsVer` 对应 `Option<i64>`，保留旧格式缺字段语义。
- Go `[]*JSONPredicateColumn` 对应 `Vec<Box<JSONPredicateColumn>>`。
- Go `slices.SortFunc(... cmp.Compare(a.ID, b.ID))` 对应 Rust `sort_by(|a, b| a.ID.cmp(&b.ID))`，排序方向和重复值处理一致。
- Go protobuf 的 `Size()` 对应 Rust protobuf 2.8 的 `compute_size()`；两侧都只累计 Histogram、CMSketch、FMSketch。

重要差异是 Go 字段带有 `json:"..."` tag，原生参与 `encoding/json`；Rust 字段目前没有 serde 派生和 rename 属性。另一个 Rust 模块 `pkg/statistics/handle/storage/json.rs` 使用蛇形字段的 `JsonTable`/`PredicateColumn` 和自有二进制 payload 编码，是独立实现而非本结构的序列化器。Go 回归测试 `pkg/statistics/handle/storage/dump_test.go::TestJSONTableToBlocks` 验证 dump、分块、还原后的 JSON 等价，并在比较前按谓词列 ID 排序；它证明 Go wire 行为，不能替代 Rust 本结构的序列化测试。

## 扩展指南

- 新增或修改字段时，应先对照 `json_objects.go` 的真实增量，保持可空性、整数宽度、字段语义和旧版本缺省行为；再同步所有直接结构字面量，尤其是 `migration_aster_unit_test.rs`、`interfaces.rs` 和 `cbo_test.rs`。
- 若要让该结构真正承担 Rust JSON 兼容层，应在本 crate 明确加入序列化依赖，为所有递归字段实现/派生编解码，并逐字段使用 Go tag 的 snake_case 名称；还需用独立测试覆盖旧 JSON 缺 `stats_ver`、空 protobuf、分区递归和未知/缺失字段。不要把 `handle/storage/json.rs::JsonTable` 的私有 payload 格式悄然当成 Go JSON 格式。
- 若改变 `Sort`，至少保留 ID 升序、负数、重复 ID、空向量测试；若期望递归排序或 map 确定性，必须新增明确 API，因为当前方法只处理顶层 `PredicateColumns`。
- 若改变 `TotalMemoryUsage` 口径，应同步 Go 方法和内存 tracker 的消费语义，并新增单项存在、组合存在、全缺失和较大消息测试；将容器开销纳入口径属于兼容/性能语义变更。
- Rust 单元测试继续放在独立的 `pkg/statistics/util/migration_aster_unit_test.rs`，不要内嵌回生产文件。生产代码行为变化时按仓库要求在独立测试中先建立回归失败证据，再验证修复。

## 验证依据

- 源码与模块：`pkg/statistics/util/json_objects.rs`、`pkg/statistics/util/lib.rs`。
- crate 边界：`pkg/statistics/util/Cargo.toml`，确认 crate 名称、lib 入口、protobuf/tipb 依赖及 tipb feature。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file pkg/statistics/util/json_objects.rs` 读取完整 92 行并报告直接使用文件；`query JSONTable/JSONColumn/JSONPredicateColumn --kind struct --json` 将本文件符号与同名 Go、其他 Rust 类型消歧；`files --filter pkg/statistics/util` 确认该目录的索引覆盖。
- Rust 调用与测试：`pkg/statistics/handle/types/interfaces.rs`、`pkg/planner/core/casetest/cbotest/cbo_test.rs`、`pkg/statistics/util/migration_aster_unit_test.rs`。独立单元测试覆盖谓词列负数/重复 ID 排序、无 protobuf 时为零、三个 protobuf 尺寸求和。
- Go 对照与回归：`pkg/statistics/util/json_objects.go`、`pkg/statistics/handle/storage/json.go`、`pkg/statistics/handle/storage/stats_read_writer.go`、`pkg/statistics/handle/storage/dump_test.go::TestJSONTableToBlocks`。
- 本任务是纯文档分析，按计划不运行 Cargo；结构校验要求文档存在且恰有十一个固定二级标题。
