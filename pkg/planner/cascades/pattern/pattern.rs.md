# `pkg/planner/cascades/pattern/pattern.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-pattern` crate，crate 根 `pkg/planner/cascades/pattern/lib.rs` 将本文件的公开项与 `engine.rs` 的引擎位集公开重导出。`Cargo.toml` 表明它唯一的直接 Rust 依赖是 `logicalop`（包名 `astersql-planner-core-operator-logicalop`）；本文件借此把完整的 `LogicalPlan` 动态对象压缩成适合规则索引和树形匹配的 `Operand`。

它位于 Cascades 规划器的规则描述层，而不是计划执行层：规则以 `Pattern` 声明期望的算子树，优化器或 Binder 再用 `GetOperand` 识别 memo 中的逻辑计划。当前仓库中可见的直接生产入口包括 `pkg/planner/cascades/rule/binder.rs` 的 `match`、`pkg/planner/cascades/task/task_opt_group_expression.rs` 的 `getValidRules`，以及旧优化器 `pkg/planner/cascades/old/optimize.rs` 的 `GetImplementationRules` 和 `findMoreEquiv`。

## 核心职责

1. `Operand` 用有限枚举表达逻辑算子类别，并以 `OperandAny` 提供通配语义、以 `OperandUnsupported` 承接尚未映射的计划类型。
2. `GetOperand` 通过 `LogicalPlan::as_any` 和逐类型 `is::<T>()` 判断，把运行时逻辑计划映射到规则可比较的类别。
3. `Pattern` 把一个 Operand、一个 `EngineTypeSet` 和有序子模式组成模式树节点；`NewPattern`、`BuildPattern`、`SetChildren` 提供构造与更新入口。
4. `Operand::Match`、`Pattern::Match` 和 `Pattern::MatchOperandAny` 定义节点级匹配判断；其中后两者同时检查引擎集合。
5. `Display for Operand` 保持与 Go 常量名一致的诊断字符串，便于跨语言日志和调试对照。

## 主要符号

- `pub enum Operand`：包含 `Any`、Join/Aggregation/Projection/Selection/Apply 等逻辑算子、扫描与 Gather 类别、`Show`/`Window`，以及兜底的 `Unsupported`。它派生 `Clone + Copy + Debug + Eq + Hash + PartialEq`，因此可低成本按值传递并作为规则映射的键。
- `OperandAny` 至 `OperandUnsupported`：枚举变体的 Go 风格兼容常量。仓库调用侧广泛使用这些名称，例如 `old/transformation_rules.rs` 用其构造规则模式。
- `impl fmt::Display for Operand`：所有已知变体输出对应的 `OperandXxx`；与 Go `Operand.String()` 不同，Rust 枚举不存在枚举范围外的整数值，因此匹配分支覆盖全部变体。
- `pub fn GetOperand(plan: &dyn LogicalPlan) -> Operand`：核心分类入口。它识别 20 种具体计划类型；未命中时返回 `OperandUnsupported`，不返回错误。
- `pub fn Operand::Match(self, target: Operand) -> bool`：任一侧为 `Any` 即成功，否则要求完全相等，因此通配关系是对称的。
- `pub struct Pattern`：三个公开字段分别为 `Operand`、`EngineTypeSet`、`Children: Vec<Pattern>`；子节点按值拥有，模式树没有共享引用或内部可变性。
- `Pattern::Match`：先由 `EngineTypeSet::Contains` 检查引擎位，再执行 Operand 匹配。
- `Pattern::MatchOperandAny`：要求引擎被集合包含且当前节点恰为 `OperandAny`。
- `Pattern::SetChildren`：整体替换子模式向量，而非追加或合并。
- `NewPattern`：创建叶模式，`Children` 初始化为空向量。
- `BuildPattern`：创建并一次性接收完整子模式列表。

## 执行流程

规则构造时，调用者先用 `NewPattern` 建叶节点，或用 `BuildPattern` 自底向上组装有序模式树。例如 `old/transformation_rules.rs::NewRulePushSelDownTableScan` 构造 `Selection(TableScan)`；`NewRulePushSelDownTiKVSingleGather` 构造 `Selection(TiKVSingleGather(Any))`。Operand 同时也是规则表的分类键。

匹配阶段的典型路径如下：

