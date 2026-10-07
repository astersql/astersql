# `pkg/executor/load_stats.rs`

## 文件定位

本文件是 `astersql-executor` crate 中 `LOAD STATS` 的 Rust 侧核心模型，模块由 [`pkg/executor/lib.rs`](lib.rs) 的 `pub mod load_stats` 对外导出。它把“执行器登记一个待上传的统计文件”和“取得文件字节后解码并写入统计系统”拆成两个阶段，并以 `LoadStatsRuntime` trait 隔离会话值存储、JSON 解码、Domain/StatsHandle 查询及 InfoSchema 选择等基础设施。

当前接线状态需要特别区分：本文件已经实现 `LoadStatsExec::{Open,Next,Close}` 与 `LoadStatsInfo::Update` 的分支逻辑，但仓库搜索没有发现 `LoadStatsRuntime` 的实现、Rust 侧 `LoadStatsExec`/`LoadStatsInfo` 的构造点或直接调用测试。Rust [`pkg/executor/builder.rs`](builder.rs) 的 `buildLoadStats` 只通过 `build_leaf(ExecutorKind::LoadStats, plan)` 构造通用叶执行器，并未实例化这里的类型。因此这里是已导出的、可供适配器接入的移植核心，不应描述为当前 Rust 主执行链已经完整连通。完整生产语义可由同路径 Go 实现 [`pkg/executor/load_stats.go`](load_stats.go) 及其构建器、文件传输处理器核对。

## 核心职责

1. `LoadStatsExec::Next` 先重置结果 chunk，再验证路径，检测并清理遗留的会话加载选项，最后安装本次 `LoadStatsOption`。该执行器不直接读文件，也不在 `Next` 内解析统计 JSON。
2. `LoadStatsInfo::Update` 接收已经读入内存的文件字节，通过运行时适配器解码；兼容 JSON `null` 对应的空统计对象；检查统计句柄存在后提交加载。
3. `LoadStatsRuntime` 定义本文件所需的最小运行时能力，使核心流程不直接依赖具体 session、domain、InfoSchema 或统计 JSON 类型。
4. `LoadStatsVarKeyType`、`LOAD_STATS_VAR_KEY` 和 `load_stats_var_key_type_string` 保留 Go 会话变量键的概念与调试名称，但本文件本身不提供会话 map，也不注册服务器文件传输回调。

## 主要符号

- `LoadStatsResult<T = ()> = Result<T, errors::SharedError>`：本模块统一错误返回类型；显式错误由 `astersql_errors::New` 创建，适配器错误原样向上传播。
- `LoadStatsContext(Arc<dyn Any + Send + Sync>)`：传给 `Open`/`Next` 的不透明上下文。`Default` 只放入 `()`；当前实现不读取其中内容。`Clone` 仅克隆 `Arc`。
- `LoadStatsOption { path: String }`：安装到运行时的待加载选项。`Next` 会克隆 `LoadStatsInfo.path`，所以安装后的路径不借用执行器。
- `DecodedStats { table_name, version, value }`：运行时解码后的载荷。核心逻辑只查看 `table_name` 和 `version`；具体统计对象由 `value: Box<dyn Any + Send>` 保持类型擦除并整体交还运行时。
- `LoadStatsRuntime`：七个必需方法分别覆盖 chunk 容量、遗留选项检测/清理/安装、JSON 解码、StatsHandle 存在性检查和最终加载。trait 方法采用 Go 风格名称，文件级 `#![allow(non_snake_case)]` 明确允许这种命名。
- `LoadStatsExec { info }`：执行器外壳。`Open` 和 `Close` 当前均为无操作成功；`Next` 是登记阶段的唯一行为入口。
- `LoadStatsInfo { path, runtime }`：同时持有文件路径和独占的 boxed 运行时适配器，`Update(&mut self, data)` 是消费文件内容后的处理入口。
- `LoadStatsVarKeyType = i32`、`LOAD_STATS_VAR_KEY = 0`：Go `loadStatsVarKeyType`/`LoadStatsVarKey` 的名义对应物；由于 Rust 使用类型别名而非新类型，它不提供 Go 那样的类型级键隔离。

## 执行流程

