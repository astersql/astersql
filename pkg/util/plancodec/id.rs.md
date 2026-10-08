# [`pkg/util/plancodec/id.rs`](id.rs)

## 文件定位

`id.rs` 属于 `astersql-util-plancodec` crate（`pkg/util/plancodec/Cargo.toml`），由 crate 根 `pkg/util/plancodec/lib.rs` 以 `mod id; pub use id::*;` 纳入并再导出。它位于执行计划文本/压缩编码与解码的基础层：为物理计划节点提供稳定的类型字符串和数字 ID，并在两者之间转换。调用者不必直接引用私有 `id` 模块，而是通过 crate 根使用这些公开常量和函数。

该文件不负责构造、优化或执行计划，也不读写协议流；真正的计划行编码、归一化和解码位于 `pkg/util/plancodec/codec.rs`。这里提供的是这些流程依赖的兼容性字典。数字 ID 会写入可持久化或可传输的计划表示，因此既有映射属于线格式兼容契约。

## 核心职责

1. 以公开 `Type*` 字符串常量命名 Selection、Projection、Join、Scan、Reader、MPP exchange、CTE、外键检查、ImportInto、Analyze 等计划节点（`id.rs:29-159`）。
2. 以私有 `type*ID` 常量冻结对应的物理计划数字 ID 1 到 64（`id.rs:161-229`）；唯一公开的数字常量是 `TypeScalarSubQueryID = 60`，与 Go 可见性保持一致。
3. `TypeStringToPhysicalID` 将已知类型名编码为稳定 ID，未知字符串回退为 `0`（`id.rs:235-308`）。
4. `PhysicalIDToTypeString` 将已知 ID 解码为类型名，未知 ID 格式化为 `UnknownPlanID{id}`（`id.rs:314-387`）。

本文件刻意不为 `TypeSequence` 分配物理 ID；`TypeSequence` 只存在于字符串常量集合中，当前 `TypeStringToPhysicalID(TypeSequence)` 的真实结果是 `0`。这一边界由 `pkg/util/plancodec/migration_aster_unit_test.rs:96-97` 固定。

## 主要符号

- `pub const TypeSel ... TypeAnalyze: &str`：共一组公开计划类型名。字符串是线格式的一部分，不能仅为风格统一而改拼写；例如 `TypePointGet` 对应 `"Point_Get"`，`TypeForeignKeyCheck` 对应 `"Foreign_Key_Check"`。
- `const typeSelID ... typeAnalyzeID: isize`：私有稳定 ID 表，当前覆盖 1..=64。编号按历史追加，不等同于源文件中字符串常量出现顺序；例如 `TypePartitionUnion` 的 ID 是 53，`TypeLocalIndexLookUp` 的 ID 是 61。
- `pub const TypeScalarSubQueryID: isize = 60`：需要从 crate 外访问的稳定 ID；Rust 和 Go 均将这一项公开。
- `pub fn TypeStringToPhysicalID(tp: &str) -> isize`：纯匹配编码函数。输入借用字符串，不分配；命中返回稳定 ID，未命中返回 0。
- `pub fn PhysicalIDToTypeString(id: isize) -> String`：纯匹配解码函数。已知 ID 将静态常量复制为 `String`，未知 ID 动态生成 `UnknownPlanID{id}`。

文件没有类型、trait、`impl`、宏、静态可变状态或条件编译项。公开 API 由 `lib.rs` 再导出；数字常量除 `TypeScalarSubQueryID` 外保持模块私有，避免调用者依赖内部名称。

## 执行流程

编码路径有两种直接形态。`pkg/util/plancodec/codec.rs:473-495` 的 `NormalizePlanNode` 将传入的 UTF-8 计划类型交给 `TypeStringToPhysicalID`，然后把十进制 ID 写入制表符分隔的归一化计划行；`codec.rs:497-507` 的 `encodeID` 同样取得 ID，再与实例 ID 拼成 `typeId_instanceId`，供 `EncodePlanNode` 在 `codec.rs:404-458` 写入普通计划行。无效 UTF-8 在调用点被替换为空字符串，因此进入本文件后按未知类型返回 0。

