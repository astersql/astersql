# `pkg/planner/core/rule_derive_topn_from_window.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate；该归属由同目录 [`Cargo.toml`](Cargo.toml) 的包名、`[lib] path = "lib.rs"` 以及 [`lib.rs`](lib.rs) 中公开的 `pub mod rule_derive_topn_from_window` 共同确认。它提供一个基于 [`task.rs`](task.rs) 中简化 `PlanNode`/`PlanKind` 模型的 Window→TopN 规则，主要用于独立规则测试和 Go 迁移语义核对。

需要区分这个文件与真实逻辑计划运行时：`optimizer_runtime.rs` 的规范规则序列虽然包含 `LogicalRule::DeriveTopNFromWindow`，但分派时调用的是 `derive_top_n_from_window_descendants`，后者对真实 `LogicalSelection` 调用 `LogicalSelection::DeriveTopN`；当前没有从该生产分派到本文件 `DeriveTopNFromWindow::Optimize` 的调用边。因此，本文件不是完整 SQL 优化主链的实现入口，而是公开、可直接测试的简化规则模型。

## 核心职责

- 自底向上遍历一棵拥有所有权的 `PlanNode` 树，使嵌套在 Projection 等任意父节点下的匹配模式也能被处理。
- 识别 `Selection(Window(child))`，并在 Selection 条件中寻找名称编码为 `row_number_le:<bound>` 的无符号整数上界。
- 匹配后把 Window 原孩子移动到新建的 TopN 下，将 Window 的 `by_items` 复制给 TopN，并把 TopN 设置为 Window 的唯一孩子，从而形成 `Selection -> Window -> TopN -> 原孩子`。
- 保留 Go 规则层契约：即使返回树已被改写，第二个返回值也固定为 `false`。

该模型只表达上述结构变换。真实 `LogicalSelection::DeriveTopN` 还会保留 `PartitionBy`、表达式方向、schema、会话上下文与 query-block offset；这些生产语义不由本文件的简化 `PlanNode` 实现承担。

## 主要符号

- `pub struct DeriveTopNFromWindow`：无字段、可 `Default` 构造的规则标记类型。它不保存优化状态，公开是因为 `lib.rs` 将所在模块公开，crate 内单元测试和外部 casetest 都直接实例化该单元结构体。
- `pub fn Optimize(&self, mut plan: PlanNode) -> (PlanNode, bool)`：消费输入计划并返回重建后的计划及规则级 `changed` 标志。命名沿用 Go 风格。函数先递归子树，再匹配当前节点；返回的布尔值始终为 `false`。
- `pub fn Name(&self) -> &'static str`：返回稳定注册名 `derive_topn_from_window`，与 Go `Name()` 完全一致。
- 本文件没有模块级常量、trait、条件编译项或错误类型；唯一依赖导入是 `crate::task::{PlanKind, PlanNode}`。

## 执行流程

1. `Optimize` 取得输入 `PlanNode` 的所有权，并对 `plan.children` 执行 `into_iter()`。
2. 对每个孩子递归调用同一个 `Optimize`，只保留递归返回的计划，刻意丢弃每层始终为 `false` 的 changed 值；收集结果后替换当前节点的全部孩子。这确定了自底向上的处理顺序。
3. 当前节点必须是 `PlanKind::Selection`，且第一个孩子必须是 `PlanKind::Window`。空孩子、非 Selection、Selection 下非 Window 都跳过。
4. 依次扫描 `plan.conditions`；对每个表达式的 `name` 去除精确前缀 `row_number_le:`，再用类型推断得到的 `u64` 解析余串。`find_map` 选择第一个成功解析的条件。
5. 匹配成功后创建 `PlanKind::TopN`，把 `count` 设为解析出的上界，把 Window 的 `by_items` 克隆到 TopN。
6. `std::mem::take` 移走 Window 的全部原孩子并赋给 TopN，然后把 Window 的孩子替换为只含该 TopN 的向量。Selection 和条件自身保持不变。
7. 无论是否改写，最终都返回 `(plan, false)`；`Name` 不参与遍历，只提供固定规则名。

## 数据与状态

