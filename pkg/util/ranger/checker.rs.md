# `pkg/util/ranger/checker.rs`

## 文件定位

`checker.rs` 属于 `astersql-util-ranger` crate。crate 根在 `pkg/util/ranger/lib.rs`，通过 `#[path = "checker.rs"] mod checker_impl;` 私有装配本文件；它没有被 crate 根重新导出，因此不是面向其他 crate 的公开 API。`pkg/util/ranger/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/util/ranger`，直接对照实现是 `pkg/util/ranger/checker.go`。

本文件位于“表达式条件”到“索引/列扫描区间”的边界：它不生成端点或 `Range`，只判断一个谓词是否适合作为 access condition，以及即使参与建 range 后是否还必须作为 residual filter 保留。直接使用者是同 crate 的 `pkg/util/ranger/detacher.rs`；后者再被规划器的基数估算、选择率估算、逻辑数据源和索引连接等路径使用。RustCodeGraph 对 `detacher.rs` 给出的上游文件包括 `pkg/planner/cardinality/cross_estimation.rs`、`pkg/planner/cardinality/selectivity.rs`、`pkg/planner/core/operator/logicalop/logical_datasource.rs` 和 `pkg/planner/core/operator/physicalop/index_join_probe.rs`。

## 核心职责

- 围绕一个目标索引列识别可用于构造 range 的表达式，入口是 `conditionChecker::check`。
- 用返回值 `(isAccessCond, shouldReserve)` 区分“能缩小扫描范围”和“range 是否足以表达原谓词”。`shouldReserve = true` 时，调用者必须保留原条件做精确过滤。
- 为比较、`IS NULL`、布尔真值判断、`IN`、`LIKE`、逻辑与/或、部分 `NOT` 和参数占位符实现准入规则；不认识或不安全的表达式统一降级为 filter。
- 对字符串排序规则、前缀索引、PAD SPACE、二进制比较和 enum `LIKE` 等不能只靠 range 保证精确语义的场景采取保守策略。

它刻意不负责端点构造、类型转换、range 合并或内存配额处理；这些分别由 `points.rs`、`ranger.rs` 和 `detacher.rs` 的后续阶段完成。

## 主要符号

- `conditionChecker<'a>`：crate 私有状态对象。`ctx: &'a dyn expression::EvalContext` 提供类型和表达式比较上下文；`checkerCol: Option<expression::Column>` 是本轮要匹配的索引列；`length: isize` 是索引前缀长度；`optPrefixIndexSingleScan: bool` 控制前缀索引上 `IS NULL` 是否仍保留 filter。
- `isFullLengthColumn(&self) -> bool`：当 `length == types::UnspecifiedLength`，或长度等于 `checkerCol.GetType(ctx).GetFlen()` 时返回真。若 `checkerCol` 缺失，Rust 实现保守返回 `false`。
- `check(&self, condition) -> (bool, bool)`：公开到当前 crate 的总入口。它按 `ScalarFunction`、`Column`、`Constant` 分派；未知表达式返回 `(false, true)`。
- `checkScalarFunction(&self, scalar)`：按 `FuncName.L` 处理受支持的标量函数，并组合递归结果。
- `checkLikeFunc(&self, scalar)`：验证 LIKE 的列、常量 pattern、collation、escape 和首个通配符，决定能否利用确定前缀及是否保留过滤。
- `matchColumn(&self, expr) -> bool`：通过 `Column::EqualByExprAndID` 在求值上下文中匹配目标列/虚拟表达式列，而不是只比较表面字段。
- `checkColumn(&self, expr) -> (bool, bool)`：匹配目标列后允许建 range；前缀列需要保留 filter，否则拒绝并保留 filter。

文件只有上述一个 struct 和六个方法，没有模块级常量、trait、enum、条件编译项或可跨 crate 导出的符号。`#![allow(...)]` 仅用于容纳从 Go 迁移而来的命名和当前未使用代码。

## 执行流程

