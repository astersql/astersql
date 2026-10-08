# `pkg/planner/indexadvisor/options.rs`

## 文件定位

该文件属于 `astersql-planner-indexadvisor` crate；同目录 `Cargo.toml` 将 `lib.rs` 设为 crate 根，`lib.rs` 再把本文件公开为 `options` 模块。它位于索引顾问的配置边界：定义四个配置名、一次推荐所用的参数快照、用户值类型、持久化抽象，以及默认值、校验、读取补缺和 Go 风格 duration 解析。

当前 Rust 接线分为两个层次。`AdvisorOptions` 已进入推荐主链：`indexadvisor.rs::advise_indexes_for_sql` 接收它并传给 `algorithm.rs::advise_indexes`，后者使用索引数量、索引宽度和超时约束搜索。另一方面，`OptionStore`、`fill_options`、`set_option`、`set_options` 和 `get_options` 的仓库内直接调用目前只出现在独立的 `options_test.rs`；没有发现把这些函数接到数据库或 `pkg/executor/recommend_index.rs::IndexAdvisor` trait 的具体生产实现。因此，本文件表达了选项读写语义，但自身不执行 SQL，也不能据此宣称 Rust 已完成 `mysql.tidb_kernel_options` 的生产接线。

源码包含 5 个公开常量和 1 个公开常量数组、2 个公开类型、1 个公开 trait、7 个公开函数、2 个私有辅助函数及 1 个 `Default` 实现；没有条件编译项或全局可变状态。文件开头的大段注释是 Go 迁移草图，不参与编译，真实行为以第 113 行后的 Rust 定义为准。

## 核心职责

- 用 `OPT_MODULE`、四个选项名和 `ALL_OPTIONS` 固定持久化键空间，避免调用方自行拼写。
- 用 `AdvisorOptions` 保存一次顾问运行的参数快照，并提供与 Go 默认值一致的 `Default`。
- 用 `OptionValue` 限定用户输入形状，用 `option_value` 对数量选项和超时选项进行按名校验并序列化。
- 用 `OptionStore` 隔离实际持久化机制；`get_options` 负责读取后补默认值，`set_option(s)` 负责校验后按顺序写入。
- 用 `fill_options` 合并“持久化值 → 用户本次覆盖 → 调用方已显式设置的字段”，构造运行参数。
- 用 `parse_duration` 在不依赖 Go 运行时的前提下解析 `time.ParseDuration` 风格的常用单位、连续片段和小数。
- 用 `description`、`default_value` 统一 SHOW/持久化描述所需的静态元数据。

## 主要符号

- `OPT_MODULE = "index_advisor"`：传给 `OptionStore::{get,set}` 的模块名，对应 Go 表记录的 `module` 列。
- `OPT_MAX_NUM_INDEX`、`OPT_MAX_INDEX_COLUMNS`、`OPT_MAX_NUM_QUERY`、`OPT_TIMEOUT`：四个稳定选项名；`ALL_OPTIONS` 以此顺序列出全部选项。
- `pub struct AdvisorOptions`：字段分别为 `max_num_indexes: usize`、`max_index_width: usize`、`max_num_query: usize` 和 `timeout: Duration`。`Default` 返回 `5 / 3 / 1000 / 30s`。
- `pub enum OptionValue`：区分 `Integer(i64)`、`Unsigned(u64)` 与 `Text(String)`，避免在本层依赖 parser AST 值类型。
- `pub trait OptionStore`：`get(module, names)` 批量返回字符串映射；`set(module, name, value, description)` 写入单项。错误边界统一为 `String`。
- `fill_options(store, current, overrides)`：先读取四项，再按传入顺序校验并覆盖内存映射，只为 `current` 中等于零的字段补值。
- `set_option` / `set_options`：前者校验并写单项；后者严格按切片顺序逐项调用前者，首错即停。
- `get_options`：读取指定名字，并对缺失名字调用 `default_value` 补值；不返回描述映射。
- `option_value`：私有的按名校验与字符串化中心。
- `parse_positive`：私有的持久化整数解析；解析失败故意返回零，以复刻 Go `fillOption` 忽略 `strconv.ParseInt` 错误的行为。
- `parse_duration`：公开的 duration 解析器，返回非负 `std::time::Duration`。
- `description` / `default_value`：对已知名字返回固定文本，对未知名字返回空字符串。

