# `pkg/planner/util/fixcontrol/get.rs`

## 文件定位

本文件属于 `astersql-planner-util-fixcontrol` crate。crate 根 `pkg/planner/util/fixcontrol/lib.rs` 以 `pub mod get` 声明模块，并通过 `pub use get::*` 将本文件的编号常量和读取函数提升到 crate 根，因此调用方使用 `fixcontrol::Fix44855`、`fixcontrol::GetBoolWithDefault` 这类路径，而不需要显式经过 `get` 模块。

`pkg/planner/util/fixcontrol/Cargo.toml` 将库入口设为 `lib.rs`、关闭自动测试发现和 doctest，并用 `package.metadata.porting.go-package` 指向 Go 包 `pkg/planner/util/fixcontrol`。本文件本身只依赖标准库的 `HashMap` 和 `IntErrorKind`；crate 清单中的 `anyhow` 供同 crate 的其他实现使用，不是本文件的直接依赖。

## 核心职责

本文件承担两类职责：

1. 定义以 TiDB issue 编号为值的 17 个 `u64` fix-control 常量，供优化器各处以稳定键名读取会话级兼容开关。
2. 把 `HashMap<u64, String>` 中的原始字符串按字符串、布尔、`i64` 或 `f64` 语义读取，并同时表达“键是否存在”“解析是否成功”以及缺失或失败时的默认值回退。

它不负责解析 `tidb_opt_fix_control` 的整段配置字符串；该职责在同 crate 的 `set.rs::ParseToMap`。生产调用通常先取得或解析会话变量映射，再调用本文件的类型化 getter。当前 Rust 生产代码的直接文本引用见 `pkg/planner/core/operator/physicalop/index_join_probe.rs` 和 `pkg/util/ranger/detacher.rs`。

## 主要符号

- 编号常量：`Fix52592`、`Fix33031`、`Fix43817`、`Fix44262`、`Fix44389`、`Fix44830`、`Fix44823`、`Fix44855`、`Fix45132`、`Fix45822`、`Fix45798`、`Fix46177`、`Fix47400`、`Fix49736`、`Fix52869`、`Fix54337`、`Fix56318`。常量值就是对应 issue 编号；其中 `Fix47400` 已在源码中标为废弃，`Fix49736` 标为测试专用。
- `GetStr(map, key) -> (String, bool)`：返回克隆后的原始字符串及存在标志；映射为 `None` 或键缺失时返回空字符串和 `false`。
- `GetStrWithDefault(map, key, default) -> String`：只在键不存在时采用默认字符串；键存在但值为空字符串时仍返回空字符串。
- `GetBool(map, key) -> (bool, bool)`：仅大小写不敏感的 `ON` 或精确字符串 `1` 为 `true`；其他已存在值均为 `false` 且存在标志仍为 `true`。
- `GetBoolWithDefault(map, key, default) -> bool`：只在键不存在时采用默认布尔值，无法识别的已存在字符串不会采用默认值。
- `GetInt(map, key) -> (i64, bool, Result<(), String>)`：按十进制有符号 64 位整数解析；存在标志与解析结果相互独立。
- `GetIntWithDefault(map, key, default) -> i64`：键缺失或整数解析失败时采用默认值。
- `GetFloat(map, key) -> (f64, bool, Result<(), String>)`：按 `f64` 解析，并额外把数值溢出得到的无穷值转换成范围错误，同时保留溢出后的无穷值。
- `is_go_infinity(value) -> bool`：私有辅助函数，识别可带正负号且大小写不敏感的 `inf`/`infinity` 字面量，避免把调用方明确输入的无穷字面量误判为数值溢出。
- `GetFloatWithDefault(map, key, default) -> f64`：键缺失或浮点解析失败时采用默认值。

所有公开 getter 的映射参数都是 `impl Into<Option<&HashMap<u64, String>>>`：调用方既可传 `&HashMap`，也可显式传 `Some(&map)` 或 `None`，从而对应 Go 的非空 map 和 nil map。

## 执行流程

类型化读取遵循一致的两阶段流程：先判定映射和键是否存在，再解释已取得的字符串。

1. `GetStr` 先把参数转成 `Option<&HashMap<...>>`；`None` 或缺键返回 `("", false)`，命中时克隆值并返回 `(value, true)`。
2. `GetBool` 使用相同的存在性检查，命中后执行 `eq_ignore_ascii_case("ON") || rawValue == "1"`。getter 本身不裁剪空白。
3. `GetInt` 命中后调用 `parse::<i64>()`。成功时返回值、`true` 和 `Ok(())`；失败时仍返回 `exists = true`。正/负溢出分别返回 `i64::MAX`/`i64::MIN`，其他解析错误返回 `0`，并把错误转成字符串。
4. `GetFloat` 命中后调用 `parse::<f64>()`。语法错误返回 `0.0` 与错误；有限数值或明确的 Go 无穷字面量正常返回；超大指数等导致的非字面量无穷值保留 `±INFINITY`，同时返回范围错误。
5. 四个 `WithDefault` 包装函数调用对应基础 getter。字符串和布尔仅按 `exists` 决定回退；整数和浮点同时按 `exists` 与解析结果决定回退。

