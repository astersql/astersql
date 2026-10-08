# [`pkg/planner/core/plan_cacheable_checker.rs`](./plan_cacheable_checker.rs)

## 文件定位

该文件属于 `astersql-planner-core` crate。`pkg/planner/core/Cargo.toml` 将 crate 根指定为 `lib.rs`；`pkg/planner/core/lib.rs` 以私有模块 `mod plan_cacheable_checker;` 装配本文件，再通过 `pub use plan_cacheable_checker::*;` 重导出其公开函数。因此模块名不对外公开，但四个判定入口可从 crate 根调用。

它位于计划缓存决策边界：一组函数检查简化 AST 能否缓存，另一组函数检查简化物理计划树能否缓存。仓库内已发现的 Rust 直接调用者均为测试（`pkg/planner/core/plan_cacheable_checker_test.rs` 和 `pkg/planner/core/casetest/plancache/plan_cacheable_checker_test.rs`）；没有发现生产 Rust 文件直接调用这些入口。故当前可确认的事实是“crate 已导出并验证这些检查器”，不能据此断言它们已接入 Rust SQL 请求主链。

## 核心职责

- `Cacheable` / `CacheableWithCtx`：先限制允许的顶层语句种类，再递归检查简化 AST 中的子查询、全局 `SET` 和显式非只读节点，返回布尔值及首个拒绝原因。
- `NonPreparedPlanCacheableWithCtx`：递归统计 `AstNode::Value` 常量数；超过调用方提供的上限时拒绝，否则复用默认禁止子查询的 AST 检查。
- `isPlanCacheable`：按固定优先级检查计划总内存、带参数的 `Dual`、TiFlash `TableReader`、`Apply`、Shuffle 类算子，然后深度优先检查子计划；首个失败向上传播。

该实现刻意只覆盖 `pkg/planner/core/util.rs::AstNode` 与 `pkg/planner/core/common_plans.rs::PlanNode` 所能表达的简化语义，不读取会话、InfoSchema、表元数据或真实 parser AST。

## 主要符号

- `pub fn Cacheable(node: &AstNode) -> bool`：便捷入口，等价于 `CacheableWithCtx(node, false, true).0`；它隐藏拒绝原因，默认不允许子查询，并视参数化 Limit 开关为开启。
- `pub fn CacheableWithCtx(node: &AstNode, subquery: bool, param_limit: bool) -> (bool, String)`：AST 主入口。顶层只接受 `Select`、`Insert`、`Update`、`Delete`；`param_limit == false` 时无条件返回 `parameterized limit disabled`。内部函数 `visit` 递归检查节点并返回静态原因。
- `visit(n: &AstNode, s: bool) -> Option<&'static str>`：局部递归函数。它遍历 `Select`、`Do`、`Subquery`、`Explain`、聚合/窗口函数参数，以及只读 `Other` 的子节点；遇到禁止的子查询、全局 `SET` 或非只读 `Other` 时停止。
- `pub fn NonPreparedPlanCacheableWithCtx(node: &AstNode, max_params: usize) -> (bool, String)`：非 Prepared 入口。内部 `walk` 统计整棵可表达 AST 中的 `Value` 节点；只有 `count > max_params` 才超限，所以恰好等于上限仍可继续检查。
- `walk(n: &AstNode, c: &mut usize)`：局部全树遍历函数，覆盖与 `visit` 相同的容器形态；它只计数，不提前短路。
- `pub fn isPlanCacheable(plan: &PlanNode, param_num: usize, max_size: i64) -> (bool, String)`：计划树入口。`max_size > 0` 时才启用大小限制；计划内存严格大于上限才拒绝。

本文件无模块级常量、类型、trait、`impl` 或条件编译项；四个模块级函数均为公开 API，两个递归辅助函数仅在各自函数体内可见。

## 执行流程

AST 路径按以下顺序执行：

1. `Cacheable` 选择默认上下文，或调用方直接进入 `CacheableWithCtx`。
2. `CacheableWithCtx` 先匹配根节点。非 `Select`/`Insert`/`Update`/`Delete` 立即拒绝；因此即使 `Set`、`Show`、`Do` 或 `Explain` 的内部节点本身可遍历，它们也不能作为顶层通过。
3. 顶层合法后检查 `param_limit`。该参数在简化实现中是整体门控，并不定位具体 Limit 节点。
4. `visit` 以枚举匹配递归。`Iterator::find_map` 保证按子节点顺序返回第一个拒绝原因；没有命中时返回 `(true, "")`。

Non-Prepared 路径先通过 `walk` 统计全部 `Value` 节点，再比较 `max_params`。未超限时调用 `CacheableWithCtx(node, false, true)`，所以它除了常量上限外仍会拒绝子查询、非法顶层、全局 `SET` 和非只读 `Other`。