规则对象自身是零大小、无状态类型。所有可变状态都在传入的 `PlanNode` 树中：节点类型由 `kind` 表示，过滤条件位于 `conditions`，排序项位于 `by_items`，TopN 数量写入 `count`，树结构位于 `children`。

所有权变化是本逻辑的关键不变量：递归阶段消费并重建孩子向量；注入阶段通过 `mem::take` 把 Window 的原孩子完整移动给 TopN，避免复制整棵子树。排序表达式则通过 `clone` 复制，因此 Window 和 TopN 各自保留一份 `by_items`。规则不修改 Selection 条件，不重算 schema、统计信息或代价，也不设置 PlanFlags。

条件协议是简化模型专用的字符串编码。由于 `top.count` 是 `u64`，负数、空串、溢出值和非数字都解析失败；`0` 是可解析值，并不会在本层被拒绝。若有多个合法条件，只采用迭代顺序中的第一个。

## 依赖与调用关系

上游直接证据如下：

- [`rule_derive_topn_from_window_test.rs`](rule_derive_topn_from_window_test.rs) 作为 `lib.rs` 中 `#[cfg(test)]` 的独立测试模块直接调用 `DeriveTopNFromWindow.Optimize`。
- [`casetest/rule/rule_derive_topn_from_window_test.rs`](casetest/rule/rule_derive_topn_from_window_test.rs) 和 [`casetest/windows/window_push_down_test.rs`](casetest/windows/window_push_down_test.rs) 从公开 crate 模块导入该类型，覆盖递归、匹配及不匹配结构。
- RustCodeGraph 的文件使用关系只列出上述三个 Rust 文件；生产运行时没有直接调用本类型。

本文件的直接下游是 [`task.rs`](task.rs) 中的 `PlanNode::new`、`PlanKind::{Selection, Window, TopN}` 以及 `PlanNode` 的 `children`、`conditions`、`by_items`、`count` 字段。递归调用 `self.Optimize` 是唯一内部调用环。

真实主链位于 [`optimizer_runtime.rs`](optimizer_runtime.rs)：`LOGICAL_RULES` 把该规则放在聚合下推之后、谓词简化和 TopN 下推之前；运行时分派到 `derive_top_n_from_window_descendants`，再调用 [`operator/logicalop/logical_selection.rs`](operator/logicalop/logical_selection.rs) 的 `LogicalSelection::DeriveTopN`。这条链是理解规则在完整应用中位置的依据，但不是本文件自身的调用者。

## 错误处理与边界

`Optimize` 没有 `Result` 返回值，也不主动产生错误。模式不匹配和上界解析失败都采用“保持计划结构不变”的静默路径。`first()` 和 `is_some_and` 安全处理无孩子节点；`strip_prefix` 保证只接受精确协议；`parse().ok()` 将数字错误转换为不匹配。

重要边界包括：只检查 Selection 的第一个孩子；只接受 `row_number_le:` 而不接受 `row_number_lt:`；不验证条件表达式是否真的引用窗口输出列；不验证 Window 函数类型、分区键合法性或上界语义；不阻止对已经含有 TopN 的相同树再次调用后继续嵌套 TopN。上述限制由当前源码结构决定，扩展时不能假定简化字符串模型已经具备真实表达式检查。

独立测试确认 `row_number_lt:5`、`row_number_le:not-a-number` 和 Selection(TableScan) 不改写；现有测试没有覆盖零、溢出、多合法条件或重复执行的行为，因此这些属于源码可推导但尚无直接测试回归的边界。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄。`&self` 可共享是因为规则对象无状态，但 `Optimize` 通过值传递独占整棵 `PlanNode`；并行调用是否可行取决于调用方是否持有彼此独立的计划所有权。

资源生命周期完全受 Rust 所有权管理：旧的孩子向量在递归收集时被替换；Window 原孩子经 `mem::take` 转移给 TopN；局部 TopN 随后转移进 Window。函数结束时，没有进入返回树的临时值自动释放，不存在需要显式清理的外部资源。

## 与 Go 版本的对应关系

同路径 [`rule_derive_topn_from_window.go`](rule_derive_topn_from_window.go) 定义同名无字段类型。Go `Optimize(context.Context, base.LogicalPlan)` 直接返回 `p.DeriveTopN(), false, nil`，`Name()` 同样返回 `derive_topn_from_window`。Rust 文件对齐了无状态规则、规则名、计划可能变化但 changed 固定为 false 的契约。

