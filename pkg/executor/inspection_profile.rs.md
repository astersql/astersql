# `pkg/executor/inspection_profile.rs`

## 文件定位

该文件属于 `astersql-executor` crate。crate 根在 `pkg/executor/lib.rs`，其中以 `pub mod inspection_profile` 公开本模块；`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 证明其 crate 边界。本文件不是 CPU pprof 实现，而是把 `metrics_schema` 中一段时间窗内的指标聚合成 Graphviz DOT 文本，用于展示 TiDB 查询链和 GC 链各阶段的耗时或次数占比（`profileBuilder::Collect`、`profileBuilder::Build`）。

RustCodeGraph 将该文件识别为 932 行的 Rust 源文件，并列出 `pkg/executor/compiler.rs`、`pkg/executor/inspection_profile_test.rs` 和 `pkg/executor/statement_ru_result_test.rs` 三个“used by”文件；但对仓库 Rust 源码做精确符号搜索后，只有 `inspection_profile_test.rs` 实际调用 `NewProfileBuilder`。因此当前可确认的事实是：模块已公开且逻辑完整，Rust 生产调用接线尚未在仓库中找到。在线 HTTP 入口仍可在 Go 路径 `pkg/server/handler/tikvhandler/tikv_handler.go:1979-1989` 看到，它调用的是 Go 版 executor 实现。

## 核心职责

本文件承担四层职责：

1. 用 `ProfileDataSource` 抽象指标 SQL 执行、指标注释读取和时间格式化，使核心逻辑不直接依赖具体 session/executor。
2. 用 `metricNode`、`metricValue` 和固定树构造函数表达查询、事务、DDL、TiKV gRPC、Raft 与 GC 指标之间的层次关系。
3. 在 `metricNode::initializeMetricValue` 中按时间窗、标签和可选条件查询 count、sum、P99/P90/P80，并懒加载到节点缓存。
4. 在 `profileBuilder` 中计算相对权重、裁剪低占比节点、生成节点/边样式，最终输出 DOT 字节。

它只负责读取和可视化指标，不执行用户 SQL 计划、不改变数据库状态，也不负责把 DOT 渲染成图片。

## 主要符号

- `ProfileError(String)` / `ProfileResult<T>`：模块的字符串错误包装与统一返回类型。`ProfileDataSource` 的三个操作及所有可失败的采集流程都通过它传播错误。
- `MetricQueryRow { value, labels }`：数据源返回的归一化行；第一列语义是指标数值，其余列已转换成标签字符串。
- `ProfileDataSource: Send + Sync`：外部适配边界。`execute_metric_sql` 执行生成的 metrics SQL，`metric_comment` 返回指标说明，`format_metric_time` 负责将 `SystemTime` 变成 SQL/标题使用的文本。
- `metricValueType::{Sum, Avg, Count}`：展示口径。`metricValueType::String` 返回与 Go API 参数一致的 `sum`、`avg`、`count`。
- `metricValue`：保存总和、次数、三个分位均值和注释。`getValue` 根据展示口径选择值并把 NaN 归零；`getComment` 生成 tooltip。
- `metricNode`：一项指标及其标签、条件、单位、子树、缓存状态。内部节点以 `NodeRef = Rc<RefCell<metricNode>>` 共享。
- `NewProfileBuilder`：公开构造入口；类型参数大小写不敏感，空串兼容为 `sum`，其他值返回 `ProfileError`。
- `profileBuilder`：持有时间窗、展示类型、名称到 DOT ID 的映射、去重集合、总值、输出缓冲和 `Arc<dyn ProfileDataSource>`。公开的主要生命周期方法是 `Collect` 与 `Build`。
- `metric_query`、`set_quantile_value`、`node_with_unit`、`node_with_condition`、`labeled_partial_node`、`tikv_grpc_tree`：内部 SQL 和树构造辅助函数。
- `format_system_time_difference`、`format_go_duration*`、`civil_from_days`：为保持 Go 输出语义而提供的时间差和 duration 格式化辅助函数。

文件没有条件编译项；测试模块由 `pkg/executor/lib.rs` 上的 `#[cfg(test)] mod inspection_profile_test` 独立装配，测试逻辑没有内嵌在生产文件中。

