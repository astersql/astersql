# `pkg/planner/cascades/pattern/engine.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-cascades-pattern`。crate 入口
[`lib.rs`](lib.rs) 将私有模块 `engine` 的公开项全部再导出，因此规划器其他 crate
以 `cascades_pattern::EngineType`、`EngineAll` 等名称使用这里的类型和常量。
[`Cargo.toml`](Cargo.toml) 表明该 crate 本身只有一个 `logicalop` 路径依赖；本文件只依赖
标准库 `std::fmt`，并不直接依赖逻辑算子实现。

它位于 Cascades 模式匹配的基础数据层：[`pattern.rs`](pattern.rs) 用这里的
`EngineTypeSet` 为每个 `Pattern` 节点记录允许的执行引擎；旧 Memo 路径的
[`pkg/planner/memo/expr_iterator.rs`](../../memo/expr_iterator.rs) 再把该约束与
`Group.EngineType` 比对。这里不选择物理计划、不移动算子，也不执行规则。

## 核心职责

- 用 `EngineType(pub u32)` 表示单个执行位置，并为 TiDB、TiKV、TiFlash 分配互不重叠的位。
- 用 `EngineTypeSet(pub u32)` 表示允许引擎的集合，提供 Only、Or、All 五个预置集合。
- 通过 `EngineTypeSet::Contains` 做无分配的位包含判断，供 Pattern 匹配过滤候选。
- 通过 `fmt::Display for EngineType` 生成与 Go 常量名一致的诊断字符串。

这里的“引擎”是规划器分组/规则的执行位置标签。Go 对照文件
[`engine.go`](engine.go) 进一步说明：Gather 上方属于 TiDB，Gather 下方可属于 TiKV
或 TiFlash；不同引擎可以支持不同算子和成本模型。本文件只编码标签与集合运算，
并不验证某个算子是否真的能在该引擎执行。

## 主要符号

- `pub struct EngineType(pub u32)`：单引擎标识。元组字段公开，因此调用方也能构造
  未预定义的位值。派生 `Clone`、`Copy`、`Debug`、`Eq`、`Hash`、`PartialEq`，适合按值
  传递、比较以及作为哈希键。
- `EngineTiDB`、`EngineTiKV`、`EngineTiFlash`：值依次为 `1 << 0`、`1 << 1`、
  `1 << 2`，三个位彼此独立。
- `pub struct EngineTypeSet(pub u32)`：引擎集合的位掩码；同样公开底层值并派生值语义
  trait。
- `EngineTiDBOnly`、`EngineTiKVOnly`、`EngineTiFlashOnly`：各包含一个已知引擎位。
- `EngineTiKVOrTiFlash`：存储侧两个引擎位的并集，不包含 TiDB。
- `EngineAll`：三个已知引擎位的并集；它不是 `u32::MAX`，因此不自动包含未来新增位。
- `EngineTypeSet::Contains(self, engine: EngineType) -> bool`：当集合与参数至少共享一个位
  时返回 `true`。
- `impl fmt::Display for EngineType`：三个精确单值分别输出 `EngineTiDB`、`EngineTiKV`、
  `EngineTiFlash`；其他值输出 `UnknownEngineType`。

本文件没有 trait 声明、异步函数、条件编译项或内部私有辅助函数。除 `fmt` 实现外，
所有定义都是公开 API；crate 入口又将其整体再导出。

## 执行流程

典型的旧 Memo 匹配链如下：

1. 规则构造函数用 `NewPattern`/`BuildPattern` 把本文件的某个集合写入
   `Pattern.EngineTypeSet`。例如
   [`join_to_apply.rs`](../rule/join/join_to_apply.rs) 的根 Join 使用
   `EngineTiDBOnly`，左孩子 Any 使用 `EngineAll`。
2. [`group.rs`](../../memo/group.rs) 在 `NewGroupWithSchema` 中把新 Group 的
   `EngineType` 初始化为 `EngineTiDB`；需要时调用 `Group::SetEngineType` 改为 TiKV
   或 TiFlash。
3. [`expr_iterator.rs`](../../memo/expr_iterator.rs) 的 `ExprIter::Reset`、
   `NewExprIterFromGroupElem` 和 `newExprIterFromGroup` 读取 `Group.EngineType`，传给
   `Pattern::Match` 或 `Pattern::MatchOperandAny`。