实现层级并非一一对应：Go 规则把遍历和节点分派交给 `LogicalPlan.DeriveTopN`；本 Rust 文件在简化 `PlanNode` 上自行递归，并以表达式名称编码上界。真实 Rust 对应语义存在于 `optimizer_runtime.rs::derive_top_n_from_window_descendants` 和 `LogicalSelection::DeriveTopN`：后者构造真实 `LogicalTopN`，保留排序方向、分区键、schema、上下文与 query-block offset。这一差异意味着本文件适合验证规则轮廓，不能单独证明真实 SQL 表达式识别和完整生产计划属性。

Go [`casetest/rule/rule_derive_topn_from_window_test.go`](casetest/rule/rule_derive_topn_from_window_test.go) 通过 SQL、TiFlash/MPP 和 golden plan 验证四个 Window 场景，并另测关闭该开关时含 `rand()` 的普通 TopN。Rust casetest 会读取相同 fixture 清单，但其中对 `rand()` 的覆盖是隔离的静态形状断言；这也应在解释迁移覆盖时如实区分。

## 扩展指南

若只扩展这个简化规则，首要修改点是 `DeriveTopNFromWindow::Optimize` 的模式识别和 TopN 构造，并同步独立的 [`rule_derive_topn_from_window_test.rs`](rule_derive_topn_from_window_test.rs)；面向 casetest 的结构、fixture 或 MPP 边界则同步更新 [`casetest/rule/rule_derive_topn_from_window_test.rs`](casetest/rule/rule_derive_topn_from_window_test.rs) 或 [`casetest/windows/window_push_down_test.rs`](casetest/windows/window_push_down_test.rs)。测试逻辑必须继续放在独立测试文件，不嵌入本生产文件。

若扩展真实 SQL 行为，还必须同时评估 `LogicalSelection::windowIsTopN`、`LogicalSelection::DeriveTopN` 和 `optimizer_runtime.rs::derive_top_n_from_window_descendants`，并对照 Go 的 `LogicalSelection.DeriveTopN` 及 SQL golden；只修改本文件不会改变真实运行时分派。新增条件形式时应明确上界类型、多个条件的优先级、重复执行幂等性、PartitionBy/OrderBy 保留方式，以及与后续 `PushDownTopN` 规则的顺序交互。

兼容性风险主要是误识别条件后改变结果语义，或丢失真实计划属性；性能风险主要是无效/重复 TopN、额外表达式克隆和递归深度。任何更改都应继续验证 changed=false 是否仍是 Go 兼容要求，不能仅因树改变就擅自改为 true。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；查询了 `DeriveTopNFromWindow`、目标文件源码、文件使用关系、`optimizer_runtime.rs` 的规则表/分派/递归函数，以及 Rust/Go `LogicalSelection::DeriveTopN`。
- 目标源码：[`rule_derive_topn_from_window.rs`](rule_derive_topn_from_window.rs) 的 `DeriveTopNFromWindow::{Optimize, Name}`。
- crate 与模块边界：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)；该 crate 没有为本规则设置专属 feature。
- 简化数据模型：[`task.rs`](task.rs) 的 `Expression`、`PlanKind`、`PlanNode`。
- Go 对照：[`rule_derive_topn_from_window.go`](rule_derive_topn_from_window.go) 和 [`operator/logicalop/logical_selection.go`](operator/logicalop/logical_selection.go)。
- Rust 测试：[`rule_derive_topn_from_window_test.rs`](rule_derive_topn_from_window_test.rs)、[`casetest/rule/rule_derive_topn_from_window_test.rs`](casetest/rule/rule_derive_topn_from_window_test.rs)、[`casetest/windows/window_push_down_test.rs`](casetest/windows/window_push_down_test.rs)。
- Go 测试：[`casetest/rule/rule_derive_topn_from_window_test.go`](casetest/rule/rule_derive_topn_from_window_test.go)。
- 本任务按计划是纯文档分析，未运行 Cargo；完成前另执行任务规定的 11 章节结构检查并人工复核调用边和迁移边界。