解码路径位于 `pkg/util/plancodec/codec.rs:372-401`：解析器从计划行第二列取出数字 ID，可选地分离下划线后的实例 ID，再调用 `PhysicalIDToTypeString` 恢复类型名；实例后缀随后重新附加。未知数字不会令解析立即失败，而会成为可显示的 `UnknownPlanID{id}`。

规划器也直接使用正向映射。例如 `pkg/planner/core/operator/logicalop/logical_selection.rs:75`、`logical_projection.rs:74`、`logical_limit.rs:69` 和 `logical_table_dual.rs:53` 把节点类型转为 `u32` 大端字节，作为逻辑计划哈希/规范化表示的一部分。因而修改映射不仅影响展示解码，还可能改变计划标识与比较结果。

## 数据与状态

全部映射数据都在编译期常量中，没有运行时注册表、缓存或可变状态。正向和反向函数分别维护显式 `match`，其关键不变量是：对每个已分配 ID，`TypeStringToPhysicalID(&PhysicalIDToTypeString(id)) == id`。`pkg/util/plancodec/id_test.rs:93-99` 对 1..=64 全区间验证这一往返性质。

`0` 是未知字符串的哨兵值，不是已分配计划 ID。反向未知值没有同样返回空字符串，而是保留原数字形成诊断性名称。因此未知输入的两条路径并非可逆：任意未知字符串都会丢失为 0，而未知 ID 会保留在回退文本中。`TypeSequence` 目前也走未知字符串路径。

`isize` 用来对应 Go 的 `int` 角色；编码调用点再按所需线格式转换为十进制文本或 `u32` 字节。现有有效范围只有 1..=64，不涉及溢出，但扩展者仍不应借由类型容量重排或复用历史编号。

## 依赖与调用关系

本文件只使用 Rust 标准库能力：字符串比较、`to_string` 和 `format!`，没有引用 `Cargo.toml` 中的 `base64`、`protobuf`、`snap`、`texttree-dependency`、`thiserror` 等依赖。`Cargo.toml` 表明它所在 crate 的库入口是 `lib.rs`，而 `lib.rs` 负责模块装配和公开再导出。

RustCodeGraph 将 `id.rs` 标为被 24 个文件使用，并识别出两个核心函数；精确图调用查询未返回边，因此调用点以源码搜索补证。确认的直接下游消费者包括 `pkg/util/plancodec/codec.rs` 和 `pkg/planner/core/operator/logicalop/` 下的 Selection、Projection、Limit、TableDual 实现。crate 级上游依赖还包括 planner、executor、server、TopSQL 和 statement summary 等 Cargo 包，但是否逐一使用本文件的具体符号必须由各调用点判断，不能仅由 Cargo 依赖推断。

两个转换函数没有业务函数被调用方；它们的下游仅是本文件常量、标准字符串转换和格式化。解码/编码错误处理发生在 `codec.rs`，不是在映射函数内部。

## 错误处理与边界

API 不返回 `Result`，也不会主动 panic。`TypeStringToPhysicalID` 对大小写、下划线和完整拼写做精确匹配，任何别名、空串、无效 UTF-8 经调用点降级后的空串以及 `TypeSequence` 都返回 0。调用者若把 0 写入计划表示，之后反解会得到 `UnknownPlanID0`，而不是原始输入。

`PhysicalIDToTypeString` 接受任意 `isize`，负数、0、超过 64 的数字及未来未知编号都会按十进制原样嵌入 `UnknownPlanID{id}`。这个回退形状与 Go 的 `"UnknownPlanID" + strconv.Itoa(id)` 一致，属于可观察行为。

`pkg/util/plancodec/id_test.rs` 的冻结列表只逐项覆盖部分历史映射，但其 `test_reverse` 覆盖完整 1..=64；`migration_aster_unit_test.rs:83-97` 还显式覆盖 ID 56–63、`Sequence` 和未知 ID 99。新增映射如果在 1..=64 中制造空洞或碰撞，会破坏现有往返测试；修改已有编号则会破坏历史编码兼容性。

## 并发与资源生命周期

两个函数都是无副作用的同步纯函数：只读取编译期常量，不持有锁，不使用原子变量、通道、线程、异步任务、事务、文件或网络资源，因此可被多线程并发调用。输入借用仅持续到 `TypeStringToPhysicalID` 返回；反向函数返回自有 `String`，其内存生命周期由调用者管理。