`LoadStatsExec::Next` 的顺序具有可观察意义：

1. 无论路径是否合法，先调用 `runtime.MaxChunkSize()`，再执行 `req.GrowAndReset(...)`，清除上一批结果并确保容量。
2. 若 `info.path.is_empty()`，返回 `"Load Stats: file path is empty"`，不安装选项。
3. 若 `runtime.HasLoadStatsOption()` 为真，先调用 `ClearLoadStatsOption()` 清除陈旧状态，再返回 `"Load Stats: previous load stats option isn't closed normally"`。本次选项不会安装。
4. 正常路径克隆字符串，构造 `LoadStatsOption` 并调用 `InstallLoadStatsOption`，随后返回成功。文件读取与 `Update` 由上层后续阶段触发。

`LoadStatsInfo::Update` 的顺序如下：

1. 调用 `DecodeStatsJSON(data)`；解码失败通过 `?` 立即返回，后续句柄查询与加载均不发生。
2. 若解码结果同时满足 `table_name.is_empty()` 与 `version == 0`，按 Go 对 JSON `null` 的兼容语义直接成功；此分支不检查 StatsHandle，也不调用最终加载。
3. 对非空统计对象调用 `HasStatsHandle()`；不存在时返回 `"Load Stats: handle is nil"`。
4. 句柄存在时，把完整 `DecodedStats` 所有权移交给 `LoadStatsFromJSON`，并原样返回其结果。

Go 生产链的直接证据是：[`pkg/executor/builder.go`](builder.go) 的 `buildLoadStats` 构造 `LoadStatsExec`；`Next` 将 `LoadStatsInfo` 放入 session；[`pkg/executor/plan_replayer.go`](plan_replayer.go) 的 `FileTransInConnHandlers` 用 `LoadStatsVarKey` 注册 `handleLoadStats`；处理器按路径读文件，空字节直接成功，非空字节调用 `LoadStatsInfo.Update`。Rust 尚缺少这段适配器接线。

## 数据与状态

- 稳定配置是 `LoadStatsInfo.path`；`Next` 为运行时状态创建一份路径副本，之后执行器与会话选项互不借用。
- 易变状态全部委托给 `Box<dyn LoadStatsRuntime>`。`&mut self` 保证同一个执行器调用期间对适配器的独占可变访问，但 trait 没有声明内部存储模型。
- 会话选项遵循“至多一个”的不变量：检测到已有选项时必须清除并报错，不能静默覆盖。这个设计既暴露前一次文件传输未正常收尾，也避免把新路径误交给旧处理流程。
- `DecodedStats.value` 的实际类型和所有权由适配器决定；核心只要求它可跨线程移动（`Send`），不要求 `Sync` 或 `Clone`。
- `LoadStatsContext` 内部对象要求 `Send + Sync` 且用 `Arc` 共享；但目前 `Open`/`Next` 忽略上下文，因此它尚未承载真实会话状态。
- chunk 是输出缓冲而不是数据来源。`LOAD STATS` 不产出结果行，`Next` 只将其重置到运行时给出的最大容量。

## 依赖与调用关系

- crate 边界：[`pkg/executor/Cargo.toml`](Cargo.toml) 声明 crate 名为 `astersql-executor`，本文件直接使用其中的 `astersql-errors` 与 `astersql-util-chunk` 路径依赖；模块没有专属 feature gate，`nextgen` feature 与本文件无直接关系。
- 下游调用：`Next → MaxChunkSize → Chunk::GrowAndReset`；随后按分支调用 `HasLoadStatsOption`、`ClearLoadStatsOption` 或 `InstallLoadStatsOption`。`Update → DecodeStatsJSON`，对非 null 载荷再调用 `HasStatsHandle → LoadStatsFromJSON`。
- 上游 Rust 状态：`lib.rs` 公开模块；RustCodeGraph 对本文件符号和 trait 方法能识别上述内部边，但仓库级精确搜索未发现 trait 实现或本文件类型的外部 Rust 构造/调用点。图对常见方法名 `Next`、`Update` 给出了跨模块同名误匹配，不能作为真实 caller 证据。
- Go 对照上游：`builder.go::buildLoadStats` 创建执行器；服务器侧文件传输通过 `plan_replayer.go::FileTransInConnHandlers` 和 `handleLoadStats` 完成第二阶段。
- Go 对照下游：`load_stats.go::LoadStatsInfo.Update` 使用 `json.Unmarshal`、`domain.GetDomain(...).StatsHandle()`、当前 InfoSchema 和 `LoadStatsFromJSON(..., 0)`；Rust 将这些动作压缩为运行时 trait 的三个方法，因此具体 InfoSchema 与最后的 `0` 参数应由适配器保证。