## 执行流程

`fill_options` 的合并流程如下：

1. 调用 `get_options(store, &ALL_OPTIONS)`；底层 `store.get` 失败时立即返回。
2. `get_options` 为存储未返回的每个名字插入 `default_value`。因为调用的是 `ALL_OPTIONS`，正常情况下四个键都会存在。
3. 依次处理 `overrides`，通过 `option_value` 校验并把字符串结果写入映射；同名覆盖项按后出现者生效。这里仅修改局部映射，不调用 `OptionStore::set`。
4. 对 `current` 的三个数量字段分别检查是否为零；非零值保持不变，零值从合并映射经 `parse_positive` 补入。
5. 仅当 `current.timeout.is_zero()` 时读取并解析 timeout；解析失败返回错误。所有补值成功后返回 `Ok(())`。

写入流程中，`set_option` 先用 `option_value` 完成全部校验，再调用一次 `store.set(OPT_MODULE, name, value, description(name))`。`set_options` 串行重复这一动作；它没有预校验整批，也没有事务回滚，所以前项成功、后项失败时会保留前项写入。`options_test.rs::option_validation_and_sequential_writes_match_go` 明确验证了这一部分成功语义。

`parse_duration` 先剥离可选正负号；精确文本 `0` 直接返回零。其余输入按“数字/可选小数 + 单位”循环解析并累计纳秒，支持连续片段，例如 `1h30m`。每段使用检查乘法和加法，累计值超过 `i64::MAX` 纳秒即失败；最终只有非零负值被拒绝。

## 数据与状态

- `AdvisorOptions` 是可克隆、可比较的值快照，不持有存储、会话或时钟句柄。零同时承担实际数值和“尚未设置”的哨兵：数量字段补齐后仍可能因持久化脏数据保持零；timeout 的显式零也会被 `fill_options` 当成待补值。
- `BTreeMap<String, String>` 用于读取和合并，提供确定性的键顺序；逻辑不依赖哈希随机性。
- `OptionValue` 在校验时被按值克隆。合法数量值最终保存为十进制字符串；timeout 保留用户原始文本，而不是归一化成纳秒或统一单位。
- `parse_duration` 以 `u128` 做中间运算，最终限制在 `i64::MAX` 纳秒并构造 `Duration::from_nanos`。小数单位换算使用整数除法，低于 1 纳秒的余数向下截断；测试中的 `0.0000000006s` 因此得到零。
- `description`、`default_value` 返回静态字符串，不分配；所有默认值也与 `AdvisorOptions::default` 一致。
- 当前 Rust 推荐算法读取 `max_num_indexes`、`max_index_width` 和 `timeout`；精确仓库检索未发现生产代码读取 `max_num_query`。该字段目前只被构造或补齐，Go 中限制 statement-summary workload 数量的生产语义尚未在本 Rust 调用链体现。

## 依赖与调用关系

本文件的直接编译依赖只有标准库的 `BTreeMap` 和 `Duration`，以及自身定义的 trait/type；不直接依赖 parser、session、executor 或数据库 crate。`Cargo.toml` 的包名为 `astersql-planner-indexadvisor`，其列出的项目依赖当前全部位于 `target.'cfg(windows)'.dependencies`；不能由该清单反推本文件在其他平台已经具备数据库接线。

已验证的 Rust 调用关系包括：

- `indexadvisor.rs::advise_indexes_for_sql -> algorithm.rs::advise_indexes(options: &AdvisorOptions)`；算法再由 `check_timeout` 等路径读取快照。
- `fill_options -> get_options -> OptionStore::get / default_value`，并调用 `option_value`、`parse_positive`、`parse_duration`。
- `set_options -> set_option -> option_value -> OptionStore::set`；timeout 校验还会进入 `parse_duration`，写入描述来自 `description`。
- `get_options -> OptionStore::get / default_value`。
- `options_test.rs` 直接覆盖 `parse_duration`、`fill_options`、`set_option` 和 `set_options`；其他索引顾问测试通过构造 `AdvisorOptions` 覆盖其算法消费面。

`pkg/executor/recommend_index.rs` 定义了另一个通用 `IndexAdvisor` trait，其 `set_options`、`get_options` 方法由执行器的 `set`/`show` action 调用，但该 trait 的签名和本文件的 `OptionStore` 并不相同。仓库内未找到二者之间的具体适配实现，文档只把它记录为未来可能的接线边界，不把同名方法当作静态调用边。

