# `pkg/planner/util/explain_misc.rs`

## 文件定位

本文件属于 `astersql-planner-util` crate，是规划器通用工具中的 EXPLAIN 文本格式化辅助。`pkg/planner/util/lib.rs` 以私有模块 `mod explain_misc` 装配它，并用 `pub use explain_misc::*` 将唯一公开函数 `ExplainByItems` 从 crate 根重新导出；调用方因此使用 `planner_util::ExplainByItems`，而不是直接访问模块。

它位于“计划节点持有的排序/分组项”与“展示给用户的 EXPLAIN 文本”之间：输入 `ByItems` 列表和表达式求值上下文，将每项表达式的说明文本、降序标记和项目分隔符追加到调用方提供的 `String`。它不负责构造计划、排序数据、决定 TopN 的 offset/count，也不负责完整 EXPLAIN 行的其他字段。

`pkg/planner/util/Cargo.toml` 将该目录定义为包名 `astersql-planner-util` 的库 crate（入口为 `lib.rs`，`autotests = false`、`doctest = false`）。本文件直接使用同 crate 的 `ByItems`，并通过 Cargo 中的 `expression = { package = "astersql-expression", path = "../../expression" }` 依赖调用表达式 API；没有条件编译项或文件级 feature 分支。

## 核心职责

`ExplainByItems` 只承担一种稳定的串接规则：按输入顺序格式化全部 `ByItems`，普通项输出表达式自身的 `ExplainInfo`，降序项在其后追加 `:desc`，相邻项之间追加 `, `，末项之后不追加分隔符（`pkg/planner/util/explain_misc.rs:30-47`）。

调用方拥有完整输出布局。本函数只“追加”而不清空 `buffer`，所以物理 TopN 可以先写 partition-by/`order by ` 前缀，再调用本函数，随后继续写 offset/count；物理 Sort 则从空字符串开始，再按需追加 TiFlash `stream_count`（`pkg/planner/core/operator/physicalop/physical_topn.rs:150-181`、`physical_sort.rs:121-141`）。

表达式内容由 `item.Expr.ExplainInfo(context)` 决定，因此列名、常量脱敏和标量函数展示等语义属于 `expression::Expression` 的实现；本函数不解析或改写表达式，也不自行处理敏感常量（trait 签名见 `pkg/expression/expression.rs:268-329`）。

## 主要符号

- `pub fn ExplainByItems<'a>(context: &dyn expression::exprctx::EvalContext, buffer: &'a mut String, items: &[ByItems]) -> &'a mut String`：文件中唯一函数，也是唯一公开符号。显式生命周期 `'a` 只把返回引用绑定到输入 `buffer`，不把返回值绑定到 `context` 或 `items`；调用者可继续链式追加同一个字符串。
- `context`：只读的表达式求值上下文，逐项传给 `Expression::ExplainInfo`。格式化结果可能受该上下文中的展示/脱敏设置影响，但本文件不读取具体设置。
- `buffer`：调用方独占借用的可变字符串；函数保留原有内容并在尾部追加结果。
- `items`：`ByItems` 的只读切片。`ByItems` 定义于 `pkg/planner/util/byitem.rs:29-34`，核心字段是表达式对象 `Expr: expression::ExprBox` 与方向标记 `Desc: bool`。

本文件没有模块级常量、struct、enum、trait、impl 或条件编译项。`use expression::Expression as _` 只把 trait 方法引入方法解析范围；`use std::fmt::Write` 则支持 `write!` 向 `String` 写入。

## 执行流程

1. 以 `enumerate()` 顺序遍历 `items`，不会排序、去重或过滤输入。
2. 对当前项调用 `item.Expr.ExplainInfo(context)`，先取得拥有所有权的 `String` 说明文本。
3. 若 `item.Desc` 为真，用 `write!(buffer, "{explanation}:desc")` 同时追加表达式文本和降序后缀；否则直接 `buffer.push_str(&explanation)`。
4. 仅当当前索引后仍有项目时追加 `, `，从而避免尾随分隔符。
5. 遍历结束后返回原 `buffer` 的可变引用。空切片不会写入任何字符；单项不会产生逗号；多项保持输入顺序。

