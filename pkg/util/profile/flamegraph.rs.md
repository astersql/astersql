# `pkg/util/profile/flamegraph.rs`

## 文件定位

本文件是 `astersql-util-profile` crate 内部的 pprof 火焰图构建与展示层。crate 入口 `pkg/util/profile/lib.rs` 以私有模块 `mod flamegraph` 装配它，只公开再导出 `profile` 模块，因此这里的类型和函数均为 `pub(crate)` 或私有实现，不是工作区外部可直接调用的 API。上游 `Collector::ProfileReaderToDatums` 在 `pkg/util/profile/profile.rs` 中完成字节读取、gzip/Protobuf 解码和 profile 校验，随后经 `profile_to_flamegraph_node` 与 `profile_to_datums` 调用本文件，把 `pprof::protos::Profile` 转成 performance schema 可消费的 `Vec<Vec<types::datum::Datum>>`。

`pkg/util/profile/Cargo.toml` 将该 crate 定义为 `astersql-util-profile`，并声明本文件直接依赖的 `pprof`（启用 `prost-codec`）、内部 `astersql-types` 和 `astersql-util-texttree`；crate 没有控制本逻辑的 feature。目录内没有 `doc.go`，最近的模块契约由 `lib.rs`、`profile.rs` 和同路径 Go 实现给出。

## 核心职责

1. `ProfileIndex` 为 pprof 的 location、function 和 string table 建立只读索引，避免构树和输出期间反复线性查找。
2. `FlamegraphNode` 将每个 sample 的调用栈按 location ID 折叠成一棵前缀树（源码沿用“DAG”称呼；实现中节点只属于一个父节点），并在路径上的每个节点累计该 sample 的最后一个 value。
3. `FlamegraphCollector` 按累计值降序展开树，生成带文本树缩进、全局占比、父节点占比、根分组号、深度和源码位置的六列 Datum 行。
4. `percentage` 与 `format_two_significant_digits` 复现 Go 的百分比展示规则，包括近似 100%、固定两位小数和小值两位有效数字/科学计数法。

本文件不负责读取 profile、解压、Protobuf 解码、合法性校验或 CPU 采样；这些边界位于 `pkg/util/profile/profile.rs`。它也不直接把结果注册成系统表，调用方只接收通用 Datum 行。

## 主要符号

- `type DatumRows = Vec<Vec<Datum>>`：火焰图结果类型别名。每行由 `make_row` 固定生成六列，依次为标识文本、相对整个 profile 的占比、相对父节点的占比、根子树序号、深度、`文件:行号`。
- `ProfileIndex<'profile>`：借用原始 `Profile`，内部保存 `locations: HashMap<u64, &Location>`、`functions: HashMap<u64, &Function>` 和字符串表切片。`new` 构建索引；`location` 按 ID 取 location；`location_name` 使用第一条 line 解析函数名和文件行，无 line 时返回两个 `<unknown>`。
- `FlamegraphNode`：含 `children`、`name`、`cumulative_value`。`children` 以 location ID 而非函数 ID 为键，因此同一函数的不同地址/行可以成为不同节点；`name` 在首次创建子节点时填入，但最终输出会由 `ProfileIndex::location_name` 再解析。
- `new_flamegraph_node`：创建累计值为 0、子表为空、名称为空的根或子节点。
- `FlamegraphNode::add`：读取 `sample.value.last()`；值为 0 时忽略该 sample，否则交给 `add_locations`。
- `FlamegraphNode::add_locations`：先给当前节点增加累计值，再从 `location_id` 切片末端取一个 location 作为子节点，递归处理余下栈帧。由此 pprof 数组末端成为输出树的根侧帧。
- `FlamegraphNode::sorted_children`：先按 `cumulative_value` 降序，再按 location ID 升序打破平局，消除 `HashMap` 遍历的不确定性。
- `FlamegraphNodeWithLocation`：遍历辅助结构，同时携带节点引用与用于反查符号信息的 location ID。
- `FlamegraphCollector<'profile>`：拥有 `ProfileIndex`、输出 `rows`、根总量 `total` 和当前根分组 `root_child`。`new_flamegraph_collector` 初始化它；`collect` 写根行并启动展开；`collect_child` 递归写各子节点。
- `make_row`：把六个逻辑字段按固定列顺序转换成字符串或整数 Datum。
- `percentage`：对 `value / total` 取绝对值后乘 100；`total == 0` 时按 0 处理，99.95 到 100.05（含端点）归一为 `100%`，至少 1 时保留两位小数，更小值交给两位有效数字格式化。
- `format_two_significant_digits`：模拟 Go `fmt.Sprintf("%.2g")`。它先按数量级四舍五入，再在指数不属于 `[-4, 2)` 时输出科学计数法，并移除不必要的 `.0` 或尾随零。