## 错误处理与边界

- 空路径是严格错误，只接受真正的空字符串；本文件不 `trim`，仅含空白的路径会被安装并留给文件读取层处理。这与 Go 的 `len(Path) == 0` 一致。
- 遗留选项分支具有“先清理、后报错”的恢复语义。扩展时不能改为直接覆盖或只报错，否则下一条语句可能持续被旧状态阻塞。
- JSON 解码、最终统计加载的错误不包装，由 `LoadStatsRuntime` 返回的 `SharedError` 直接传播；本文件自己只生成三类固定语义中的两类错误（空路径、遗留选项、空句柄中的后者位于 `Update`）。
- null 兼容判断必须同时满足空表名和版本为零。只满足一个条件的载荷仍进入句柄检查和加载，不能扩大为任一条件成立即跳过。
- `Update` 不对空字节做专门处理。Go 的 `handleLoadStats` 在调用 `Update` 前把空文件视为 no-op；Rust 若要保持完整链路一致，应在未来文件传输适配层保留这一外层边界，而不是假定解码器会接受空输入。
- 类型擦除意味着本文件无法验证 `DecodedStats.value` 的具体类型；适配器必须保证 `DecodeStatsJSON` 与 `LoadStatsFromJSON` 的载荷契约一致。

## 并发与资源生命周期

- 本文件不创建线程、异步任务、锁、通道、事务或文件句柄。文件 I/O 明确位于上层传输处理器。
- `LoadStatsContext` 的 `Arc` 只提供共享所有权；`Any + Send + Sync` 允许宿主放入可跨线程共享的对象，但本文件没有 downcast 或同步操作。
- `LoadStatsInfo` 独占 `Box<dyn LoadStatsRuntime>`，`Next`/`Update` 均要求 `&mut self`，所以核心 API 不允许对同一实例并发推进。运行时内部若采用锁或线程，责任属于具体实现。
- `Open`/`Close` 不分配或释放资源，也不会清除已安装选项。Go 生命周期同样是空实现；会话文件传输层必须在正常完成时消费/清理选项，在失败场景按服务器协议保留或处理状态。
- `DecodedStats` 在 null 分支结束时被丢弃；正常分支则移动给 `LoadStatsFromJSON`。不存在重复提交或借用越过调用边界。

## 与 Go 版本的对应关系

- `LoadStatsExec`、`LoadStatsInfo.path`、`Open`、`Next`、`Close`、`Update` 的主要分支和错误文本与 [`pkg/executor/load_stats.go`](load_stats.go) 对齐：先重置 chunk；空路径报错；遗留会话选项清理并报错；JSON null no-op；句柄为空报错；其余载荷交给统计句柄。
- Go 将 `exec.BaseExecutor` 和 `sessionctx.Context` 直接嵌入/保存，Rust 改用 `LoadStatsRuntime` 与不透明 `LoadStatsContext`。这是依赖反转，不代表真实 session/domain 适配已经存在。
- Go 的 `loadStatsVarKeyType` 是独立命名类型并实现 `Stringer`，可避免 context key 碰撞；Rust 目前只是 `i32` 类型别名加独立字符串函数，类型系统不能阻止与其他 `i32` 键混用。
- Go 的 `LoadStatsInfo.Update` 直接构造 `util.JSONTable` 并选择当前 InfoSchema；Rust 的 `DecodedStats.value` 把具体 JSON 类型擦除，这些语义必须由 `LoadStatsRuntime` 实现补齐。
- Go 的 `handleLoadStats` 负责按 `Path` 读文件并将空数据视为成功；该函数不在 `load_stats.rs` 中，Rust [`pkg/executor/plan_replayer.rs`](plan_replayer.rs) 虽有另一个泛型 `handleLoadStats`，它面向 `PlanReplayerBackend`，没有引用本文件的 `LoadStatsInfo`，不能视为同一接线已完成。

