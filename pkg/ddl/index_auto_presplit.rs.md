# `pkg/ddl/index_auto_presplit.rs`

源码：[`index_auto_presplit.rs`](./index_auto_presplit.rs)

## 文件定位

本文件属于 `astersql-ddl` crate；`pkg/ddl/Cargo.toml` 的 `[lib]` 将 crate 根设为 `pkg/ddl/lib.rs`，后者以 `pub mod index_auto_presplit;` 对外公开本模块，并仅在 `cfg(test)` 下装配独立测试 `pkg/ddl/index_auto_presplit_test.rs`。它实现新增索引时 `PRE_SPLIT_REGIONS AUTO` 的确定性规划核心：接收调用方已经整理好的表/索引适用性事实与首列统计分布，计算分位边界并构造索引 Region split key，同时表达手工预切分与 AUTO 预切分不同的失败策略（源码模块注释及 `plan_auto_pre_split`、`run_pre_split`）。

它不是完整 DDL job 状态机，也不直接修改 schema、持久化 job、执行 backfill 或推进 schema version。按 Go 主链，预切分发生在 add-index reorg 前的辅助优化阶段；RustCodeGraph 当前只找到 `pkg/ddl/index_auto_presplit_test.rs` 调用本文件三个公开函数，没有找到 Rust 生产调用者。因此当前 Rust 模块应视为已公开、已单测的规划组件，而不能据此宣称已接入 Rust DDL 执行主链。

## 核心职责

1. `plan_auto_pre_split` 先执行适用性门禁：分区表、部分索引、无首列、字符串前缀索引、统计健康度不可用或不足、表行数不足、非 Analyze V2 统计均返回带原因的 `Skipped`，而不是错误。
2. 将 `NullCount`、TopN 和 histogram bucket 的质量统一为 `WeightedValue`；直方图累计计数先转换成相邻 bucket 增量，再按比较编码排序并合并同值。
3. 按 `boundary_ratio_step` 采样内部累计分布分位点；一个热点值即使跨过多个阈值也只产生一个边界，100% 终点不产生边界（`sample_boundaries`）。
4. 调用 `index_presplit::get_split_keys_from_value_list` 把单列边界转成 index key，随后删除空 key、排序和去重，形成 `Planned` 结果。
5. `select_pre_split_mode` 保证同时出现手工配置和 AUTO 标记时手工配置优先；`run_pre_split` 保证手工路径严格返回规划/切分错误，而 AUTO 作为可选优化吞掉规划和切分失败。

## 主要符号

- `AutoPreSplitConfig { min_table_rows, min_stats_healthy, boundary_ratio_step }`：规划阈值。`Default` 与 Go `getAutoPreSplitConfig` 的三个确定性参数一致，分别为 `1_000_000`、`80`、`0.02`；Go 的 `statsLoadTimeout` 不在此纯规划结构中。
- `AutoPreSplitEligibility`：由上层从表、索引和统计元数据派生的扁平事实，包括物理表/索引 ID、分区/部分索引标志、首列存在性与前缀属性、行数和可选健康度。此类型本身不加载元数据。
- `HistogramPoint { upper, cumulative_count }` 与 `DistributionStats`：表示完整 Analyze V2 首列统计快照。`DistributionStats` 保存统计版本、NULL 数量、TopN `(Datum, count)` 和累计直方图点。
- `AutoPreSplitPlanState::{Invalid, Planned, Skipped}`：计划状态；默认值为 `Invalid`。正常返回只显式构造 `Planned` 或 `Skipped`，错误通过 `Result::Err(String)` 表达。
- `AutoPreSplitPlan`：输出 split keys、用于追溯的原始 `boundary_rows` 和可选跳过原因。`Planned` 要有非空 key；`Skipped` 由私有 `skipped` 辅助函数构造。
- `WeightedValue`：内部值、比较编码和质量三元组。它不暴露到模块外。
- `plan_auto_pre_split(...) -> Result<AutoPreSplitPlan, String>`：主要公开规划入口。
- `weighted` / `encode_comparison_value`：创建内部加权值；当前编码按 `Null < Int < UInt < Bytes/Text` 的 tag 和字节序构造，带符号整数通过翻转符号位保持数值顺序，`Bytes` 与 `Text` 使用同一 tag。
- `sample_boundaries`：私有累计质量采样器。
- `PreSplitMode::{None, Manual, Auto}`、`select_pre_split_mode`、`run_pre_split`：执行模式与严格/尽力而为策略边界。`run_pre_split` 用闭包注入 AUTO 规划器和实际 split 操作，因此自身不持有存储客户端。

