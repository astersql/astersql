# [`pkg/util/tracing/opt_trace.rs`](./opt_trace.rs)

## 文件定位

本文件属于 `astersql-util-tracing` crate。crate 入口 `pkg/util/tracing/lib.rs` 以 `pub mod opt_trace` 声明模块，并通过 `pub use opt_trace::*` 将这里的三个公开符号重导出到 crate 根：`CETraceRecord`、`DedupCETrace` 和 `OptimizeTracer`。`pkg/util/tracing/Cargo.toml` 指定 `lib.rs` 为库入口，并以 `package.metadata.porting.go-package = "pkg/util/tracing"` 标明对应的 Go 包。

它是 Go 文件 `pkg/util/tracing/opt_trace.go` 的小范围迁移：定义基数估计（Cardinality Estimation，CE）追踪记录、按完整记录值去重的辅助函数，以及优化器 tracer 的空标记类型。它不负责启动 tracing、收集执行期 span、写文件或把 CE 记录挂入语句上下文；这些职责应由调用方和同 crate 的 `util.rs` 承担。

当前 Rust 生产接线仍不完整。全仓 Rust 精确引用只找到独立测试 `pkg/util/tracing/migration_aster_unit_test.rs` 使用 `CETraceRecord` 和 `DedupCETrace`；`OptimizeTracer` 没有 Rust 调用点。对应的 Rust `pkg/sessionctx/stmtctx/stmtctx.rs::StatementContext` 虽保留 `EnableOptimizeTrace`、`EnableOptimizerCETrace` 等字段名，却分别用 `Option<CacheValue>` 和 `Vec<CacheValue>` 承载 tracer/CE 记录，而不是本文件的具体类型。因此本文只把本文件描述为可公开使用的数据模型和去重工具，不把 Go 侧已有生产链路推断成 Rust 已接线能力。

## 核心职责

- 用 `CETraceRecord` 保存一次 CE trace 的表名、记录类型、表达式、内部表 ID 和估算行数，并保持 Go JSON 字段形态。
- 用 `DedupCETrace` 按 `CETraceRecord` 的全部字段值去重，保留每个不同值第一次出现的原始 `Box` 及输入顺序。
- 用零字段 `OptimizeTracer` 保留 Go API 中的优化器 tracer 名称和类型位置，为后续真实 tracer 状态迁移提供接入点。
- 通过派生的相等、哈希、克隆和 serde 能力，把比较键与 JSON 表示集中定义在同一数据类型上。

## 主要符号

- `pub struct CETraceRecord`：公开记录类型，派生 `Clone`、`Debug`、`Deserialize`、`Eq`、`Hash`、`PartialEq`、`Serialize`。字段沿用 Go 导出字段命名，因此用 `#[allow(non_snake_case)]` 抑制 Rust 命名警告。
  - `TableName: String`：所属表名，JSON 键为 `table_name`。
  - `Type: String`：trace 项类型，JSON 键为 `type`。
  - `Expr: String`：被估算的表达式文本，JSON 键为 `expr`。
  - `TableID: i64`：内部表 ID，参与相等与哈希，但由 `#[serde(skip)]` 排除在序列化和反序列化数据之外。
  - `RowCount: u64`：估算行数，JSON 键为 `row_count`。
- `pub fn DedupCETrace(records: Vec<Box<CETraceRecord>>) -> Vec<Box<CETraceRecord>>`：消费输入向量，以 `HashSet<CETraceRecord>` 保存已出现的值。返回值只包含首次出现的记录，且首次记录的 `Box` 被原样移动而不是重新分配。
- `pub struct OptimizeTracer {}`：派生 `Clone`、`Copy`、`Debug`、`Default` 的空结构体。当前没有字段、方法或 trait 实现；它只是与 Go 空结构体同名的占位类型。

本文件没有模块级常量、trait、条件编译项、私有函数或 `impl` 块。

## 执行流程

`DedupCETrace` 的执行过程如下：

