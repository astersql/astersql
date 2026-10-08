# `pkg/planner/util/handle_cols.rs`

## 文件定位

本文件属于 `astersql-planner-util` crate；crate 根模块 `pkg/planner/util/lib.rs` 将私有模块 `handle_cols` 的全部公开 API 再导出，因此上层通常通过 `planner_util::HandleCols`、`planner_util::NewIntHandleCols` 等路径使用它。`pkg/planner/util/Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/planner/util`，表明它对应同目录 Go 包。

它位于“规划表达式列”与“存储层行标识”之间：规划阶段用 `expression::Column` 描述句柄在当前 schema/行中的位置，执行阶段通过 `HandleCols` 把普通行、Datum 行或索引回表行转换为 `kv::Handle`。`pkg/planner/core/logical_plan_builder_runtime.rs` 会按表的 `PKIsHandle`、`IsCommonHandle` 状态创建整型或公共句柄列；`pkg/planner/core/operator/physicalop/physical_union_scan.rs` 则在物理计划固化 schema 时调用 `ResolveIndices`。

## 核心职责

- `HandleCols` trait 统一两类表行句柄：单列整型主键/`_tidb_rowid` 与一列或多列聚簇主键（common handle）。调用者无需按句柄类型分别处理构造、列迭代、比较、下标解析、Explain、哈希和相等判断。
- `CommonHandleCols` 保存表元信息、主键索引元信息和按索引顺序排列的表达式列；构造句柄前调用 `tablecodec::TruncateIndexValues`，再按语句时区调用 `codec::EncodeKey`，最后交给 `kv::NewCommonHandle`。
- `IntHandleCols` 保存唯一的表达式列，直接读取 `i64` 并包装成 `kv::IntHandle`，不需要键编码。
- 索引回表有明确的行尾协议：普通索引行的末尾是句柄列；分区索引行的最后一列是物理分区 ID，紧邻其前的是句柄列。两种实现都遵守该布局。
- 类型还参与 Cascades 结构哈希/相等和 Explain 输出，并提供 `CloneHandleCols` 以便计划对象拥有独立副本。

## 主要符号

- `pub trait HandleCols`：公共接口。除四个构造句柄方法外，还提供 `ResolveIndices`、`IsInt`、`GetCol`、`NumCols`、`Compare`、`GetFieldsTypes`、`MemoryUsage`、克隆和两种列迭代接口。它继承 `StringerWithCtx + Hash64 + Equals`。
- `HandleCols::CacheCommonHandleMetadata`：默认返回 `None`；仅 `CommonHandleCols` 返回克隆后的 `(model::TableInfo, model::IndexInfo)`。直接使用者是 `pkg/planner/core/operator/physicalop/cache_snapshot.rs`，用于捕获和恢复 `UnionScan` 的句柄描述。
- `pub struct CommonHandleCols`：拥有 `Box<model::TableInfo>`、`Box<model::IndexInfo>` 和 `Vec<expression::Column>`。字段私有，避免调用者绕过构造器破坏“列顺序与索引列一致”的约束。
- `CommonHandleCols::buildHandleByDatumsBuffer`：公共句柄的共享编码核心。`BuildHandle`、`BuildHandleByDatums`、`BuildHandleFromIndexRow` 和 `BuildPartitionHandleFromIndexRow` 最终都汇入此函数。
- `CommonHandleCols::GetColumns`：只读暴露已对齐的句柄列切片。
- `NewCommonHandleCols`：按每个 `IndexColumn.Offset` 从完整表列数组取列，建立索引顺序；要求 offset 对输入切片有效。
- `NewCommonHandlesColsWithoutColsAlign`：直接接受已按正确顺序准备好的列，主要用于计划缓存恢复等已经保存列顺序的路径。
- `pub struct IntHandleCols`：单个 `expression::Column` 的拥有型包装，实现同一 trait。
- `NewIntHandleCols`：返回 `Box<dyn HandleCols>`，隐藏具体实现。
- `GetCommonHandleDatum`：公共句柄时按各列当前 `Index` 从 `chunk::Row` 取出 Datum；整型句柄返回空向量。

