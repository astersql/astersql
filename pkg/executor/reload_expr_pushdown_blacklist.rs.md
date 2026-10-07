# `pkg/executor/reload_expr_pushdown_blacklist.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根在 [`pkg/executor/lib.rs`](lib.rs) 中以 `pub mod reload_expr_pushdown_blacklist` 公开该模块，而 [`pkg/executor/Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确认了这一归属。它承载 `ADMIN RELOAD EXPR_PUSHDOWN_BLACKLIST` 对应的 Rust 侧核心语义：读取 `mysql.expr_pushdown_blacklist`，把函数名和存储类型转换为进程内黑名单表示，并仅在内容变化时替换状态。

本文件同时定义了一个可注入的运行时边界和一个泛型执行器，但不拥有 SQL 解析、计划生成或实际全局状态实现。相邻的 [`pkg/executor/builder.rs`](builder.rs) 能识别 `Plan::ReloadExprPushdownBlacklist`，再以 `ExecutorKind::ReloadExprPushdownBlacklist` 委托 `ExecutorBuilderDependencies::build_executor`；当前直接证据没有显示该 builder 会构造本文件的 `ReloadExprPushdownBlacklistExec`。因此，本文件应视为已实现并有独立单元测试的加载逻辑，而不能仅凭模块存在断言其已接入完整 Rust SQL 执行主链。

## 核心职责

- `LOAD_EXPR_PUSHDOWN_BLACKLIST_SQL` 固定受限 SQL 文本，并用 `HIGH_PRIORITY` 读取系统表的 `name`、`store_type` 两列。
- `LoadExprPushdownBlacklist` 将查询结果归一化为 `HashMap<String, u32>`：函数名转小写并映射别名，存储名转小写后以逗号切分，每个可识别存储转换成位掩码并按位或聚合。
- `isSameExprPushDownBlackList` 按 map 长度及所有键值对判断新旧黑名单是否完全一致；相同则避免全局替换和重载时间更新。
- `ExprPushdownBlacklistRuntime` 抽象受限 SQL、存储名到掩码的映射、当前快照、时钟和原子替换，使核心算法不绑定具体会话或全局变量。
- `FUNC_NAME_2_ALIAS_SYMBOLS` 与 `func_name_to_alias` 保存 Go AST 名称对照及实际 SQL 规范名转换，尤其处理 `<< → leftshift`、`<> → ne`、`xor_logic → xor` 等非同名情况。

## 主要符号

- `pub const LOAD_EXPR_PUSHDOWN_BLACKLIST_SQL: &str`：唯一查询文本，值为 `select HIGH_PRIORITY name, store_type from mysql.expr_pushdown_blacklist`。
- `pub trait ExprPushdownBlacklistRuntime`：通过关联类型 `Context`、`Error` 和五个方法定义环境契约。`query_blacklist` 是唯一可失败操作；`store_mask` 决定支持哪些存储；`current_blacklist` 提供比较快照；`unix_nanos` 和 `replace_blacklist` 只在内容变化时调用。
- `pub struct ReloadExprPushdownBlacklistExec<R>`：只持有 `pub runtime: R`。其 `Next<T>(&mut self, &mut R::Context, &mut T)` 忽略输出请求对象，并同步委托加载函数。
- `pub fn LoadExprPushdownBlacklist<R>(...) -> Result<(), R::Error>`：本文件主入口，完成查询、规范化、聚合、差异判断及替换。
- `pub fn isSameExprPushDownBlackList(...) -> bool`：精确 map 相等判断；不依赖迭代顺序。
- `pub const FUNC_NAME_2_ALIAS_SYMBOLS: &[(&str, &str)]`：列出原始函数名与对应 Go `ast.*` 常量名，第二列用于跨语言审计，不直接参与返回值计算。
- `pub fn func_name_to_alias(name: &str) -> Option<&str>`：先用对照表确认名称受支持，再对运算符等特殊名称返回规范拼写，其余受支持名称原样返回；不在表中的名称返回 `None`。