## 执行流程

典型流程是 `NewProfileBuilder(data_source, start, end, type) → Collect() → Build()`：

1. `NewProfileBuilder` 校验并保存展示类型，初始化 DOT ID 分配器（从 1 开始）、去重集合和缓冲区。
2. `Collect` 调用 `genTiDBQueryTree` 构造以 `tidb_query` 为根的固定树，主干为 parse/compile/execute，execute 下再展开 TSO、auto-ID、cop、txn、DDL，KV 请求继续连到 PD、TiKV gRPC、scheduler、storage 和 raft。
3. `init` 先写 DOT 头和包含类型、开始时间、时间跨度的标题节点，再由 `GetTotalValue` 确定占比分母。Sum/Count 使用根值；Avg 使用 `GetMaxNodeValue` 递归寻找树内最大值。零值分母被替换为 1，避免除零。
4. `traversal` 深度优先遍历。节点首次取值时，`metricNode::getValue` 调用 `initializeMetricValue`；后续读取复用缓存。
5. `initializeMetricValue` 依次查询 `<table>_total_count`、`<table>_total_time` 和 `<table>_duration` 的 0.99/0.90/0.80 分位。查询总是限制时间窗、非 NULL 且正值，并叠加节点自定义条件；有标签时按标签分组。
6. 节点总次数为零时提前返回，不再查询耗时、分位或注释。否则聚合总值和标签值，并通过 `metric_comment` 填充 tooltip 文本。
7. `traversal` 先画父子边，再以“节点总值减去非 `is_part_of_parent` 子节点值”计算 self cost。标签节点被拆成独立 DOT 节点；占总量不足 0.01% 的值由 `ignoreFraction` 裁剪。
8. 查询树之后，`Collect` 再遍历 `genTiDBGCTree` 创建的 GC 子树。`unique_map` 按显示名去重，因此两个树中同名节点不会重复输出。
9. `Build` 追加右花括号并返回当前缓冲区字节。它不重置状态；同一 builder 重复调用 `Collect` 或 `Build` 会继续修改已有缓冲，不应视为可重复执行 API。

## 数据与状态

`metricNode` 的配置状态包括指标表前缀 `table`、可覆盖的 DOT 名称 `name`、标签列、附加 SQL 条件、单位因子、子节点和 `is_part_of_parent`。运行时状态包括 `value`、按拼接标签键排序保存的 `label_value` 以及 `initialized`。`BTreeMap`/`BTreeSet` 使标签、ID 映射与去重遍历具有确定的键顺序；树的子节点顺序则由构造代码中的 `Vec` 决定。

节点由 `Rc<RefCell<_>>` 管理，以便 `tidb_kv_request` 等子树被多个父路径共享并在懒加载时原地更新。builder 的 `id_map` 保证相同显示名获得同一个 DOT 数字 ID，`unique_map` 保证该显示名只被 DFS 展开一次。名称是去重键，因此新增两个同名但条件或语义不同的节点时，必须像 snapshot/write 节点那样设置不同 `name`。

单位换算由 `metricNode::unit` 控制。当前 `tidb_get_token` 使用 1,000,000，`tidb_batch_client_wait` 使用 1,000,000,000。实现与 Go 版一致：`queryRowsByLabel` 先除一次 unit，sum/quantile 聚合回调中又除一次；这是现有兼容行为，修改前必须用 Go 对照与真实指标单位确认，不能只按直觉“修正”。

## 依赖与调用关系

上游边界如下：

- Rust crate 入口：`pkg/executor/lib.rs:128` 公开模块，`lib.rs:130` 仅在测试配置下装配独立测试。
- Rust 已确认调用者：`pkg/executor/inspection_profile_test.rs` 构造 `EmptyDataSource`，调用 `NewProfileBuilder` 和 `Collect`。仓库精确搜索没有发现其他 Rust 代码调用该构造函数，故不能宣称 Rust server 已使用本模块。
- Go 生产对照入口：`pkg/server/handler/tikvhandler/tikv_handler.go:1979-1989` 解析 HTTP 参数后调用 Go 版 `executor.NewProfileBuilder`、`Collect`、`Build` 并写响应。这证明功能在完整应用中的业务位置，但不是 Rust 调用边。

