# `pkg/util/plancodec/binary_plan_decode.rs`

## 文件定位

本文件是 `astersql-util-plancodec` crate 中的二进制执行计划展示层：它接收由 `codec.rs` 定义的 base64 + Snappy 字符串，反序列化为 tipb `ExplainData`，再生成类似 `EXPLAIN ANALYZE` 的文本表格或连接侧所需的二维列值。模块由 `pkg/util/plancodec/lib.rs:85-90` 通过 `include!` 纳入并公开再导出；crate 边界和生成 `ExplainData` protobuf 的构建脚本由 `pkg/util/plancodec/Cargo.toml`、`build.rs` 与 `lib.rs:91-96` 确定。

当前 Rust 生产链中，`pkg/server/extract_runtime.rs:293-298` 的 `decode_binary_plan` 调用 `DecodeBinaryPlan`，用于生成计划提取包中的已解码计划。Go 对照还把两个公开入口接到 SQL 函数 `tidb_decode_binary_plan`、`EXPLAIN FOR CONNECTION` 和 TopSQL/内存告警路径；这些 Go 调用说明该移植文件所保持的接口语义，但不代表所有 Go 上游都已在 Rust 中接线。

## 核心职责

- `DecodeBinaryPlan`：完整展开主计划、CTE 和子查询，选择有/无运行时统计的表头，按 Unicode 字符数对齐列并返回带前导换行的文本表格（`binary_plan_decode.rs:29-80`）。
- `DecodeBinaryPlan4Connection`：展开主计划和 CTE，按 `brief`、`row`、`plan_tree`、`verbose` 以及 `forTopsql` 选择列，返回结构化行；它有意不展开子查询（`binary_plan_decode.rs:106-160`，测试见 `migration_aster_unit_test.rs:252-299`）。
- `decodeBinaryOperator`：以前序遍历把 `ExplainOperator` 树扁平化，生成树形 id、估算值、任务/存储位置、访问对象、运行时统计和算子信息，并保持 Go 的 Build-before-Probe 展示顺序（`binary_plan_decode.rs:209-291`）。
- 格式兼容辅助：处理 Go 风格非有限浮点拼写、`uint64` 到 `int64` 的回绕、负内存/磁盘值的 `N/A`、任务与存储枚举、算子标签及动态/扫描访问对象（`binary_plan_decode.rs:293-429`）。

该文件只负责解码与展示；编码、压缩格式和错误类型属于相邻 `codec.rs`，protobuf 类型由构建时生成，执行计划本身由上游规划与执行链产生。

## 主要符号

- `pub fn DecodeBinaryPlan(&str) -> Result<String, Error>`：面向完整文本显示的公开入口。压缩或 protobuf 失败时返回 `Error`；过长丢弃时返回 `PLAN_DISCARDED_DECODED`；无行时返回空字符串。
- `pub fn DecodeBinaryPlan4Connection(&str, &str, bool) -> Result<Option<Vec<Vec<String>>>, Error>`：面向连接/TopSQL 的公开入口。丢弃或无行返回 `Ok(None)`；已解码但格式未知时，每个输出行会因空列索引而成为空向量，而不是报格式错误。
- `pub static noRuntimeStatsTitleFields` / `fullTitleFields`：分别定义 6 列和 10 列的内部行布局契约；`calculateMaxFieldLens` 和连接侧列下标都依赖这个顺序。
- `fn writeRow`：用 `chars().count()` 计算显示填充，把字段写成 `| field | ... |`；要求字段数和 `rune_max_lengths` 长度一致。
- `fn calculateMaxFieldLens`：同时计算字符宽度和 UTF-8 字节长度；前者用于对齐，后者用于 `String` 容量预估。
- `fn decodeBinaryOperator`：核心递归函数。参数 `indent`/`isLastChild` 控制文本树，`hasRuntimeStats` 控制行宽，`isBrief` 控制名称和 operator info 的选择，`output` 累积前序遍历结果。
- `formatFloatFixed2`、`appendExecutionInfo`、`formatBytesOrUnavailable`：分别维护 Go 数值拼写、执行信息片段连接和资源统计展示兼容性。
- `taskTypeName`、`storeTypeName`、`printDriverSide`：把 tipb 枚举稳定映射为用户可见字符串。
- `printDynamicPartitionObject`、`printAccessObject`：格式化动态分区、表/分区/索引扫描及任意字符串访问对象。