1. `detacher.rs` 为当前索引列构造 `conditionChecker`，填入列、索引长度、`OptPrefixIndexSingleScan` 和表达式求值上下文，再对候选条件调用 `check`。
2. `check` 遇到标量函数时进入 `checkScalarFunction`；裸字符串列被拒绝，其他裸列交给 `checkColumn`；常量返回 `(true, false)`；未知表达式返回 `(false, true)`。
3. `LogicAnd`/`LogicOr` 递归检查两个参数，只有两侧都能作为 access 条件时整体可用，保留标志取两侧逻辑或。这里仅判断准入，CNF/DNF 的拆分和重组由 `detachColumnCNFConditions`、`detachColumnDNFConditions` 完成。
4. 比较运算 `EQ/NE/GE/GT/LE/LT/NullEQ` 要求一侧是常量、另一侧匹配目标列。字符串还要检查函数与列的 collation 是否兼容；不兼容通常拒绝，但二进制 collation 下的 `EQ/NullEQ` 可建立近似 range，并强制保留 filter。前缀索引上的普通比较保留 filter；`NE` 只有完整长度列才可作为 access 条件。
5. `IsNull` 匹配目标列即可建 range。通常前缀列仍保留 filter；启用 `optPrefixIndexSingleScan` 时，由于任意长度前缀仍能判断 NULL，可不保留。真值/假值判断拒绝裸字符串列，其余交给 `checkColumn`。
6. `UnaryNot` 要求参数本身是标量函数；`NOT LIKE` 和 `NOT NullEQ` 明确拒绝，其余递归检查已归一化的内部函数。`In` 要求第一个参数匹配目标列且其余参数全是常量；二进制 collation 的不兼容情况允许近似 range，但保留 filter。
7. `Like` 进入 `checkLikeFunc`：先验证 collation、列匹配、非 NULL 常量 pattern；再以 escape 字节跳过被转义字符。pattern 以 `%` 或 `_` 开头时没有可用前缀而拒绝；中途 `%`、任意 `_`、前缀索引或 PAD SPACE collation 会要求保留 filter；enum 遇到 `%`/`_` 暂不支持 range。空 pattern 和没有通配符的 pattern 可以建 range，但仍继承前缀长度/PAD SPACE 的保留要求。
8. `GetParam` 当前按 Go 行为直接返回 `(true, false)`，源码保留 `TODO`。
9. 调用方依据结果分流：不可用条件进入 `RemainedConds`；可用条件进入 `AccessConds`；`shouldReserve` 为真时同时进入两者。后续 range 构建可能因配额或类型处理再把条件退回残余集合。

## 数据与状态

`conditionChecker` 只保存一次检查所需的借用上下文和少量配置，不缓存检查结果，也不修改表达式、列或上下文。`checkerCol` 使用拥有所有权的 `Column` 克隆，`ctx` 是只读 trait object 引用；所有方法都接收 `&self`。

核心不变量如下：

- `checkerCol` 应与 `length` 描述同一索引列。正常构造点在 `detacher.rs`，从同一 `cols[index]`/`lengths[index]` 位置取值。
- `(false, true)` 是保守失败值：不能用于 range，但原语义仍由 filter 执行。
- `(true, true)` 表示 range 只是候选缩小手段，不能替代原谓词；前缀索引、PAD SPACE 和部分二进制 collation 路径依赖这一点避免漏判。
- `(true, false)` 只应在 range 能表达检查器所负责的条件语义时返回。后续构造阶段仍可能因为其他原因保留或回退条件。
- `isFullLengthColumn` 将 `types::UnspecifiedLength` 视为完整列；显式长度只有等于字段 `flen` 才视为完整。

本文件不拥有 `Range`、端点、事务状态、锁、任务或通道。

## 依赖与调用关系

直接依赖来自 `crate::{ast, collate, mysql, types}` 和依赖 crate `expression`：

- `expression::{Expression, ScalarFunction, Column, EvalContext}` 提供运行时表达式分类、参数、类型信息和列等价判断。
- `ast` 提供函数名常量，如 `EQ`、`LogicOr`、`In`、`Like`。
- `collate` 提供 `CompatibleCollate`、`IsBinCollation`、`IsPadSpaceCollation`，决定字符串 range 是否可用及是否精确。
- `mysql::TypeEnum` 用于禁止尚未支持的 enum 通配符 LIKE range。
- `types::{ETString, UnspecifiedLength}` 用于类型和前缀长度判断。