## 执行流程

`plan_auto_pre_split` 的主流程如下：

1. 按固定顺序检查 eligibility。最先命中的不适用条件以 `Ok(Skipped)` 返回，使上层能区分“无需优化”与“统计损坏/构键失败”。
2. 校验 `stats_version == 2`；负 `null_count` 返回错误；`boundary_ratio_step` 必须严格位于 `(0, 1)`。注意默认 `1.0` 不被 Rust 接口接受，而 Go 私有采样函数的单元测试可直接以 `1` 验证零边界，这是接口层级差异。
3. 若 NULL 质量大于零，加入 `Datum::Null`；过滤计数为零的 TopN；逐项检查 histogram 累计计数单调不减并计算增量，零增量不加入分布。
4. 每个 datum 经 `encode_comparison_value` 转成排序/等值合并依据；排序后把相同编码的 TopN 和 histogram 质量合并。总质量使用 `saturating_add`，避免 `u64` 溢出回绕。
5. 总质量为零时跳过。否则 `sample_boundaries` 逐值累加质量，在每个内部阈值首次被越过时输出该值，并把下一个阈值推进到已跨越阈值之后，避免热点值被重复输出。
6. 没有内部边界时跳过；否则按 `table_id`、`index_id` 和边界行构造 key，转换底层 `SplitError` 为带上下文的字符串，清理空 key、排序、去重；清理后无 key 被视为错误。
7. 返回同时含 `split_keys` 和 `boundary_rows` 的 `Planned`，便于测试和上层观测。

执行策略入口先由 `select_pre_split_mode` 选择模式：`manual_rows: Some(_)` 永远优先于 AUTO。`run_pre_split` 对 `None` 返回 `Ok(None)`；对 `Manual` 先以 `(table_id, index_id) = (0, 0)` 调用通用构键函数，再严格传播 `SplitError`；对 `Auto`，规划错误、非 `Planned` 结果和 split 错误均降级为 `Ok(None)`，成功时返回 `Ok(Some(split_count))`。

## 数据与状态

本文件没有全局可变状态、缓存、持久化字段或 DDL job checkpoint。所有输入均以借用或值传入，计划在单次同步调用内构造并返回。

`AutoPreSplitPlanState::Invalid` 主要充当默认/未初始化状态；`skipped` 会设置原因且保持 keys/边界为空，`Planned` 会设置 keys/边界且清空原因。当前类型字段公开，Rust 类型系统并未禁止调用方手工构造不一致组合，因而扩展调用方应优先消费 `plan_auto_pre_split` 的结果并按 `state` 判断，而不要仅以 key 是否为空推断状态。

质量口径是 NULL、TopN 计数和 histogram bucket 增量的总和，不直接采用 `eligibility.row_count` 作为分位分母。`row_count` 只参与大表门禁。统计合并与总数采用饱和加法；histogram 的 `i64` 差值在验证单调后转成 `u64`。

## 依赖与调用关系

直接 Rust 依赖都位于同一 `astersql-ddl` crate：

- `crate::index_cop::Datum` 提供 `Null`、`Int`、`UInt`、`Bytes`、`Text` 五类边界值。
- `crate::backfilling::Key` 是 split key 类型。
- `crate::index_presplit::{get_split_keys_from_value_list, SplitError}` 提供边界行到 key 的转换与 split 错误类型。

关键内部调用边为 `plan_auto_pre_split -> weighted -> encode_comparison_value`、`plan_auto_pre_split -> sample_boundaries`、`plan_auto_pre_split -> get_split_keys_from_value_list`，以及 `run_pre_split -> get_split_keys_from_value_list / 调用方注入的 plan_auto 与 split 闭包`。