文件没有 trait、struct、impl 或条件编译项；测试条件编译发生在 `lib.rs:98-110`，测试逻辑保存在独立文件中。

## 执行流程

`DecodeBinaryPlan` 的主流程如下：

1. 调用 `Decompress` 解码 base64 并 Snappy 解压，再由 `protobuf::parse_from_bytes` 构造 `tipb::ExplainData`。
2. 若 `discarded_due_to_too_long` 为真，直接返回与 Go 相同的丢弃哨兵文案。
3. 读取 `with_runtime_stats`，依次把 `main`、所有 `ctes`、所有 `subqueries` 交给 `decodeBinaryOperator`；每棵树都从空缩进、末子节点状态开始。
4. 若没有生成行则返回空字符串；否则计算各列最大字符/字节宽度，按运行时统计状态选择 6 列或 10 列表头。
5. 根据字节宽度预分配输出，以一个换行开头，随后用 `writeRow` 写表头和每个数据行。

`DecodeBinaryPlan4Connection` 复用相同的解压、解析和树展开逻辑，但只处理 `main` 与 `ctes`。`brief` 格式使递归函数优先使用 `brief_name` 和非空 `brief_operator_info`。之后按固定索引投影内部行：正常带运行时统计时可返回 8 至 10 列；TopSQL 或无运行时统计时返回 5 或 6 列。`plan_tree` 保留 cost、去掉 estRows，其他格式的具体索引见 `binary_plan_decode.rs:134-149`。

`decodeBinaryOperator` 对单个节点先生成自身行，再递归孩子，因此结果是前序遍历。它将首标签分别渲染为 Build/Probe 等后缀；当恰有两个孩子且顺序为 Probe、Build 时先交换引用，使 Build 子树优先展示。孩子缩进由 `texttree::Indent4Child` 继承，末孩子标志由当前下标决定。

## 数据与状态

输入状态完全来自不可变的 `tipb::ExplainData` / `ExplainOperator`：主树、CTE、子查询、运行时统计开关和访问对象均不会被本文件修改。树扁平化过程中唯一累积状态是按值传入并返回的 `Vec<Vec<String>>`；每个节点产生 6 列或 10 列，列序必须与两个表头静态切片一致。

资源容量有两级预估：单行预留 10 个字段，访问对象预留其元素数；完整文本先用每列最大 UTF-8 字节数估算容量。字符对齐则按 Rust Unicode scalar value 数量计算，与 Go 的 `[]rune` 计数语义对应，但并非终端“显示单元宽度”，因此宽字符或组合字符仍可能视觉不齐。

`root_basic_exec_info`、以 `", "` 连接的 `root_group_exec_info`、`cop_exec_info` 按非空顺序拼接。`memory_bytes`/`disk_bytes` 小于零表示不可用；非负值交给 `memory::FormatBytes`。`act_rows` 通过 `as i64` 保留 Go `int64(uint64)` 的二进制回绕行为。

## 依赖与调用关系

RustCodeGraph 索引状态为 11,467 个文件、307,296 个节点；目标文件被识别为 19 个符号。对 `DecodeBinaryPlan`、`DecodeBinaryPlan4Connection` 和 `decodeBinaryOperator` 的精确 `callers`/`callees` 查询没有产出边，因此以下直接引用由 `rg` 补证。