## 执行流程

1. `Collector::ProfileReaderToDatums`（`profile.rs`）读取全部输入，`parse_profile_data` 按 gzip 魔数选择是否解压，解码 `Profile` 并调用 `validate_profile`。
2. `Collector::profile_to_flamegraph_node` 再次校验 profile，创建 `ProfileIndex` 和空根节点，然后对 `profile.sample` 顺序调用 `FlamegraphNode::add`。
3. `add` 选择 sample 的最后一个 value。零值不贡献节点；非零值由 `add_locations` 加到根节点及整条栈路径。对每一层，`locations.split_last()` 取得当前 location ID，按父节点局部的 `children` 表复用或创建子节点，再递归处理剩余 ID。
4. `Collector::profile_to_datums` 创建新的 `FlamegraphCollector` 并调用 `collect`。`collect` 无条件先写 `root / 100% / 100% / 0 / 0 / root`；空树到此返回。
5. 非空树把根累计值保存为 `total`，对 `root.sorted_children()` 逐一展开。根子树按 1 开始编号，每个节点由 `collect_child` 生成一行。
6. `collect_child` 通过 location ID 取得函数名和文件行，使用 `texttree::PrettyIdentifier` 生成 `├─`、`└─` 和竖线缩进，分别以 `total` 与父节点累计值计算两种占比，然后通过 `texttree::Indent4Child` 下钻。叶节点写行后立即返回。

fixture 中可见这一顺序：`runtime.main` 的累计值最大，成为第一棵根子树；同值节点再由 location ID 排序，所以输出不受哈希随机化影响。`pkg/util/profile/flamegraph_test.rs::test_profile_to_datum` 对整份 41 行 fixture 的六列内容逐行核对。

## 数据与状态

所有核心状态都局限于一次转换：`ProfileIndex` 只借用输入 profile；树节点拥有子节点和 `i64` 累计值；collector 消费自身并返回行缓冲。没有全局可变状态。

关键不变量如下：

- location ID、function ID 和 string table 下标必须已经由 `profile.rs::validate_profile` 验证；`ProfileIndex::location` 和部分字符串访问使用索引操作而非返回 `Result`。
- 每个非空 sample 的 value 数量与 `sample_type` 一致，因此 `FlamegraphNode::add` 的 `last().expect(...)` 在标准入口后成立。
- 累计值使用普通 `i64 +=`，与 Go `int64` 语义目标一致；实现没有溢出检查或饱和处理。
- 节点身份是“当前父节点下的 location ID”。相同 location 出现在不同父路径时不会共享同一个节点；相同函数的不同 location 也不会合并。
- `FlamegraphCollector::collect` 消费 collector，防止旧的 `rows`、`total` 或 `root_child` 被跨次收集复用。

## 依赖与调用关系

