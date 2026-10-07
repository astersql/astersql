# `build/linter/staticcheck/util.rs`

## 文件定位

该文件是 Go 文件 [`build/linter/staticcheck/util.go`](./util.go) 的 Rust 语义移植，位于构建期 linter 的 staticcheck 适配目录。它不是 SQL 请求、存储或后台任务运行链的一部分；职责是把多个 Staticcheck analyzer 家族整理成“名称到 analyzer”的注册表，供同目录的 [`analyzer.rs`](./analyzer.rs) 按构建时注入的名称选择一个 analyzer。

当前仓库中没有 `build/linter` 下的 `Cargo.toml`，根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace members 也未列出该目录。RustCodeGraph 对 `build/linter/staticcheck/analyzer.rs` 报告 `used by 0 files`，文本检索也只找到 `analyzer.rs` 调用本文件。因此，这份 Rust 实现应视为已表达移植语义、并由源码契约测试检查的候选实现；没有证据表明它已被 Cargo crate 编译或接入实际 lint 可执行链。实际 Bazel lint 接线仍由 Go 源码和 [`BUILD.bazel`](./BUILD.bazel) 完成。

## 核心职责

- `Analyzers` 汇总 `quickfix`、`simple`、`staticcheck`、`stylecheck` 和 `unused` 五组 analyzer，并以底层 `analysis::Analyzer.Name` 为键建立索引。
- `FindAnalyzerByName` 返回注册表中共享的 `&'static analysis::Analyzer`，使调用方取得原 analyzer 身份，而不是复制或重新构造定义。
- 对未知名称立即 `panic!`，把构建 stamping、analyzer 清单或接线错误作为不可恢复的配置错误暴露出来。

本文件不负责运行 analyzer，也不负责应用仓库的跳过规则；后者位于同目录 `analyzer.rs::init`，由 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer` 完成。

## 主要符号

- `pub static Analyzers: once_cell::sync::Lazy<HashMap<String, &'static analysis::Analyzer>>`：公开、惰性初始化的只读注册表。第一次解引用时构建，之后在进程生命周期内复用同一张表。
- `pub fn FindAnalyzerByName(name: &str) -> &'static analysis::Analyzer`：公开查找入口。命中时解引用 `HashMap::get` 返回的双重引用并返回共享静态引用；未命中时 panic。
- 初始化闭包中的 `resMap`：仅在 `Analyzers` 首次初始化期间存在的局部可变映射，不对外暴露。

文件没有自定义类型、trait、`impl`、模块级普通常量或条件编译项。符号名称沿用 Go 的导出命名风格（`Analyzers`、`FindAnalyzerByName`），并非惯用 Rust snake_case。

## 执行流程

1. 上游首次访问 `Analyzers`，或调用 `FindAnalyzerByName` 间接访问它时，`once_cell::sync::Lazy` 执行初始化闭包。
2. 闭包按固定顺序遍历 `quickfix::Analyzers`、`simple::Analyzers`、`staticcheck::Analyzers`、`stylecheck::Analyzers`，最后把单例 `unused::Analyzer` 包装为单元素 slice。
3. 每个外层元素是一组 `lint` analyzer；内层循环取其底层 `a.Analyzer`，用 `a.Analyzer.Name.clone()` 作为拥有所有权的字符串键，将静态 analyzer 引用写入 `HashMap`。
4. 若不同家族出现同名 analyzer，后遍历者通过 `HashMap::insert` 覆盖先前值；这保留了 Go `map` 赋值的顺序语义。
5. `FindAnalyzerByName(name)` 调用 `Analyzers.get(name)`。命中则返回相同的静态 analyzer 引用；未命中则以 `not a valid staticcheck analyzer: <name>` panic。
6. 预期上游 `analyzer.rs::Analyzer` 在自身首次初始化时以 stamping 名称调用该函数，随后 `analyzer.rs::init` 把选中的 analyzer 交给跳过规则处理。但当前 Rust Cargo 接线未验证；这一调用链是源文件间的直接关系，不代表已进入实际构建产物。

## 数据与状态