## 错误处理与边界

- 所有可失败 API 使用 `Result<_, String>` 并通过 `?` 原样传播存储错误、未知选项、类型错误、数值错误和 duration 错误；没有结构化错误种类或额外上下文包装。
- 三个数量选项只接受正的 `Integer`，或 `1..=i64::MAX` 的 `Unsigned`；零、负数、文本和过大的无符号数都返回 `"invalid value for <name>"`。这比 Go 在 64 位平台把 `uint64` 直接转换为 `int` 的形状更明确地拒绝溢出区间。
- timeout 只接受 `Text`。空串、缺少单位、未知单位、格式破损、计算溢出和非零负值均失败；`0`、`0s`、`-0s`、带正号、小数、连续单位以及 `ns/us/µs/μs/ms/s/m/h` 可被解析。
- `fill_options` 的 override 名字也经过 `option_value`，因此未知名字会失败，不能静默加入。若前面的 override 已修改局部映射、后面的 override 失败，`current` 尚未开始补值，调用方快照保持原状。
- `parse_positive` 对缺失键报错，但对无法解析为 `usize` 的值返回零；这是刻意保持 Go 行为，不表示脏持久化值有效。其结果也受目标平台 `usize` 范围影响。
- `get_options` 对未知名字不会报错，而是以空字符串补齐；`description` 对未知名字同样返回空串。正常公开写路径会在 `option_value` 先拒绝未知名字。
- `set_options` 不具备原子性；调用者若需要整批原子写入，必须在 `OptionStore` 上层提供事务或改变 API 契约，不能假设该循环会回滚。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接。合并映射和解析器状态都是单次调用内的局部值，调用结束后释放。

`OptionStore` 方法只接收 `&self`，允许实现通过内部可变性执行读写；trait 没有 `Send`、`Sync`、事务或一致性约束。本文件按调用顺序同步调用 store，不提供并发控制。独立测试用 `RefCell` 实现 store，也说明单线程实现是合法的。若生产实现被多线程共享，线程安全、读写隔离、连接生命周期和批量写原子性均由实现方负责。