在实际规划链中，`pkg/planner/core/operator/physicalop/index_join_probe.rs::access_rows_floor` 读取 `Fix44855`，默认 `true`，决定是否用已使用连接键前缀的 NDV 抬高 IndexJoin 内侧访问行数下界；同文件构造 probe 候选时再次读取该键、默认 `false`，决定是否应用另一项独立的 NDV 上界。`pkg/util/ranger/detacher.rs` 读取 `Fix54337` 决定是否尝试 CNF range 的 subset/intersection，并读取 `Fix44389` 决定是否采用特定非 point CNF range。

## 数据与状态

核心输入状态是调用方持有的 `HashMap<u64, String>`：键是 fix-control 编号，值保留会话变量中的文本表示。本文件只借用映射且不修改它；除 `GetStr` 命中时克隆返回字符串、错误路径构造错误字符串、默认字符串按需转换外，其余 getter 返回标量。

三类状态必须区分：键缺失、键存在且解析成功、键存在但解析失败。例如 `GetInt` 的三元组允许调用方在解析失败时同时看到 `exists = true` 和 Go 兼容的饱和值；`GetIntWithDefault` 则有意折叠后两种失败信息，只返回默认值。

常量自身不保存开关状态。实际会话状态由上游 `OptimizerFixControl` 映射承载；例如 ranger 直接借用上下文中的映射，IndexJoin 当前通过本地 `fix_map` 从系统变量字符串重新调用 `ParseToMap` 得到临时映射。

## 依赖与调用关系

- 上游装配：`pkg/planner/util/fixcontrol/lib.rs` 声明并重导出本模块；`Cargo.toml` 定义 crate 边界。工作区及 `sessionctx/variable`、`planner/indexadvisor`、`executor`、`planner/core/operator/physicalop`、`planner/core/rule`、`util/ranger` 的 Cargo 清单声明了该 crate 或其 facade/path 依赖。
- 当前 Rust 生产调用：`index_join_probe.rs` 使用 `GetBoolWithDefault`/`Fix44855`；`detacher.rs` 使用 `GetBoolWithDefault`/`Fix54337`/`Fix44389`。仓库文本检索未发现其他非测试 Rust 文件直接调用本文件 API。
- 下游标准库：`HashMap::get` 提供只读键查找；`str::parse::<i64/f64>` 执行数值解析；`IntErrorKind` 用于把整数正负溢出映射到 Go `strconv.ParseInt` 的饱和值行为。
- 内部调用：四个 `WithDefault` 分别调用相应基础 getter；`GetFloat` 调用私有 `is_go_infinity`。其余基础 getter 彼此独立。
- 测试接线：`lib.rs` 在 `cfg(test)` 下单独挂载 `get_test.rs` 和 `fixcontrol_test.rs`，符合测试逻辑不内嵌生产文件的仓库约定。

RustCodeGraph 的文件节点把 `get.rs` 标为被 `index_join_probe.rs` 及其测试引用，但函数 ID `GetBool` 的 callers 查询返回空数组；因此上述函数级生产调用以文件节点和精确文本引用交叉确认，不把空调用边解释为“没有调用者”。

## 错误处理与边界

- `None` 映射和缺失键都不是错误：基础 getter 返回类型零值、`exists = false`，数值 getter 同时返回 `Ok(())`。
- 空字符串若作为已存在的值，`GetStr` 认为读取成功；布尔读取为 `false`；数值读取返回解析错误。默认包装器因此会对数值回退，但不会对字符串或布尔回退。
- 布尔语义严格对齐 Go `TiDBOptOn`：`ON` 不区分 ASCII 大小写，`1` 必须精确匹配；`true`、`yes`、带空白的 ` ON ` 等已存在值均解释为 `false`。
- 整数解析限制为十进制 `i64`。溢出返回对应符号的边界值并保留错误；其他非法文本返回 `0` 和错误。调用基础 getter 的代码不能仅检查数值，必须同时检查第三个返回项。
- 浮点解析区分显式无穷字面量与计算溢出：前者可成功，后者返回无穷值和错误。`get_test.rs::TestGetFloatPreservesGoRangeError` 固定了 `±1e400` 的这一行为。
- 本文件不验证编号是否属于已声明常量；任意 `u64` 键都可读取。它也不记录 warning、相关 fix 使用情况或解析上下文，这些职责留给调用方及 `set.rs`。

## 并发与资源生命周期

本文件没有全局可变状态、锁、线程、异步任务、通道、文件句柄或网络资源。所有函数都是同步只读操作；共享映射的并发策略由所有者决定，只要调用方能提供合法的共享引用，getter 不会引入额外同步需求。