计划路径先对根计划调用 `PlanNode::MemoryUsage()`。该方法已经递归包含子树和部分字符串字段，因此每次递归进入子节点时会再次计算该子树大小；随后依次检查当前节点的 `Dual`、TiFlash `TableReader`、`Apply`、`Shuffle`/`ShuffleReceiver`，最后按 `children` 顺序递归。任何子节点失败即原样返回原因，整棵树全部通过才返回 `(true, "")`。

## 数据与状态

所有状态均为栈上局部数据：AST 检查只借用不可变的 `AstNode`；Non-Prepared 检查使用一个 `usize` 计数器；计划检查借用不可变的 `PlanNode` 并接收复制的限制参数。函数不会修改 AST、计划或全局状态。

关键不变量如下：

- 成功结果始终是 `(true, String::new())`；失败结果始终带当前实现定义的非空原因。
- AST 与计划检查均采用“首个失败胜出”，原因受节点遍历顺序与检查顺序影响。
- `max_params` 的边界为严格大于；`max_size` 同样为严格大于，且零或负数表示不启用大小限制。
- `Dual` 只有在 `param_num > 0` 时拒绝；无参数的 `Dual` 可缓存。
- TiFlash 限制只在 `PlanKind::TableReader` 且 `store_type == StoreType::TiFlash` 时触发。

## 依赖与调用关系

直接源码依赖全部来自当前 crate 根：`AstNode` 定义于 `pkg/planner/core/util.rs`；`PlanKind`、`PlanNode`、`StoreType` 定义于 `pkg/planner/core/common_plans.rs`。计划大小判断下调用 `PlanNode::MemoryUsage()`，它累计节点自身、全部子树以及 `access_object`、`operator_info` 的长度。

内部调用边为：`Cacheable -> CacheableWithCtx -> visit`；`NonPreparedPlanCacheableWithCtx -> walk`，未超限后再调用 `CacheableWithCtx`；`isPlanCacheable` 对自身递归。RustCodeGraph 精确查询确认了四个模块级函数与两个局部函数；调用边命令未返回额外生产调用者，`rg` 的 Rust 调用点复核只找到本文件自调用及两处独立测试文件。

`Cargo.toml` 声明 crate 名为 `astersql-planner-core`，没有为本模块设置专属 feature；`nextgen` feature 只转发到配置依赖，源文件中也没有 `cfg` 分支。本文件本身未直接引用 Cargo 外部依赖。

## 错误处理与边界

这些函数不返回 `Result`、不 panic，也不执行可失败的 I/O；“不可缓存”属于正常判定结果，以 `(false, reason)` 表示。原因字符串会成为兼容表面，测试对多个字符串做精确比较，修改文字可能影响上层警告或测试预期。

当前边界与保守项包括：

- `CacheableWithCtx` 的错误文案提到 `SET`，但顶层白名单实际上不包含 `AstNode::Set`；嵌套的全局 `SET` 才能命中特定原因。
- `param_limit` 是抽象布尔门控，并未检查 AST 中是否真的存在参数化 Limit。
- `visit` 对 `Insert`、`Update`、`Delete` 视为叶节点；简化枚举也没有表达真实语句的完整子树。
- Non-Prepared 路径只计算常量数量，没有检查常量类型、系统表、函数、列类型、Hint、锁、视图等 Go 路径的条件。
- 计划检查没有读取每个计划节点可能携带的独立不可缓存原因，也未覆盖 MemTable、IndexMerge/MVI、IndexMerge 全扫和 DML SelectPlan 解包等 Go 规则。
- 深层退化树会消耗递归栈；源码没有显式深度限制。

## 并发与资源生命周期

文件没有锁、原子变量、线程、异步任务、通道、事务或连接。所有输入均为共享不可变借用，因此函数自身不制造数据竞争；返回的 `String` 由调用者拥有。局部计数器和递归栈在调用结束时释放，没有跨调用缓存。