## 扩展指南

- 接入真实 Rust 执行链时，优先新增独立适配器类型实现 `LoadStatsRuntime`，并让 executor builder 构造本文件的 `LoadStatsExec`；不要把 session/domain 细节重新塞回核心分支。需要同时核对服务器文件传输注册、正常清理和失败保留语义。
- 新增或修改 JSON 字段时，修改 `DecodeStatsJSON` 的实现和 `DecodedStats` 契约；务必保留 `table_name == "" && version == 0` 的精确 null 判定，并验证具体载荷能被 `LoadStatsFromJSON` 接受。
- 修改会话键时，应考虑把 `LoadStatsVarKeyType` 从别名升级为新类型以获得 Go 的碰撞隔离，并同步键字符串、文件传输注册和所有读取/清理位置。
- 调整 `Next` 时应保持 chunk 重置发生在校验之前、遗留状态清理发生在返回错误之前。改变这两个顺序会产生兼容性差异。
- 测试必须放在独立 Rust 文件（建议 `pkg/executor/load_stats_test.rs`，并在 `lib.rs` 用 `#[cfg(test)] mod load_stats_test;` 接入），不要内嵌进生产源文件。至少覆盖：空路径仍重置 chunk；遗留选项被清理且不安装新值；正常安装路径副本；解码错误短路；null no-op 不查句柄；无句柄错误；最终加载成功/失败传播。完整接线还应覆盖空文件 no-op 和文件读取失败后的会话状态。
- 性能风险主要在路径克隆、JSON 解码与统计载荷大小；本核心只发生一次路径克隆，不应在这里增加第二次解析或大对象复制。兼容性风险集中于固定错误文本、null 判定、InfoSchema 时点和最终加载参数。

## 验证依据

- 源码：[`pkg/executor/load_stats.rs`](load_stats.rs)，逐项核对全部类型、trait、常量、方法及分支；文件无条件编译项，只有模块级 `allow(non_snake_case)`。
- 模块与依赖：[`pkg/executor/lib.rs`](lib.rs) 的 `pub mod load_stats`；[`pkg/executor/Cargo.toml`](Cargo.toml) 的 crate 定义以及 `astersql-errors`、`astersql-util-chunk` 路径依赖。`pkg/executor/doc.go` 不存在，故无额外包级契约可读。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`explore "pkg/executor/load_stats.rs LoadStatsExec LoadStatsInfo LoadStatsRuntime"` 返回本文件完整源码及 `Next`/`Update` 到 trait 方法的边。对 `Next`/`Update` 的全局 callers 查询受同名符号歧义影响，未作为调用证据。
- Rust 接线搜索：对 `impl LoadStatsRuntime`、`LoadStatsExec {`、`LoadStatsInfo {`、`LOAD_STATS_VAR_KEY`、`DecodeStatsJSON` 等执行仓库级 `rg`；除本文件外未发现运行时实现或本类型的 Rust 构造/调用点。[`pkg/executor/builder.rs`](builder.rs) 仅显示通用 `ExecutorKind::LoadStats` 叶节点构建。
- Go 对照：[`pkg/executor/load_stats.go`](load_stats.go)、[`pkg/executor/builder.go`](builder.go)、[`pkg/executor/plan_replayer.go`](plan_replayer.go)，共同证明构造、会话登记、文件读取、空文件处理、JSON null、StatsHandle 与 InfoSchema 语义。
- 测试证据：未发现本文件对应的独立 Rust 单元测试。Go [`pkg/executor/statement_ru_plan_walk_integration_test.go`](statement_ru_plan_walk_integration_test.go) 覆盖 `LoadStatsVarKey` 遗留文件传输状态对语句完成流程的影响；[`pkg/server/handler/optimizor/statistics_handler_test.go`](../server/handler/optimizor/statistics_handler_test.go) 包含实际 `load stats` SQL 使用；parser 测试仅证明语法接受，不能证明本执行器运行语义。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级标题，并人工复核所有“已接线/未接线”结论均有上述源码或搜索证据。