1. 按 `records.len()` 为结果 `Vec` 和去重 `HashSet` 预分配容量；空输入自然得到两个空容器并返回空向量。
2. `for rec in records` 消费输入向量，逐个取得非空的 `Box<CETraceRecord>` 所有权。
3. 对 `*rec` 克隆出一个拥有所有字段的 `CETraceRecord`，作为集合键 `key`。
4. 若 `exists` 尚不包含该键，先把原始 `Box` 移入结果，再把克隆键插入集合；若已包含，则丢弃当前重复 `Box`。
5. 遍历结束后返回 `ret`。由于只在首次遇到某值时追加，结果顺序等于各唯一值在输入中的首次出现顺序，而不受 `HashSet` 自身无序性的影响。

序列化流程与去重流程相互独立：serde 按字段属性生成 JSON 表示；`Hash`/`Eq` 则按结构体的所有字段派生。特别是 `TableID` 虽不出现在 JSON 中，仍会改变去重键。

## 数据与状态

`CETraceRecord` 是拥有数据的值类型，三个文本字段均持有自己的 `String`。它没有内部可变性或引用生命周期。`Eq + Hash` 的派生实现覆盖五个字段，所以两条记录只有在 `TableName`、`Type`、`Expr`、`TableID` 和 `RowCount` 全部相等时才被视为重复。独立测试以仅改变 `RowCount` 和仅改变 `TableID` 的记录验证了这项不变量。

`DedupCETrace` 的临时状态只存在于单次调用中：`ret` 持有首次记录的 `Box`，`exists` 持有每个首次值的一份深克隆。设输入记录数为 `n`，平均哈希查找下时间复杂度为 `O(n)`，额外空间为 `O(u)`，其中 `u` 是唯一记录数；每条被遍历记录都会先克隆其字符串字段，即使它随后被判定为重复。预分配容量取输入长度，避免常见情况下的容器多次扩容，但可能为高重复输入保留多于最终结果所需的容量。

`Vec<Box<CETraceRecord>>` 对应 Go 的 `[]*CETraceRecord` 所有权形态，同时在类型层面排除了空指针元素。函数消费整个 `Vec`，调用者若仍需原输入，必须在调用前自行克隆或重组数据。

`OptimizeTracer` 是零尺寸、无状态值；`Copy` 和 `Default` 不涉及资源复制或初始化逻辑。

## 依赖与调用关系

模块装配关系为 `pkg/util/tracing/lib.rs -> opt_trace.rs`，并由 crate 根重导出公开 API。目标文件直接依赖：

- `serde::{Deserialize, Serialize}`：生成 `CETraceRecord` 的序列化/反序列化实现；`Cargo.toml` 为 `serde` 启用了 `derive` feature。
- `std::collections::HashSet`：实现按完整记录值去重。
- Rust 标准库的 `Vec`、`Box`、`String` 及派生 trait。

直接上游证据只有 `pkg/util/tracing/migration_aster_unit_test.rs::ce_trace_dedup_preserves_first_value_and_go_json_shape`，它构造记录并调用 `DedupCETrace`。测试使用 `serde_json` 检查输出字段；该依赖只列在 crate 的 `[dev-dependencies]`，生产实现并不依赖 `serde_json`。

RustCodeGraph 的文件反向边同样只把 `opt_trace.rs` 指向该独立测试。仓库中多个 crate 依赖 `astersql-util-tracing`，但精确符号搜索没有证明这些依赖方使用本文件的三个符号；它们可能只消费同 crate 的 `util.rs` API，不能据此扩张本文件的调用面。Go 侧直接下游是 `pkg/sessionctx/stmtctx/stmtctx.go::StatementContext` 的 `OptimizeTracer *tracing.OptimizeTracer` 与 `OptimizerCETrace []*tracing.CETraceRecord` 字段。

`DedupCETrace` 没有业务函数下游调用；其被调用者只有标准库容器操作、记录克隆和派生的哈希/相等比较。`OptimizeTracer` 当前也没有内部行为可调用。

