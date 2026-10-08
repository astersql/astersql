# `pkg/planner/property/logical_property.rs`

## 文件定位

本文件属于 `astersql-planner-property` crate，定义规划器 memo 中与具体物理实现无关的输出属性载体。`pkg/planner/property/lib.rs` 以私有模块 `logical_property` 装入本文件，再通过 `pub use logical_property::*` 将 `LogicalProperty` 和 `NewLogicalProp` 暴露为 crate 公共 API。`pkg/planner/property/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认了这一 crate 边界；同一 manifest 还声明了它所需的 expression、funcdep、统计与属性辅助依赖。

该类型同时服务两套 memo：`pkg/planner/cascades/memo/group_expr.rs::DeriveLogicalProp` 为新 Cascades Group 一次性派生完整属性，`pkg/planner/memo/group.rs::NewGroupWithSchema` 及其后续推导流程则在旧 memo 的 `Group::Prop` 中分阶段填充属性。文件自身只定义数据结构和空值构造器，不负责计算统计信息、Schema、函数依赖或物理候选顺序。

## 核心职责

- `LogicalProperty` 把一个逻辑等价类共享的六类事实聚合在一起：统计信息、输出 Schema、函数依赖、至多一行保证、候选顺序以及 TiFlash 可达性。
- `#[derive(Default)]` 规定“尚未派生”的初态：三个 `Option<Box<_>>` 为 `None`，两个布尔值为 `false`，`PossibleProps` 为空数组。这一初态只表示没有已记录的事实，不能解释为已经证明“无 Schema/FD/统计”。
- `NewLogicalProp` 提供与 Go 构造函数同名的堆分配入口，返回装箱后的默认实例。当前 Rust 生产源码中没有调用该函数；实际调用者直接使用结构体字面量或 `LogicalProperty::default()`。
- 本结构是 Group 级共享属性，而不是单个物理计划的要求。新 Cascades 的 `Group` 在 `pkg/planner/cascades/memo/group.rs` 中保存 `Option<property::LogicalProperty>`，旧 memo 的 `Group` 在 `pkg/planner/memo/group.rs` 中直接保存 `LogicalProperty`。

## 主要符号

### `pub struct LogicalProperty`

- `Stats: Option<Box<StatsInfo>>`：输出行数、列 NDV、列组 NDV 等统计摘要。`None` 表示尚无统计对象；具体定义在 `pkg/planner/property/stats_info.rs`。
- `Schema: Option<Box<expression::Schema>>`：输出列及键约束的 Schema。新 Cascades 派生时来自 `LogicalPlan::Schema().Clone()`；旧 memo 的 `NewGroupWithSchema` 只复制传入 Schema 的 `Columns`，键集合需由后续 `BuildKeyInfo` 补充。
- `FD: Option<Box<funcdep::FDSet>>`：列间函数依赖集合。新 Cascades 在 `GroupExpression::DeriveLogicalProp` 中调用被包装逻辑算子的 `ExtractFD` 并写入；旧 memo 当前主要围绕 Schema、Stats、MaxOneRow 和候选属性工作。
- `MaxOneRow: bool`：是否保证结果至多一行。新 Cascades直接取 `LogicalPlan::MaxOneRow()`；旧 memo 的 `BuildKeyInfo` 还会按算子类型与子 Group 的保证传播该值。
- `PossibleProps: Vec<Vec<expression::Column>>`：可利用的候选列顺序集合；外层是一组候选顺序，内层是一个顺序中的列序列。新 Cascades 的基础派生继承第一个孩子的候选顺序，旧 Cascades 优化器在 `preparePossibleProperties` 阶段写入推导结果。
- `HasTiFlash: bool`：当前逻辑子树是否具有 TiFlash 路径。新 Cascades 叶子读取 `PreparePossiblePropertiesValue()`，非叶子调用 `PreparePossibleProperties(&child_has_tiflash)`；旧 Cascades 与各逻辑算子的 possible-properties 推导也消费或更新它。

### `pub fn NewLogicalProp() -> Box<LogicalProperty>`