直接上游均在 `pkg/util/ranger/detacher.rs`：

- `detachColumnCNFConditions` 和 `detachColumnDNFConditions` 调用 `check`，分别收集 access/filter，或在任一 DNF 分支不可用时放弃整个 DNF range。
- 索引 range 主流程在处理完等值/IN 前缀后，为下一个索引列构造检查器；`considerDNF` 分支调用上述拆分函数，非 DNF 分支逐条件调用 `check`。
- `ExtractAccessConditionsForColumn` 风格的筛选路径以及 DNF/列 range 辅助路径也构造检查器，仅消费 `isAccessCond` 或同时消费两个返回值。

crate 根 `lib.rs` 只私有装配本模块并公开 `detacher_impl` 的外层 API，例如 `DetachCondAndBuildRangeForIndex`。因此外部规划器不会直接构造 `conditionChecker`，而是经 detacher 的公开入口间接使用它。

## 错误处理与边界

本文件的 API 不返回 `Result`。不支持、类型不匹配、列不匹配、非恒定 `IN`/`LIKE` 参数和不兼容 collation 都通过 `(false, true)` 安全降级。`LIKE` pattern 的 `Datum::ToString()` 失败也被转成该保守结果，不向上传播错误。

需要维护者特别注意的边界：

- 代码按受支持 SQL 内建函数的固定 arity 直接索引 `args[0..2]`；它依赖表达式构造/规范化层保证参数个数，畸形内部表达式可能 panic。
- Rust 的 `LIKE` escape 提取使用 `as_constant().map(...).unwrap_or_default()`；与 Go 直接断言常量相比更宽容，非恒定 escape 会按 `0` 处理。这是可观察的迁移差异，不应在无测试情况下随意改变。
- `isFullLengthColumn` 在 `checkerCol == None` 时返回 `false`，而 Go 若走到对应解引用会失败；正常构造路径应始终提供列，这一分支是 Rust 的保守防护，不代表支持无目标列检查。
- pattern 扫描按 UTF-8 字节进行，通配符和 escape 本身是 ASCII 字节；范围边界的实际字符串编码与增量计算不在本文件完成。
- 以通配符开头的 LIKE、enum 通配符 LIKE、`NOT LIKE`、非恒定列表/模式均明确不作为 access 条件；不能把“退回 filter”误写成不支持 SQL 语义。

## 并发与资源生命周期

`conditionChecker` 没有内部可变状态，所有方法只读访问 `self`，单次调用只分配少量局部变量。它不启动线程或异步任务，不持有锁、通道、文件、网络连接、事务或需要显式释放的资源。

生命周期参数 `'a` 只约束 `ctx` 不得比检查器先失效；`checkerCol` 由检查器拥有，传入的表达式均只在调用期间借用。对象通常是 `detacher.rs` 中的栈上局部值，在完成一个索引列的条件拆分后即销毁。能否跨线程共享取决于 `expression::EvalContext` trait object 及 `Column` 的线程安全约束，本文件没有声明或依赖跨线程共享。

## 与 Go 版本的对应关系

`pkg/util/ranger/checker.rs` 的结构和分支顺序与 `pkg/util/ranger/checker.go` 基本一一对应：同名 `conditionChecker` 字段、六个方法，以及比较、逻辑、NULL、真值、NOT、IN、LIKE、参数占位的返回策略均保留。`Cargo.toml` 也明确声明 Go package 为 `pkg/util/ranger`。

已核对的重要一致性包括：

- `isAccessCond`/`shouldReserve` 的语义和 CNF/DNF 调用方消费方式一致。
- 前缀索引上的 `NE` 拒绝、其他比较保留 filter，`OptPrefixIndexSingleScan` 对 `IS NULL` 的特殊处理一致。
- 二进制 collation 下 `EQ/NullEQ/IN` 可建近似 range 并保留 filter；普通不兼容 collation 拒绝。
- LIKE 的前导通配符拒绝、escape 跳过、PAD SPACE 保留 filter、enum 通配符拒绝和中途通配符保留策略一致。