核心状态是一张进程级 `HashMap<String, &'static analysis::Analyzer>`。键在建表时从 analyzer 名称克隆，值借用上游 analyzer 集合中的静态对象；因此本文件不拥有或销毁 analyzer，也不会为每次查询分配 analyzer。建表成本只发生一次，查询平均为哈希表常数时间；每个名称只在初始化时产生一个 `String` 键分配。

注册表初始化完成后，本文件没有可变共享状态。返回值带 `'static` 生命周期，调用者可以在整个进程期间持有它。文件不缓存查找失败，也没有按请求、会话或租户区分的状态。

## 依赖与调用关系

直接依赖如下：

- 标准库 `std::collections::HashMap` 提供名称索引。
- `once_cell::sync::Lazy` 提供线程安全的一次性初始化。
- `analysis::Analyzer` 是对外返回的底层 analyzer 类型。
- `quickfix`、`simple`、`staticcheck`、`stylecheck` 的 `Analyzers` 集合以及 `unused::Analyzer` 提供注册数据；每项通过其 `lint` 包装对象的 `Analyzer` 字段解包。

直接上游是 `build/linter/staticcheck/analyzer.rs::Analyzer` 的 Lazy 初始化闭包：`FindAnalyzerByName(name)`。仓库 Rust 文本检索未发现其他运行时代码调用。`analyzer_test.rs` 通过 `include_str!("util.rs")` 读取源码，验证家族顺序、插入表达式、返回共享引用和 panic 文本，但不执行真实 analyzer 查找。

Go/Bazel 主链与 Rust 状态必须区分：`build/linter/staticcheck/BUILD.bazel` 对 `def.bzl::ANALYZERS` 中每个名称生成一个 `go_library`，把 `analyzer.go` 与 `util.go` 编入目标，并用 `x_defs = {"name": analyzer}` 注入名称；`build/BUILD.bazel` 再通过 `staticcheck_analyzers(STATICCHECK_ANALYZERS)` 纳入这些目标。未发现对应的 Rust Cargo 依赖声明或模块入口。

## 错误处理与边界

唯一显式错误路径是名称不存在时 panic。其信息与 Go 的 `fmt.Sprintf("not a valid staticcheck analyzer: %s", name)` 对齐。函数不返回 `Option` 或 `Result`，因为未知名称被视为静态清单/stamping 不一致，而非正常用户输入；调用者无法在本层恢复。

空字符串、大小写不同、前后空白或拼写错误都不会被规范化，只有与注册表键完全相等的名称才能命中。代码也不主动检测跨家族重名：后写覆盖是刻意保留的 Go 行为。若上游 analyzer 的 `Name` 在建表期间不唯一，最终映射取决于文件中固定的家族遍历顺序。

当前测试是源码契约测试，不会证明外部 analyzer 集合在真实 Rust 类型系统下可链接，也不会枚举 `def.bzl` 中每个名称并执行查找。由于未见 Cargo 接线，编译可用性与真实运行行为在本任务中均未验证；本任务按要求不运行 Cargo。

## 并发与资源生命周期

`once_cell::sync::Lazy` 保证多线程首次访问时初始化闭包只成功执行一次，其他线程共享完成后的映射，不需要本文件显式加锁。初始化阶段的 `resMap` 只由执行闭包的线程持有；发布后仅通过共享引用读取，因此本文件没有后续写竞争。

analyzer 值本身来自静态集合，生命周期不短于注册表；注册表及其字符串键也持续到进程结束，没有显式清理、文件句柄、网络连接、任务、channel、事务或取消协议。若 Lazy 初始化期间发生 panic，恢复/再次访问的具体行为取决于 `once_cell`，本文件没有额外恢复逻辑。

## 与 Go 版本的对应关系

Rust `Analyzers` 对应 Go 的包级变量 `var Analyzers = func() map[string]*analysis.Analyzer { ... }()`。两者使用相同的五组来源和相同顺序，并都以底层 analyzer 名称为键；Rust 用 `std::slice::from_ref(&unused::Analyzer)` 表达 Go 的 `{unused.Analyzer}` 单元素列表，用 `Lazy` 替代 Go 包初始化时的立即执行函数。