4. [`pattern.rs`](pattern.rs) 的这两个方法先调用 `EngineTypeSet::Contains`，再结合
   Operand 是否匹配决定候选能否进入后续子树匹配；若引擎位不相交，候选被拒绝。

`Contains` 自身只有一次按位与：`self.0 & engine.0 != 0`。因此传入单个预定义引擎时，
结果就是集合成员判断；若调用方手工构造包含多个位的 `EngineType`，语义会变成“任一位
相交”，而不是要求集合包含参数的全部位。

显示流程与匹配独立：格式化 `EngineType` 时仅对三个精确预定义值做模式匹配。组合值、
零值或未知位都走 `UnknownEngineType` 分支。

## 数据与状态

两个类型都只是一个 `u32`，没有堆分配、引用或内部可变状态。所有预置值均为编译期
常量，`Contains` 按值接收集合与引擎，不修改任何对象。

关键不变量是三个已知引擎必须占用不同的单比特位；Only 集合等于对应单值，Or/All
集合由这些位做按位或得到。`EngineAll` 当前数值为 `0b111`。由于元组字段为 `pub`，
Rust 类型系统不强制“单个 `EngineType` 只能含一位”或“集合只能含已知位”，调用方需要
自行维护这些约束。

规划状态真正存放在相邻模块：`Pattern.EngineTypeSet` 保存规则侧约束，
`memo::Group.EngineType` 保存候选 Group 的实际引擎。本文件只定义双方共享的值域。

## 依赖与调用关系

直接下游只有 `std::fmt`。直接上游首先是 [`pattern.rs`](pattern.rs)：
`Pattern.EngineTypeSet` 使用集合类型，`Pattern::Match` 与 `Pattern::MatchOperandAny`
调用 `Contains`，`NewPattern`/`BuildPattern` 接收集合参数。

再上一层，旧 Memo 的 [`expr_iterator.rs`](../../memo/expr_iterator.rs) 是已核实的运行时
过滤调用方；[`group.rs`](../../memo/group.rs) 持有实际 `EngineType`。各 Cascades 规则
则通过 Pattern 构造间接消费预置集合，例如
[`join_to_apply.rs`](../rule/join/join_to_apply.rs)。精确检索还显示旧变换规则以及 Memo
测试广泛使用 `EngineTiDBOnly`、`EngineTiKVOnly`、`EngineTiFlashOnly`、
`EngineTiKVOrTiFlash` 和 `EngineAll`。

需要注意两条匹配实现的差异：旧 Memo `ExprIter` 会调用 `Pattern::Match*`，因而执行本文件
定义的引擎过滤；当前 [`binder.rs`](../rule/binder.rs) 的 `r#match` 只通过
`GetOperand(...).Match(pattern.Operand)` 检查 Operand，没有读取 Group 的
`EngineType`，也没有调用 `Pattern::Match`。因此不能仅凭规则 Pattern 中存在
`EngineTypeSet` 就断言新 Binder 路径已经按引擎过滤。

## 错误处理与边界

本文件没有 `Result`、`Option`、panic 或 I/O 错误路径。对任意 `u32` 都能完成包含判断
与格式化：未知值不会报错，而是显示为 `UnknownEngineType`。

边界语义包括：空集合 `EngineTypeSet(0)` 不包含任何非零预定义引擎；零值
`EngineType(0)` 与任何集合按位与都为零；带多个位的 `EngineType` 只要与集合有一个公共
位，`Contains` 就返回真；带组合位的值即使完全由三个已知位组成，`Display` 也输出
`UnknownEngineType`。现有 Rust/Go 单元测试验证五个预置集合对三个单引擎的成员关系，
但没有覆盖零值、未知位、组合 `EngineType` 或 `Display`。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件句柄或网络资源。两个 `Copy` 值可独立
复制，不共享可变资源；所有操作都是常量时间的本地整数运算，因此自身没有生命周期或
清理要求。

并发正确性取决于持有这些值的上层结构。例如 Memo Group 使用 `Rc<RefCell<_>>` 管理
状态，但那是 [`group.rs`](../../memo/group.rs) 的责任；`engine.rs` 不提供跨线程共享
保证，也不参与借用管理。