文件没有条件编译项、全局可变状态或模块级常量。`NilFlag` 虽被导入，但当前实现未使用；两种 Rust 结构均以非空拥有型字段表达有效状态。

## 执行流程

1. 规划器创建句柄描述。`pkg/planner/core/logical_plan_builder_runtime.rs` 对整型 PK 调用 `NewIntHandleCols`，对 common-handle 表查找 primary index 后调用 `NewCommonHandleCols`，非聚簇表则为额外的 `_tidb_rowid` 列创建 `IntHandleCols`；同时通过 `CloneHandleCols` 保存不可变副本。
2. 若输出 schema 改变，物理算子调用 `ResolveIndices`。例如 `PhysicalUnionScan::ResolveIndices` 先解析子计划和条件，再用子节点 schema 生成新的句柄对象；旧对象不原地修改。
3. 普通表行构造时，`BuildHandle` 按每个 `expression::Column.Index` 取值。`BuildHandleByDatums` 对 Datum 切片执行相同的索引选择。
4. 索引回表时不使用表达式列的 `Index` 定位行中句柄：`BuildHandleFromIndexRow` 从行尾向前取 `NumCols()` 个值；分区版本再为最后一列预留分区 ID。整型版本对应读取末列或倒数第二列。
5. 公共句柄将值按表/索引元信息截断，再使用 `StatementContext::TimeZone()` 编码。编码错误先交给 `StatementContext::HandleError`；若上下文返回错误则转为 `expression::Error`，若上下文消化错误则继续以空编码创建句柄。`kv::NewCommonHandle` 的错误始终转换为 `expression::Error`。
6. `Compare` 用于句柄顺序比较。公共句柄按句柄列顺序逐列比较并在首个非零结果处返回；整型句柄只比较唯一列。

## 数据与状态

`CommonHandleCols` 同时持有三类快照数据：表元信息决定列类型等编码信息，索引元信息决定前缀截断规则，`columns` 决定当前行/schema 的取值位置和句柄列顺序。`Clone` 深克隆表、索引和列向量；`ResolveIndices` 也克隆元信息并创建新的列向量，因而不会修改共享计划对象。

`IntHandleCols` 只持有一列。其 `GetFieldsTypes` 固定返回一个 `TypeLonglong`，不复用列上可能存在的其他字段类型描述；`NumCols` 恒为 1，`IsInt` 恒为 `true`。公共句柄相应返回实际列数和各列克隆的 `RetType`，`IsInt` 为 `false`。

`Hash64` 与 `Equals` 覆盖决定句柄语义的字段。公共句柄依次纳入表、索引、列数和每列；整型句柄纳入唯一列。公共实现写入 `NotNilFlag` 后再散列拥有型字段，不存在 Go 指针实现中的 nil 分支。`MemoryUsage` 是估算值：公共实现包含结构体静态大小、列向量容量对应的指针大小及每列报告的内存；它未另外累加表/索引元信息的深层占用。整型实现只返回列的 `MemoryUsage`。

## 依赖与调用关系

上游直接证据包括：

- `pkg/planner/core/logical_plan_builder_runtime.rs`：构造 DataSource 的句柄列，并把克隆分别放入 `UnMutableHandleCols` 与 `HandleCols`。
- `pkg/planner/core/operator/physicalop/physical_union_scan.rs`：对 `HandleCols` 调用 `ResolveIndices`，使列下标与子计划输出 schema 对齐。
- `pkg/planner/core/operator/physicalop/cache_snapshot.rs`：调用 `IsInt`、`IterColumns` 和 `CacheCommonHandleMetadata` 捕获快照；恢复公共句柄时调用不对齐构造器，因为缓存列已经按句柄顺序保存。
- `pkg/planner/core/point_get_plan.go` 是 Go 主线的对应入口：点查计划同样根据表属性选择 `NewIntHandleCols` 或 `NewCommonHandleCols`。

