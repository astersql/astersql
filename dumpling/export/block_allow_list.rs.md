# `dumpling/export/block_allow_list.rs`

## 文件定位

本文件属于 `astersql-dumpling-export` library crate。`dumpling/export/Cargo.toml` 将 `lib.rs` 指定为 crate 根，而 `dumpling/export/lib.rs` 通过 `include!("block_allow_list.rs")` 将本文件并入与 Go `dumpling/export` 包相近的单包符号空间。因此，文件中的三个 `pub fn` 实际成为 crate 根的公开函数，而不是独立 Rust 子模块的成员。

它位于 Dumpling 导出准备阶段的“候选对象裁剪”位置：数据库列表从服务端取回后由 `prepareDumpingDatabases` 调用 `filterDatabases`；表清单来自显式配置或数据库枚举后，由 `prepareTableListToDumpInner` 调用 `filterTables`。此外，锁表一致性流程会在发现已不存在的表后直接调用 `filterTablesFunc`，从待导出集合中再次排除这些表（`dumpling/export/consistency.rs::consistency_lock_setup`）。

## 核心职责

文件只负责缩小已经存在的数据库/表候选集合，不负责解析过滤规则、查询数据库元数据或执行导出：

- `filterDatabases` 按 `Config.TableFilter.MatchSchema` 筛选数据库名并返回新向量。
- `filterTables` 把配置中的 `TableFilter.MatchTable` 适配为回调，交给通用过滤骨架。
- `filterTablesFunc` 按调用方提供的表匹配函数重建 `Config.Tables`，同时实现 `DumpEmptyDatabase` 的空库保留规则。
- 三个入口都会借助 `tcontext::Context` 记录开始过滤及被忽略对象，便于解释对象为何未进入最终导出清单。

过滤是保留式重建：命中项携带完整 `TableInfo` 进入新的 `DatabaseTables`，未命中项仅用于 debug 日志。最终不会修改 `TableInfo` 的名字、平均行长或对象类型。

## 主要符号

- `pub fn filterDatabases(tctx: &tcontext::Context, conf: &Config, databases: Vec<String>) -> Vec<String>`：消费数据库名向量，用 `conf.TableFilter.MatchSchema` 将其分为保留项和忽略项。返回值保持输入向量中保留项的相对顺序；容量按输入长度预分配。该函数不修改 `Config`。
- `pub fn filterTables(tctx: &tcontext::Context, conf: &mut Config)`：常规表过滤入口。它先克隆 `conf.TableFilter`（字段类型为 `Arc<dyn Filter>`），避免在可变借用 `conf` 时继续从其中捕获过滤器，再以闭包调用 `filterTablesFunc`。
- `pub fn filterTablesFunc<F>(tctx: &tcontext::Context, conf: &mut Config, match_table: F) where F: Fn(&str, &str) -> bool`：可注入匹配逻辑的核心实现。它复制当前 `conf.Tables` 的键和值形成快照，遍历每个 `(database, TableInfo)`，分别累积命中集合和忽略集合，最后整体替换 `conf.Tables`。
- `DatabaseTables`：定义在 `dumpling/export/prepare.rs`，实际为 `HashMap<String, Vec<TableInfo>>`。本文件使用其 `DatabaseTablesExt::AppendTable` 和 `Literal` 扩展方法追加对象并生成日志文本。
- `Config.TableFilter`、`Config.Tables`、`Config.DumpEmptyDatabase`：定义在 `dumpling/export/config.rs`，分别承载规则、当前候选集合和空库输出开关。`TableFilter` 的 trait 定义来自 workspace 依赖 `astersql-util-table-filter`；`MatchSchema` 与 `MatchTable` 的具体规则优先级由该 crate 决定。

本文件没有模块级常量、类型、trait、`impl` 或条件编译项。

## 执行流程

数据库过滤路径如下：

1. `prepareDumpingDatabases` 通过 `ShowDatabases` 得到实例中真实存在的数据库。
2. `filterDatabases` 逐个调用 `MatchSchema`；命中项进入返回向量，未命中项进入日志集合。
3. 若存在忽略项，函数记录一次 `ignore database` debug 日志；随后返回筛选结果。
4. `prepareDumpingDatabases` 再根据 `Config.Databases` 是否为空，决定直接使用筛选结果，还是校验并返回用户显式指定的数据库。

表过滤路径如下：

1. `prepareTableListToDumpInner` 在显式表模式下直接过滤已有 `Config.Tables`；普通模式则先枚举各数据库的表、视图和序列，再过滤枚举结果。原始 SQL 导出模式会提前返回，不进入本文件。
2. `filterTables` 克隆 `TableFilter`，将 `MatchTable(database, table)` 包装为不可变回调。
3. `filterTablesFunc` 对 `Config.Tables` 做拥有所有权的快照；每个表只调用一次匹配回调，命中则追加到新集合，否则追加到忽略集合。
4. 每处理完一个数据库，若 `DumpEmptyDatabase` 为真、该库尚无命中表且 `MatchSchema` 仍允许该库，则在新集合中插入空 `Vec<TableInfo>`。因此，“表全部被排除但 schema 允许”和“输入原本就是空库”都可以保留空库占位；schema 不允许时不会保留。
5. 若存在被忽略的表，函数用 `ignored.Literal()` 记录一次 debug 日志，然后以新集合整体覆写 `conf.Tables`。