下游边界集中在 `ProfileDataSource`：所有 SQL、指标注释和时间文本都由注入实现提供。本文件自身仅依赖 Rust 标准库（`Rc`、`RefCell`、`Arc`、有序集合、时间与格式化）；`pkg/executor/Cargo.toml` 没有为本文件声明专属 feature，唯一 crate feature `nextgen` 与该模块无直接条件关系。

内部关键调用边为：`Collect → genTiDBQueryTree/init/traversal/genTiDBGCTree`；`init → GetTotalValue → metricNode::getValue`；`traversal → getValue/addNodeEdge/addNode/traversal`；`getValue → initializeMetricValue → metric_query/queryRowsByLabel/ProfileDataSource`；DOT 输出则由 `addNode/addNodeEdge → addNodeDef/addEdge → getNameID/dotColor/formatValueByTp` 完成。

## 错误处理与边界

- 不支持的展示类型在 `NewProfileBuilder` 立即失败；空串明确回退为 Sum，以兼容 Go 的旧行为。
- `ProfileDataSource` 的 SQL、注释或时间格式化错误通过 `?` 原样提升为 `ProfileError`，`Collect` 随即停止，缓冲区可能保留部分 DOT 内容；没有回滚或自动重试。
- 带标签的查询若返回空标签，会被 `queryRowsByLabel` 丢弃。数值按 `as i64` 截断为次数，继承了 Go `int(v)` 的离散化语义。
- count 汇总为零时节点提前结束；分位查询没有行时会计算 `0/0 = NaN`，`metricValue::getValue` 会将其显示值归零，但 tooltip 格式化可能保留非有限值文本。这是当前实现边界，不应写成已做完整空结果清洗。
- `GetMaxNodeValue` 没有接受空节点；Rust 类型要求调用方传入 `&NodeRef`，与 Go 版允许 nil 并返回零不同。
- DOT 文本通过字符串插值生成，节点名、标签、注释没有单独执行 DOT 转义；数据源或标签若含引号、反斜杠或换行，可能影响输出语法或展示。扩展外部输入前需补转义与回归测试。
- `ignoreFraction` 固定裁剪低于 0.01% 的节点/边。负 self cost 不会被该函数直接裁剪（它接收节点总值），而颜色函数允许负权重并映射到绿色方向。
- `Build` 每调用一次都会追加 `}`；调用顺序和次数是隐含前置条件。

## 并发与资源生命周期

`ProfileDataSource` 要求 `Send + Sync`，并由 `Arc` 持有，允许适配器在外部安全共享；但 `profileBuilder` 内部含 `Rc<RefCell<metricNode>>` 构成的临时树，因此 builder 本身不是为跨线程共享设计的。采集过程全部同步串行执行，没有 spawn、异步任务、通道、锁或后台生命周期。