例如表达式说明依次为 `a`、`b` 且第二项 `Desc = true` 时，追加片段为 `a, b:desc`。若传入的 buffer 已经是 `order by `，最终内容就是 `order by a, b:desc`；前缀并非本函数生成。

## 数据与状态

函数无自有持久状态，也不修改 `ByItems` 或表达式对象。每轮创建一个局部 `explanation: String`，随后复制其字符到调用方 buffer；时间复杂度由逐项表达式格式化与总输出长度共同决定，除表达式说明本身外，本层为线性遍历。

唯一可观察状态变化是 `buffer` 变长。该变化是增量式的：如果后续项目的表达式格式化发生不可恢复行为，本函数没有回滚此前已追加内容的事务语义。当前 `Expression::ExplainInfo` 返回普通 `String` 而非 `Result`，所以本函数没有业务错误返回通道。

方向只编码降序：`Desc = true` 显式追加 `:desc`，`Desc = false` 不追加 `:asc`。这与 Go 版本保持一致，也意味着消费者需把“无后缀”理解为默认升序。

## 依赖与调用关系

RustCodeGraph 的文件查询显示本文件有两个 Rust 使用者：

- `PhysicalSort::ExplainInfo`（`pkg/planner/core/operator/physicalop/physical_sort.rs:121-141`）从计划上下文取得 `EvalContext`，格式化 `self.ByItems`，随后可追加 TiFlash shuffle 流数。
- `PhysicalTopN::ExplainInfo`（`pkg/planner/core/operator/physicalop/physical_topn.rs:150-181`）先格式化 partition-by 片段及必要的 `order by ` 连接词，再格式化 `self.ByItems`，最后按脱敏模式追加 offset/count，必要时还追加前缀列信息。

向下调用只有核心语义依赖 `Expression::ExplainInfo(context)`；字符串追加依赖标准库 `String::push_str` 和 `std::fmt::Write`。`ByItems` 由同 crate 的 `byitem` 模块提供，`lib.rs` 的再导出将它和 `ExplainByItems` 一并暴露给 planner core。

精确的 RustCodeGraph `callers`/`callees` 命令未输出函数级边，因此上述调用点进一步由局部 `rg` 和源码读取核验；不要据此推断逻辑 Sort/TopN 的 Rust 实现也调用该函数。当前仓库文本搜索只找到上述两个 Rust 调用点。

## 错误处理与边界

- 空 `items`：原样返回 buffer，不添加空格、逗号或占位文本。
- 单项/末项：通过 `index + 1 < items.len()` 保证没有尾随 `, `。
- 降序标记：严格追加小写 `:desc`；升序没有显式后缀。
- buffer 已有内容：不插入自动边界字符。调用方必须自行保证已有前缀与首项之间，以及本函数输出与后续字段之间的空格/逗号正确。
- 写入失败：`write!` 的结果使用 `expect("writing into String cannot fail")`。对标准 `String` 的 `fmt::Write` 实现没有 I/O 失败面，这个 `expect` 表达的是内部不变量，而不是可向上传播的业务错误。普通分支的 `push_str` 同样没有 `Result`。
- 表达式展示：函数相信每个 `ExprBox` 都能按 `Expression` trait 返回说明字符串；它不验证空文本、逗号等表达式内部字符，也不捕获 panic。

该函数不做空指针检查：Rust 的引用与切片签名已排除 null。与 Go 的 `[]*ByItems` 相比，Rust 切片元素是值 `ByItems`，不存在单个 nil item 的合法输入。

## 并发与资源生命周期

函数不创建线程、任务、锁、通道、事务或外部资源，也没有静态可变状态。并发安全由借用关系和依赖对象的 trait 约束共同限定：调用期间 `&mut String` 保证当前调用独占修改 buffer；`context` 和 `items` 仅共享只读借用。

返回引用的生命周期与 buffer 相同，调用结束后局部 `explanation` 立即释放；返回值不能比输入 buffer 活得更久。函数不会保存 `context`、`items` 或 buffer 的引用，也不会跨异步边界，因此没有后台生命周期或清理动作。