RustCodeGraph 给出的主调用链为 `ProfileReaderToDatums`（`profile.rs:121`）→ `profile_to_datums`（`profile.rs:146`）→ `new_flamegraph_collector`（`flamegraph.rs:162`）。源码补充的并列构树链是 `profile_to_datums` → `profile_to_flamegraph_node` → `new_flamegraph_node` / `FlamegraphNode::add`，之后 `new_flamegraph_collector(...).collect(&root)` 展开结果。

下游依赖分工：

- `pprof::protos::{Profile, Sample, Location, Function}` 提供已解码的数据模型。
- `types::datum::{Datum, NewIntDatum, NewStringDatum}` 构造 SQL 层可传递的单元格值。
- `texttree::PrettyIdentifier` 与 `texttree::Indent4Child` 负责树形前缀和下一层缩进；本文件负责决定兄弟次序与末节点标志。
- 标准库 `HashMap` 用于 O(1) 期望复杂度的 ID 查找和按父节点聚合；为稳定输出，遍历前显式排序。

对 `s` 个 sample、总计 `f` 个栈帧和 `n` 个生成节点，构树的期望时间为 O(f)，索引与树空间为 O(profile locations + functions + n)。展开会对每个节点的子列表排序，总成本是各节点 `k log k` 的和；递归深度等于最长样本栈深度。

## 错误处理与边界

本文件没有返回业务错误，依赖 `profile.rs` 在进入前拒绝畸形 profile。`validate_profile` 会检查字符串表、重复/保留 ID、location 到 mapping/function 的引用、sample value 数量和 sample location 引用；错误由 `ProfileError` 传回调用方。因此，绕过标准入口直接构造无效数据调用 crate 内部函数可能在 `HashMap`/切片索引或 `expect` 处 panic，这是内部 API 的前置条件，不应被描述为容错行为。

已明确处理的边界包括：sample 最后一个值为 0 时整条 sample 被忽略；无任何非零路径时仍返回唯一 root 行；location 没有 line 时名称与位置均为 `<unknown>`；`percentage(_, 0)` 为 `0%`；百分比取绝对值，因此负累计值显示正比例。当前排序仍按有符号累计值降序，负值样本的相对顺序可能与其绝对占比不同，这是与 Go 当前实现一致、扩展时不可悄然改变的行为。

函数信息只采用 `location.line.first()`。内联调用产生的后续 line 不会展开成额外帧。sample 有多个 value 时只取最后一个；Go 源码对此保留了 FIXME，并列举 allocs、block、cpu、heap、mutex 等多 value profile，新增 sample 类型选择策略必须同时修改两端并补测试。

## 并发与资源生命周期

本文件没有线程、任务、通道、锁、I/O 或外部资源。转换对象均为函数局部所有权或绑定于输入 `Profile` 生命周期的不可变借用；`Box<FlamegraphNode>` 形成独占树，结束后递归释放；返回的 Datum 行拥有字符串数据，不再借用 profile。