每次 `Collect` 创建查询树和 GC 树；树在调用结束后随局部 `Rc` 引用释放。树没有父指针，当前结构不会形成 `Rc` 引用环。指标 SQL 结果被聚合进节点，原始行在每次 `queryRowsByLabel` 返回后释放。builder 保留 DOT 缓冲、ID 和去重集合直到自身析构；同一实例不应并发调用或复用来生成第二份独立报告。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/inspection_profile.go`。Rust 基本保留了 Go 的类型和方法分层、固定指标树、查询语句、0.01% 裁剪、pprof 风格配色、空类型回退、共享 KV 子树和 DOT 格式，方法名也刻意保留 Go 风格以方便逐项核对。

主要适配差异为：

- Go builder 直接持有 `sessionctx.Context`，通过 restricted SQL executor 查询，并从 `infoschema.MetricTableMap` 取注释；Rust 把三项能力抽象为 `ProfileDataSource`，因此真正接入 session 的适配器不在本文件中，仓库中也未找到生产实现。
- Go 用指针、map 和 bytes.Buffer；Rust 用 `Rc<RefCell<_>>`、`BTreeMap/BTreeSet` 和 `String`。有序集合提高了输出确定性，但共享可变借用违规会在运行时 panic，当前调用顺序避免重叠借用。
- Go 用 `time.Time` 和 `time.Duration.String()`；Rust 用 `SystemTime` 及本地格式化函数模拟 Go duration。独立 Rust 测试专门覆盖零时长、毫秒精度和负时间差。
- Rust `ProfileError` 只保存字符串，没有 Go 错误链或具体错误类型；适配器若需保留根因，必须在消息中加入上下文或扩展错误模型。

Go 测试未发现同名 `inspection_profile_test.go`；当前直接回归证据来自 Rust 独立测试和 Go 生产实现/调用入口的源码对照，而不是 Go 专项单元测试。

## 扩展指南

- 新增指标节点：优先修改 `genTiDBQueryTree`、`genTiDBGCTree` 或 `tikv_grpc_tree`；明确 table、标签、条件、单位和 `is_part_of_parent`，并检查名称是否会被 `unique_map` 合并。同步在 `pkg/executor/inspection_profile_test.rs` 增加独立测试，不要把测试写回生产源文件。
- 改变查询或聚合：修改 `metric_query`、`initializeMetricValue` 或 `queryRowsByLabel` 时，逐项对照 Go 的 count/sum/quantile 顺序、标签拼接、正值过滤和单位换算。应使用可记录 SQL 与返回行的 fake `ProfileDataSource` 覆盖无行、空标签、多标签、SQL 错误、注释错误、unit 和 NaN 边界。
- 接入 Rust server：需要在调用侧实现真实 `ProfileDataSource`，并明确时间格式、restricted/internal SQL 上下文、权限和取消语义；这属于本文件之外的接线，不能仅凭公开模块推断已经存在。
- 改变 DOT 输出：重点同步 `init`、`traversal`、`addNode*`、`addEdge`、`dotColor` 和 `formatValueByTp`，补充完整字符串或可解析 DOT 的回归测试。若标签/注释来源扩大，先实现统一 DOT 转义。
- 改变时间语义：同步 `format_system_time_difference` 与 `format_go_duration_nanos` 的 Go 兼容测试，尤其覆盖负值、零、纳秒/微秒/毫秒边界和超过一分钟/一小时的组合格式。
- 性能注意：一项非空节点最多发起 count、sum 和三次 quantile 查询；扩大树会线性增加同步 SQL 次数。新增节点前应评估 HTTP 请求延迟和 metrics_schema 压力，必要时在保持结果语义的前提下讨论批量查询，而不是悄然改变采样口径。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/executor/inspection_profile.rs --offset 1 --limit 500` 与 `--offset 501 --limit 500` 覆盖源码 1-932 行，并报告三个文件级使用方；`query NewProfileBuilder --kind function` 同时定位 Go `:300` 与 Rust `:329` 定义。`callers/callees` 的组合查询在 30 秒内没有返回结果，因此调用关系又以精确仓库搜索核验，没有将超时当作“无调用者”的单独证据。
- Rust 源与装配：`pkg/executor/inspection_profile.rs`、`pkg/executor/lib.rs:125-130`。
- crate 边界：`pkg/executor/Cargo.toml` 的 package、lib、features 和依赖声明。
- Rust 独立测试：`pkg/executor/inspection_profile_test.rs`，覆盖 Go 风格零 duration、毫秒精度和结束时间早于开始时间时的负 duration 标题。
- Go 语义对照：`pkg/executor/inspection_profile.go`；生产入口证据：`pkg/server/handler/tikvhandler/tikv_handler.go:1969-1989`。
- 仓库精确搜索：`rg "NewProfileBuilder\\(" --glob '*.go' --glob '*.rs'` 只找到两份定义、Rust 测试调用和 Go HTTP handler 调用；据此将 Rust 生产接线标为“未找到”，而非推测其已启用。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节的结构命令和人工事实复核验收。