- 上游：Rust 生产代码 `pkg/server/extract_runtime.rs:297` 调用 `astersql_util_plancodec::DecodeBinaryPlan`；独立测试文件 `binary_plan_decode_test.rs` 和 `migration_aster_unit_test.rs` 调用两个公开入口。
- Go 上游对照：`pkg/expression/builtin_info.go:1476-1487` 实现 SQL 解码函数；`pkg/planner/core/common_plans.go:955-966` 为 `EXPLAIN FOR CONNECTION` 解码 brief plan；`pkg/util/memoryusagealarm/memoryusagealarm.go:297-307` 以 TopSQL 模式生成五列表格。
- 下游：`Decompress`、`Error`、`PLAN_DISCARDED_DECODED` 来自 crate 私有 `codec` 模块；`protobuf::parse_from_bytes` 与生成的 tipb 类型完成协议解析；`texttree::PrettyIdentifier` / `Indent4Child` 生成树形前缀；`memory::FormatBytes` 格式化资源值；`types::ExplainFormat*` 提供格式名。
- crate 依赖：`Cargo.toml` 声明 `protobuf` 及其 codegen、`texttree-dependency`、`base64`、`snap`、`thiserror`、`log`；其中本文件直接使用 protobuf、texttree，压缩和错误路径则通过 `codec.rs` 间接使用其余依赖。Cargo 未声明 feature，因此本文件没有 feature 分支。

## 错误处理与边界

解压失败和 protobuf 解析失败使用 `?` 原样转换/传播为 crate 的 `Error`。完整文本入口在计划被截断时给出稳定哨兵文案；连接入口用 `None` 同时表示“计划被截断”和“未生成行”，调用者不能仅凭返回值区分两者。Rust 生产调用 `extract_runtime.rs:297` 用 `unwrap_or_default()` 把错误降为空字符串；这是上游策略，不是本文件吞错。

内部边界包括：未知连接格式不会报错而产生空列行；连接入口不包含 `subqueries`；动态分区对象集合为空时立即返回空字符串，单对象时只显示分区、不显示表名；扫描对象会顺序附加所有索引；缺失 oneof 被忽略。`taskTypeName`、`storeTypeName` 和标签匹配穷尽当前生成枚举，协议新增枚举时需要同步编译期匹配分支。

`calculateMaxFieldLens` 索引 `rows[0]`，`writeRow` 和连接列投影也按固定列位索引；公开入口先排除空行，而每个递归节点严格生成与 `hasRuntimeStats` 一致的列数，从而维持这些内部前置条件。深度极大的恶意算子树会消耗递归栈；本文件没有独立的深度或输出大小限制，依赖上游编码阶段的长度控制和 protobuf 输入约束。

## 并发与资源生命周期

本文件没有全局可变状态、锁、通道、后台任务、文件句柄或事务。两个静态表头是只读切片；公开函数只使用调用栈和函数局部所有权容器，因此可被并发调用，调用之间没有共享生命周期。