下游依赖由 `pkg/planner/util/Cargo.toml` 声明：`expression`/`chunk` 提供列、Datum、Schema 和行读取；`model` 提供表与索引元信息；`tablecodec` 负责索引值截断；`codec` 负责可排序键编码；`kv` 提供 `Handle`、`IntHandle`、common handle 和 `PartitionHandle`；`stmtctx` 决定时区和错误策略；`collate`、`mysql`、`size` 分别支撑比较、字段类型和内存估算；`cascades-base` 提供结构哈希/相等协议。

RustCodeGraph 的文件节点显示本文件被 35 个文件使用，并能识别 `HandleCols`、两个实现及构造器；但本次索引对部分 Rust trait 方法/自由函数的 `callers` 未返回边，因此上面的具体入口又以精确源码搜索和文件读取核验，未把缺失的图边当作“没有调用者”。

## 错误处理与边界

- 可恢复错误通过 `Result<_, expression::Error>` 传播：列下标解析失败、Datum 比较失败、公共键编码未被语句上下文消化，以及 `kv::NewCommonHandle` 拒绝编码。
- `buildHandleByDatumsBuffer` 必须经过 `StatementContext::HandleError`；独立测试 `pkg/planner/util/handle_cols_test.rs::common_handle_routes_encode_errors_through_statement_context` 以源码约束验证了这一与 Go 一致的错误路由。
- 多处前置条件由索引操作或 `expect` 强制，而非返回错误：列的 `RetType` 必须存在；`Column.Index`、`IndexColumn.Offset` 必须落在输入范围内；索引回表行必须至少包含规定的句柄列和分区 ID；公共句柄比较的 `collators` 必须至少覆盖句柄列数。违反这些计划内部不变量会 panic。
- `GetCol` 对越界返回 `None`；这与 Go 版本对 common handle 直接下标可能 panic 的行为不同，Rust 两种实现都提供安全的可选返回值。
- `GetCommonHandleDatum` 对整型句柄返回空向量，因此它不能作为通用“提取任意句柄原值”的 API 使用。
- 编码错误被 `StatementContext` 消化时，代码以空字节继续调用 `NewCommonHandle`；是否最终成功由 `kv` 层校验决定，调用者不应假定被消化的错误一定得到有效句柄。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或 I/O 资源。所有构造和比较均为同步调用，临时 Datum 向量与编码字节在调用栈内拥有并随调用结束释放。

trait 方法只接收 `&self`，正常执行不修改句柄描述；`ResolveIndices` 和 `CloneHandleCols` 返回新对象。公共句柄还拥有表/索引元信息的拷贝，避免依赖外部引用生命周期；缓存提取再次克隆元信息。能否在线程间共享取决于各字段及 `dyn HandleCols` 的上层包装，本 trait 本身没有声明 `Send` 或 `Sync`，因此本文件不提供跨线程保证。

