# `pkg/util/texttree/texttree.rs`

## 文件定位

本文件是 `astersql-util-texttree` crate 的核心实现，负责把层次结构编码为 Unicode 文本树前缀。crate 入口 `pkg/util/texttree/lib.rs` 将本文件声明为私有模块，并公开再导出全部五个字符常量以及 `Indent4Child`、`PrettyIdentifier`；因此调用方通过 crate 根使用 API，而不直接依赖内部模块路径。`pkg/util/texttree/Cargo.toml` 指定 `lib.rs` 为库入口、没有运行时依赖，并以 `package.metadata.porting.go-package = "pkg/util/texttree"` 标明对应的 Go 包。

它位于展示层的通用工具边界，不解析计划或采样数据，也不持有树节点。当前可核实的两个生产消费方是 `pkg/util/plancodec/binary_plan_decode.rs::decodeBinaryOperator`（生成二进制执行计划的树形算子 id）和 `pkg/util/profile/flamegraph.rs::FlamegraphCollector::{collect,collect_child}`（生成火焰图文本树行）。对应 Cargo 依赖分别见 `pkg/util/plancodec/Cargo.toml` 与 `pkg/util/profile/Cargo.toml`。

## 核心职责

1. 用 `TreeBody`、`TreeMiddleNode`、`TreeLastNode`、`TreeGap`、`TreeNodeIdentifier` 定义稳定的文本树字符协议。
2. `Indent4Child` 根据当前节点是否为同级末项，生成传给下一层孩子的缩进状态；结束的祖先分支不再向下绘制竖线。
3. `PrettyIdentifier` 将一段缩进状态变成当前节点的可见分支，并拼接节点标识符。
4. 所有扫描和改写都基于 Rust `char`，对应 Go 的 `[]rune`，避免把 UTF-8 盒线字符拆成字节。

本文件只负责前缀变换，不负责遍历、孩子排序、节点命名、列对齐或输出 I/O。这些策略由调用方掌握，例如 `decodeBinaryOperator` 决定 Build/Probe 的显示顺序，`FlamegraphNode::sorted_children` 决定火焰图孩子顺序。

## 主要符号

- `pub const TreeBody: char = '│'`：活动中的祖先分支；后续同级节点仍需沿该列连接。
- `pub const TreeMiddleNode: char = '├'`：当前节点不是同级末项时的分支头。
- `pub const TreeLastNode: char = '└'`：当前节点是同级末项时的分支头。
- `pub const TreeGap: char = ' '`：已结束分支或树线间隔使用的空格。
- `pub const TreeNodeIdentifier: char = '─'`：紧邻节点 id 之前的横向连接符。
- `pub fn Indent4Child(indent: &str, isLastChild: bool) -> String`：复制输入缩进并返回下一层状态。非末孩子直接追加 `│ `；末孩子先把从右向左找到的第一个 `│` 改为空格，再追加 `│ `。
- `pub fn PrettyIdentifier(id: &str, indent: &str, isLastChild: bool) -> String`：空缩进时原样复制 `id`；否则把最右侧 `│` 改成 `├` 或 `└`，无条件把缩进最后一个字符改成 `─`，再拼接 `id`。

文件级 `#![allow(non_snake_case, non_upper_case_globals)]` 保留 Go 导出名风格，令 Rust API 与迁移源中的 `Indent4Child`、`PrettyIdentifier` 名称一致。文件没有类型、trait、`impl`、宏或条件编译项。

## 执行流程

典型递归调用遵循以下顺序，证据见两个生产调用方：

1. 遍历者携带代表当前层的 `indent` 和当前节点的 `isLastChild`。
2. 调用 `PrettyIdentifier(id, indent, isLastChild)` 渲染当前节点。函数从右向左寻找最近的活动树干：末孩子把它变为 `└`，其他孩子变为 `├`；随后把缩进末字符变为 `─` 并追加 id。
3. 若存在孩子，调用 `Indent4Child(indent, isLastChild)` 生成所有直接孩子共享的基础缩进。若当前节点是末孩子，最近的祖先活动树干会先被关闭；之后总会为新一层追加 `│ `。
4. 遍历者逐个递归孩子，并自行计算每个孩子是否为末项。