## 错误处理与边界

本文件没有 `Result`/`Option` 返回、显式错误、日志或 panic 分支。内存分配失败等进程级条件之外，`DedupCETrace` 对任意可构造的输入都直接返回结果。

Rust API 通过 `Box<CETraceRecord>` 排除了 Go 输入切片中的 `nil` 元素。Go `DedupCETrace` 会在 `exists[*rec]` 解引用 `nil` 时 panic；Rust 安全调用者无法构造对应状态。因此两者对有效非空记录语义一致，但 Rust 不复现 Go 的 nil-panic 边界。

空输入返回空向量；单元素输入原样返回；所有元素相同时只保留第一个 `Box`；不同顺序的字段相同记录仍按第一次出现的位置保留。大小写、空白和表达式文本不做规范化，任何字符串字节差异都形成不同键。

`TableID` 的 `#[serde(skip)]` 同时作用于序列化与反序列化。反序列化时该字段需要使用类型默认值，因此缺失的内部 ID 会成为 `0`；随后它仍参与去重。调用者不能假定两条 JSON 表示相同的记录一定会在所有构造路径中去重，因为内存中的 `TableID` 可能不同。

## 并发与资源生命周期

本文件没有全局变量、锁、原子量、线程、异步任务、通道或外部资源。一次 `DedupCETrace` 调用完全拥有输入、结果与临时集合，不与其他调用共享状态；只要调用者在线程边界上正常转移所有权，多次调用之间天然独立。

首次唯一记录的 `Box` 从输入移动到输出，其堆分配地址和内部值保持不变。重复记录的 `Box` 在对应循环迭代结束时释放；临时克隆键由 `HashSet` 持有到函数返回前，集合销毁时释放。返回后，唯一记录的生命周期完全归返回向量所有者管理。

算法没有背压或长期缓存，但大批量输入会在调用期间同时保留结果记录和唯一值的深克隆。若将来用于高频或超大 CE trace，应评估字符串克隆与双份存储成本；不能在未保持“按完整值去重”和“保留首个 Box”语义的前提下直接改成引用集合。

## 与 Go 版本的对应关系

`pkg/util/tracing/opt_trace.rs` 逐项对应 `pkg/util/tracing/opt_trace.go`：

- Rust `CETraceRecord` 的五个字段、整数有符号性/位宽和 JSON 键对应 Go 同名结构体；`TableID` 在两边都不进入 JSON。
- Rust `HashSet<CETraceRecord>` 对应 Go `map[CETraceRecord]struct{}`，两边都以结构体完整值作为键，而非按指针地址去重。
- Rust 的两次 `with_capacity(records.len())` 对应 Go 为结果切片和 map 传入 `len(records)` 容量提示。
- 两边都按输入顺序遍历，只追加首次记录，因此都保留首个对象及稳定的首次出现顺序。
- Rust `OptimizeTracer {}` 对应 Go 空结构体 `OptimizeTracer struct {}`，当前两边该文件内都没有方法。

语言层面的主要差异是所有权与空值。Go 接收指针切片且不消费调用者切片，返回切片与输入共享记录指针；Rust 接收并消费 `Vec<Box<_>>`，把唯一 `Box` 移到结果并销毁重复项。Go 允许 `nil` 并会在函数内解引用 panic，Rust 类型排除了 `None`。Rust 为了让记录可充当 `HashSet` 键而显式派生 `Eq`/`Hash`，并因集合拥有键而克隆记录；Go map 会复制结构体键，字符串底层数据仍由运行时共享管理。

测试覆盖也不完全对称：Go 目录的 `main_test.go` 只提供测试进程初始化，`util_test.go` 与 `noop_bench_test.go` 不包含 `DedupCETrace` 专项断言；Rust 的 `migration_aster_unit_test.rs` 已明确覆盖去重顺序、`RowCount`/`TableID` 差异和 JSON 形态。Rust 生产 `StatementContext` 尚未用本文件的具体类型替换 `CacheValue`，这是迁移接线缺口，不应在本文中标记为已完成能力。