Rust `FindAnalyzerByName(&str) -> &'static analysis::Analyzer` 对应 Go `FindAnalyzerByName(string) *analysis.Analyzer`。二者命中时返回共享 analyzer 身份，失败时使用相同文本快速失败。Rust 的所有权差异是键需要 `Name.clone()`，而值以静态引用保存；Go map 保存指针。

同目录 `analyzer.go` 在 `init` 中先调用查找，再配置跳过规则；Rust `analyzer.rs` 将查找推迟到自己的 `Lazy` 首次访问，并另设普通 `init()` 函数。Go 的 `x_defs` 注入已在 Bazel 规则中得到验证，Rust 注释虽声称 stamping 会替换 `name`，但仓库中未找到相应 Rust 构建接线，因此不能宣称两端启动时机或构建注入已经等价。

## 扩展指南

- 新增 analyzer 家族时，应在 `Analyzers` 初始化闭包中按 Go 版本确定的顺序加入其 slice，并同步 `util.go`；先判断重名时应由哪个家族获胜，避免无意改变覆盖顺序。
- 新增或移除具体 analyzer 名称通常应更新 `def.bzl::ANALYZERS` 及其生成流程，而不是在 `FindAnalyzerByName` 中硬编码分支。Go 实际构建还依赖 `BUILD.bazel` 的依赖列表和 `x_defs`。
- 若要把 Rust 版本真正接入构建，应在独立任务中建立明确的 Cargo crate/module 边界，并声明 `once_cell`、analysis/lint 与各 analyzer 家族依赖；不得仅凭当前源文件存在就假定可编译。
- 改变未知名称的策略（例如返回 `Option`/`Result`）会改变 stamping 错误的失败边界，必须同时修改 `analyzer.rs` 调用方、Go 对照语义和独立测试。
- 测试应继续放在独立的 `build/linter/staticcheck/analyzer_test.rs` 或新建同目录独立测试文件，不能嵌入生产 `util.rs`。至少覆盖家族顺序、同名覆盖策略、共享引用身份、有效/无效名称及 panic 文本；接入可编译 crate 后还应增加真实行为测试，而不只检查源码字符串。

兼容性风险主要是 analyzer 名称清单和 Go/Rust 遍历顺序漂移；正确性风险是未知名称直接 panic；性能风险集中在一次性建表的名称克隆与哈希容量，当前规模下没有每次查找重建。改变静态引用模型还可能影响 analyzer 的共享身份和线程安全假设。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`node --file build/linter/staticcheck/util.rs` 读取了完整 64 行并识别两个主要符号；同一命令显示文件级使用关系，`node` 对 `analyzer.rs` 显示其通过 `FindAnalyzerByName(name)` 选取 analyzer。精确 `callers/callees` 查询在本地超时且无输出，因此调用点另用定向文本检索核对。
- 生产源码：[`util.rs`](./util.rs) 的 `Analyzers`、初始化闭包和 `FindAnalyzerByName`；直接上游 [`analyzer.rs`](./analyzer.rs) 的 `name`、`Analyzer` 与 `init`。
- Go 对照：[`util.go`](./util.go) 的注册表与查找函数；[`analyzer.go`](./analyzer.go) 的 stamping 变量和包初始化流程。
- 构建证据：[`BUILD.bazel`](./BUILD.bazel) 的逐名称 `go_library`、`x_defs` 与 Go 依赖；[`def.bzl`](./def.bzl) 的 `ANALYZERS` 和 `staticcheck_analyzers`；[`build/BUILD.bazel`](../../BUILD.bazel) 的 lint 目标聚合；根 [`Cargo.toml`](../../../Cargo.toml) 未把 `build/linter` 列为 workspace member，目录内也没有独立 Cargo manifest。
- 独立测试：[`analyzer_test.rs`](./analyzer_test.rs) 的 `analyzer_families_and_unused_singleton_match_go_order`、`lookup_returns_shared_analyzer_and_preserves_panic_text`、`stamped_name_and_selected_analyzer_use_thread_safe_lazy_identity` 与许可证检查。它们是 `include_str!` 源码契约测试，不是已接线的运行时集成测试。
- 文档结构按任务指定命令验证；本任务为纯文档分析，未运行 Cargo、Go 或 Bazel 测试。