文件级 `#![allow(non_snake_case)]` 保留 Go 移植符号的命名形状。文件没有条件编译项；测试模块由 `lib.rs` 中单独的 `#[cfg(test)] mod reload_expr_pushdown_blacklist_test` 引入。

## 执行流程

1. `ReloadExprPushdownBlacklistExec::Next` 接收运行时上下文和调用方请求对象；请求对象不承载结果，因为该管理语句只执行状态更新。
2. `Next` 调用 `LoadExprPushdownBlacklist(&mut self.runtime, context)`。
3. 加载函数以 `LOAD_EXPR_PUSHDOWN_BLACKLIST_SQL` 调用 `runtime.query_blacklist`。若查询失败，`?` 立即返回同一 `R::Error`，后续比较、时钟读取和替换均不发生。
4. 函数按查询行数预分配新 `HashMap`。每行的 `name` 使用 Unicode `to_lowercase`；若 `func_name_to_alias` 识别该名称则使用规范名，否则保留小写名。
5. `store_type` 同样转小写后按字面逗号切分。每个片段交给 `runtime.store_mask`；已知存储的掩码以 `|=` 合并，未知片段被忽略。重复函数行先读取已聚合值，因此可跨行合并多个存储。
6. 新 map 与 `runtime.current_blacklist()` 的快照交给 `isSameExprPushDownBlackList`。长度相等且每个键值相同即直接返回 `Ok(())`。
7. 仅在内容变化时读取 `runtime.unix_nanos()`，随后一次性调用 `runtime.replace_blacklist(new_blacklist, reload_time)`，最后返回 `Ok(())`。

## 数据与状态

黑名单的内存形态为 `HashMap<String, u32>`。键是小写、必要时别名化后的表达式名；值是由运行时定义的 store 位掩码。本文件不假设 TiDB、TiKV、TiFlash 的具体位号，只要求同一存储名稳定映射到同一 `u32` 位。独立测试中的 `TestRuntime` 用 `tidb = 1`、`tikv = 2`、`tiflash = 4` 模拟 Go 的位集合语义。

同一规范键可以来自重复行或多个原始拼写，已有值会参与下一次按位或。已知函数但无需特殊拼写转换时，`func_name_to_alias` 返回输入切片本身；未知函数返回 `None`，加载逻辑仍保留其小写名称，而不是丢弃该行。若一行的所有 store 都未知，该键仍进入 map，值为 `0`；这与 Go 实现的行为一致。

当前状态不保存在执行器结构中：`current_blacklist` 返回拥有所有权的快照，最终提交由 `replace_blacklist` 完成。时间戳与 map 作为同一次运行时调用的两个参数传入，但 trait 本身没有规定二者必须在一个不可分割的硬件原子操作内更新；具体一致性保证属于运行时实现。

## 依赖与调用关系

本文件唯一直接 Rust 库依赖是标准库 `std::collections::HashMap`，没有使用 `Cargo.toml` 中的第三方或同工作区 crate，也没有 feature 分支。

RustCodeGraph 对 `reload_expr_pushdown_blacklist.rs::LoadExprPushdownBlacklist` 的节点追踪给出的直接上游是本文件的 `Next` 以及独立测试中的加载用例；直接下游是 `LOAD_EXPR_PUSHDOWN_BLACKLIST_SQL`、`query_blacklist`、`store_mask`、`current_blacklist`、`unix_nanos`、`replace_blacklist`、`isSameExprPushDownBlackList` 和 `func_name_to_alias`。`func_name_to_alias` 只被加载函数调用并引用 `FUNC_NAME_2_ALIAS_SYMBOLS`。

模块外的计划侧路径为 `builder.rs` 的 `Plan::ReloadExprPushdownBlacklist → buildReloadExprPushdownBlacklist → build_leaf(ExecutorKind::ReloadExprPushdownBlacklist) → dependencies.build_executor(...)`。这是管理计划的构建分发证据，不是本文件泛型执行器的直接构造边。生产代码搜索未发现本文件 `ReloadExprPushdownBlacklistExec` 的构造点；现有直接构造只在 [`pkg/executor/reload_expr_pushdown_blacklist_test.rs`](reload_expr_pushdown_blacklist_test.rs) 中。