RustCodeGraph 对公开入口的调用者结果为：`plan_auto_pre_split` 被四个 Rust 测试调用，`select_pre_split_mode` 和 `run_pre_split` 被策略测试调用；目标生产文件除测试外没有静态上游调用。Go 的真实生产链则是 `index.go` 调用 `preSplitIndexRegions`，后者在 AUTO 分支调用 `autoPreSplitIndexRegion -> planAutoPreSplitWithCache`，最终调用存储的 Region split/scatter API（`pkg/ddl/index_presplit.go` 与 `pkg/ddl/index_auto_presplit.go`）。该 Go 链只能说明移植目标，不能当作 Rust 已接线证据。

## 错误处理与边界

可预期的不适用条件返回 `Ok(Skipped)` 并带稳定原因，包括分区表、部分索引、缺少首列、字符串前缀索引、健康度缺失/不足、小表、非 V2 统计、空分布和无内部分位点。数据或配置不变量破坏返回 `Err(String)`：负 NULL 数、非单调累计 histogram、比例不在 `(0,1)`、构键失败、清理后无 key。

零计数 TopN 与零增量 histogram 被忽略；相同编码值合并；空 key 被删除；输出 key 排序并去重。这些操作使规划结果不依赖 TopN/histogram 输入顺序，并避免重复 Region 边界。

`run_pre_split` 有意形成不对称语义：手工模式的构键和 split 错误必须返回，AUTO 模式的规划/执行错误被视为非致命优化失败。可是当前 Rust 函数没有 cancellation cause、DDL pause、超时或 unsupported-storage 的单独表示；这些是 Go 上层 `preSplitIndexRegions` / `autoPreSplitIndexRegion` 的职责。若未来接线，暂停和取消不应被普通 AUTO best-effort 分支吞掉。

当前比较编码是受限 `Datum` 集合上的本地规则，并不等同于 Go `codec.EncodeKey` 加字段类型/排序规则规范化。特别是字符串 collation、时间/小数等更多 datum 类型、真实 index key 前后缀，以及 `Manual` 路径使用零 ID，都需要由实际接线层补齐或替换，不能把现实现直接视为 Go 全类型兼容。

## 并发与资源生命周期

模块完全同步，不创建线程、任务、channel、锁、事务、context、计时器或存储连接。输入统计被借用，TopN/histogram datum 在形成内部值和边界时按需 clone；局部 `Vec` 在函数返回后释放，只有结果中的 keys/边界转移给调用方。

`run_pre_split` 的 AUTO 规划闭包是 `FnOnce`，保证最多规划一次；split 闭包是 `FnMut`，但每次调用 `run_pre_split` 最多调用一次。手工模式不会调用 AUTO 闭包，AUTO 规划失败或返回非 `Planned` 时不会调用 split 闭包，这些约束由 `manual_policy_overrides_auto_and_auto_failures_are_best_effort` 测试验证。

Go 版本在外层拥有更复杂的生命周期：统计加载使用带超时 context，同一批索引共享 AUTO deadline，并按首列 ID 缓存边界结果；split 后等待 scatter，并优先传播 DDL pause/cancel cause。Rust 当前没有这些资源与并发语义，未来接线必须在上层明确实现，而不应把它们隐含进这个纯函数模块。

## 与 Go 版本的对应关系

Rust `AutoPreSplitConfig` 对应 Go `autoPreSplitConfig` 的行数、健康度和分位步长；`AutoPreSplitPlanState` / `AutoPreSplitPlan` 对应 `autoPreSplitPlanState` / `autoPreSplitPlanResult`；`WeightedValue`、合并循环和 `sample_boundaries` 分别对应 `autoPreSplitValue`、`mergeAutoPreSplitValues`、`sampleAutoPreSplitValues`。门禁原因、TopN 与 histogram 合并、热点值只发一个边界、终点排除、key 排序去重，以及手工严格/AUTO 尽力而为的意图均与 Go 测试一致。

Rust 把 Go 多层流程压平成调用方提供的 `AutoPreSplitEligibility` 与 `DistributionStats`，因此没有移植以下职责：`autoPreSplitStatsProvider`、统计缺失/淘汰后的存储加载、30 秒加载超时、按 leading-column 缓存边界、字段类型转换与 collation key 处理、真实 `TableInfo/IndexInfo` 构键、临时索引 key 转换、共享 AUTO deadline、Region split/scatter、日志及 pause/cancel cause 优先传播。Go 还验证最多 49 个默认内部边界、共享首列缓存、unsupported storage、真实 collation 字符串与 reorg keyspace；Rust 独立测试目前只覆盖确定性子集。