`StatementContext` 仅在一次构造调用期间借用，用于时区和错误处理，不被保存。文件顶部及 Go 注释共同体现了重要生命周期约束：句柄列对象可能进入执行器或计划缓存，不应向结构中加入 session/statement context 之类的请求态字段。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/util/handle_cols.go`。Rust 保留了 Go 的核心接口、两种实现、四条构造路径、行尾布局、前缀截断与时区编码、比较、字段类型、内存估算、构造器及 common-handle Datum 提取逻辑。

主要表示差异如下：

- Go 使用指针字段和 nil 语义；Rust 使用拥有型 `Box`/值字段，因此 `Hash64` 不需要 `NilFlag` 分支，`Equals` 也不表示 nil 接收者。
- Go 的 `Clone` 对列逐个调用动态克隆；Rust 的 `expression::Column: Clone` 和元信息 `Clone()` 完成拥有型深拷贝。
- Go 的 `ResolveIndices` 在 common handle 中复用表/索引指针；Rust 克隆元信息，隔离性更强。
- Rust trait 增加了 `CacheCommonHandleMetadata`，这是缓存快照序列化所需的局部接线；Go 接口中没有该方法。
- Rust `GetCol` 返回 `Option<&Column>`，显式表达越界；Go 返回指针且 common-handle 实现直接下标。
- Rust 当前独立测试仅覆盖 EncodeKey 错误必须经过 `StatementContext::HandleError` 的移植约束，未完整复刻 Go 版本所有行为分支。Go 同目录没有专属 `handle_cols_test.go`；其行为更多由 planner/executor 的上层测试间接覆盖。

## 扩展指南

- 新增句柄表示时，应实现 `HandleCols` 的全部协议，而不只实现构造函数：尤其要保证 `Hash64` 与 `Equals` 字段集合一致、`ResolveIndices` 返回新对象、索引行和分区索引行遵循既有尾部布局，并明确计划缓存如何捕获元数据。
- 修改 common-handle 编码必须集中审查 `buildHandleByDatumsBuffer`，保持 `TruncateIndexValues -> EncodeKey(TimeZone) -> StatementContext::HandleError -> NewCommonHandle` 顺序，并同步核对 `pkg/planner/util/handle_cols.go`。
- 修改构造器对齐规则时，要同时检查 `logical_plan_builder_runtime.rs` 的 DataSource 建立流程和 `cache_snapshot.rs` 的恢复流程；后者有意使用 `NewCommonHandlesColsWithoutColsAlign`，不应再次按 `IndexColumn.Offset` 重排。
- 修改索引行布局时，要成对更新 common/int 两种实现的普通与分区方法，并检查生产者是否仍把分区 ID 放在最后一列。
- 测试应继续放在独立文件 `pkg/planner/util/handle_cols_test.rs`，不要嵌入生产源文件。建议补充公共/整型句柄的普通行、Datum 行、索引尾部布局、分区 ID、前缀截断、比较 collation、下标解析失败、哈希/相等与缓存快照往返测试。
- 兼容风险主要是编码字节或行尾位置变化导致读错 KV；正确性风险是 schema 下标或 collator 下标错配；性能风险集中在每行 Datum 收集、元信息克隆和编码分配。新增字段时还应同步评估 `MemoryUsage`。

## 验证依据

- 源码与模块边界：`pkg/planner/util/handle_cols.rs`、`pkg/planner/util/lib.rs`、`pkg/planner/util/Cargo.toml`。
- Rust 独立测试：`pkg/planner/util/handle_cols_test.rs`。
- Go 语义对照：`pkg/planner/util/handle_cols.go`；点查入口补充证据为 `pkg/planner/core/point_get_plan.go::buildHandleCols`。
- Rust 直接入口与调用证据：`pkg/planner/core/logical_plan_builder_runtime.rs` 的 DataSource 句柄构造，`pkg/planner/core/operator/physicalop/physical_union_scan.rs::ResolveIndices`，以及 `pkg/planner/core/operator/physicalop/cache_snapshot.rs::CachedHandleCols::{capture,restore}`。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`node --file pkg/planner/util/handle_cols.rs --offset 1 --limit 700` 读取完整 575 行并报告 35 个文件级使用者；`query HandleCols`、`query IntHandleCols`、`query CommonHandleCols` 定位 Rust/Go 对应符号；`callers`/`callees` 查询暴露了同名歧义和部分自由函数调用边缺失，故再用精确 `rg` 核验上述入口。
- 本任务为纯文档分析，没有运行 Cargo 或代码测试。交付前执行任务指定的 11 章节结构校验，并人工检查文档只描述源码能够支持的现状。