该函数仅执行 `Box::new(LogicalProperty::default())`，不做派生、不验证字段组合，也不可能失败。RustCodeGraph 对本函数未给出 Rust 调用边，仓库级 `rg` 也只命中函数定义；因此它目前主要保留 Go API 对齐和未来调用兼容性。

## 执行流程

新 Cascades 的主流程由本文件之外的 `pkg/planner/cascades/memo` 驱动：

1. `Memo::CopyIn` 自底向上插入逻辑计划；新建 Group 时调用 `GroupExpression::DeriveLogicalProp`，因而孩子 Group 已先拥有逻辑属性。
2. `DeriveLogicalProp` 若发现 owner Group 已有属性便直接返回，保证同一等价类只初始化一次。
3. 它先从孩子属性收集 `PossibleProps` 和 `HasTiFlash`，缺少孩子属性时以 `expect` 中止，而不是制造不完整属性。
4. 它从当前 `LogicalPlan` 克隆 Schema、可选 Stats 和 `ExtractFD()` 的结果，计算 `MaxOneRow` 与 TiFlash 可达性，再以结构体字面量一次性构造六字段齐全的 `LogicalProperty`。
5. `Group::SetLogicalProperty` 把结果写入 owner Group，后续同一 Group 的所有等价表达式通过 `GetLogicalProperty` 共享这份事实。

旧 memo 的流程不同：`pkg/planner/memo/group.rs::NewGroupWithSchema` 先用 `LogicalProperty { Schema: Some(...), ..Default::default() }` 创建 Group；`BuildKeyInfo` 补充 Schema 键信息和 `MaxOneRow`，`pkg/planner/cascades/old/optimize.rs` 的统计及 possible-properties 阶段再写入 `Stats`、`PossibleProps` 和 `HasTiFlash`。因此消费者必须结合所处 memo 与阶段判断哪些字段已完成派生。

## 数据与状态

`LogicalProperty` 拥有其装箱字段：替换 `Stats`、`Schema` 或 `FD` 会转移并释放旧值，不依赖外部借用生命周期。`PossibleProps` 同样拥有各候选中的 `Column` 值；这与 Go 的 `[][]*expression.Column` 指针切片表示不同，Rust 在派生和传播时通常显式 `clone`，以避免别名修改。

重要状态不变量来自调用链，而不是类型系统：新 Cascades 中设置完成的属性应至少有 `Schema` 和 `FD`，孩子 Group 在父 Group 派生前必须已有属性；`Stats` 仍允许为 `None`。默认实例则允许所有可选字段为空，供旧 memo 分阶段初始化。不要仅凭 `Default` 实例推断完整计划性质。

六个字段均为 `pub`，没有 setter 强制字段之间的一致性。例如替换 Schema 时不会自动重算 FD、统计 NDV 或候选顺序；调用者必须把这些派生数据视为一个相关集合，并在改变输出列语义后同步刷新。

## 依赖与调用关系

- 上游装配：`pkg/planner/property/lib.rs` 声明模块并再导出公共符号。
- 数据类型依赖：本文件通过 crate 再导出的 `expression::Schema`、`expression::Column` 和 `funcdep::FDSet`，以及同 crate 的 `StatsInfo` 组成结构。对应 Cargo 依赖为 `astersql-expression` 与 `astersql-planner-funcdep`。
- 新 Cascades 写入者：`pkg/planner/cascades/memo/group_expr.rs::DeriveLogicalProp` 构造全部字段；`pkg/planner/cascades/memo/group.rs::{GetLogicalProperty, SetLogicalProperty}` 管理 Group 中的实例。
- 旧 memo 写入者：`pkg/planner/memo/group.rs::NewGroupWithSchema` 创建带 Schema 的默认属性，`BuildKeyInfo` 更新键与 `MaxOneRow`；`pkg/planner/cascades/old/optimize.rs` 更新 Stats、PossibleProps 与 HasTiFlash。
- 典型读取者：新 Cascades 的 `GroupExpression::{GetInputSchema, GetChildStatsAndSchema, GetJoinChildStatsAndSchema}` 读取孩子的 Schema/Stats；旧 Cascades 的 transformation、implementation、enforcer 和 stringer 代码读取 Group 的属性以改写、枚举或展示计划。
- 构造器关系：Go 的 `GroupExpression::DeriveLogicalProp` 调用 `property.NewLogicalProp()` 后逐步填值；当前 Rust 新 Cascades 改为一次性结构体字面量，旧 memo 使用 `Default` 更新语法。因此 `NewLogicalProp` 在 Rust 中暂无线上的调用者。