锁表重试路径复用同一骨架：`consistency_lock_setup` 成功锁表且 `backoffer.block_list` 非空时，以“表不在 block list 中”为回调调用 `filterTablesFunc`，删除重试期间确认不存在的表；这里不改写用户的 `TableFilter`。

## 数据与状态

输入数据库向量由值传入并被消费；输出是新 `Vec<String>`。表过滤则原地修改 `Config.Tables`，但实现先完整克隆原 map 的数据库名和 `Vec<TableInfo>`，直到过滤结束才一次性替换，因此回调看到的是过滤开始时的稳定快照。

主要不变量是：

- 输出只含输入中已有的数据库或 `TableInfo`，不会生成新表元数据。
- 每个保留表仍归属于原数据库，且 `TableInfo` 字段原样保留。
- 普通表过滤由 `MatchTable` 决定；空库占位还必须同时满足 `DumpEmptyDatabase == true` 和 `MatchSchema(database) == true`。
- 未命中对象不会残留在 `conf.Tables`；它们只短暂保存在局部集合中用于日志。

`DatabaseTables` 基于 `HashMap`，所以数据库之间的遍历和日志次序没有稳定性保证；单个数据库的 `Vec<TableInfo>` 按原向量顺序遍历并追加，因而该库内部保留表的相对顺序不变。快照、结果集合和忽略集合都会在函数返回时释放。

## 依赖与调用关系

RustCodeGraph 给出的直接上游边为：

- `dumpling/export/prepare.rs::prepareDumpingDatabases -> filterDatabases`。
- `dumpling/export/dump.rs::prepareTableListToDumpInner -> filterTables`。
- `dumpling/export/parity_test.rs::contract_normal -> filterTables`（契约测试）。
- `dumpling/export/block_allow_list.rs::filterTables -> filterTablesFunc`。
- `dumpling/export/consistency.rs::consistency_lock_setup -> filterTablesFunc`。

直接下游依赖包括 `Config`、`DatabaseTables`、`DatabaseTablesExt`、`TableInfo`、`tcontext::Context`、日志 `Field`，以及 `astersql-util-table-filter::Filter` 的 `MatchSchema`/`MatchTable` 接口。`dumpling/export/Cargo.toml` 明确声明了 `astersql-dumpling-context`、`astersql-dumpling-log` 和 `astersql-util-table-filter` 三个相关 workspace path 依赖；本文件自身不接触 SQL 连接、存储或网络。

从应用主链看，输出 `Config.Tables` 会继续供锁表、任务生成和实际 dump 路径消费。本文件的边界是生成正确的候选清单，而不是决定后续对象以何种格式或并发度导出。

## 错误处理与边界

三个函数均不返回 `Result`，也没有显式错误分支。过滤器接口返回 `bool`，日志接口在当前抽象下也不向调用者传播失败，因此函数对正常内存分配以外的情况是不可失败的同步变换。

边界行为包括：

- 空数据库输入返回空向量，不记录 `ignore database`。
- 空 `Config.Tables` 会被空结果替换，不记录 `ignore table`。
- 所有表均被拒绝时，数据库是否仍出现只由 `DumpEmptyDatabase` 与 `MatchSchema` 的组合决定。
- `filterTablesFunc` 的自定义回调只约束表级接受条件；空库回填仍使用 `conf.TableFilter.MatchSchema`，调用方必须认识到这两套判断可能来自不同规则。锁表重试正是有意利用这一点：回调排除消失的表，schema 资格仍沿用用户配置。
- 名称匹配的大小写、否定规则及“后写规则优先”等语义不在本文件实现，而由 `pkg/util/table-filter` 的具体 `Filter` 实现负责；本文件按原字符串传入。
- `ignored.Literal()` 的数据库顺序来自 `HashMap`，不应把 debug 文本的顺序当作稳定协议。

当前 Rust 实现为解决借用关系会克隆整个 `Config.Tables` 快照，峰值内存和复制成本与候选数据库、表及 `TableInfo` 数据量线性相关；扩展时不能误认为这是零拷贝过滤。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部句柄；所有操作均在调用线程同步完成。`filterDatabases` 只借用不可变配置，`filterTables`/`filterTablesFunc` 要求对整个 `Config` 的独占可变借用，因此 Rust 类型系统阻止同一 `Config` 在过滤期间被另一安全 Rust 调用同时修改。

`Config.TableFilter` 是 `Arc<dyn Filter>`，其 trait 还要求 `Send + Sync`。`filterTables` 只增加一次 `Arc` 引用计数并在返回时释放局部克隆，不会改变过滤器内部状态；传入 `filterTablesFunc` 的回调约束为 `Fn`，不会从接口层允许依赖可变闭包状态。