`pkg/util/plancodec/binary_plan_decode.rs::decodeBinaryOperator` 在第 223 行格式化当前算子 id，在第 279 行生成孩子缩进，再以 `index + 1 == children.len()` 传递末项状态。`pkg/util/profile/flamegraph.rs::FlamegraphCollector::collect_child` 在第 202 行和第 214 行执行同样的两阶段操作；根节点本身固定显示为 `root`，其孩子缩进由 `Indent4Child("", false)` 初始化。

## 数据与状态

缩进字符串本身就是全部状态：每个 `char` 位置代表显示列，`│` 表示该祖先层仍有待显示的同级节点，空格表示该层已结束，末字符通常是节点连接前的间隔。函数不会修改调用方的 `&str`，而是通过 `indent.chars().collect::<Vec<char>>()` 建立局部可变副本，最后收集为新的 `String`。

最重要的不变量是按 Unicode 标量值而不是字节索引操作。盒线字符在 UTF-8 中占多个字节，但在 `Vec<char>` 中各占一个元素；因此“最右侧活动树干”和“末字符”都与 Go `[]rune` 语义对齐。节点 id 仅在最后通过 `push_str` 追加，可为空、可包含中文或其他 Unicode，函数不会转义、截断或测量显示宽度。

## 依赖与调用关系

下游依赖仅为 Rust 标准库的 `str::chars`、`Vec<char>`、迭代收集和 `String` 拼接；`pkg/util/texttree/Cargo.toml` 没有 `[dependencies]`。crate 根 `pkg/util/texttree/lib.rs` 是公开边界并挂接三个独立测试模块，不把测试代码内嵌到生产文件。

上游直接调用边经 RustCodeGraph 文件关系与源码位置核验如下：

- `pkg/util/plancodec/binary_plan_decode.rs::decodeBinaryOperator` → `texttree::PrettyIdentifier`、`texttree::Indent4Child`。
- `pkg/util/profile/flamegraph.rs::FlamegraphCollector::collect_child` → 两个函数；`FlamegraphCollector::collect` → `Indent4Child`。

RustCodeGraph 的精确 `callers` 查询未返回函数级边，但 `node --file pkg/util/texttree/texttree.rs` 报告该文件被 `pkg/util/plancodec/binary_plan_decode.rs`、`pkg/util/profile/flamegraph.rs`、`pkg/util/texttree/lib.rs` 和测试装配使用；随后用上述索引文件源码及限定 Rust 搜索确认了具体调用点。因此这里不声称存在索引未证明的其他生产调用者。

## 错误处理与边界

两个函数均为无 `Result` 的纯变换，不执行 I/O，也没有显式错误分支。`PrettyIdentifier` 先处理空缩进，避免访问空向量末项；空缩进直接返回 id，且此时 `isLastChild` 不影响结果。

应特别保留以下与 Go 一致但对调用者有约束的边界：

- `Indent4Child` 的末孩子分支即使找不到 `TreeBody`，仍会原样保留已有字符并追加 `│ `；测试覆盖纯空格输入。
- `PrettyIdentifier` 的非空缩进即使找不到 `TreeBody`，仍会把最后一个 `char` 改成 `TreeNodeIdentifier`。`migration_aster_unit_test.rs::nearest_branch_and_unicode_text_are_handled_as_runes` 用 `"abc"` 验证结果为 `"ab─node"`。
- 函数不检查缩进是否以 `TreeGap` 结尾；任何非空输入的末字符都会被覆盖。安全扩展时不能擅自把这一行为改为拒绝或修复畸形输入，否则会偏离 Go 兼容契约。
- “字符数”不等于终端显示宽度；本模块保证 rune/`char` 级结构正确，但不处理全角、组合字符或 ANSI 转义序列的可视宽度。

## 并发与资源生命周期

本文件没有全局可变状态、锁、原子变量、线程、异步任务、通道、文件句柄或事务。五个常量是不可变值；函数只借用输入并分配局部 `Vec<char>` 与返回 `String`，所有权随返回值交给调用方，临时缓冲在函数返回时释放。因此可被多个线程并发调用，彼此没有共享状态或清理顺序要求。