这与 Go 的 Non-Prepared 实现不同：Go 使用 `sync.Pool` 复用检查器并在 `reset` 中清理可变字段；Rust 简化实现每次调用只创建局部 `usize`，不存在池化对象的重置或归还生命周期。`PlanNode::MemoryUsage()` 和随后计划递归都可能遍历子树，因而主要资源风险是大树上的 CPU 开销与递归栈，而非共享资源争用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/plan_cacheable_checker.go`，crate 元数据也用 `package.metadata.porting.go-package = "pkg/planner/core"` 标明 Go 包来源。

- Rust `Cacheable` / `CacheableWithCtx` 对应 Go 同名测试辅助入口及 `IsASTCacheable` 的基础意图，但签名以简化 `AstNode` 和两个布尔量替代 `PlanContext`、`InfoSchema` 与完整 parser AST。
- 两端共有的语义包括顶层 DML/SELECT 白名单、子查询开关、全局 `SET` 拒绝、参数数量保护，以及部分物理计划拒绝原因。
- Go AST 检查还处理 SetOpr、Insert/In-list 参数上限、用户变量、不可缓存函数、Order/Group/Limit 参数、窗口 frame、CTE 作用域与表元数据；Rust 当前未移植这些结构和规则。
- Go `NonPreparedPlanCacheableWithCtx` 有语句快速检查、DML/锁/Hint 限制、表名抽取、列类型和常量类型校验、系统 schema、指标计数及 `sync.Pool`；Rust 当前只做 `Value` 数量与基础 AST 检查。
- Rust `isPlanCacheable` 将 Go `isPlanCacheable` 与 `isPhysicalPlanCacheable` 的少量规则合并到一个 `PlanNode` 递归函数，保留大小、Dual、TiFlash、Apply、Shuffle 检查；Go 还受会话开关、节点自带原因和多种 reader/index-merge 结构影响。

测试对应关系：`pkg/planner/core/casetest/plancache/plan_cacheable_checker_test.rs` 明确以 Go `TestCacheable`、`TestNonPreparedPlanCacheable` 和 `isPhysicalPlanCacheable` 为对齐目标，覆盖当前 Rust 子集；`pkg/planner/core/plan_cacheable_checker_test.rs` 额外聚焦 TiFlash TableReader。Go 的广泛行为由 `pkg/planner/core/casetest/plancache/plan_cacheable_checker_test.go` 等测试覆盖，但不能视为 Rust 已实现证据。

## 扩展指南

扩展时应先决定规则属于 AST、Non-Prepared 特有检查还是物理计划检查，并在对应入口保持首个失败原因稳定：

- 新 AST 容器或叶节点：同步修改 `CacheableWithCtx::visit`；若其中可能包含常量，还必须同步修改 `NonPreparedPlanCacheableWithCtx::walk`，否则两个遍历会产生覆盖差异。
- 新 Non-Prepared 规则：优先在计数后、调用 `CacheableWithCtx` 前加入明确分支；若需要类型或表元数据，应先扩展输入模型和上下文，而不是用字符串猜测。
- 新不可缓存计划算子：在 `isPlanCacheable` 当前节点检查区加入分支，并确保相关 `PlanKind` 子计划不藏在 `children` 之外；reader/index-merge 一类特殊子计划若不在 `children`，需要显式遍历。
- 改动阈值时保留 `>`、禁用值语义及原因优先级，除非兼容性需求明确要求改变。

测试逻辑必须继续放在独立文件：同 crate 单元测试更新 `pkg/planner/core/plan_cacheable_checker_test.rs`，完整计划缓存案例更新 `pkg/planner/core/casetest/plancache/plan_cacheable_checker_test.rs`；涉及 Go 对齐时同时核对 `pkg/planner/core/casetest/plancache/plan_cacheable_checker_test.go`。新增规则应覆盖成功边界、恰好等于阈值、超过阈值、嵌套子节点传播以及原因字符串。主要风险是 Rust/Go 规则漂移、错误原因兼容性变化、漏遍历新 AST/计划容器，以及重复 `MemoryUsage` 带来的大树性能开销。

## 验证依据

- 源码：`pkg/planner/core/plan_cacheable_checker.rs`，确认四个公开入口、两个局部递归函数、分支顺序和原因字符串。
- 模型与装配：`pkg/planner/core/util.rs::AstNode`、`pkg/planner/core/common_plans.rs::{StoreType, PlanKind, PlanNode, PlanNode::MemoryUsage}`、`pkg/planner/core/lib.rs` 的模块声明与重导出。
- crate 边界：`pkg/planner/core/Cargo.toml` 的 `[package]`、`[lib]`、`[features]`、依赖与 `package.metadata.porting`。
- RustCodeGraph：`status` 显示索引可用；`query CacheableWithCtx`、`query NonPreparedPlanCacheableWithCtx`、`query isPlanCacheable` 定位目标函数及局部函数；`query AstNode`、`query PlanNode`、`query PlanKind`、`query StoreType` 定位直接模型定义。`callers`/`callees` 未给出额外边，随后用 Rust 调用点搜索复核。
- Rust 测试：`pkg/planner/core/plan_cacheable_checker_test.rs`；`pkg/planner/core/casetest/plancache/plan_cacheable_checker_test.rs`，覆盖顶层白名单、子查询开关、参数门控、嵌套常量计数、阈值边界、计划大小、Dual、TiFlash、Apply、Shuffle 与子树传播。
- Go 对照：`pkg/planner/core/plan_cacheable_checker.go`；Go 测试入口 `pkg/planner/core/casetest/plancache/plan_cacheable_checker_test.go`。这些文件用于确认共同意图和当前未移植能力。
- 本任务是纯文档分析，按任务约束不运行 Cargo；完成证据由固定章节结构检查、引用路径检查、差异复核和上述源码事实组成。