## 错误处理与边界

本文件没有 `Result`、显式错误分支或输入参数校验。`NewLogicalProp` 是不可失败的普通内存构造；实际分配失败遵循 Rust 进程级分配失败行为。

边界主要由消费者约束：`None` 需要解释为“未提供/未派生”，不能无条件解引用；空 `PossibleProps` 表示当前没有记录候选顺序；`MaxOneRow == false` 只代表没有至多一行保证；`HasTiFlash == false` 代表推导结果中没有 TiFlash 路径。新 Cascades 对“不应缺失”的 owner、孩子属性和 Schema 使用 `expect`，这是内部不变量失败时的 panic，而不是本类型返回的可恢复错误。

扩展时还需注意旧 memo 的分阶段状态：添加必须始终存在的新字段会使 `Default` 初始化与后续补全链产生新的中间不完整状态；添加可选或有安全默认值的字段则仍需核查所有结构体字面量和传播点，避免默认值静默改变优化决策。

## 并发与资源生命周期

`LogicalProperty` 本身不创建线程、任务、锁、通道、文件或网络资源，也没有自定义 `Drop`。其资源生命周期完全由拥有它的 Group 控制；被替换或 Group 释放时，`Box`、`Vec` 及其中数据按 Rust 所有权规则递归释放。

