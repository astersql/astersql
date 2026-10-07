# `pkg/expression/pushdown_context.rs`

源码：[pushdown_context.rs](pushdown_context.rs)

## 文件定位

本文件属于 `astersql-expression` crate，是规划器把表达式或聚合描述编码为 TiPB 时使用的持有式上下文。`pkg/expression/lib.rs` 以 `pushdown_context_kernel` 挂载该文件，并在 crate 根公开再导出 `NewPushDownContext`、`PushDownContext` 及三个引用别名，因此规划器和聚合模块通过 `expression::PushDownContext` 使用它，而不需要知道内部模块名。

这里的 `PushDownContext` 不等同于 `pkg/expression/infer_pushdown.rs::PushDownContext`：后者位于 `expression::infer_pushdown` 命名空间，保存黑名单判定、强制下推和调试开关；本文件的类型保存真实 `BuildContext`、KV 客户端、告警追加器和 `GROUP_CONCAT` 上限，服务于 PB 构造。两者名称相同但当前接线和职责不同，扩展时不可混用。

## 核心职责

- 用 `Arc<dyn ...>` 持有规划器提供的表达式构建上下文、KV 客户端和告警追加器，使下推上下文可克隆且不受构造函数局部变量生命周期限制。
- 在 `NewPushDownContext` 中复刻 Go 的告警选择规则：只有普通与额外告警处理器同时存在时才启用告警；`in_explain_stmt == true` 选择普通处理器，否则选择额外处理器。
- 通过 `EvalCtx`、`Client`、`GetGroupConcatMaxLen` 暴露 PB 编码所需的最小数据面，并通过 `PbConverter` 把客户端与求值上下文组合成借用型转换器。
- 通过 `AppendWarning` 把规划期告警转发到选定处理器；未配置处理器时有意静默丢弃。

本文件不判断表达式是否允许下推，也不发送 KV 请求。能力判定主要在 `infer_pushdown.rs`，具体表达式编码在 `expr_to_pb.rs`，聚合编码在 `aggregation/agg_to_pb.rs`。

## 主要符号

- `PushDownBuildContextRef = Arc<dyn exprctx::BuildContext>`：共享拥有表达式构建上下文。`BuildContext::GetEvalCtx` 是本文件实际使用的入口。
- `PushDownClientRef = Arc<dyn kv::Client + Send + Sync>`：共享拥有且要求线程安全的 KV 客户端。其 `IsRequestTypeSupported` 会被下游 PB 编码用于能力探测。
- `PushDownWarnAppenderRef = Arc<dyn contextutil::WarnAppender + Send + Sync>`：共享拥有且要求线程安全的告警接收器。
- `PushDownContext`：可克隆的四字段结构；`eval_ctx` 必填，`client` 与 `warn_handler` 可选，`group_concat_max_len` 为值语义配置。
- `NewPushDownContext(...) -> PushDownContext`：唯一公开构造函数，完成告警处理器选择并保存其余参数。
- `PushDownContext::EvalCtx(&self) -> &dyn exprctx::EvalContext`：经 `BuildContext::GetEvalCtx` 返回求值上下文视图。
- `PushDownContext::PbConverter(&self) -> PbConverter<'_>`：调用 `NewPBConverter(self.Client(), self.EvalCtx())`，返回生命周期绑定到当前上下文的转换器。
- `PushDownContext::Client(&self) -> &dyn kv::Client`：解包可选客户端；未注入时以固定消息 panic。
- `PushDownContext::GetGroupConcatMaxLen(&self) -> u64`：返回构造时的协议配置值。
- `PushDownContext::AppendWarning(&self, SharedError)`：处理器存在时调用 `WarnAppender::AppendWarning`，否则不做任何事。

本文件没有常量、枚举、trait、条件编译分支或异步函数；全部公开 API 均由 crate 根再导出。

## 执行流程

1. 规划器从 `BuildPBContext` 收集 `GetExprCtx()`、`GetClient()`、`InExplainStmt`、两类告警处理器和 `GroupConcatMaxLen`。通用桥接入口是 `pkg/planner/util/misc.rs::GetPushDownCtxFromBuildPBContext`；`physical_hash_agg.rs::aggregate_to_pb` 也直接构造上下文。
2. `NewPushDownContext` 同时匹配两个告警处理器。二者都为 `Some` 时依据 `in_explain_stmt` 保留其中一个；任何一个缺失都会把内部 `warn_handler` 设为 `None`。随后原样保存求值上下文、可选客户端和长度上限。
3. 调用方进入 PB 构造。例如 `aggregation/agg_to_pb.rs::AggFuncToPBExpr` 先调用 `PbConverter` 和 `Client`，再用客户端检查请求类型支持，逐个编码聚合参数。
4. `PbConverter` 先通过 `Client` 强制取得客户端，再通过 `EvalCtx` 取得求值上下文，调用 `expr_to_pb.rs::NewPBConverter` 生成只借用两者的转换器；转换器不能活得比 `PushDownContext` 更久。
5. 对 `GROUP_CONCAT`，`AggFuncToPBExpr` 调用 `GetGroupConcatMaxLen`，将上限编码到 TiPB `Expr.val`；其他聚合不会使用该字段。
6. 需要报告不可下推等非致命问题时，调用方可调用 `AppendWarning`；它只负责转发，不转换错误、计数或返回结果。

