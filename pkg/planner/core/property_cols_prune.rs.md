# `pkg/planner/core/property_cols_prune.rs`

## 文件定位

该文件属于 `astersql-planner-core` crate：`pkg/planner/core/Cargo.toml` 将库根指定为 `lib.rs`，而 `pkg/planner/core/lib.rs` 以私有模块 `property_cols_prune` 装载它，并通过 `pub use property_cols_prune::*` 重导出公开入口。

文件名沿用 Go 的 `property_cols_prune.go`，但这里实现的不是删除无用输出列的 `ColumnPruner` 规则，而是为逻辑计划树收集“可能物理属性”（主要是可利用的列顺序，并携带 TiFlash 可用性信号）。真正的逻辑列裁剪规则位于 `pkg/planner/core/rule/rule_column_pruning.rs`。

## 核心职责

- `prepare_possible_properties_for` 提供与具体计划类型解耦的后序遍历骨架：先完整计算每个直接子树的 `PossiblePropertiesInfo`，再计算当前节点。
- `preparePossibleProperties` 把通用骨架适配到 `base_dependency::LogicalPlan`：用 `LogicalPlan::logical_children` 枚举子节点，并把当前节点的 `schema()` 与按子节点顺序排列的结果传给动态分派方法 `prepare_possible_properties`。
- 本文件只负责遍历和结果传递。排序属性的生成、保留、过滤以及 `has_tiflash` 的具体规则属于各逻辑算子实现，不在这里编码。

## 主要符号

- `prepare_possible_properties_for<T, Children, Prepare>(plan, children, prepare) -> PossiblePropertiesInfo`（第 23 行）：crate 内可见的泛型递归器。`Children` 对任意借用生命周期返回子节点引用列表；`Prepare` 接收当前节点与子结果切片。两个闭包均要求 `Copy`，以便递归层重复传递而不转移所有权。
- `preparePossibleProperties(plan: &dyn LogicalPlan) -> PossiblePropertiesInfo`（第 40 行）：公开的逻辑计划适配入口。名称保留 Go 风格；输入是只读 trait object，返回根节点最终属性值。
- `PossiblePropertiesInfo`（`pkg/planner/core/base/plan_base.rs`）：`orders: Option<Vec<Vec<Option<Column>>>>` 保存候选有序列组，`has_tiflash: bool` 保存运行时裁剪信号。本文件只搬运这个值，不读取或修改字段。
- `LogicalPlan::prepare_possible_properties`（`pkg/planner/core/base/plan_base.rs`）：算子级扩展点，签名要求同时接收当前 schema 和所有直接子节点的属性。

文件中没有常量、结构体、枚举、`impl` 块、条件编译项或可变全局状态。

## 执行流程

1. 调用者把根逻辑计划交给 `preparePossibleProperties`。
2. 适配入口调用 `prepare_possible_properties_for`，并提供“取逻辑子节点”与“准备当前节点属性”两个操作。
3. 通用递归器调用 `children(plan)`，保持返回列表的原始顺序遍历子节点。
4. 对每个子节点递归执行相同步骤，并以 `collect::<Vec<_>>()` 收集完整结果；因此父节点不会在任何直接子节点完成前执行。
5. 所有子结果完成后，调用 `prepare(plan, &children_properties)`。逻辑计划适配器在此动态分派 `node.prepare_possible_properties(node.schema(), children)`。
6. 当前节点的返回值逐层向上传播，最终函数返回根节点的 `PossiblePropertiesInfo`。

`pkg/planner/core/property_cols_prune_test.rs::possible_properties_are_prepared_post_order_from_child_results` 用根 `3` 和两个叶子 `1、2` 验证访问序列严格为 `[1, 2, 3]`，并验证根节点看见的子结果数等于子节点数。

## 数据与状态

- 每个递归层都会创建一个新的 `Vec<PossiblePropertiesInfo>`；其元素与 `logical_children()` 返回的子节点一一对应且顺序一致。这一位置对应关系是 join、union 等多子节点算子解释子属性的前提。
- 叶子节点得到空切片，而不是缺失值；叶子的算子实现据此自行构造属性。
- 结果按值拥有。遍历器不缓存结果到计划节点、不修改 schema，也不共享子结果容器；具体 `PossiblePropertiesInfo` 内部的列对象按其类型自身的克隆/所有权规则管理。
- `PossiblePropertiesInfo.orders` 的 `None` 与 `Some` 语义由 `base/plan_base.rs` 保留；`has_tiflash` 是结果的一部分，但本文件不合并它，合并策略完全由算子方法决定。

## 依赖与调用关系

- 直接依赖只有 `base_dependency::{LogicalPlan, PossiblePropertiesInfo}`；`base_dependency` 在 `pkg/planner/core/Cargo.toml` 中映射到同仓库 crate `astersql-planner-core-base`（路径 `base`），没有由本文件单独启用的 feature。
- RustCodeGraph 记录的内部调用边为：`preparePossibleProperties` 调用 `prepare_possible_properties_for`；后者递归调用自身并调用传入的 `prepare` 闭包；公开适配闭包进一步调用 `LogicalPlan::schema` 与 `LogicalPlan::prepare_possible_properties`。
- RustCodeGraph 记录 `prepare_possible_properties_for` 的直接测试调用者为 `possible_properties_are_prepared_post_order_from_child_results`，并记录 `property_cols_prune_test.rs` 对该内部函数的导入。
- `pkg/planner/core/lib.rs` 重导出 `preparePossibleProperties`，但当前仓库的 Rust 生产源码搜索未发现该公开函数的调用点。因此可以确认它已暴露为 crate API，不能据现有证据声称它已进入 Rust `physicalOptimize` 主链。
- Go 对照主链明确为 `pkg/planner/core/optimizer.go::physicalOptimize` 调用 `preparePossibleProperties(logic)`，随后构造根 `PhysicalProperty` 并执行 `FindBestTask`。这是 Go 的接线证据，不应外推为 Rust 已完成接线。