日志上下文仅按引用使用，不由本文件创建或关闭。局部 `Vec`、`HashMap`、过滤器 `Arc` 克隆和忽略项日志集合都遵循普通作用域生命周期；`conf.Tables` 的旧值在最终赋值时释放。

## 与 Go 版本的对应关系

`dumpling/export/block_allow_list.go` 具有同名的三个函数，控制流与 Rust 版本逐项对应：数据库按 `MatchSchema` 分流，默认表入口委托 `filterTablesFunc`，核心函数按回调重建表集合，并在允许导出空库且 schema 命中时插入空列表。`dumpling/export/block_allow_list_test.go` 的两个测试场景也由独立 Rust 文件 `dumpling/export/block_allow_list_test.rs` 对应移植：

- `TestFilterTables` / `test_filter_tables` 验证全匹配、只允许 `xxx` schema，以及最终仅剩 `xxx.yyy`。
- `TestFilterDatabaseWithNoTable` / `test_filter_database_with_no_table` 验证空库在 schema 不匹配、schema 匹配和关闭 `DumpEmptyDatabase` 三种组合下的结果。

结构上的主要 Rust 适配是：Go 的 `DatabaseTables` 保存 `[]*TableInfo`，Rust 保存拥有所有权的 `Vec<TableInfo>`；Go 可直接遍历 map 并写入另一个 map，Rust 为避开同时借用 `conf` 的限制先克隆输入快照；Go 传入普通函数值，Rust 用泛型 `Fn(&str, &str) -> bool`；Go 日志用 `zap.Strings` 记录数据库切片，Rust 当前将数据库名以逗号连接后写入单个字符串字段。最后一点可能造成日志字段表现差异，但不改变筛选结果。

Rust 的 `filterTables` 还有 `dumpling/export/parity_test.rs::contract_normal` 作为补充契约锚点，确认 `db.*` 规则保留 `db` 并排除 `other`。当前代码并非桩或未接线门面：RustCodeGraph 已找到生产调用者 `prepareDumpingDatabases`、`prepareTableListToDumpInner` 和 `consistency_lock_setup`。

## 扩展指南

若增加新的筛选维度，优先判断其归属：规则语法或大小写语义应修改 `pkg/util/table-filter`；Dumpling 候选集合的组合与空库行为才应修改本文件。常规用户过滤应从 `filterTables` 接入，运行时临时排除条件可复用 `filterTablesFunc`，但必须明确它与 `conf.TableFilter.MatchSchema` 的组合语义。

修改时应保持以下兼容约束：数据库和单库内表的保留顺序、完整 `TableInfo` 元数据、`DumpEmptyDatabase` 双条件、忽略项日志仅在非空时产生，以及过滤完成后原子式替换 `conf.Tables`。若希望降低全量克隆成本，需要重新设计所有权转移（例如暂时取出 `conf.Tables`），并验证回调及空库判断仍能安全读取配置；这是性能优化，不应以改变 Go 可观察行为为代价。

测试应放在独立文件 `dumpling/export/block_allow_list_test.rs`，不要内嵌到生产源文件。新增普通过滤行为时同步 Go 对照 `dumpling/export/block_allow_list_test.go` 的测试意图；涉及 crate 公共契约时可补充 `dumpling/export/parity_test.rs`；涉及锁表后动态排除时还应扩展独立的 `dumpling/export/consistency_test.rs`。重点风险是规则优先级或大小写兼容、空库被误留/误删、表元数据丢失，以及大规模候选集的克隆峰值。

## 验证依据

本说明基于以下可复核证据：

- RustCodeGraph 索引状态：仓库索引包含 7,032 个 Rust 文件；`node --file dumpling/export/block_allow_list.rs` 读取到完整 82 行实现及被 `consistency.rs`、`dump.rs`、`parity_test.rs`、`prepare.rs` 使用的信息。
- RustCodeGraph 符号查询：`query filterDatabases`、`query filterTables`、`query filterTablesFunc` 均同时定位到 Go/Rust 定义；`node`/`explore` 给出 `prepareDumpingDatabases -> filterDatabases`、`prepareTableListToDumpInner -> filterTables`、`filterTables -> filterTablesFunc`、`consistency_lock_setup -> filterTablesFunc` 和 `contract_normal -> filterTables` 调用边。
- Rust 实现与入口：`dumpling/export/block_allow_list.rs`、`dumpling/export/lib.rs`、`dumpling/export/prepare.rs`、`dumpling/export/dump.rs`、`dumpling/export/consistency.rs`、`dumpling/export/config.rs`。
- crate 与过滤器边界：`dumpling/export/Cargo.toml`、`pkg/util/table-filter/table_filter.rs`。
- 独立测试：`dumpling/export/block_allow_list_test.rs` 与 `dumpling/export/parity_test.rs`。
- Go 对照：`dumpling/export/block_allow_list.go` 与 `dumpling/export/block_allow_list_test.go`。

任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `rg` 命令验证本文恰含十一个固定二级章节，并人工核对上述符号、调用边、边界条件和扩展位置均有直接源码依据。