1. `task_opt_group_expression.rs::getValidRules` 对 `GroupExpression` 的包装逻辑计划调用 `GetOperand`，再用所得 Operand 查找候选规则，并比较规则根 Operand。
2. `rule/binder.rs::NewBinder` 调用 `dfsMatch`；其内部 `match` 对 `OperandAny` 直接放行，否则调用 `GetOperand(...).Match(pattern.Operand)`。
3. 根节点命中后，Binder 要求 `Pattern.Children.len()` 与表达式输入数相同，再按位置递归匹配每个子 Group，并组合各子位候选；空子模式只约束当前节点，不约束更深子树。
4. 旧优化器 `old/optimize.rs::findMoreEquiv` 同样先按 `GetOperand` 找规则，再以 `rule_pattern.Operand.Match(operand)` 复核根节点，之后交给 memo 表达式迭代器完成树绑定。

`GetOperand` 的判断顺序是行为的一部分：`LogicalApply` 必须先于 `LogicalJoin`，源码注释指出 Apply 复用 Join 语义；其余类型按明确分支匹配，最终统一落入 `OperandUnsupported`。

## 数据与状态

`Operand` 只保留算子类别，不携带 schema、谓词、统计信息或实际孩子。其 `Copy`/`Hash` 属性使其适合成为 `HashMap<Operand, ...>` 键；旧优化器的 `implementation_rule_map` 和规则批次正这样使用。

`Pattern` 是有序、拥有式的树。`Children` 的位置必须与逻辑表达式输入位置一致，Binder 以 `iter().zip(inputs)` 对齐，并在数量不等时直接判不匹配。空 `Children` 表示“不继续约束子树”，而不是要求计划节点必须没有孩子，这一点由 `binder.rs::dfsMatch` 的提前返回体现。

`EngineTypeSet` 是 `engine.rs` 中的 `u32` 位集，支持 TiDB、TiKV、TiFlash 及其组合；`Contains` 以按位与非零判定包含。需要注意：本文件确实保存并可检查这项状态，但当前检索到的 Rust 生产匹配路径直接读取 Operand/Children，未调用 `Pattern::Match` 或 `MatchOperandAny`。因此不能仅凭 Pattern 中的 `EngineTypeSet` 断言 Binder 已实施引擎过滤。

## 依赖与调用关系

下游依赖只有两类：`logicalop::LogicalPlan` 及其具体逻辑算子类型用于 `GetOperand` 的运行时分类；同 crate 的 `EngineType`/`EngineTypeSet` 及 `Contains` 用于 Pattern 的引擎约束。标准库仅使用 `std::fmt`。

主要上游关系由代码搜索确认：

- `rule/binder.rs`：`GetOperand`、`OperandAny`、`Pattern`；消费根 Operand 和 `Children` 完成 DFS 绑定。
- `task/task_opt_group_expression.rs`：以 `GetOperand` 选择规则集合。
- `old/optimize.rs`：以 `GetOperand` 索引实现/变换规则，并调用 `Operand::Match`。
- `old/transformation_rules.rs`：大量使用 `NewPattern`、`BuildPattern`、Operand 常量和 Engine 集合声明规则形状。
- `pattern/lib.rs`：通过 `pub use pattern::*` 暴露全部公开 API。

RustCodeGraph 对目标文件报告 11 个符号并识别上述 crate 文件；精确 `callers` 子命令在本次环境中长时间无输出，故调用点以目标限定的 `rg` 与 RustCodeGraph 文件节点互相核验。未发现目标目录的 `doc.go`，因此没有额外包级契约需要合并。

## 错误处理与边界

本文件没有 `Result`、错误类型、panic 或 I/O。未知逻辑计划类型被保守映射为 `OperandUnsupported`；调用方若没有为该 Operand 注册规则，就自然得到空候选，而不是中断优化。

边界语义包括：`Any` 在匹配两侧都具有通配能力；空 Pattern 子列表不校验表达式的实际子节点；`SetChildren` 会丢弃旧列表；`EngineTypeSet(0)` 不包含任何引擎；包含多个位或未知组合位的 `EngineType` 只要与集合有任一公共位，`Contains` 就返回真。最后一点源自 `engine.rs::Contains` 的位运算，而不是本文件额外验证的“单一引擎”约束。

现有 Rust 单元测试覆盖 14 个常见 `GetOperand` 分支、Any/相等/不相等匹配、叶节点构造和一/多子节点替换；尚未直接覆盖 TiKVSingleGather、三类 Scan、Show、Window、Unsupported、Display、BuildPattern、Pattern 的引擎匹配方法。因此修改这些分支时不能把当前测试通过等同于全分支已验证。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。`Operand` 和引擎位集均为按值小对象；`Pattern` 独占其递归 `Vec<Pattern>`，离开作用域后由 Rust 正常递归释放。

