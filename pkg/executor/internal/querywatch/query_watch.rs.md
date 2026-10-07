# `pkg/executor/internal/querywatch/query_watch.rs`

## 文件定位

本文件是 `astersql-executor-internal-querywatch` crate 的业务实现，负责把 ADD / DROP QUERY WATCH 所需的选项转换为 runaway query（失控查询）监视记录，并通过抽象接口完成资源组校验、规则注册和删除。crate 根文件 [`lib.rs`](./lib.rs) 以 `pub mod query_watch` 导出本模块；[`Cargo.toml`](./Cargo.toml) 将其登记为 Go 包 `pkg/executor/internal/querywatch` 的移植单元。workspace 根 `Cargo.toml` 纳入该 crate，`pkg/lib.rs` 又通过 `pkg::executor::internal::querywatch` facade 再导出。

当前接线边界需要特别说明：RustCodeGraph 与仓库文本检索只发现本模块的独立测试直接调用这里的生产符号；`astersql-executor` 虽声明了对该 crate 的依赖，但尚未发现 Rust executor builder、session 或 SQL 分派路径调用 `AddExecutor` / `exec_drop_query_watch`。完整 SQL 到 domain/runaway manager 的在线链路目前仍可在同目录 [`query_watch.go`](./query_watch.go) 中看到。因此，本文件是可测试的 Rust 业务逻辑移植，不应被描述为已经接管线上 QUERY WATCH SQL 执行。

文件前部第 22～239 行是 Go 控制流的注释映射，不参与编译；实际 Rust 实现从 `use std::collections::BTreeMap` 开始。文件还以 `#[path = "../../../parser/keywords.rs"]` 和 `include!("../../../parser/digester.rs")` 复用 parser 的关键字表与 digest 规则。

## 核心职责

本文件承担四组职责：

1. 用 `QueryWatchOption`、`QuarantineRecord`、`RunawayAction`、`WatchType` 和 `DropQueryWatch` 表达 QUERY WATCH 的输入、持久化前记录和删除目标。
2. `from_option_list` 按选项顺序构造记录：选择资源组与动作，校验单条 SQL，并为 Exact、Similar、Plan 三种监视方式产生不同的 `watch_text`。
3. `validate_watch_record` 解析默认资源组和默认 runaway action，并拒绝不存在的资源组、缺失的默认配置或未指定的监视类型。
4. `AddExecutor::next` 与 `exec_drop_query_watch` 分别编排新增和删除；真实外部状态由 `ResourceGroupController`、`RunawayManager`、`PlanDigester` 三个 trait 注入。

此外，`single_sql`、`normalize_digest`、`local_sha2::Sha256` 和 `sha256_bytes` 提供 SQL 文本边界处理与 TiDB normalize digest 所需的本地哈希适配。它们是实现细节，不是公开 API。

## 主要符号

- `DEFAULT_RESOURCE_GROUP: &str = "default"`：未显式指定资源组时的回退值，与 Go 的 `resourcegroup.DefaultResourceGroupName` 对齐。
- `RunawayAction::{None, DryRun, CoolDown, Kill, SwitchGroup}`：规则命中后的动作。`None` 是待校验/待继承状态，不是有效终态。
- `WatchType::{None, Exact, Similar, Plan}`：匹配原始 statement text、规范化 SQL digest 或 plan digest；`None` 必须在校验前被替换。
- `QuarantineRecord`：规则数据载体。`Default::default` 设置 `source = "manual"`、当前 `SystemTime`、`end_time = None`、`exceed_cause = "None"`，其余业务字段为空或 `None` 枚举值。
- `QueryWatchOption`：ADD 的输入枚举。`ResourceGroup(String)` 和 `Action(RunawayAction, Option<String>)` 直接赋值；`Text { watch, value, type_specified }` 决定 SQL 解析及 digest 计算方式。
- `ResourceGroup` 与 `ResourceGroupController::resource_group`：资源组及其可选默认 `(action, switch_group)` 的最小抽象。
- `RunawayManager::{add_watch, remove_watch, remove_group_watches}`：唯一的规则状态写入边界。
- `PlanDigester::plan_digest`：Plan 模式的外部计算边界；Go 版本在系统 session 中执行 `EXPLAIN` 并读取 statement context 的 plan digest。
- `from_option_list(options, digester)`：公开的记录构造入口。
- `validate_watch_record(record, controller)`：公开的资源组及终态校验入口，会原地补写默认字段。
- `AddExecutor::new` / `AddExecutor::next`：一次性 ADD 编排器。首次 `next` 返回 `Ok(Some(id))`，后续调用返回 `Ok(None)`。
- `DropQueryWatch::{Group, GroupVariable, Id}` 与 `exec_drop_query_watch`：三种 DROP 目标及分派入口。
- `single_sql`：识别注释、引号、反引号与语句分号的单语句扫描器。
- `normalize_digest`：调用纳入本模块的 `parser_digester::NormalizeDigest`；`local_sha2::Digest` 为该代码提供所需的最小增量哈希接口。
- `sha256_bytes`：标准 SHA-256 的分块、消息扩展和 64 轮压缩实现，供 normalize digest 最终生成 64 字符十六进制摘要。