## 错误处理与边界

- 查询错误原样传播为 `Err(R::Error)`；测试 `query_error_is_returned_without_replacement` 证明错误后不会替换状态。
- `store_mask` 返回 `None` 不视为错误，未知 store 被静默忽略；空字符串、带空格的片段也不会被修剪，除非运行时特意识别它们。
- 函数名和 store 使用 Rust Unicode 小写转换；Go 使用 `strings.ToLower`。测试以 `ÄBS → äbs` 覆盖了非 ASCII 小写行为，但没有穷举所有 Unicode 差异。
- 别名表的成员资格与特殊映射是两层逻辑：新增 `FUNC_NAME_2_ALIAS_SYMBOLS` 项但未新增 `match` 分支时，会采用原名；新增特殊 Go AST 拼写时必须同步检查 `match`，否则可能得到错误规范键。
- `isSameExprPushDownBlackList` 同时检查长度和键值，因此不会把“缺键但碰巧其余值相同”视为相等，也不受 `HashMap` 遍历次序影响。
- `replace_blacklist` 无返回值，所以运行时替换失败无法通过此接口表达；若未来需要可失败提交，必须调整 trait 和加载函数返回路径。

## 并发与资源生命周期

函数本身同步执行，不创建线程、异步任务、锁、通道或事务。查询返回的行和新 map 都由本次调用拥有，函数退出时未提交的临时数据自动释放。`ReloadExprPushdownBlacklistExec::Next` 通过 `&mut self`、加载函数通过 `&mut R` 防止同一运行时实例在安全 Rust 中被同时可变调用。

跨请求共享状态的并发安全不由本文件实现。Go 对照使用 `atomic.Pointer` 和 `atomic.Int64` 保存全局 map 与时间戳；Rust trait 只暴露快照和替换边界，实际运行时必须自行保证读取、替换及可见性。无变化分支既不调用 `unix_nanos` 也不调用 `replace_blacklist`，避免产生虚假的重载时间和不必要的共享状态写入。