## 错误处理与边界

- 两个函数均不返回 `Result`，也没有显式错误分支；算子属性接口本身同样直接返回 `PossiblePropertiesInfo`。若计算不能失败，这是清晰契约；若未来算子准备可能失败，必须同步把错误类型贯穿递归器、trait 与调用链，不能在闭包内吞掉错误。
- 输入是引用，Rust 类型系统排除了 Go 的空计划指针。函数假定 `logical_children()` 形成有限、无环的树；代码没有环检测或递归深度保护，异常深树可能耗尽调用栈，共享成环结构则不符合接口预期。
- Go 实现会把子节点或当前节点返回的 `nil` 规范化为空 `PossiblePropertiesInfo`；Rust 返回值不是 `Option`，从类型层面禁止这种 `nil` 状态。这是表达方式差异，不是遗漏的空值分支。
- 通用递归器没有校验 `children` 在递归期间是否稳定；当前只读 `&T` 接口阻止普通可变修改，但采用内部可变性的自定义 `T` 仍应保证同一次遍历的树形关系稳定。

## 并发与资源生命周期

遍历是同步、串行、深度优先的；兄弟节点按 `children` 给出的顺序依次处理，没有线程、异步任务、锁、通道、事务、文件句柄或网络资源。生命周期通过高阶 trait bound `for<'a> Fn(&'a T) -> Vec<&'a T>` 绑定：子引用不能活得比被借用节点更久；子结果 `Vec` 只存活到当前节点的 `prepare` 调用结束，随后被释放。

主要资源风险是计划树深度带来的调用栈和每层临时向量分配。正常 SQL 计划通常满足树和深度约束；若要改成并行或迭代遍历，必须先证明算子计算无副作用、兄弟顺序不影响结果，并保持子结果的位置对应关系。

## 与 Go 版本的对应关系

Rust `preparePossibleProperties` 对应 `pkg/planner/core/property_cols_prune.go::preparePossibleProperties`：两者都以后序方式递归 `Children`，把当前 `Schema` 和全部子结果交给算子接口，再返回当前节点属性。

主要差异如下：

- Go 直接写循环，Rust 将递归抽成可单测的 `prepare_possible_properties_for`，公开适配器只负责绑定 trait 方法。
- Go 使用 `*PossiblePropertiesInfo`，并显式把子结果和根结果的 `nil` 转为空结构；Rust 使用拥有的非可空值，类型上消除了这两个分支。
- Go 入口是包内小写函数且已由 `optimizer.go::physicalOptimize` 调用；Rust 入口是 `pub` 并由 `lib.rs` 重导出，但当前 Rust 生产源码没有调用证据。迁移状态应描述为“遍历器和单元测试已存在，生产优化主链接线未验证/未发现”，而不是宣称与 Go 运行路径完全等价。
- 独立 Rust 测试覆盖后序顺序、子结果数量和根结果传播；本次搜索未发现 Go 为该小函数设置独立测试，Go 行为主要由优化器及算子相关测试间接覆盖。

## 扩展指南

- 新增或修改算子属性策略时，优先实现/调整对应算子的 `LogicalPlan::prepare_possible_properties`，不要把算子类型判断塞入本遍历器；本文件应继续只维护遍历不变量。
- 改变子节点枚举或多子节点规则时，必须保持 `children_properties[i]` 对应 `logical_children()[i]`，并在独立测试文件 `pkg/planner/core/property_cols_prune_test.rs` 增加不对称子树、多层树和空子节点用例。
- 若把公开入口接入 Rust 物理优化主链，应对照 Go `optimizer.go::physicalOptimize` 的调用时机：统计推导之后、根物理属性和最优任务搜索之前；同时增加主链级测试，证明算子准备结果确实影响候选物理属性，而不只证明递归器可调用。
- 若引入可失败计算，应设计 `Result<PossiblePropertiesInfo, E>` 并保持首个失败的上下文；若为性能改写为显式栈，应保留后序语义和兄弟顺序，并用宽树、深树回归测试内存与栈行为。
- 不要把 Rust 单元测试内嵌到生产文件；继续使用同目录独立的 `property_cols_prune_test.rs`，并保留 `lib.rs` 中的 `#[cfg(test)] mod property_cols_prune_test;` 接线。

## 验证依据

- 源码：`pkg/planner/core/property_cols_prune.rs`（两个函数及其完整控制流）。
- crate 与模块边界：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`。
- trait 与数据模型：`pkg/planner/core/base/plan_base.rs::LogicalPlan::prepare_possible_properties`、`PossiblePropertiesInfo`。
- Go 对照与生产调用：`pkg/planner/core/property_cols_prune.go::preparePossibleProperties`、`pkg/planner/core/optimizer.go::physicalOptimize`。
- 独立 Rust 测试：`pkg/planner/core/property_cols_prune_test.rs::possible_properties_are_prepared_post_order_from_child_results`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点；`query prepare_possible_properties_for` 定位唯一实现；`node property_cols_prune.rs::preparePossibleProperties` 给出到泛型递归器的调用边；`node prepare_possible_properties_for` 给出自递归、公开包装器和测试调用边；`node LogicalPlan::prepare_possible_properties` 核对 trait 签名。
- 全仓文本核验：`rg` 确认 Rust 生产源码中没有 `preparePossibleProperties` 的外部调用点，而 Go `optimizer.go` 有明确调用；因此文档将 Rust 生产接线标为未发现，不推测其运行效果。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 章节的结构命令和人工事实复核验收。