## 执行流程

ADD 的主流程由 `AddExecutor::next` 定义：

1. 先检查 `done`；已执行过则立即返回 `Ok(None)`。首次进入时在任何可能失败的工作之前把 `done` 置为 `true`，所以失败也不会重试。
2. `from_option_list` 创建默认 `QuarantineRecord`，再按切片顺序应用每个选项；重复类型的选项以后出现者覆盖此前字段。
3. 对 `Text` 且 `type_specified = true` 的选项，`single_sql` 要求输入恰有一个非空 statement；随后 Exact 保存 statement text，Similar 调用 `normalize_digest`，Plan 调用注入的 `PlanDigester`。`WatchType::None` 在这里直接报错。
4. 对 `type_specified = false` 的 Text，只检查 Rust 字符串的字节长度是否为 64，然后原样保存；实现刻意不校验十六进制字符，也不重新计算摘要。
5. `validate_watch_record` 先为空资源组填入 `default`，再通过 controller 查询。若 action 为 `None`，从资源组的 `default_action` 补齐 action 与 switch group；最后拒绝 `WatchType::None`。
6. 校验通过后调用 `RunawayManager::add_watch`；返回的 ID 被包为 `Some(id)`。

DROP 的主流程由 `exec_drop_query_watch` 定义：`Group` 直接调用 `remove_group_watches`；`GroupVariable` 从传入的 `BTreeMap` 解析变量值后按组删除；`Id` 调用 `remove_watch`。该函数本身不读取 session，也不持有 domain，调用方必须把用户变量快照与 manager 显式传入。

`single_sql` 逐字节扫描输入：在单引号、双引号、反引号及行/块注释内部忽略分号；只把含非空白 token 的片段计为 statement；遇到第二条有效语句立即失败。返回文本保留前置注释、空 statement、终止分号和普通空格，但去除终止分号后的注释/空 statement，并按 Go scanner 行为最多裁掉边界上的一个换行。未闭合引号或块注释返回 `invalid SQL syntax`。

## 数据与状态

`QuarantineRecord` 是本文件的核心可变状态。构造阶段写入选项字段，校验阶段可能补写 `resource_group`、`action`、`switch_group`，注册后所有权移交给 `RunawayManager::add_watch`。`start_time` 使用本地 `SystemTime::now()`；与 Go 的 `time.Now().UTC()` 不同，`SystemTime` 本身不携带时区。`end_time = None` 对应手工规则的 Go `runaway.NullTime`，但最终存储编码不在本文件内。

选项应用顺序是可观察语义：`from_option_list` 不做重复项检测，后项可以覆盖资源组、动作和监视文本。若处理中途失败，局部 record 被丢弃，不会传给 manager。

`AddExecutor` 仅额外保存一个私有 `done: bool`。它借用 controller、manager 和 digester，不拥有这些服务；`options` 则由 executor 自身拥有。DROP 路径只借用 manager 与只读 `BTreeMap`。

`local_sha2::Sha256` 在 `Vec<u8>` 中累积 parser digester 的增量输入；`finalize_reset` 通过 `mem::take` 清空缓冲并返回摘要，因此同一实例可继续使用。代价是计算前会保留全部输入，而不是像流式 SHA-256 那样固定内存。

## 依赖与调用关系

直接向上的已验证调用者是 [`query_watch_test.rs`](./query_watch_test.rs)：它直接覆盖 `from_option_list`、`validate_watch_record`、`AddExecutor` 和 `exec_drop_query_watch`。RustCodeGraph 对公开入口的 callers 查询没有返回生产调用边；仓库检索同样只找到测试引用。crate 级上游装配包括 workspace 根 `Cargo.toml`、`pkg/executor/Cargo.toml` 的路径依赖，以及 `pkg/lib.rs` 的 facade 再导出，但这些是可见性/依赖边，不等同于运行时调用。

向下调用关系经 RustCodeGraph 核验如下：