性能成本与缩进字符数和 id 字节数线性相关：两个函数都完整复制缩进，`PrettyIdentifier` 还追加 id；反向搜索在最坏情况下扫描全部缩进。递归调用方通常对每个节点调用一次或两次，因此整棵树的前缀构造成本取决于所有节点深度之和。修改时应避免额外的重复扫描或按字节误切 Unicode。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/texttree/texttree.go`。五个 Rust `char` 常量逐一对应五个 Go rune 常量，两个 Rust 函数也逐分支保留 Go 算法：Go 的 `[]rune(indent)` 对应 Rust 的 `indent.chars().collect::<Vec<char>>()`，Go 的反向循环、首次命中后 `break`、末位覆盖以及字符串拼接均有等价实现。

可见差异仅属于语言接口：Go 接收并返回 `string`，Rust 输入为借用的 `&str`、返回拥有所有权的 `String`；Rust 通过 crate 根显式再导出 API，并用 lint allowance 保留 Go 命名。当前没有算法简化。`pkg/util/texttree/texttree_test.go` 与 `pkg/util/texttree/texttree_test.rs` 共享空缩进、中间/末孩子、空格和制表符案例；Rust 的 `migration_aster_unit_test.rs` 额外固定常量值、中文 id、多个活动树干选择以及无活动树干时覆盖末字符的行为。

Go 的 `pkg/util/texttree/main_test.go::TestMain` 还负责通用测试初始化与 goroutine 泄漏检查；Rust 的 `main_test.rs` 明确只保存相应顺序契约，不把它伪装成已执行的等价运行时检查。这一差异属于包级测试基础设施，不改变本文件函数语义。

## 扩展指南

- 若新增树线字符或改变前缀协议，应首先修改本文件常量/函数，同时更新 `pkg/util/texttree/lib.rs` 的公开再导出（若是新 API）、`texttree_test.rs` 和 `migration_aster_unit_test.rs`；若要求 Go/Rust 对齐，还需同步核对 `texttree.go` 与 `texttree_test.go`。
- 若只调整计划树或火焰图的遍历、排序、节点命名，应在各自调用方实现，不应把领域策略塞入此通用工具。
- 若要接受新的缩进表示，必须保留“最近的 `TreeBody`”和“非空输入末字符必被连接符覆盖”的既有兼容语义，或者明确评估两个生产调用方及 Go 输出兼容性。
- 回归测试应继续放在同目录独立测试文件，不能放入 `texttree.rs`。至少覆盖空缩进、有/无活动树干、多个活动树干、末/非末孩子、空 id、Unicode id、制表符/空格以及精确 UTF-8 输出。
- 性能优化可考虑减少中间分配，但不得退化为字节索引；任何原地/缓冲区 API 都应保留现有易用入口，避免迫使计划解码和火焰图调用方共享可变状态。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的 `texttree.rs`、`lib.rs` 和三个 Rust 测试文件均在索引中。
- RustCodeGraph `node --file pkg/util/texttree/texttree.rs`：读取完整 126 行实现并取得文件使用关系；`query PrettyIdentifier` 同时定位 Go/Rust 定义。
- RustCodeGraph `node`：读取 `pkg/util/plancodec/binary_plan_decode.rs::decodeBinaryOperator` 与 `pkg/util/profile/flamegraph.rs::FlamegraphCollector` 的实际调用上下文，确认当前节点格式化、孩子缩进生成和末孩子判定顺序。
- Cargo/入口：`pkg/util/texttree/Cargo.toml`、`pkg/util/texttree/lib.rs`、根 `Cargo.toml`，以及 `pkg/util/plancodec/Cargo.toml`、`pkg/util/profile/Cargo.toml` 的直接依赖声明。
- Go 对照：`pkg/util/texttree/texttree.go`、`pkg/util/texttree/texttree_test.go`、`pkg/util/texttree/main_test.go`。
- Rust 测试：`pkg/util/texttree/texttree_test.rs`、`pkg/util/texttree/migration_aster_unit_test.rs`、`pkg/util/texttree/main_test.rs`。
- 限定源码搜索确认生产调用点仅出现在 `binary_plan_decode.rs` 与 `flamegraph.rs`。本任务是纯文档分析，按计划不运行 Cargo；最终仅执行任务指定的 11 章节结构校验并人工复核上述事实链。