受限 SQL 所需的内部事务来源标记也由运行时负责。Go 的 `LoadExprPushdownBlacklist` 显式创建带 `kv.InternalTxnSysVar` 来源的 context；Rust 抽象仅把调用方 `Context` 传给 `query_blacklist`，本文件无法证明具体实现已设置同等标记。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/executor/reload_expr_pushdown_blacklist.go`](reload_expr_pushdown_blacklist.go)。两者保持以下主干语义：执行器 `Next` 触发一次加载；以相同的 `HIGH_PRIORITY` SQL 读取两列；名称和 store 转小写；别名归一化；逗号分隔 store 并以位或聚合；新旧 map 完全相同时跳过；变化时记录纳秒时间并替换黑名单。

实现边界存在明确差异：Go 执行器嵌入 `exec.BaseExecutor`，从会话取得 restricted SQL executor，并直接操作 `expression.DefaultExprPushDownBlacklist` 与 `ExprPushDownBlackListReloadTimeStamp`；Rust 将这些能力抽成 `ExprPushdownBlacklistRuntime`。Go builder 直接返回 `&ReloadExprPushdownBlacklistExec{base}`，Rust builder 当前只按 `ExecutorKind` 委托通用依赖，未显示与本文件泛型类型的绑定。

Go 的 `funcName2Alias` 直接存放 AST 常量的实际字符串；Rust 的 `FUNC_NAME_2_ALIAS_SYMBOLS` 第二列存放常量符号名用于审计，`func_name_to_alias` 另行编码实际特殊拼写，对多数普通函数返回输入。维护时必须同时核对 Go map 与 Rust 两处表示，尤其是运算符、同义写法和新增 AST 常量。

Go 侧计划缓存场景 [`pkg/executor/test/plancache/plan_cache_test.go`](test/plancache/plan_cache_test.go) 证明 `ADMIN reload expr_pushdown_blacklist` 会使相关已缓存计划失效；它验证完整 Go SQL 链路，不等同于 Rust 运行时接线测试。Rust 独立测试只验证本文件算法和委托行为。

## 扩展指南

- 增加或修改函数别名时，先在 `FUNC_NAME_2_ALIAS_SYMBOLS` 保持与 Go `funcName2Alias` 的一对一审计项；若实际 SQL 拼写不等于原始名称，再同步修改 `func_name_to_alias` 的 `match`。在独立测试中加入普通名称、特殊运算符和同义输入的断言。
- 支持新存储类型不应硬编码进加载循环；应在具体 `ExprPushdownBlacklistRuntime::store_mask` 实现中增加稳定且不冲突的位。测试应覆盖与已有 store 跨列、跨重复行的按位或结果，以及未知 store 的兼容行为。
- 接入生产 Rust 执行链时，应在 `ExecutorBuilderDependencies` 的实现中证明 `ExecutorKind::ReloadExprPushdownBlacklist` 构造了具备真实 restricted SQL、全局快照、时钟和替换能力的执行器，并补独立集成测试；不要把测试运行时搬入生产文件。
- 若要求 map 与时间戳具备整体原子性，应把语义封装在 `replace_blacklist` 的运行时实现中，或升级 trait 契约；不能依赖当前两个参数自然提供原子保证。
- 修改查询列、错误策略、空白处理或大小写规则时，应同时检查 Go 对照、系统表数据约束及计划缓存失效行为。主要兼容风险是规范键变化导致已有黑名单失效，性能风险是大表读取和重复 map 克隆，正确性风险是位号冲突或非原子发布。
- 测试继续放在独立的 `pkg/executor/reload_expr_pushdown_blacklist_test.rs`，遵守源文件与 Rust 单元测试分离要求；完整 SQL 行为则应在相应集成测试表面补充，而不是在本文件内添加 `#[cfg(test)]` 模块。

## 验证依据

- RustCodeGraph：`status` 显示索引可用；`query` 找到 `LoadExprPushdownBlacklist`、`ReloadExprPushdownBlacklistExec`、`isSameExprPushDownBlackList`、`func_name_to_alias`；对精确限定名执行 `node` 得到 `Next → LoadExprPushdownBlacklist`、加载函数到五个运行时方法/比较函数/别名函数的调用边，以及执行器仅被独立测试构造的上游证据。独立 `callers` 子命令未输出结果，故调用者结论采用同一节点的 `Called by` trail 并以代码搜索交叉核对。
- Rust 源码：[`pkg/executor/reload_expr_pushdown_blacklist.rs`](reload_expr_pushdown_blacklist.rs) 的运行时 trait、执行器、加载算法、比较函数、别名表和别名转换；[`pkg/executor/lib.rs`](lib.rs) 的生产模块与独立测试模块声明。
- crate 与接线：[`pkg/executor/Cargo.toml`](Cargo.toml) 的 crate 根和 feature/依赖声明；[`pkg/executor/builder.rs`](builder.rs) 的 `Plan`、`ExecutorKind`、`buildReloadExprPushdownBlacklist`、`build_leaf` 委托链。
- Rust 测试：[`pkg/executor/reload_expr_pushdown_blacklist_test.rs`](reload_expr_pushdown_blacklist_test.rs) 覆盖 Unicode 小写、特殊别名、跨行 store 聚合、未知 store、相同状态跳过、查询错误不替换、`Next` 委托和 map 精确相等。
- Go 对照：[`pkg/executor/reload_expr_pushdown_blacklist.go`](reload_expr_pushdown_blacklist.go) 核对 restricted SQL、内部事务来源、store 位、原子全局状态及别名表；[`pkg/executor/builder.go`](builder.go) 核对 Go 的直接构造；[`pkg/executor/test/plancache/plan_cache_test.go`](test/plancache/plan_cache_test.go) 核对完整 Go 链路对计划缓存的可见效果。
- 本任务为纯文档分析，按总计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核仅新增本说明文件、无运行时代码或 `plan.md` 改动。