- `from_option_list` → `QuarantineRecord::default`、`single_sql`、`normalize_digest`、`PlanDigester::plan_digest`。
- `validate_watch_record` → `ResourceGroupController::resource_group`。
- `AddExecutor::next` → `from_option_list` → `validate_watch_record` → `RunawayManager::add_watch`。
- `exec_drop_query_watch` → `RunawayManager::remove_group_watches` 或 `remove_watch`。
- `normalize_digest` → 被 `include!` 纳入的 parser `NormalizeDigest` → 本地 `Digest`/`sha256_bytes` 适配器。

本 crate 的 manifest 没有普通 `[dependencies]`，只有测试初始化相关的三个 `[dev-dependencies]`。生产实现只用标准库，并以源码包含方式复用 `pkg/parser/keywords.rs` 与 `pkg/parser/digester.rs`；这形成了未由 Cargo 依赖声明表达的源码级耦合，parser 文件接口或相对路径变动会直接影响本模块编译。

Go 在线实现的下游是真实 `domain.ResourceGroupsController()`、`domain.RunawayManager()`、系统 session SQL executor 与 chunk；Rust 版本用三个 trait 和 `Option<u64>` 替代这些具体组件，因此目前不能从 Rust 类型本身推导持久化、集群同步或 SQL result chunk 行为。

## 错误处理与边界

所有公开操作使用 `Result<_, String>`，保留消息但不保留结构化错误类型或堆栈。关键错误包括：多条/零条有效 SQL 的 `only support one SQL`、未闭合引号或块注释的 `invalid SQL syntax`、缺失监视类型、64 字节格式错误、不存在/未配置 runaway 的资源组、缺失用户变量，以及三项 trait 实现透传的任意错误。

边界顺序很重要：`validate_watch_record` 会先把空组名改成 `default`，再调用 controller；所以 controller 失败时 record 仍已发生这项修改。若 action 显式非 `None`，不会要求资源组存在默认 runaway action。与 Go 一致，当前仍没有校验 `SwitchGroup` 的目标组是否存在或是否为空。

`type_specified = false` 只执行 `value.len() == 64`，这是 UTF-8 字节长度检查，不是“64 个字符”或合法 hex 检查。`single_sql` 是针对 statement 边界的轻量扫描器而非完整 SQL parser：它处理注释、引号和转义边界，但不会验证一般 SQL 语法；`invalid SQL syntax` 目前只覆盖未闭合 quote/block comment。扩展时不能把它误当成 parser crate 的完整语法校验。

`AddExecutor::next` 在构造、校验或 manager 写入失败前已经设置 `done = true`；调用方重试同一实例只会得到 `Ok(None)`。这一点由测试明确锁定并与 Go 的执行顺序一致。DROP 的 manager 错误直接透传；按组删除是否把“不存在”视为成功由具体 manager 决定。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或后台 GC。trait 方法均接收 `&self`，线程安全能力没有通过 `Send` / `Sync` 约束表达；具体实现能否跨线程共享由外部类型和调用环境决定。`AddExecutor::next` 需要 `&mut self`，从 Rust 借用规则上阻止同一 executor 的并发调用，但 `done` 不是原子量，类型也没有承诺跨线程执行。

规则本身的持久化、集群传播、匹配、过期与删除生命周期属于 `RunawayManager` 实现，不在本文件中。资源组读取的一致性也由 `ResourceGroupController` 决定。本文件只保证在 `add_watch` 前完成本地记录构造与校验，并将 record 按值移交。

哈希适配器的缓冲在 `finalize_reset` 时释放原内容并复位；`single_sql` 仅借用输入并在成功时复制出一个 `String`。Plan 模式的外部资源使用完全由 `PlanDigester::plan_digest` 控制；Rust 实现没有 Go 系统 session 的获取、归还或 statement context 清理逻辑。

## 与 Go 版本的对应关系

[`query_watch.go`](./query_watch.go) 是逐项对照基准：Rust `from_option_list` 合并了 Go `fromQueryWatchOptionList` 与 `setWatchOption` 的主要业务分支；`validate_watch_record` 对应 `validateWatchRecord`；`AddExecutor::next` 对应 `(*AddExecutor).Next`；`exec_drop_query_watch` 对应 `ExecDropQueryWatch`。默认组、默认 action 继承、三种 watch 类型、只接受一条 SQL、digest 只验长度、switch group 暂不校验以及 done 的设置时机均保持一致。

仍存在清晰的移植边界/差异：