此外，Go 的 `sampleAutoPreSplitValues` 私有函数允许测试传入步长 `1` 并返回零边界；Rust 的公开 `plan_auto_pre_split` 在采样前要求步长严格小于 `1`。这是当前接口防御性校验差异，应在未来追求逐语义完全对齐时显式决策并补测试，而不是默默假设等价。

## 扩展指南

- 若新增适用性条件，修改 `AutoPreSplitEligibility` 和 `plan_auto_pre_split` 的门禁顺序，并同时扩展 `pkg/ddl/index_auto_presplit_test.rs` 的 skip-reason 表；原因文本可能被日志/测试依赖，应保持稳定。
- 若扩展 datum 类型或排序语义，应优先让比较编码与实际 index key 编码共享权威实现，重点验证负数、无符号数、Bytes/Text、NULL、collation、前缀索引及相同逻辑值的合并；不要仅在 `encode_comparison_value` 中增加随意 tag。
- 若修改采样策略，保持“只输出内部边界”“热点值跨多个阈值只输出一次”“输出有序去重”不变量，并同步 Rust 的 TopN/histogram/热点测试及 Go `TestPlanAutoPreSplitIndexRegionsTopN`。
- 若接入生产 DDL，最可能的入口是 add-index reorg 前的预切分协调层。上层必须提供真实元数据/统计快照、按字段类型构键、临时索引转换、共享 deadline、缓存、日志和 Region 客户端；还必须像 Go 一样先传播 pause/cancel，再把普通 AUTO 失败降级。
- `run_pre_split` 的手工构键目前传入零表/索引 ID，说明它更像策略测试适配器；生产接线前应让真实 ID/已构造 keys 进入接口，并增加独立测试，不能依赖零 ID 产生真实存储 key。
- Rust 单元测试继续放在同目录独立文件 `pkg/ddl/index_auto_presplit_test.rs`，不要嵌回生产源文件。涉及完整存储与 DDL 生命周期的测试应放在相应独立集成测试面。
- 性能风险主要在复制/排序全部 TopN 与 histogram 值、key 构造及边界数量；兼容风险主要在编码顺序、collation、统计版本与 skip/error 分类；正确性风险主要在累计计数、溢出和取消错误被误吞。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ddl/index_auto_presplit.rs` 显示目标文件有 27 个符号。
- RustCodeGraph `node --file pkg/ddl/index_auto_presplit.rs --offset 1 --limit 500`：核对全部 334 行源码、公开类型/函数、内部算法和模块注释。
- RustCodeGraph `query` / `explore`：精确定位 `plan_auto_pre_split`（第 115 行）、`select_pre_split_mode`（第 302 行）、`run_pre_split`（第 313 行）；调用者只落在 `pkg/ddl/index_auto_presplit_test.rs`，未发现 Rust 生产调用边。
- `pkg/ddl/index_auto_presplit_test.rs`：核对 TopN+histogram+NULL 合并、热点值跨阈值、各跳过原因、负 NULL 错误、空分布跳过、手工优先和 AUTO best-effort。
- `pkg/ddl/index_auto_presplit.go`、`pkg/ddl/index_presplit.go`、`pkg/ddl/index_auto_presplit_test.go`：核对 Go 生产链、统计加载/缓存/超时、真实构键、split/scatter、取消传播、默认参数、collation 与共享 deadline 测试。
- `pkg/ddl/lib.rs`：核对生产模块公开和独立测试仅在 `cfg(test)` 下装配。
- `pkg/ddl/Cargo.toml`：核对 crate 名 `astersql-ddl`、crate 根 `lib.rs` 及 Go package 映射 `pkg/ddl`；本文件直接使用的三个模块均为 crate 内部模块。
- `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`：核对 DDL 包的 Online DDL/job 背景；本文件不承担 schema 状态推进、版本同步或 job 持久化。
- 按任务约束未运行 Cargo；交付验证仅执行文档的 11 章节结构检查，并人工复核源码链接、当前接线限制与 Go/Rust 差异。