多个线程可以各自使用独立 buffer 调用；共享同一个 buffer 必须由调用方在函数外完成同步。函数自身没有提供或要求锁。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/util/explain_misc.go:24-38`。两版都按原顺序遍历所有项目，都调用表达式的 `ExplainInfo(ctx)`，都只为降序项追加 `:desc`，都用 `, ` 分隔相邻项，并返回原输出缓冲区以支持继续拼接。

实现载体有以下差异：Go 使用 `*bytes.Buffer`、`[]*ByItems` 和 `fmt.Fprintf`；Rust 使用 `&mut String`、`&[ByItems]`、`push_str`/`write!`，并用生命周期保证返回引用来自输入 buffer。Rust 的 `write!` 以 `expect` 固化“写 String 不失败”的不变量，而 Go 忽略 `fmt.Fprintf` 返回值。

调用覆盖尚不完全对称。Go 当前由逻辑 Sort、逻辑 TopN、物理 Sort、物理 TopN 四处直接调用（`logical_sort.go:42-46`、`logical_top_n.go:52-59`、`physical_sort.go:94-101`、`physical_topn.go:137-174`）；RustCodeGraph 加文本搜索只确认物理 Sort 和物理 TopN 两处直接调用。文档因此不声称 Rust 逻辑算子已经通过本文件接线。

测试方面，同目录没有 `explain_misc_test.rs`。`pkg/planner/core/operator/logicalop/logical_sort_test.rs:40-51` 和 `logical_top_n_test.rs:40-53` 间接固定了降序说明包含/结尾为 `:desc` 的兼容语义，但它们并不直接调用本函数；现有 `physical_sort_test.rs`、`physical_topn_test.rs` 也没有针对该格式化函数的直接断言。

## 扩展指南

若要改变项目间分隔、升降序后缀或空列表行为，应集中修改 `ExplainByItems`，并先核对 Go 同名函数是否需要同步，以避免跨语言 EXPLAIN 输出漂移。由于输出会嵌入 Sort/TopN 的更大字符串，还要检查调用方自行添加的 `order by `、offset/count、prefix 和 `stream_count` 分隔规则。

若新增纯格式化分支，建议在同目录新增独立 `pkg/planner/util/explain_misc_test.rs`，并按当前 `autotests = false` 约束在 `pkg/planner/util/lib.rs` 中以 `#[cfg(test)]` 和 `#[path = "explain_misc_test.rs"]` 显式挂载；不要把测试内嵌进生产源文件。最小用例应覆盖空列表、单个升序、单个降序、多个混合方向、已有 buffer 前缀，以及受 `EvalContext` 脱敏行为影响的表达式。

若只是让新的计划节点展示 `ByItems`，优先复用 crate 根再导出的 `planner_util::ExplainByItems`，由新调用方负责前后字段布局。兼容风险主要是 EXPLAIN 黄金结果和下游解析器对精确文本的依赖；性能风险主要来自每项先生成临时 `String`，在大列表或昂贵表达式说明下可能放大分配与复制。不要在此函数中加入计划改写、求值或排序副作用。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为 48 行、2 个索引符号。
- RustCodeGraph `files --filter pkg/planner/util/explain_misc.rs` 与 `node --file ... --offset 1 --limit 400`：确认文件全貌，并报告使用文件为 `physical_sort.rs`、`physical_topn.rs`。
- RustCodeGraph `query ExplainByItems --kind function --json` 与 `node ExplainByItems`：确认 Rust/Go 两个同名定义及其签名和源码；精确 `callers`/`callees` 无输出，已用局部源码搜索补证。
- 源码与装配：`pkg/planner/util/explain_misc.rs`、`byitem.rs:26-34`、`lib.rs:15-44`、`Cargo.toml`、`pkg/expression/expression.rs:268-329`。
- Rust 调用方：`pkg/planner/core/operator/physicalop/physical_sort.rs:121-141`、`physical_topn.rs:150-181`。
- Go 对照与调用方：`pkg/planner/util/explain_misc.go:24-38`，以及逻辑/物理 Sort、TopN 的四个上述 Go 文件。
- 测试证据：`pkg/planner/core/operator/logicalop/logical_sort_test.rs:40-51`、`logical_top_n_test.rs:40-53`；同时人工检查 `physical_sort_test.rs` 与 `physical_topn_test.rs`，确认没有本函数的直接格式断言。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰好具有 11 个固定二级标题，并人工复核唯一生产物、链接路径和“已接线/未验证”边界。