显式差异是 Rust 用 `Option<Column>` 表达 Go 的可空指针，并在缺列时保守返回；Rust 对非恒定 LIKE escape 使用默认字节 0，而 Go 假定参数是常量并直接类型断言。另有命名风格保留 Go 形式，因此文件级允许 `non_snake_case` 和 `non_camel_case_types`。

测试对应关系：Rust 独立测试位于 `pkg/util/ranger/ranger_test.rs`，其中 `test_prefix_index_range_scan` 验证前缀值裁剪后条件仍在 `RemainedConds`，`test_min_access_conds_for_dnf_cond` 验证 DNF 路径，`test_bin_collation_range_for_index` 验证二进制字符串点范围。Go 的 `pkg/util/ranger/ranger_test.go` 提供更完整的行为基线：`TestPrefixIndexRangeScan`、`TestIndexRange`、`TestPrefixIndexRange`、`TestBinCollationRangeForIndex` 覆盖 LIKE、IN、逻辑组合、collation、前缀索引、NULL 与 should-reserve 分支。当前没有同名 `checker_test.rs`，也没有只针对私有检查器方法的 Rust 单元测试。

## 扩展指南

- 新增可下推函数时，优先在 `checkScalarFunction` 增加分支，并同时确认 `points.rs`/`ranger.rs` 真能为它构造正确端点；仅让检查器返回 true 而没有下游支持会造成错误接线。
- 修改字符串条件时必须分别验证：完整索引与前缀索引、兼容与不兼容 collation、binary 与 PAD SPACE、NULL、enum、常量在左右两侧。近似 range 必须返回 `shouldReserve = true`。
- 修改 LIKE 时同步检查 pattern 常量性、escape、首字符通配符、中途 `%`、`_`、尾随空格和多字节字符串；避免把 SQL 匹配实现塞入检查器，它只负责准入。
- 修改逻辑组合或 NOT 时同时审查 `detacher.rs` 的 CNF/DNF 拆分及表达式规范化假设，尤其是“任一 OR 分支没有 access 条件时不能抽取部分 DNF”的不变量。
- 新增 Rust 回归测试应放在独立的 `pkg/util/ranger/ranger_test.rs`（或新建独立 `*_test.rs` 并在 `lib.rs` 的 `#[cfg(test)]` 下装配），不要内嵌到 `checker.rs`。还应对照并在需要时同步 `pkg/util/ranger/ranger_test.go` 的测试意图。
- 性能风险主要来自递归检查深层逻辑树和重复的类型/collation 查询；正确性风险则是错误返回 `(true, false)` 导致残余过滤被删除。评审时应优先证明“不会漏行或误收行”，再考虑减少 filter。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录列出 `checker.rs` 的 8 个符号。使用 `node --file` 阅读了 `pkg/util/ranger/checker.rs`、`pkg/util/ranger/lib.rs`、`pkg/util/ranger/detacher.rs`、`pkg/util/ranger/ranger_test.rs`、`pkg/util/ranger/checker.go` 和 `pkg/util/ranger/ranger_test.go`。
- RustCodeGraph 精确查询确认 Rust/Go 两侧都有 `conditionChecker`、`checkScalarFunction`、`checkLikeFunc`、`matchColumn` 等对应符号。图的 `callers`/`callees` 命令对这些私有/重名方法没有返回可用结果，且目标文件的文件级 “used by” 显示无关文件，因此调用边改用精确文本搜索补证，没有把该误配结果当作事实。
- `rg` 在 `pkg/util/ranger/detacher.rs` 找到 `conditionChecker` 的导入、多个构造点，以及 `checker.check(...)` 在 CNF、DNF、逐列和简化筛选路径中的调用；RustCodeGraph 对 `detacher.rs` 的文件级使用关系提供了规划器上游证据。
- crate 边界由 `pkg/util/ranger/Cargo.toml` 与 `pkg/util/ranger/lib.rs` 核对：crate 名为 `astersql-util-ranger`，依赖 `astersql-expression`，本模块私有、detacher/ranger 外层接口公开。
- Go 对照与边界测试由 `pkg/util/ranger/checker.go`、`pkg/util/ranger/ranger_test.go` 核对；Rust 独立测试由 `pkg/util/ranger/ranger_test.rs` 核对。未运行 Cargo，符合本纯文档任务要求。