## 扩展指南

新增 `CETraceRecord` 字段时，应先核对 Go 同名结构及 JSON 标签，再同时决定该字段是否参与 `Eq`/`Hash`。按当前派生策略，任何新字段都会自动改变去重语义；若字段只用于展示而不应区分记录，就不能只添加字段而不设计显式键类型或手写相等/哈希实现。序列化字段还应在独立测试中同时验证键名、缺省值和是否暴露内部标识。

修改 `DedupCETrace` 时必须保留三项契约：按值而非地址去重、保留首次出现顺序、返回首次记录本身。若为减少克隆改用借用键或 entry API，需要证明移动 `Box` 后集合键的生命周期安全，并用高重复输入评估性能收益。不要把测试内嵌到 `opt_trace.rs`；按仓库约定扩展 `pkg/util/tracing/migration_aster_unit_test.rs` 或新增同目录独立 `*_test.rs` 文件。

为 `OptimizeTracer` 增加真实状态或方法前，应先追踪 Go 优化器 trace 的实际写入/读取点，并把 Rust `StatementContext` 的 `Option<CacheValue>` 有计划地替换为具体类型；同时考虑语句级创建、重置和释放边界。不能仅在空结构体中加字段而不更新上下文接线与独立测试。

若把 `CETraceRecord` 接入 Rust planner/session 主链，应补充至少以下测试：生产者生成字段的准确性、语句间状态不泄漏、去重在真实收集顺序下保留首项、JSON 输出隐藏 `TableID`，以及大 trace 的内存/性能边界。Go 侧既有行为可作为语义基线，但不能替代 Rust 独立回归测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/tracing` 确认目标 Rust/Go 文件、crate 入口和独立测试均已索引。
- RustCodeGraph `node --file pkg/util/tracing/opt_trace.rs --offset 1 --limit 240`：读取目标文件完整 81 行，核对全部公开符号、字段属性、派生项和去重分支。
- RustCodeGraph `query DedupCETrace`、`query CETraceRecord`、`query OptimizeTracer`：定位 Rust/Go 对应定义和 Rust 测试导入。精确 `callers DedupCETrace` 在 90 秒内无输出后终止；调用关系改由文件反向边与全仓精确符号搜索补证，不把宽泛搜索结果当作调用边。
- RustCodeGraph 文件节点：`pkg/util/tracing/opt_trace.go` 完整 41 行、`pkg/util/tracing/migration_aster_unit_test.rs` 完整 181 行、`pkg/util/tracing/lib.rs` 完整 35 行，以及 `pkg/sessionctx/stmtctx/stmtctx.{rs,go}` 的相关字段和默认初始化区段。
- crate/装配证据：`pkg/util/tracing/Cargo.toml`、`pkg/util/tracing/lib.rs`、`pkg/util/tracing/BUILD.bazel`。目标目录及上级 `pkg/util` 未发现 `doc.go`，因此没有额外的 Go package contract 文件可读。
- Rust 测试：`migration_aster_unit_test.rs::ce_trace_dedup_preserves_first_value_and_go_json_shape` 验证四条输入去重为三条、保留首次值、`RowCount` 与 `TableID` 都参与区分，以及 JSON 使用 snake_case 且不暴露表 ID。
- Go 对照与测试：`pkg/util/tracing/opt_trace.go`；`main_test.go`、`util_test.go`、`noop_bench_test.go` 经精确搜索未发现该 API 的专项测试。`pkg/sessionctx/stmtctx/stmtctx.go` 证明 Go 的语句上下文持有这两个具体类型。
- 全仓 `rg` 精确符号引用确认 Rust 生产源码没有使用本文件三个公开符号，Rust `StatementContext` 当前使用 `CacheValue` 占位。本文据此明确迁移状态，未把 Go 接线描述为 Rust 现状。
- 本任务是纯文档分析，按计划未运行 Cargo；最终以固定 11 章节结构命令、链接检查和人工事实复核验收。