实现本身没有共享可变状态，因此不同线程可各自处理独立 profile；但文件没有承诺或实现并行构树。递归构树与递归输出都受输入栈深度影响，极深且合法的调用栈存在调用栈空间风险。性能扩展应优先保留稳定排序和输出顺序，不应简单改为并行遍历而改变行序。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/profile/flamegraph.go`，Rust 基本保持其结构：`flamegraphNode`/`FlamegraphNode`、`newFlamegraphNode`/`new_flamegraph_node`、`add`、`sortedChildren`、collector 的 `collectChild`/`collect_child` 和 `collect`、以及 `percentage` 一一对应。两端都按 location ID 聚合，取 sample 最后一个 value，忽略零值，按累计值降序且按 location ID 处理平局，并输出相同六列。

实现差异主要来自数据模型与安全边界：Go 的 `Sample` 直接持有 `*Location` 和 `*Function`，Rust protobuf 使用 ID，因此 Rust 增加 `ProfileIndex`，并由 `profile.rs::validate_profile` 先验证所有引用。Go collector 只缓存 locations，Rust 同时缓存 functions 和 string table。Go 的 collector 原地填充 `rows`，Rust 的 `collect(mut self)` 消费 collector 并返回 rows。Go `fmt.Sprintf("%.2g")` 无 Rust 等价格式器，所以 Rust 用 `format_two_significant_digits` 显式模拟边界。

`pkg/util/profile/flamegraph_test.go::TestProfileToDatum` 是 fixture 行为基准；`pkg/util/profile/flamegraph_test.rs::test_profile_to_datum` 复刻同一 41 行结果。Rust 额外的 `percentage_rounding_crosses_fixed_notation_boundary_like_go` 锁定四舍五入跨越固定/科学计数法边界；`migration_aster_unit_test.rs` 还验证原始 Protobuf 与 gzip 得到相同行序、极小百分比和畸形 profile 拒绝行为。

## 扩展指南

- 若新增输出列或改变列语义，应从 `make_row` 和 `DatumRows` 的消费者一起评估，并同步 `flamegraph_test.rs::datum`、fixture 逐行预期以及 Go `types.MakeDatums` 输出；这属于 SQL 可见兼容性变化。
- 若改变节点归并键（例如从 location ID 改为 function ID），修改点是 `FlamegraphNode::children`、`add_locations`、`FlamegraphNodeWithLocation` 和 `ProfileIndex` 查名路径。必须新增同函数多 location、同 location 多父路径的独立测试，确认期望的聚合与文件行展示。
- 若支持选择非末尾 sample value，应在 `FlamegraphNode::add` 之前明确 sample type 选择规则，而不是只改数组下标；同步 Go 的 `flamegraphNode.add`，覆盖空/多 value、负值和零值。
- 若改变排序或格式，应保持 `sorted_children` 的确定性平局规则，并为 `percentage` 的 0、1%、99.95/100.05、科学计数法阈值和负值增加表驱动测试。Rust 测试继续放在独立的 `flamegraph_test.rs` 或 `migration_aster_unit_test.rs`，不要嵌入生产源文件。
- 若需要承受不可信但“结构合法”的超深栈，应考虑把 `add_locations` 与 `collect_child` 改成显式栈；同时用深链测试验证行序、缩进、depth 和资源上界。
- 任何 Rust 行为变更都应先以 `flamegraph.go` 和 `flamegraph_test.go` 为语义基线；如果有意分叉，文档和兼容风险必须明确记录。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件，其中 `pkg/util/profile/flamegraph.rs` 被识别为 309 行、24 个符号；`files --filter pkg/util/profile` 确认实现、模块入口及 Rust/Go 测试均在该目录。
- RustCodeGraph 源码与调用证据：`node --file pkg/util/profile/flamegraph.rs --offset 1 --limit 500` 覆盖全文件；精确 `explore` 给出 `ProfileReaderToDatums (profile.rs:121) → profile_to_datums (profile.rs:146) → new_flamegraph_collector (flamegraph.rs:162)`。单独的 `callers/callees` 查询在本地索引上未于 30 秒内返回，因此没有据此扩张结论，调用边又由同一索引的 `explore` 和源码交叉确认。
- 已读生产与配置：`pkg/util/profile/flamegraph.rs`、`pkg/util/profile/profile.rs`、`pkg/util/profile/lib.rs`、`pkg/util/profile/Cargo.toml`、`pkg/util/profile/flamegraph.go`、`pkg/util/profile/profile.go` 的直接入口引用。
- 已读测试：`pkg/util/profile/flamegraph_test.rs`、`pkg/util/profile/flamegraph_test.go`，以及 `pkg/util/profile/migration_aster_unit_test.rs` 中的 `protobuf_and_gzip_profiles_match_go_flamegraph_order_and_rows`、`fixture_and_small_percentages_match_go_formatting`、`malformed_profiles_are_rejected_like_go_check_valid`。
- 本任务只生成文档，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工核对本说明能够回答文件存在目的、运行链、数据不变量、失败边界和安全扩展位置。