## 数据与状态

`PushDownContext` 构造后没有字段修改方法，公开操作都是只读借用，因此其配置在生命周期内保持稳定。`#[derive(Clone)]` 对三个 `Arc` 做浅克隆并复制 `Option`/`u64`；克隆实例共享同一底层构建上下文、客户端和告警处理器，而不是复制这些对象的内部状态。

`client: Option<_>` 表示构造阶段允许尚无客户端，但这不是所有操作都安全的状态：只读取 `EvalCtx`、长度上限或追加告警不需要客户端，`Client` 和 `PbConverter` 则要求它存在。`warn_handler: Option<_>` 同时编码“告警被路由到哪个处理器”和“告警被禁用”两种状态。`group_concat_max_len` 不在本文件校验，按构造输入原样传给下游协议编码。

## 依赖与调用关系

上游直接证据：

- `pkg/expression/lib.rs` 的 `#[path = "pushdown_context.rs"] mod pushdown_context_kernel` 负责装配，随后在 crate 根再导出本文件的五个公开符号。
- `pkg/planner/util/misc.rs::GetPushDownCtxFromBuildPBContext` 把 `BuildPBContext` 的六项数据传入 `NewPushDownContext`；`GetPushDownCtx` 是面向 `PlanContext` 的上一层入口。
- `pkg/planner/core/operator/physicalop/physical_hash_agg.rs::aggregate_to_pb` 在构造 TiPB 聚合执行器前创建本上下文，并传给 `AggFuncToPBExpr`。
- Rust 独立测试 `aggregation/agg_to_pb_test.rs`、`aggregation/window_func_test.rs`、`aggregation/aggregation_aster_unit_test.rs` 和 `aggregation/go_merge_44_test.rs` 直接构造该类型，覆盖聚合编码或 TiFlash 下推场景。

下游直接证据：

- `exprctx::BuildContext::GetEvalCtx` 提供 `EvalCtx` 返回值。
- `expr_to_pb.rs::NewPBConverter` 接收 `&dyn kv::Client` 与 `&dyn EvalContext`，构造 `PbConverter<'_>`。
- `contextutil::WarnAppender::AppendWarning` 接收共享错误并承接告警。
- `aggregation/agg_to_pb.rs::AggFuncToPBExpr` 消费 `PbConverter`、`Client`、`EvalCtx` 与 `GetGroupConcatMaxLen`，是当前最完整的消费者。

Cargo 边界由 `pkg/expression/Cargo.toml` 确认：crate 名为 `astersql-expression`，本文件用到的 `exprctx`、`kv`、`contextutil` 分别来自路径依赖 `astersql-expression-exprctx`、`astersql-kv`、`astersql-util-context`；TiPB 由 crate 的 Git 依赖提供，但本文件不直接引用协议类型。

## 错误处理与边界

- `NewPushDownContext` 不返回 `Result`，也不验证 `group_concat_max_len`；这些参数被视为上游已验证的规划上下文数据。
- 缺少任一告警处理器不会报错，而会禁用告警转发。这是与 Go `NewPushDownContext` 一致的显式语义，不能改成“哪个存在就用哪个”。
- `AppendWarning` 在无处理器时静默返回；有处理器时直接转发 `SharedError`，本文件不捕获处理器内部的 panic，也不改变错误内容。
- `Client` 在 `client == None` 时 panic：`PushDownContext requires a KV client for PB conversion`。因此任何会调用 `Client` 或 `PbConverter` 的路径必须在构造时注入客户端。需要可恢复错误的上游应像 `physical_hash_agg.rs::aggregate_to_pb` 一样先检查 `BuildPBContext::GetClient()` 并返回业务错误。
- `PbConverter` 的生命周期与 `&self` 绑定，避免转换器悬垂引用；它本身不拥有客户端或求值上下文。
- 本文件没有直接网络 I/O、序列化失败或可恢复错误返回；这些边界由 `PbConverter` 和具体 PB 构造函数负责。

## 并发与资源生命周期

客户端与告警处理器别名显式要求 `Send + Sync`，并用 `Arc` 共享所有权，适合在多个上下文克隆之间复用。`PushDownBuildContextRef` 仅声明为 `Arc<dyn BuildContext>`，而 `BuildContext` trait 本身没有 `Send + Sync` 超 trait；因此不能仅根据另外两个字段断言整个 `PushDownContext` 可跨线程发送或共享，是否满足自动 trait 受该 trait object 限制。

克隆或销毁 `PushDownContext` 只增减 `Arc` 引用计数，不启动任务、不持有锁、不打开连接，也不发送请求。最后一个 `Arc` 被释放时底层对象才析构。告警处理器的内部同步、告警容量和错误处理均属于其具体实现；本文件只要求接口可并发共享。`PbConverter` 是短生命周期借用对象，不延长任何资源寿命。

## 与 Go 版本的对应关系