映射借用只持续到函数返回。`GetStr` 返回拥有所有权的克隆字符串，因此结果不绑定映射生命周期；数值与布尔结果是复制标量。`GetBoolWithDefault(Some(&fix_map(ctx)), ...)` 中临时 `HashMap` 的引用仅在单次调用期间使用，返回后即可释放。

性能上，每次基础查找平均为一次哈希查询；数值读取额外承担与字符串长度线性相关的解析成本。`GetStr` 的克隆成本与值长度线性相关。当前 IndexJoin 的 `fix_map` 会重新解析整段会话变量，成本来自调用方而非 getter 本身。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/util/fixcontrol/get.go`。17 个常量的名称和值一一对应，8 个公开 getter 的分层、存在标志和默认回退条件也保持一致。Rust 的 `Option<&HashMap<...>>` 显式表达 Go 的 nil map；Rust 的 `Result<(), String>` 表达 Go 数值 getter 的 `error`，同时把解析得到的数值作为独立元组元素保留。

需要注意的实现差异：Go 使用 `strconv.ParseInt(..., 10, 64)`，Rust 通过 `IntErrorKind` 显式恢复 Go 在范围错误时返回 `MaxInt64`/`MinInt64` 的行为；Go 使用 `strconv.ParseFloat(..., 64)`，Rust 额外检查“非显式无穷字面量却解析为无穷”的情况，以恢复 Go 的范围错误。对应证据在 `get_test.rs` 的两个回归测试中。

Go 的 `fixcontrol_test.go::TestFixControl` 通过 SQL 会话和记录数据验证整条集成链；Rust 的 `fixcontrol_test.rs::TestFixControl` 以独立的解析与类型化读取测试回放成功数据和默认值语义，但不启动 SQL session。该差异说明当前 Rust 测试覆盖的是 crate 逻辑对齐，而不是 Go 测试的完整会话集成环境。

## 扩展指南

- 新增 fix-control 时，在本文件增加与 Go `get.go` 同名同值的常量，并同步用途、默认值及废弃/测试专用属性；不要复用无关 issue 编号。
- 新增读取类型时，延续“基础 getter 保留存在/解析信息，`WithDefault` 明确折叠失败”的两层 API，并先定义 nil/缺键、空值、非法文本、范围溢出和默认回退语义。
- 修改布尔或数值解析时，必须逐项核对 Go 的 `TiDBOptOn`、`strconv.ParseInt`、`strconv.ParseFloat`，尤其不能用“解析失败即零值”替代 Go 同时返回部分数值与错误的契约。
- 生产接线应在实际消费点选择默认值，并在优化器需要追踪时像 `index_join_probe.rs` 一样调用 `RecordRelevantOptFix`；getter 本身不应承担业务默认值或使用记录。
- 测试应继续放在独立文件：通用读取矩阵扩展 `pkg/planner/util/fixcontrol/fixcontrol_test.rs`，Go 兼容的解析边界扩展 `pkg/planner/util/fixcontrol/get_test.rs`；涉及具体优化行为时还应同步相应消费模块的独立测试，例如 `index_join_probe_test.rs` 或 ranger 测试。
- 兼容风险主要是改变已存在值与缺失值的区分、默认值触发条件或 Go 的范围错误返回值；性能风险主要来自在热点中重复解析整张映射或不必要地克隆长字符串，而不是单次标量 getter。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 7,032 个 Rust 文件；`files --filter pkg/planner/util/fixcontrol` 确认目标源、Go 对照、crate 入口和独立测试均已索引。
- RustCodeGraph `node --file pkg/planner/util/fixcontrol/get.rs`：核对 216 行源码、17 个常量、8 个公开 getter、私有 `is_go_infinity` 及文件级使用者。
- RustCodeGraph 文件节点：读取 `get.go`、`lib.rs`、`get_test.rs`、`fixcontrol_test.rs`、`index_join_probe.rs` 和 `pkg/util/ranger/detacher.rs`，核对 Go 语义、模块重导出、测试边界和生产消费流程。
- RustCodeGraph 精确查询：`query` 找到 `get.rs::GetBool`、`GetFloat`、`GetFloatWithDefault`、`is_go_infinity` 等符号；按函数 ID查询 `GetBool` callers 得到空数组，已通过文件节点与 `rg` 精确引用补证该索引限制。批量按名称查询曾无输出并被中止，未作为结论依据。
- 配置与文本核验：读取 `pkg/planner/util/fixcontrol/Cargo.toml`；检索各 Cargo 清单中的 `astersql-planner-util-fixcontrol` 依赖；检索非测试 Rust 源中的 `fixcontrol::Get*`/`Fix*` 直接引用。
- 未运行 Cargo 或代码测试：本任务只新增文档，按任务约束以事实复核和固定章节结构检查作为验证。