解析开销与输入长度线性相关；持久化开销由 `OptionStore` 决定。`set_options` 每项调用一次 `set`，没有批处理，选项数当前固定且很小，但扩展大量选项时需要评估往返次数。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/indexadvisor/options.go`，相关行为测试为 `options_test.go`。Rust 保留了以下核心语义：模块名和四个选项名、`5/3/1000/30s` 默认值、持久化值先读而用户运行时选项后覆盖、只补零值字段、数量必须为正、timeout 必须是 duration 字符串且不能为负、逐项写入遇错即停、缺失持久化项回落默认值，以及固定英文描述。

当前差异和迁移状态如下：

- Go `SetOption`/`GetOptions` 通过 session context 和 `exec` 直接 UPSERT/查询 `mysql.tidb_kernel_options`；Rust 改为 `OptionStore` 抽象，仓库内尚无生产数据库实现的直接证据。
- Go 接收 `ast.RecommendIndexOption`/`ast.ValueExpr`；Rust 使用独立的 `(String, OptionValue)`，尚需 parser/executor 适配层。
- Go `GetOptions` 同时返回值和描述两个 map；Rust `get_options` 只返回值，描述需由调用方另行调用 `description`。
- Go `fillOption` 的 `Option` 定义在其他文件并使用 `int`/`time.Duration`；Rust 使用本文件的 `AdvisorOptions`、`usize` 和非负 `Duration`。
- Rust duration 解析器覆盖测试中所需的 Go 风格常用语法，但不是对 Go `time.ParseDuration` 源码的逐字移植。任何新增单位、极值或舍入兼容要求都应先增加 Go/Rust 对照测试。
- Go 的 `max_num_query` 会限制从 statement summary 取得的 workload；当前 Rust 生产检索只发现该字段的定义和测试构造，没有发现对应消费点。
- Go 端 `options_test.go` 还以真实 SQL 覆盖 SET/SHOW/RUN 与 `tidb_kernel_options`；Rust `options_test.rs` 使用内存 store 验证本文件局部语义，不能替代数据库集成覆盖。

## 扩展指南

- 新增选项时必须同步更新选项名常量、`ALL_OPTIONS`、`AdvisorOptions`、`Default`、`fill_options`、`option_value`、`description`、`default_value`，并在独立的 `options_test.rs` 添加默认值、覆盖优先级、非法类型和写入顺序测试；不要把测试写进生产文件。
- 若接入真实持久化，应实现明确的 `OptionStore` 适配器，并决定如何与 `pkg/executor/recommend_index.rs::IndexAdvisor` 组合。必须保留 Go 的模块/名称键、描述文本、UPSERT 行为、错误传播和部分成功顺序，或明确引入事务后的兼容性变化。
- 若让 Rust 支持 statement-summary workload，消费 `AdvisorOptions::max_num_query` 的位置应对照 Go 的 workload 获取路径，而不是在显式 SQL 的 `advise_indexes_for_sql` 中随意截断；后者在现有 Rust 注释中明确处理全部用户 SQL。
- 修改零值语义前需区分“显式配置为零”和“未设置”。特别是 timeout 的零既是 Go 可持久化值，又是 `fill_options` 的补值哨兵；改变这一点可能影响立即超时行为和覆盖优先级。
- 扩展 `parse_duration` 时应保持检查运算、非负 `Duration` 边界、连续单位和小数截断规则，并补充溢出、Unicode 微秒单位、正负零及非法尾部测试。
- 若改为结构化错误，调用方和 store 适配器需同步迁移；用户可见错误文本、Go 兼容性和 executor 的错误转换都应纳入审查。
- 性能方面，现有四项配置不构成热点；真实风险主要来自将来 store 的逐项网络/SQL 往返。批处理优化不得悄然改变 `set_options` 的有序部分成功语义。

## 验证依据

- 目标源码：`pkg/planner/indexadvisor/options.rs`，通过 RustCodeGraph 文件节点完整核对 384 行、27 个索引符号、公开性、分支、错误路径和 duration 算法。
- crate 与模块边界：`pkg/planner/indexadvisor/Cargo.toml`、`pkg/planner/indexadvisor/lib.rs`；目标目录没有 `doc.go`，因此无额外包契约可读。
- Rust 生产消费面：`pkg/planner/indexadvisor/indexadvisor.rs::advise_indexes_for_sql`、`pkg/planner/indexadvisor/algorithm.rs::{advise_indexes, check_timeout}`。
- executor 边界：`pkg/executor/recommend_index.rs::{IndexAdvisor, RecommendIndexExec::Next, showOptions}`；仅作为未适配的相邻接口证据。
- Rust 独立测试：`pkg/planner/indexadvisor/options_test.rs::{option_duration_parser_matches_go_units_and_rejects_invalid_values, fill_options_ignores_invalid_persisted_integers_like_go, option_store_reads_defaults_and_applies_overrides_in_go_order, option_validation_and_sequential_writes_match_go}`。`algorithm_test.rs`、`indexadvisor_sql_test.rs`、`indexadvisor_test.rs` 和 `indexadvisor_tpch_test.rs` 另有 `AdvisorOptions` 的直接或间接算法消费证据。
- Go 对照：`pkg/planner/indexadvisor/options.go::{fillOption, SetOptions, optionVal, SetOption, GetOptions, description, defaultVal, intVal}`；`options_test.go` 覆盖四项 SET、默认/持久化 SHOW、多项部分成功、运行时 override 和 timeout 行为。
- RustCodeGraph：`status` 显示索引含 11,467 个文件，目标目录 27 个被索引代码文件；精确 `query` 定位 `AdvisorOptions`、`fill_options`、`set_option(s)`、`get_options`、`option_value`、`parse_positive`、`parse_duration`、`description`、`default_value`。文件节点确认 `AdvisorOptions` 被 `indexadvisor.rs` 与 `algorithm.rs` 消费。通用名称使部分 `callers/callees` 查询出现同名噪声，`fill_options` 调用查询还超时无输出，因此按技能规则用限定目录的精确 `rg` 补证，确认存储 API 目前仅由 `options_test.rs` 直接调用。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行任务指定的结构验证，确认文件存在且恰含 11 个固定二级标题，并人工复核未把迁移草图、同名 executor trait 或 Go 集成能力误写成当前 Rust 已接线事实。