## 与 Go 版本的对应关系

Rust 文件逐项对齐 [`engine.go`](engine.go)：Go 的 `type EngineType uint` 对应 Rust 的
`EngineType(u32)`，三个 `iota` 位值对应 Rust 的三个显式移位；Go 的
`type EngineTypeSet uint`、五个集合常量和 `Contains` 的非零按位与判断均被保留；Go 的
`EngineType.String()` 对应 Rust 的 `fmt::Display`，三个已知名称和未知回退文本一致。

已知语言层差异是 Go 使用平台宽度的 `uint`，Rust 固定为 32 位；当前只占三位，因此
不影响既有值。Rust 使用 newtype 区分单值与集合，并额外派生比较、哈希和调试 trait，
但公开元组字段仍允许构造任意值。命名保留 Go 风格大写形式，crate 根的
`#![allow(non_snake_case, non_upper_case_globals)]` 为此关闭对应警告。

[`engine_test.go`](engine_test.go) 与 [`engine_test.rs`](engine_test.rs) 的断言集合一一
对应：均验证 All、三个 Only 以及 TiKV-or-TiFlash 对三个预定义引擎的真假矩阵。Rust
测试独立放在 `engine_test.rs`，符合生产源码与测试分文件的仓库约束。

## 扩展指南

新增执行引擎时，至少应同步修改 `EngineType` 常量、需要包含它的集合（尤其评估
`EngineAll` 是否应扩大）以及 `Display` 分支，并在独立的
[`engine_test.rs`](engine_test.rs) 增加新引擎对所有预置集合的正反断言；Go 仍作为对照
实现时，还应同步 [`engine.go`](engine.go) 与 [`engine_test.go`](engine_test.go)。

若新增的是集合组合而非引擎位，只需定义新的 `EngineTypeSet` 常量并验证完整成员矩阵，
但要搜索所有规则 Pattern，确认哪些规则应采用该集合。改变 `Contains` 为“包含全部位”
会影响手工组合 `EngineType` 的语义；虽然正常调用传入单引擎，仍应先补零值、未知位和
组合值测试，再评估兼容性。

若目标是让新 Binder 路径执行引擎过滤，接入点不在本文件，而在
[`binder.rs`](../rule/binder.rs) 的 `r#match`/`dfsMatch` 及其可获得的 Group 引擎信息；
应为 Binder 增加独立回归测试，不能通过修改位运算掩盖上层缺少调用的问题。性能方面，
本文件的位运算已是常量时间；扩展时应继续避免把热路径成员判断改成分配型集合。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；
  `files --filter pkg/planner/cascades/pattern` 返回本 crate 的 Rust/Go 源与测试文件。
- RustCodeGraph `node --file pkg/planner/cascades/pattern/engine.rs`：核对完整 66 行源码、
  两个 newtype、八个常量、`Contains` 和 `Display`。
- RustCodeGraph `query EngineType --kind struct` 与 `query EngineTypeSet --kind struct`：
  定位到本文件第 25、36 行的公开类型；`query EngineTypeSet::Contains` 未产生方法节点，
  因而使用精确 `rg` 引用检索补足调用证据。
- RustCodeGraph `node` 读取
  [`pattern.rs`](pattern.rs)、[`lib.rs`](lib.rs)、[`engine_test.rs`](engine_test.rs)、
  [`group.rs`](../../memo/group.rs)、[`expr_iterator.rs`](../../memo/expr_iterator.rs)、
  [`binder.rs`](../rule/binder.rs) 和 [`join_to_apply.rs`](../rule/join/join_to_apply.rs)，
  核实 crate 再导出、Pattern 调用、Group 状态、旧 Memo 过滤和 Binder 边界。
- 直接读取未由调用图完整表达的 [`Cargo.toml`](Cargo.toml)、[`engine.go`](engine.go) 与
  [`engine_test.go`](engine_test.go)，核对 crate 边界、Go 语义和测试矩阵；精确 `rg`
  检索核对 Rust/Go 的常量及 `Contains` 使用点。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前另运行任务指定的 11 章节结构
  命令，并人工复核唯一生产物、链接、事实边界和扩展建议。