当前反向转换每次都会分配 `String`，已知和未知 ID 均如此。若未来为了减少分配而改变返回类型，需要同时处理静态已知名称与动态未知名称，且不能改变未知 ID 的文本契约；这属于 API 和性能语义变更，不能只在本文件局部替换。

## 与 Go 版本的对应关系

Rust 实现逐项对应 `pkg/util/plancodec/id.go`：公开 `Type*` 字符串、私有数字常量、`TypeStringToPhysicalID` 和 `PhysicalIDToTypeString` 的分支及回退语义一致。Go 使用 `int`，Rust 使用 `isize`；Go 反向函数直接返回常量字符串，Rust 因为未知分支需要动态格式化而统一返回 `String`。Go 用 `strconv.Itoa` 拼接未知 ID，Rust 用 `format!` 得到同形字符串。

Go 测试 `pkg/util/plancodec/id_test.go` 冻结代表性历史 ID，并验证 1..=64 的往返；Rust 独立测试 `pkg/util/plancodec/id_test.rs` 保留相同意图，并额外点名 Analyze=64。AsterSQL 迁移测试 `migration_aster_unit_test.rs` 补齐外键、Expand、ScalarSubQuery、物理 CTE 等映射以及未知值边界。

已核对的差异不是功能简化：Rust 将 Go 包级导出改为 crate 根再导出，并将字符串常量显式声明为 `&str`；转换结果及编号保持一致。两端都声明 `TypeSequence` 但没有为它分配物理 ID。

## 扩展指南

新增计划类型时，应在 `id.rs` 的字符串常量区添加 `TypeXxx`，在数字区只追加一个从未使用的新 ID，并在两个转换函数中各添加一个方向的分支。不得重排、复用或修改 1..=64 的既有 ID，也不得仅添加单向映射。若 Go 仍是兼容性基准，应同步修改 `pkg/util/plancodec/id.go`，保持字符串拼写、可见性和编号一致。

测试必须放在独立文件中：优先扩展 `pkg/util/plancodec/id_test.rs` 的冻结表和完整往返范围，并同步 `pkg/util/plancodec/id_test.go`；针对迁移边界可扩展 `migration_aster_unit_test.rs`。不要把测试嵌入 `id.rs`。若新增编号不再形成连续范围，应把往返测试改为显式映射集合，避免连续区间对保留空洞产生错误假设。

扩展还需审查 `codec.rs` 的线格式消费者和 planner 计划哈希调用点。兼容性风险是旧数据被解为错误算子或计划哈希改变；性能风险主要来自继续扩大线性 `match` 代码及反向分配，不过当前规模下应以稳定性和清晰审计优先。若要给 `TypeSequence` 正式分配 ID，必须视为新的协议能力而非修复拼写，并同时增加双向映射与跨语言测试。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；索引包含 `pkg/util/plancodec/id.rs`、Go 对照和独立测试。
- RustCodeGraph `node --file` 阅读：`pkg/util/plancodec/id.rs:1-387`、`pkg/util/plancodec/id.go:1-492`、`pkg/util/plancodec/id_test.rs:1-105`、`pkg/util/plancodec/id_test.go:1-98`、`pkg/util/plancodec/codec.rs:360-524`、`pkg/util/plancodec/migration_aster_unit_test.rs:70-114`。
- RustCodeGraph `query` 核对：Rust/Go 两端的 `TypeStringToPhysicalID` 与 `PhysicalIDToTypeString` 均被识别为独立函数；`id.rs` 的签名分别为 `(&str) -> isize` 和 `(isize) -> String`。精确 `callers/callees` 查询未输出调用边，故没有把图中缺失边写成不存在调用者，而是用 `rg` 对直接调用点补证。
- 配置与装配：`pkg/util/plancodec/Cargo.toml`、`pkg/util/plancodec/lib.rs`；调用证据：`pkg/util/plancodec/codec.rs` 和 `pkg/planner/core/operator/logicalop/*.rs`。
- 测试证据：`pkg/util/plancodec/id_test.rs`、`pkg/util/plancodec/id_test.go`、`pkg/util/plancodec/migration_aster_unit_test.rs`。本任务为纯文档分析，按计划不运行 Cargo 或代码测试。
