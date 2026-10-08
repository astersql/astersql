# `pkg/util/table-filter/compat.rs`

## 文件定位

本文件是 `astersql-util-table-filter` crate 的 MySQL 复制规则兼容层。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod compat` 声明模块并通过 `pub use compat::*` 重导出其公开项；[`Cargo.toml`](Cargo.toml) 将该 crate 对应到 Go 包 `pkg/util/table-filter`，直接依赖 `regex` 与 `regex-syntax`。它不负责读取配置或执行数据复制，而是把旧式 `DoDBs`、`IgnoreDBs`、`DoTables`、`IgnoreTables` 数据转换成该 crate 的统一 `Filter` 接口。

当前 Rust 生产接线需要分两层理解：`pkg/util/filter/filter.rs` 将这里的 `Table` 和 `MySQLReplicationRules` 分别复用为 `Table`、`Rules` 类型别名，并在大小写不敏感构造路径调用 `Rules::ToLower`；`pkg/util/regexpr-router/regexpr_router.rs` 再借助这组类型组装路由规则。相对地，仓库搜索未发现 `ParseMySQLReplicationRules`、`NewSchemasFilter` 或 `NewTablesFilter` 在非测试 Rust 文件中的直接调用，它们目前是 crate 对外兼容 API，并由同目录独立测试固定行为。Go 生产代码则仍在 BR、Dumpling 和 BRIE 等路径直接使用对应构造器。

## 核心职责

- `Table` 提供旧配置需要的 schema/table 值对象，包括构造、字典序比较、显示和堆分配克隆。
- `MySQLReplicationRules` 保存旧式复制过滤四元组，并通过 `ToLower` 提供与 Go `strings.ToLower` 兼容的原地归一化。
- `NewSchemasFilter` 和 `NewTablesFilter` 把显式名称集合变成精确匹配的 `Filter`；其中 `?`、`*`、反斜线等字符在这两个入口中都只是普通字符。
- `ParseMySQLReplicationRules` 把旧规则中的字面量、glob 和 `~` 前缀正则编译成两组 `tableRule`，再用 `bothFilter` 取交集，从而要求 schema 侧与 table 侧同时通过。
- 私有 `matcherFromLegacyPattern` 集中实现旧模式语法到统一 matcher 的转换和错误传播。

## 主要符号

- `Table { Schema, Name }`：公开库表标识。`Table::new` 接受可转换为 `String` 的两个参数；`lessThan` 先比较 schema、再比较 table；`Clone` 返回 `Box<Table>`；`Display` 输出 `` `schema`.`table` ``，表名为空时只输出 `` `schema` ``。字段公开，但 Rust 类型没有 Go 字段上的 TOML/JSON/YAML 标签。
- `MySQLReplicationRules { DoTables, DoDBs, IgnoreTables, IgnoreDBs }`：公开规则容器，表规则用 `Vec<Box<Table>>` 保留 Go `[]*Table` 的指针形状；派生 `Default` 后四个列表均为空。
- `MySQLReplicationRules::ToLower(Option<&mut MySQLReplicationRules>)`：接受 `None` 时直接返回；否则原地小写化所有 schema/table。辅助函数 `goToLower` 对每个 Unicode 标量只取简单小写映射的第一个字符，以避免 Rust 完整映射扩张，测试用 U+0130 固定这一 Go 兼容语义。
- `schemasFilter` / `NewSchemasFilter(Vec<String>)`：私有集合实现与公开构造器。`MatchTable` 忽略 table 参数并委托 `MatchSchema`；空集合拒绝所有 schema。
- `tablesFilter` / `NewTablesFilter(Vec<Table>)`：按 `HashMap<schema, HashSet<table>>` 精确匹配。`MatchSchema` 只检查 schema 键是否存在；重复输入自然去重。
- `bothFilter`：持有两个 `Box<dyn Filter>`；`MatchTable`、`MatchSchema` 都短路执行逻辑与，`toLower` 分别转换两侧后重新组合。
- `matcherFromLegacyPattern(&str)`：空串返回 `FilterError("pattern cannot be empty")`；首字符为 `~` 时把余串直接交给 `newRegexpMatcher`；不含 `?`、`*`、`[` 时使用 `stringMatcher`；否则先 `regex::escape`，再恢复旧 glob 的 `*`、`?`、`[!...]` 和 `[...]` 含义，最后生成带 `(?s)^...$` 的整串正则。
- `ParseMySQLReplicationRules(Option<&MySQLReplicationRules>)`：兼容层主入口，成功返回 `Box<dyn Filter>`，模式非法时返回 `FilterError`。

## 执行流程

1. `ParseMySQLReplicationRules(None)` 直接调用 `All()`，得到 schema/table 都无条件通过的过滤器。
2. 有规则时先处理 schema 侧：`DoDBs` 非空便选它作为正向白名单，并完全忽略 `IgnoreDBs`；否则选 `IgnoreDBs` 作为负向黑名单，并在其后追加一条全匹配正向规则。
3. 每个 schema 模式由 `matcherFromLegacyPattern` 转成 matcher，table matcher 固定为 `trueMatcher`，再组成 `tableRule`。黑名单后的默认正向规则保证“未命中 ignore 即通过”；规则顺序配合 `tableFilter` 的“首个命中决定结果”语义。
4. table 侧采用相同优先级：`DoTables` 非空时忽略 `IgnoreTables` 并形成白名单；否则把 `IgnoreTables` 形成负向规则，再追加默认放行规则。每条表规则的 schema 与 table 模式分别编译。
5. 两组规则各自构造成 `tableFilter`，随后装入 `bothFilter { a, b }`。最终库表必须同时通过 schema 规则和 table 规则；例如 `DoDBs = [foo, bar]` 与 `DoTables = [*.a, *.b]` 的结果是两集合的交集。
6. 调用者如需大小写不敏感，应在解析后使用 `CaseInsensitive` 包装。该包装会调用 `bothFilter::toLower` 递归转换规则，并在匹配时小写化输入；解析函数本身不隐式改变大小写。

## 数据与状态

所有过滤状态都在构造阶段建立于内存中。`schemasFilter` 使用 `HashSet<String>`，平均常数时间完成 schema 查询；`tablesFilter` 先按 schema 查 `HashMap`，再在内部 `HashSet` 查 table。集合构造会去重，且 `toLower` 时原本仅大小写不同的键或表名会合并。

`ParseMySQLReplicationRules` 不修改传入的 `MySQLReplicationRules`，而是为 matcher 和规则分配新对象。返回的 `bothFilter` 拥有两个子过滤器；源规则离开作用域后仍可匹配。只有显式调用 `MySQLReplicationRules::ToLower` 才会原地修改四个列表。该文件没有全局可变状态、缓存、I/O 或持久化行为。

## 依赖与调用关系

下游依赖来自同 crate：`Filter`、`All` 和 `tableFilter` 位于 [`table_filter.rs`](table_filter.rs)，`tableRule`、`stringMatcher`、`trueMatcher`、`newRegexpMatcher` 与 `FilterError` 由匹配器/解析模块经 `lib.rs` 汇入。`matcherFromLegacyPattern` 的 glob 转换使用外部 `regex::escape`；编译与错误包装交给 `newRegexpMatcher`。

RustCodeGraph 对 `compat.rs` 的文件节点报告 28 个符号并显示它被 9 个文件使用；精确 `query` 能定位本文件与 Go 对照文件中的 `MySQLReplicationRules`、`NewSchemasFilter`、`NewTablesFilter`、`ParseMySQLReplicationRules`、`matcherFromLegacyPattern`。调用者命令对 `ParseMySQLReplicationRules` 未在可用时间内返回，因此又用精确仓库搜索补证：当前非测试 Rust 生产代码直接复用的是 `pkg/util/filter/filter.rs` 中的类型别名和 `ToLower`，而解析/集合构造器的直接 Rust 引用位于 [`compat_test.rs`](compat_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。

Cargo 层面，`pkg/util/filter` 以依赖别名 `tfilter` 引用本 crate；`dumpling/export`、`br/pkg/utils`、`pkg/executor`、`pkg/importsdk` 也声明了 crate 依赖，但不能仅凭 Cargo 依赖推断它们已调用本文件的兼容入口。

## 错误处理与边界

唯一可失败的公开路径是 `ParseMySQLReplicationRules`。空模式在进入正则编译前返回稳定消息 `pattern cannot be empty`；非法 `~` 正则或非法 glob 字符类由 `newRegexpMatcher` 返回 `FilterError`，并通过 `?` 原样向上游传播。构建 schema 侧任一 matcher 失败后不会继续 table 侧；一条表规则的 schema matcher 成功而 table matcher 失败时，整个函数仍返回错误且不暴露部分过滤器。

关键边界如下：空规则对象与 `None` 都匹配全部，但实现路径不同；`NewSchemasFilter([])` 和 `NewTablesFilter([])` 则匹配不到任何对象。`Do*` 只要非空就压过对应 `Ignore*`，不是二者叠加。字面入口的特殊字符不具模式含义，而旧规则解析入口会把 `*`、`?`、`[...]` 当 glob；`~` 只在模式首位触发正则。`Table::Display` 只加反引号而不转义字段内已有反引号，因此它是诊断显示，不应当作安全 SQL 标识符生成器。

## 并发与资源生命周期

`Filter` trait 要求 `Debug + Send + Sync`，因此本文件生成的过滤器可以在线程间转移或共享。这里的 `HashMap`、`HashSet` 与 matcher 在构造完成后只通过共享引用读取，不需要锁；`bothFilter` 的短路逻辑也没有可观察副作用。`CaseInsensitive` 的外层实现会在 `table_filter.rs` 中用 `Arc<dyn Filter>` 共享已小写化的过滤器。

资源生命周期完全由所有权管理：构造器取得输入集合所有权，解析器从借用的规则复制出 matcher 状态，`Box<dyn Filter>` 释放时递归释放两侧规则。没有线程、异步任务、通道、文件句柄、网络连接或事务需要显式关闭。`ToLower` 需要独占可变借用，因而不能与同一规则对象的并发读取同时发生。

## 与 Go 版本的对应关系

[`compat.go`](compat.go) 是逐符号对照基线：Rust 保留 `Table`、`MySQLReplicationRules`、两个集合过滤器、`bothFilter`、旧模式转换和解析优先级。Go 的 variadic 构造器在 Rust 中变为 `Vec` 参数；Go `*Table`/接口值在 Rust 中分别表现为 `Box<Table>`/`Box<dyn Filter>`；Go 可在 nil 接收者调用的 `ToLower` 被表达为接收 `Option<&mut ...>` 的关联函数。Rust 另外提供 `Table::new`，并显式实现 `Display` 代替 Go `Stringer`。

行为对照由 [`compat_test.go`](compat_test.go) 和 [`compat_test.rs`](compat_test.rs) 的同组用例覆盖：Do 优先于 Ignore、纯 ignore 默认放行、schema/table 两侧取交集、`~` 正则、glob 字符类及反斜线边界、nil/空规则与解析失败。Rust 测试还增加 U+0130 用例，验证小写不会采用会扩张字符数的 Rust 完整映射。

需要保留的差异风险是正则引擎：Go 使用 `regexp`，Rust 使用 `regex` crate；当前测试证明已列模式的结果一致，但新增高级正则语法时不能自动假设两个方言完全等价。Rust 数据结构也没有 Go 的序列化标签，因此配置反序列化应由上层明确适配，不能仅依赖字段同名。

## 扩展指南

新增旧模式语法时应集中修改 `matcherFromLegacyPattern`，同时在独立的 [`compat_test.rs`](compat_test.rs) 和 Go 对照测试中加入相同的接受、拒绝与失败用例；尤其要覆盖转义、锚点、换行、Unicode 和两个正则引擎的差异。不要把模式语法塞进 `NewSchemasFilter`/`NewTablesFilter`，因为这两个 API 的既有契约是精确集合匹配。

改变 Do/Ignore 优先级或默认行为时，应修改 `ParseMySQLReplicationRules` 的两段对称逻辑，并保留“黑名单规则在默认放行规则之前”以及 schema/table 结果取交集的不变量。若抽取共用构造逻辑，仍需分别验证 `MatchSchema` 和 `MatchTable`，避免表级负向规则意外否决整个 schema。

扩充公开数据结构时还应同步 `pkg/util/filter/filter.rs` 的 `Rules`/`Table` 使用处和 `pkg/util/regexpr-router/regexpr_router.rs` 的规则构造处，并评估 Go 序列化标签是否需要 Rust 上层支持。测试必须继续放在独立测试文件，不能内嵌到 `compat.rs`。性能上应避免在每次匹配时重新编译正则或重建集合；兼容性上需优先保持 Go 规则优先级、简单小写映射和错误类别。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/table-filter` 确认目标、入口、对照测试均已索引；`node --file pkg/util/table-filter/compat.rs` 读取 261 行源码和 28 个符号；`query` 定位 `MySQLReplicationRules`、`NewSchemasFilter`、`NewTablesFilter`、`ParseMySQLReplicationRules`、`matcherFromLegacyPattern`；`node` 另核对 `table_filter.rs`、`pkg/util/filter/filter.rs`、`pkg/util/regexpr-router/regexpr_router.rs` 和迁移测试。`callers ParseMySQLReplicationRules` 在 40 秒内无输出后终止，因此调用边结论以精确引用搜索补足，不把超时当作“无调用者”的证明。
- 源与边界：[`compat.rs`](compat.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`table_filter.rs`](table_filter.rs)。目标目录没有 `doc.go`，故以 crate 入口 `lib.rs` 作为最近模块契约。
- Go 对照：[`compat.go`](compat.go) 与 [`compat_test.go`](compat_test.go)。
- Rust 测试：[`compat_test.rs`](compat_test.rs)；补充构造、显示和 ignore 优先级证据来自 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。
- 上游与接线：`pkg/util/filter/filter.rs` 的 `Table`/`Rules` 类型别名及 `New` 中的 `Rules::ToLower`，`pkg/util/regexpr-router/regexpr_router.rs` 的规则构造；Cargo 依赖由各自 `Cargo.toml` 精确搜索核对。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证文档存在且恰有 11 个固定二级标题，并人工复核公开 API、真实生产接线和仅测试调用之间的区分。