`Pattern` 未使用 `Rc`/`Arc` 或内部可变性，修改子树需要 `&mut self`。上游 memo/Binder 可能使用 `Rc<RefCell<_>>` 管理组表达式，但 Pattern 自身会按值移入 Binder 或规则对象；其资源与并发保证不应和 memo 的共享所有权混为一谈。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/cascades/pattern/pattern.go`，测试对照为 `pattern_test.go`。Rust 保留了 Go 的 Operand 集合、字符串、类型映射顺序、双向 Any 匹配、Pattern 三元数据以及两个构造入口。

关键表示差异如下：Go `Operand` 是 `int` 加 `iota` 常量，Rust 是封闭枚举并额外提供同名常量；Go 的 Pattern 匿名嵌入 Operand/EngineTypeSet，Rust 使用同名公开字段；Go `Children` 是 `[]*Pattern`，空构造结果为 `nil`，Rust 是 `Vec<Pattern>`，空构造结果为空向量；Go 构造函数返回指针且子节点为可变参数，Rust 返回拥有值并接收 `Vec<Pattern>`。两者在“没有子模式”的匹配意义上对应，但 nil 与空向量、指针共享与值所有权并非表示级等价。

Go `Operand.String()` 的 `default` 会把任意未知整数显示为 `OperandUnsupported`；Rust `Display` 对封闭枚举逐项穷举。Go 与 Rust 当前测试都只覆盖 `GetOperand` 的前 14 类，且都未直接测试 BuildPattern 和 Pattern 的引擎匹配。Rust 测试额外以循环压缩了相同的 Operand 匹配断言，但没有改变语义。

## 扩展指南

新增逻辑算子类别时，应同步完成四处最小闭环：给 `Operand` 增加变体和 Go 风格兼容常量；扩充 `Display`；在 `GetOperand` 的正确优先级位置加入具体类型识别；在独立的 `pattern_test.rs` 增加映射、显示及必要的匹配回归。若 Go 版本也包含该类别，还应同步检查 `pattern.go`/`pattern_test.go`，避免跨语言枚举或诊断名漂移。

新增规则时，叶模式用 `NewPattern`，固定形状的父节点用 `BuildPattern`；子模式顺序必须与逻辑计划输入顺序一致。若只是匹配根而不约束后代，应保留空 Children。需要共享或复用子模式时，应先评估 Rust 当前按值树的克隆成本和语义，不能照搬 Go 指针别名行为。

需要让引擎约束真正参与某条匹配链时，应在对应消费者中明确调用 `Pattern::Match`/`MatchOperandAny` 或等价检查，并增加消费者侧独立测试；仅设置 `EngineTypeSet` 不足以证明约束被执行。此类改动还应关注旧优化器和新 Binder 两条路径的一致性。

测试逻辑应继续放在同目录独立文件 `pkg/planner/cascades/pattern/pattern_test.rs`，不要内嵌进生产源。高风险点是 Apply/Join 判断顺序、未知类型的 Unsupported 兜底、空 Children 的通配子树语义、子节点位置，以及 Go 指针树向 Rust 值树迁移产生的所有权差异；性能风险主要来自深/宽 Pattern 的克隆和 Binder 的组合增长，而非本文件的 Operand 比较。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标 `pattern.rs` 有 11 个符号；`files --filter pkg/planner/cascades/pattern` 确认 Rust/Go 源与测试；`node --file` 完整读取 `pattern.rs`、`pattern_test.rs`、`lib.rs`、`engine.rs`、`pattern.go`、`pattern_test.go`，并读取 `rule/binder.rs`、`task/task_opt_group_expression.rs`、`old/optimize.rs`、`old/transformation_rules.rs` 的直接消费片段。
- crate 边界：`pkg/planner/cascades/pattern/Cargo.toml` 声明库入口 `lib.rs`、关闭自动测试与 doctest，并只直接依赖 `logicalop`；porting 元数据指向 Go 包 `pkg/planner/cascades/pattern`。
- Rust 独立测试：`pattern_test.rs::TestGetOperand`、`TestOperandMatch`、`TestNewPattern`、`TestPatternSetChildren`。
- Go 对照：`pattern.go::{Operand.String, GetOperand, Operand.Match, Pattern.Match, Pattern.MatchOperandAny, NewPattern, Pattern.SetChildren, BuildPattern}` 与 `pattern_test.go` 四组对应测试。
- 调用证据：`binder.rs::match/dfsMatch`、`task_opt_group_expression.rs::getValidRules`、`old/optimize.rs::GetImplementationRules/findMoreEquiv`、`old/transformation_rules.rs::NewRulePushSelDownTableScan/NewRulePushSelDownIndexScan/NewRulePushSelDownTiKVSingleGather`。
- 本任务是只新增说明文档的分析任务，按计划不运行 Cargo；交付前以任务指定命令验证文档存在且固定二级标题恰好为 11 个，并人工复核未将未接线的引擎过滤描述成已生效行为。