- Go 接收 AST 节点并通过 planner expression rewrite/evaluation 支持资源组表达式和 watch text 表达式；Rust 接收已经求值的 `String`/枚举，没有表达式求值、NULL 分支或 AST 类型断言。
- Go Exact/Similar 使用完整 parser 解析并取 AST statement text；Rust 用 `single_sql` 做边界扫描，再用移植的 digester 计算 Similar。独立测试覆盖了注释、空 statement、引号内分号和 normalize reductions，但不能据此声称轻量扫描器等价于完整 parser 的全部语法行为。
- Go Plan 获取系统 session、执行内部 `EXPLAIN` 并从 `StmtCtx` 读取 digest；Rust 抽象为 `PlanDigester`，没有实现系统 session 生命周期，也没有单独的“无 plan digest”分支，具体错误由实现者决定。
- Go 直接访问 domain 的资源组控制器/runaway manager，并把 ID 写入 `chunk.Chunk`；Rust 依赖注入 trait 并返回 `Option<u64>`。
- Go 使用 protobuf/runtime 的动作与 watch 枚举、`runaway.QuarantineRecord` 和 infoschema 结构化错误；Rust 使用本地类型与 `String` 错误。
- Go DROP 从 session user vars 读取 datum 并调用 `ToString`；Rust 只读取预先构造的 `BTreeMap<String, String>`。

[`query_watch_test.rs`](./query_watch_test.rs) 直接驱动 Rust API，覆盖 Go 的主要 fixture 及额外的移植语义测试；[`query_watch_test.go`](./query_watch_test.go) 则通过 SQL、系统表、failpoint 和真实 Go 组件验证完整链路。因此 Rust 单测证明的是本文件逻辑，不证明 Rust SQL 主链已接通。

## 扩展指南

新增 watch 类型时，应同步修改 `WatchType`、`from_option_list` 的匹配分支、相关持久化/协议映射，并在独立的 [`query_watch_test.rs`](./query_watch_test.rs) 中增加文本生成、错误传播和 manager 副作用测试；不要把测试内嵌回生产 `.rs`。若需要与 Go AST 直接接轨，应在外层适配 AST/表达式求值，而不是让核心记录类型隐式依赖 session。

替换或增强 SQL 解析时，重点保护 `exact_watch_preserves_go_parser_statement_text` 和 `similar_watch_uses_tidb_normalize_digest` 等用例所表达的 statement text 兼容性。采用 parser crate 正式 API会减少源码 `include!` 耦合，但必须核对 digest 输出、关键字表版本与性能；修改 `single_sql` 时要覆盖注释、空 statement、转义、双引号/反引号、尾随内容及无效语法。

接入 Rust 线上执行链时，最可能新增的适配点是：从 Rust AST 构造 `QueryWatchOption`、实现基于 domain 的三个 trait、把 `Option<u64>` 写入 executor 输出，以及为系统 session/EXPLAIN 建立可清理的 `PlanDigester`。还需补充 SQL 级独立集成测试，验证持久化表、information schema、规则实际命中和 DROP 的最终一致性，而不能只复用当前内存 mock。

改变 `AddExecutor` 的重试语义、digest 合法性检查或 switch group 校验会产生 Go 兼容风险；改变 `local_sha2` 或 parser include 会产生所有 Similar 规则不再匹配的高正确性风险。`local_sha2` 累积全部输入的策略通常面对短 SQL，但若接受超大文本会线性占用内存；任何优化必须用已知 Go digest 向量验证结果不变。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `query_watch.rs` 被识别出 33 个符号。
- RustCodeGraph 源码与图查询：读取 `query_watch.rs` 全部 724 行；查询 `from_option_list`、`validate_watch_record`、`AddExecutor`、`exec_drop_query_watch`；对 `from_option_list`、`validate_watch_record`、`AddExecutor::next`、`exec_drop_query_watch`、`single_sql`、`normalize_digest` 执行 callers/callees。精确图结果确认了 `from_option_list` 的四条下游边、controller 查询边和两个删除接口边；没有得到 Rust 生产调用者。
- crate/装配证据：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、workspace 根 `Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/lib.rs`。目标目录不存在 `doc.go`；最近的模块契约由 Rust `lib.rs` 和 Go package 文件提供。
- Go 对照证据：[`query_watch.go`](./query_watch.go) 的 `setWatchOption`、`fromQueryWatchOptionList`、`validateWatchRecord`、`AddExecutor.Next`、`ExecDropQueryWatch`。
- 测试证据：[`query_watch_test.rs`](./query_watch_test.rs)、[`main_test.rs`](./main_test.rs)、[`query_watch_test.go`](./query_watch_test.go)、[`main_test.go`](./main_test.go)。Rust 测试覆盖默认值、资源组/action 校验、Exact/Similar/Plan、64 字节 digest、三种删除方式、一次性状态、错误顺序、statement text 和 normalize digest；Go 测试补充完整 SQL 与系统表行为。
- 本任务按计划只做文档分析，未运行 Cargo 或代码测试。结构验证命令及其退出状态在交付前单独执行并报告。