两套当前 memo 都以 `Rc<RefCell<_>>` 管理 Group 图，属于单线程共享可变模型；`LogicalProperty` 没有提供跨线程同步保证。新 Cascades 在派生时短暂借用孩子再克隆所需数据，随后可变借用当前表达式和 owner Group；后续修改仍必须遵守 `RefCell` 的运行时借用规则，否则会 panic。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/property/logical_property.go`。两端均有相同名称与顺序语义的六个字段，并提供空逻辑属性构造器：Go 用可空指针表示 Stats、Schema、FD，用 `[][]*expression.Column` 表示候选列；Rust 分别使用 `Option<Box<_>>` 和拥有值的 `Vec<Vec<Column>>`。Go 的零值与 Rust 的 `Default` 在可选字段、布尔值和空集合上语义一致。

主要接线差异位于 `pkg/planner/cascades/memo/group_expr.go::DeriveLogicalProp` 与对应 Rust 函数：Go 先调用 `NewLogicalProp`，再执行 Stats/Schema/FD/possible-properties 派生并逐字段写入，且函数可返回派生错误；当前 Rust 从逻辑计划已有的缓存/接口取值并一次性构造属性，函数签名不返回错误。文档仅记录现状，不据此宣称两端完整优化流程已经等价。

旧 memo 的 `pkg/planner/memo/group.go::BuildKeyInfo` 及 Rust `pkg/planner/memo/group.rs::BuildKeyInfo` 都把 Schema 键与 `MaxOneRow` 作为后续推导状态。Go 测试 `pkg/planner/memo/group_test.go::TestBuildKeyInfo` 验证主键、聚合、Selection 与 Limit 场景；Rust 独立测试 `pkg/planner/memo/group_test.rs::build_key_info_inherits_pk_and_max_one_row_per_operand_rules` 验证对应传播规则，并额外明确 Join、SemiJoin 与不继承算子的边界。

## 扩展指南

若增加或改变逻辑属性字段，应按以下连接点同步检查：

1. 在本文件决定安全的 `Default` 语义；如默认值不能代表“未派生”，优先使用 `Option`，避免旧 memo 的阶段性构造产生错误事实。
2. 更新新 Cascades 的 `pkg/planner/cascades/memo/group_expr.rs::DeriveLogicalProp`，明确叶子与非叶子的计算、孩子传播规则以及 Group 已初始化时的行为。
3. 更新旧 memo 的 `pkg/planner/memo/group.rs::{NewGroupWithSchema, BuildKeyInfo}` 和 `pkg/planner/cascades/old/optimize.rs` 中相应推导阶段；搜索所有 `LogicalProperty { ... }` 字面量及直接字段读写。
4. 同步 Go 对照文件与 Go 派生链，或明确记录尚未移植的差异；不能只修改 Rust 数据结构而遗漏 Go 的实际算法意图。
5. 测试逻辑必须保留在独立测试文件。结构与派生字段优先扩展 `pkg/planner/cascades/memo/group_and_expr_test.rs`、`pkg/planner/core/casetest/cascades/memo_test.rs`、`pkg/planner/memo/group_test.rs` 和 `pkg/planner/cascades/old/optimize_test.rs`；不要把测试内嵌到本生产源文件。

兼容风险集中在公共字段类型和默认语义，正确性风险集中在 Schema/FD/Stats/顺序之间失配，性能风险集中在扩大 `Column`、Schema 或统计对象的克隆量。新增字段后还应检查调试格式与计划枚举读取者是否需要展示或消费它。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，可用于本次 Rust/Go 符号与文件关系核验。
- RustCodeGraph `node --file pkg/planner/property/logical_property.rs`：确认文件仅含 `LogicalProperty`、六个公开字段和 `NewLogicalProp`，并报告直接使用文件为 `pkg/planner/cascades/memo/group_expr.rs` 与 `pkg/planner/memo/group.rs`。
- RustCodeGraph `explore "pkg/planner/property/logical_property.rs LogicalProperty NewLogicalProp"`：确认属性进入 Cascades Group，并定位 `GroupExpression::DeriveLogicalProp`、Group 存取接口和 memo 格式化测试。
- RustCodeGraph 对 `pkg/planner/cascades/memo/group_expr.rs`、`pkg/planner/memo/group.rs` 及相关测试的精确 `node --file` 查询：确认新 memo 的一次性六字段派生、旧 memo 的阶段性初始化，以及测试覆盖的传播边界。
- 已读取生产与装配文件：`pkg/planner/property/logical_property.rs`、`pkg/planner/property/lib.rs`、`pkg/planner/property/Cargo.toml`、`pkg/planner/cascades/memo/group_expr.rs`、`pkg/planner/cascades/memo/group.rs`、`pkg/planner/memo/group.rs`、`pkg/planner/cascades/old/optimize.rs`。
- 已读取 Go 对照：`pkg/planner/property/logical_property.go`、`pkg/planner/cascades/memo/group_expr.go`、`pkg/planner/cascades/memo/memo.go`、`pkg/planner/memo/group.go`。
- 已核对独立测试：`pkg/planner/cascades/memo/group_and_expr_test.rs::TestDeriveLogicalPropPreservesFD` 验证 Stats/Schema/FD；`pkg/planner/core/casetest/cascades/memo_test.rs` 验证逻辑属性格式与 GroupNDV；`pkg/planner/cascades/old/optimize_test.rs::test_prepare_possible_properties` 验证 PossibleProps；`pkg/planner/memo/group_test.rs::build_key_info_inherits_pk_and_max_one_row_per_operand_rules` 验证 MaxOneRow；逻辑算子独立测试覆盖 HasTiFlash 的叶子与传播行为。Go 侧还核对了 `pkg/planner/memo/group_test.go::TestBuildKeyInfo` 和 `pkg/planner/core/casetest/cascades/memo_test.go`。
- 仓库级 `rg` 确认 Rust `NewLogicalProp` 没有定义之外的调用；Go 构造器由 Cascades `DeriveLogicalProp` 调用。本文没有把未观察到的 Rust 调用关系写成已接线事实。