解压后的 `Vec<u8>` 活到 protobuf 解析完成；解析得到的 `ExplainData` 持有树数据直至函数返回。递归时孩子以共享引用收集到临时 `Vec<&ExplainOperator>`，交换的只是引用顺序，不修改 protobuf 树。输出行拥有自己的 `String`，连接入口在投影时再次克隆选中字段；完整文本入口在写完后释放中间 rows。主要资源风险是计划规模带来的解压内存、行字符串、投影克隆和递归深度，而不是同步竞争。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/plancodec/binary_plan_decode.go`。Rust 保留了 Go 的三段完整解码流程、主树/CTE/子查询顺序、连接侧省略子查询、Build/Probe 重排、表头与列投影、访问对象文案和资源统计规则。

明确做过的兼容修正包括：`formatFloatFixed2` 输出 `+Inf`、`-Inf`、`NaN`，对应 `strconv.FormatFloat(..., 'f', 2, 64)`；`act_rows as i64` 对大于 `MaxInt64` 的 protobuf 值进行与 Go 转换相同的补码回绕。`binary_plan_decode_test.rs:7-48` 分别固定这两项行为。

Rust 的 `Option<Vec<Vec<String>>>` 显式表达 Go `nil, nil`，而正常的未知格式仍对应 Go 的非 nil 行集合、每行零列。访问对象的生成 Rust oneof 避免了 Go 指针包装的部分 nil 形态；Rust 对 `None` 静默跳过，而 Go 对某些带 nil 包装/消息的分支直接返回空字符串。现有迁移测试覆盖正常协议对象，未覆盖所有 Go nil-only 边界。

Go 集成测试 `pkg/planner/core/casetest/binaryplan/binary_plan_core_test.go:326-416` 将多类 `EXPLAIN ANALYZE FORMAT='verbose'`（含 join、递归 CTE、静态/动态分区）与 `tidb_decode_binary_plan` 结果逐字段比较。它是 Go 行为基线；本任务没有运行 Go 或 Cargo 测试。

## 扩展指南

- 新增或调整展示列时，必须同步修改 `decodeBinaryOperator` 的行顺序、两个表头、`DecodeBinaryPlan4Connection` 的所有列索引和容量假设；在独立的 `binary_plan_decode_test.rs` 或 `migration_aster_unit_test.rs` 增加测试，不能把测试嵌入生产文件。
- 支持新的 explain format 时，修改连接入口的投影匹配，并分别覆盖“有运行时统计”“无运行时统计/TopSQL”两条分支；需明确未知格式是否继续保持空列兼容行为。
- tipb 新增 `TaskType`、`StoreType`、`OperatorLabel` 或 `AccessObject` 变体时，更新对应格式函数并对照 Go 文案；协议生成代码仍应由 `build.rs`/protobuf 依赖维护，不在本文件复制生成定义。
- 改动树遍历时要保持主树、CTE、子查询的入口差异以及 Build-before-Probe 不变量；若加入深度保护或迭代遍历，需要验证文本树缩进和顺序完全一致。
- 调整错误语义时要同时审视 Rust 的 `extract_runtime.rs` 降级逻辑及 Go 的 SQL warning 行为，避免把“无计划”“被丢弃”“解码失败”无意合并或拆分。
- 性能修改应关注大计划的解压峰值、中间 `Vec<Vec<String>>`、连接投影克隆和递归栈，并保留 Unicode 字节容量与字符填充的双重计算语义。

## 验证依据

- RustCodeGraph：`status` 确认索引可用；`files --filter pkg/util/plancodec` 确认目标、Go 对照、模块入口和独立测试均已索引；`node --file pkg/util/plancodec/binary_plan_decode.rs --offset 1 --limit 500` 读取 429 行及 19 个符号；对三个关键函数执行了 `query`、`callers`、`callees`，图未返回调用边，随后用精确文本引用补证。
- 源与边界：`pkg/util/plancodec/binary_plan_decode.rs`、`pkg/util/plancodec/lib.rs`、`pkg/util/plancodec/Cargo.toml`。
- Go 对照与上游：`pkg/util/plancodec/binary_plan_decode.go`、`pkg/expression/builtin_info.go:1463-1487`、`pkg/planner/core/common_plans.go:955-966`、`pkg/util/memoryusagealarm/memoryusagealarm.go:297-307`。
- Rust 调用与测试：`pkg/server/extract_runtime.rs:293-298`、`pkg/util/plancodec/binary_plan_decode_test.rs`、`pkg/util/plancodec/migration_aster_unit_test.rs:183-299`。
- Go 行为测试：`pkg/planner/core/casetest/binaryplan/binary_plan_core_test.go:326-416`。
- 人工复核结论：本文说明了文件存在原因、两个入口的运行路径、内部不变量、已接线范围、Go 对照差异和安全扩展点；未把 Go 上游误写为已完成的 Rust 接线，也未将测试逻辑建议放回生产源文件。