直接 Go 对照位于 `pkg/expression/infer_pushdown.go` 的 `PushDownContext`、`NewPushDownContext`、`EvalCtx`、`PbConverter`、`Client`、`GetGroupConcatMaxLen` 和 `AppendWarning`。

- 字段一一对应：Go 的 `EvalContext`、`kv.Client`、`contextutil.WarnAppender`、`uint64` 在 Rust 中分别映射为 `Arc<dyn BuildContext>`（再取其 `EvalContext`）、可选 `Arc<dyn kv::Client + Send + Sync>`、可选 `Arc<dyn WarnAppender + Send + Sync>`、`u64`。
- 告警路由完全保留 Go 分支：两个 handler 都非空才选择；EXPLAIN 用普通 handler，其他情况用 extra handler。
- Go 的接口值可为 `nil`；Rust 用 `Option<Arc<dyn ...>>` 表达客户端和 handler 的可空性。Rust `Client` 明确 panic，而 Go 返回 nil 接口后通常会在实际调用处触发 nil 问题，因此 Rust 把失败位置和消息固定下来。
- Go `PbConverter` 按值保存接口；Rust `PbConverter<'_>` 借用上下文中对象，减少额外所有权并由生命周期保证有效性。
- Go 还提供 `NewPushDownContextFromSessionVars`；本文件没有该便捷构造器，Rust 目前由规划器桥接函数显式拆出各字段。

Go `aggregation/agg_to_pb_test.go` 使用会话变量构造上下文并验证聚合 PB，尤其验证 `GROUP_CONCAT` 的长度上限进入协议值。Rust `aggregation/agg_to_pb_test.rs::TestAggFunc2Pb` 对应验证 `GetGroupConcatMaxLen` 编码为 `Expr.val`，并验证常见聚合类型、返回类型和 `DISTINCT` 标志；`TestAggFuncSumIntToPb` 验证同一上下文在 TiKV/TiFlash 编码路径可用。

## 扩展指南

- 新增 PB 构造所需的会话配置时，应先确认它属于持有式 PB 上下文而非 `infer_pushdown` 能力判定上下文；随后同步修改 `PushDownContext` 字段、`NewPushDownContext` 参数、`planner/util/misc.rs::GetPushDownCtxFromBuildPBContext` 接线和所有直接构造点。
- 若新增只读访问器，保持 `&self` 和借用返回值优先，避免不必要地克隆大对象；若返回下游转换器，像 `PbConverter<'_>` 一样显式绑定生命周期。
- 调整告警路由必须同步核对 Go `NewPushDownContext`，并在独立 Rust 测试文件中覆盖 EXPLAIN、普通规划、缺少 normal、缺少 extra 四类组合；不要把测试写入本源文件。
- 若希望缺少客户端变为可恢复错误，需要同时改造 `Client`/`PbConverter` 签名及所有消费者，尤其是 `AggFuncToPBExpr`、窗口函数 PB 路径和规划器桥接；单独把 panic 改成默认客户端会掩盖接线错误。
- 新增字段需评估 `Clone` 是否仍应浅克隆、是否引入锁或跨线程约束，以及 Cargo 中是否已有相应 crate 依赖。协议字段变化还应同步 Go 聚合测试与 Rust `aggregation/*_test.rs`。
- 性能方面，本文件当前构造和克隆仅涉及少量 `Arc` 计数及标量复制；避免在频繁的表达式逐节点编码路径中加入分配、锁获取或深复制。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/pushdown_context.rs` 确认目标文件被索引且含 8 个符号。
- RustCodeGraph 源码/符号：读取 `pushdown_context.rs` 全部 101 行，并查询 `PushDownContext`、`NewPushDownContext`、`PbConverter`；`PbConverter` 的调用边指向本文件的 `Client`、`EvalCtx`，再落到 `expr_to_pb.rs::NewPBConverter`。
- RustCodeGraph 上下游：核对 `pkg/expression/lib.rs` 的模块挂载与再导出、`pkg/planner/util/misc.rs::GetPushDownCtxFromBuildPBContext`、`physical_hash_agg.rs::aggregate_to_pb`、`aggregation/agg_to_pb.rs::AggFuncToPBExpr`、`expr_to_pb.rs::NewPBConverter`。
- crate/trait 边界：读取 `pkg/expression/Cargo.toml`、`pkg/expression/exprctx/context.rs::BuildContext`、`pkg/kv/kv.rs::Client`、`pkg/util/context/warn.rs::WarnAppender`。
- Go 对照：读取 `pkg/expression/infer_pushdown.go` 的上下文实现、`pkg/expression/aggregation/agg_to_pb.go::AggFuncToPBExpr` 和 `agg_to_pb_test.go`。
- Rust 测试：读取 `pkg/expression/aggregation/agg_to_pb_test.rs`、`go_merge_44_test.rs`、`window_func_test.rs` 和 `aggregation_aster_unit_test.rs` 的直接构造与消费路径。未发现与 `pushdown_context.rs` 同名的独立测试；现有测试以聚合/窗口行为间接覆盖该上下文，告警选择和缺少客户端 panic 尚未见针对本类型的独立断言。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以固定 11 章节结构命令和人工事实复核验证。
