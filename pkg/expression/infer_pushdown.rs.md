# `pkg/expression/infer_pushdown.rs`

## 文件定位

本文件是 `astersql-expression` crate 内的表达式下推策略实现，回答一个规划期问题：某个 `Expression` 或一组表达式是否能编码并交给 TiKV、TiFlash 或 TiDB 侧执行。`pkg/expression/lib.rs:344-345` 以私有模块名 `infer_pushdown_kernel` 装入本文件，再由 `pkg/expression/lib.rs:66-79` 的公开 `infer_pushdown` 模块同时再导出表达式模型与判定 API；因此外部调用者使用 `expression::infer_pushdown::*`，不直接依赖私有模块名。

它位于 SQL 规划到存储执行的策略边界：例如 `pkg/planner/core/operator/physicalop/physical_projection.rs:148` 将投影表达式转换为本文件使用的策略表达式后调用 `can_exprs_push_down`，而 `pkg/expression/expr_to_pb.rs:303-310` 将生产编码路径中的 `kv::StoreType` 映射到这里的 `StoreType` 并复用黑名单判定。RustCodeGraph 将本文件识别为 41 个符号，并给出了 `can_expr_push_down -> can_func_be_pushed` 的核心调用边及 `pkg/planner/core/integration_test.rs` 的直接使用证据。

## 核心职责

- 维护每个存储引擎的标量函数能力集合：`TIKV_FUNCTIONS`、`TIFLASH_FUNCTIONS` 以及 `scalar_expr_supported_by_tikv`、`scalar_expr_supported_by_flash` 的签名级特例。
- 递归验证表达式树：`can_expr_push_down` 先执行 TiFlash 类型门禁，再区分常量、列、不支持节点和标量函数，并要求标量函数的所有参数都可下推。
- 实施动态黑名单：既检查规范化后的函数名，也检查 `函数名.签名名`，并用 `StoreType` 位掩码做到按引擎禁用。
- 把单点判定包装成稳定保序的批量接口：`push_down_exprs_with_extra_info` 分出可下推与保留在 TiDB 的两组，`can_exprs_push_down_with_extra_info` 判断是否全部可下推。
- 将不可下推原因路由到规划期警告收集器，并保留 `GROUP_CONCAT` 长度、强制下推和未指定签名 panic 等上下文状态。

本文件只做能力推断，不生成 TiPB，也不执行表达式；真正的表达式数据模型来自 `pkg/expression/fts_to_like.rs`，生产 PB 编码还由 `pkg/expression/expr_to_pb.rs` 负责。

## 主要符号

- `StoreType { Unspecified, TiKV, TiFlash, TiDB }`：本文件的目标引擎枚举。`mask()` 以 `1 << enum_value` 生成位；`Unspecified` 特判为三个实际引擎位的并集。私有 `name()` 只服务警告文案。
- `WarningHandler = Arc<Mutex<Vec<String>>>`：可跨克隆上下文共享的警告缓冲区。
- `PushDownContext`：保存可选警告处理器、`group_concat_max_len`、可选强制函数列表和 `panic_on_unspecified`。`new` 仅在 normal/extra 两个处理器都存在时启用警告；EXPLAIN 选 normal，其余路径选 extra。构建器 `with_force_pushdown`、`with_panic_on_unspecified` 返回更新后的上下文。
- `DEFAULT_EXPR_PUSH_DOWN_BLACKLIST`：`LazyLock<RwLock<HashMap<String, u32>>>` 全局黑名单。键在写入和查询时都转 ASCII 小写。
- `EXPR_PUSH_DOWN_BLACKLIST_RELOAD_TIMESTAMP`：每次 `replace_pushdown_blacklist` 后以 `SeqCst` 加一的全局版本号，供黑名单变化后的缓存失效协作使用；本文件本身不读取该值。
- `replace_pushdown_blacklist` / `clear_pushdown_blacklist` / `is_push_down_enabled`：替换、清空和查询黑名单的公开状态接口。
- `can_expr_push_down`：核心公开递归判定入口；`can_func_be_pushed` 是它对标量函数进行强制开关、能力表和两级黑名单检查的内部入口。
- `scalar_expr_supported_by_tikv` / `scalar_expr_supported_by_flash` / `scalar_expr_supported_by_tidb`：按引擎定义函数能力；TiDB 的集合明确是 TiKV 与 TiFlash 集合的并集，而非“接受任意函数”。
- `flash_supports_cast`、`can_enum_pushdown_preliminarily`、`signature_is`、`signature_in`、`is_binary_literal`：承载 CAST、ENUM、签名集合和 TiKV `conv` 二进制字面量等局部规则的辅助函数。
- `push_down_exprs[_with_extra_info]`、`can_exprs_push_down[_with_extra_info]`：公开批量 API；无 `extra_info` 后缀的版本固定传入 `can_enum_push = false`。

本文件没有 trait、条件编译项或自定义错误类型；两个静态量、两个常量能力表、一个枚举、一个类型别名、一个上下文结构及一组函数构成全部行为。

## 执行流程

1. 调用者构造 `PushDownContext` 并选择 `StoreType`。若需要批量分类，进入 `push_down_exprs_with_extra_info`；它按输入次序逐个调用 `can_expr_push_down`，分别追加到 `pushed` 和 `remained`，所以两组内部都保持原相对顺序。
2. 对 TiFlash，`can_expr_push_down` 首先查看节点返回类型：默认拒绝 ENUM、BIT、SET、GEOMETRY、Unspecified 以及非法精度/小数位的 DECIMAL。只有上层标量函数允许 ENUM 时，ENUM 节点能越过此门禁。
3. 节点分派中，常量直接通过；列取决于 `Column.encodable`；`Unsupported` 直接失败；标量函数继续执行签名与能力检查。
4. 标量函数的 `Signature::Unspecified` 永远失败；调试开关启用时先 panic，否则追加警告。合法签名交给 `can_func_be_pushed`：若设置了强制列表，则只有 `all` 或列表中不区分大小写的函数名返回 true，这一路径跳过常规能力表与黑名单；未设置时才依次检查目标存储能力、函数名黑名单和 `函数.签名` 黑名单。
5. 能力通过后，`can_enum_pushdown_preliminarily` 仅对返回 Int/Real/Decimal 的 `cast` 允许其子树携带 ENUM。随后递归检查全部参数；任一参数失败即短路失败。
6. 参数全部通过后仍要求 `ScalarFunction.metadata_serializable == true`。这是 Rust 简化模型对 Go `proto.Marshal(metadata)` 成功条件的显式表示；失败时没有额外警告。
7. TiKV 特例包括：无参当前时间形态的 `unix_timestamp` 不下推；`conv` 的 hybrid/binary CAST 首参不下推；`round`、`rand` 仅允许指定签名；binary charset/collation 的 regexp 家族不下推。TiFlash 还按签名限制 Duration/JSON、字符串 UTF-8 变体、CAST、日期运算、round/truncate/least/greatest 等，并只允许自然语言且无 query expansion 的 `fts_mysql_match_against`。

## 数据与状态

全局可变状态只有黑名单和版本号。黑名单值是被禁用引擎的位集合；`is_push_down_enabled_with_map` 仅在 `disabled & requested_mask == requested_mask` 时判定为禁用。因此查询单一引擎时命中对应位就失败；查询 `Unspecified` 时，只有三个实际引擎位全部被禁用才失败。这与 `pkg/expression/infer_pushdown.go:482-492,593-598` 的 Go 掩码语义一致。

`replace_pushdown_blacklist` 在写锁内清空并整体写入新集合，函数名规范化为 ASCII 小写，然后递增时间戳。它不比较新旧内容；避免无变化更新是上游黑名单加载器的责任（`pkg/executor/reload_expr_pushdown_blacklist.rs` 的 `LoadExprPushdownBlacklist` 先比较 map，再调用其运行时替换边界）。

`PushDownContext` 的克隆共享同一个 `Arc<Mutex<Vec<String>>>` 警告容器，但复制标量配置。`group_concat_max_len` 目前只由 getter 暴露，在本文件判定流程中不参与分支；它为与 Go `PushDownContext.GetGroupConcatMaxLen` 及聚合下推调用保持上下文契约而保留。

表达式树和字段类型是只读借用；批量接口取得 `Vec<Expression>` 所有权，并把原节点移动到两个结果向量，不复制表达式。

## 依赖与调用关系

直接语言依赖仅为标准库的 `HashMap`、原子类型与同步原语。业务类型全部来自同 crate 的 `crate::fts_to_like_kernel::{CastFamily, Expression, FieldKind, ScalarFunction, Signature}`。`pkg/expression/Cargo.toml` 将该 crate 定义为 `astersql-expression`、入口为 `lib.rs`、`autotests = false`；因此 `pkg/expression/lib.rs:689-691` 显式以 `#[cfg(test)]` 装入独立的 `infer_pushdown_test.rs`。

关键内部调用链为：

`can_exprs_push_down -> can_exprs_push_down_with_extra_info -> push_down_exprs_with_extra_info -> can_expr_push_down -> can_func_be_pushed -> scalar_expr_supported_by_* / is_push_down_enabled_with_map`。

`can_expr_push_down` 还递归调用自身检查参数，并调用 `can_enum_pushdown_preliminarily`。能力函数再调用 `flash_supports_cast`、`signature_is`、`signature_in` 或 `is_binary_literal`。

已核实的上游包括：

- `pkg/planner/core/operator/physicalop/physical_projection.rs:148-161`：决定物理投影表达式能否整体下推。
- `pkg/expression/expr_to_pb.rs:303-310`：PB 编码路径通过公开桥接函数查询同一黑名单。
- `pkg/planner/core/integration_test.rs:1559-1588,3021-3044`：集成层分别验证通用下推契约与 TiFlash 的 Time-as-Duration CAST。
- `pkg/expression/aggregation/window_func_test.rs`：通过公开 API 替换黑名单，验证窗口函数的 TiFlash 存储特定禁用行为。

RustCodeGraph 的无文件限定 `explore` 还同时返回了 Go 同名符号；本分析用 `query` 确认 Rust `can_expr_push_down` 只有 `pkg/expression/infer_pushdown.rs:211` 一个定义。精确 `callers/callees --file` 在本地索引上未能在 30 秒工具窗口内返回，因此调用关系以上述 `explore` 流程图、索引文件使用信息和源码/调用点交叉核验。

## 错误处理与边界

本文件的正常“不支持”不是错误，而是 `false` 或进入 `remained`，必要时附加字符串警告。TiFlash 类型不支持、非法 DECIMAL、未指定签名、能力表/黑名单拒绝都会产生警告；不可编码列、`Unsupported` 节点、子参数失败和 metadata 不可序列化本身不额外生成警告（子参数可能已生成自己的警告）。

以下情况会 panic，而非返回错误：黑名单 `RwLock` 中毒、警告 `Mutex` 中毒，以及显式启用 `panic_on_unspecified` 后遇到 `Signature::Unspecified`。这些 panic 文案分别由 `expect("blacklist poisoned")`、`expect("warning handler poisoned")` 和 `panic!("unspecified PbCode: ...")` 给出。

强制下推不是“在正常结果上补充允许”：一旦 `force_pushdown` 存在，未列出的函数会立即被 `can_func_be_pushed` 拒绝，列出的函数会跳过能力表和黑名单；但它不能绕过更早的 Unspecified 签名检查、TiFlash 类型门禁、参数递归或 metadata 条件。空表达式列表会得到两个空向量，且“全部可下推”接口返回 true。

当前 Rust 模型没有 Go 的 `CorrelatedColumn` 独立分支，也不在本文件现场调用 `PbConverter` 验证常量/列编码；常量固定通过，列用 `encodable` 字段承载编码结果。这是已实现模型边界，扩展时不能误认为 Rust 已复制 Go 的完整运行时转换器。

## 并发与资源生命周期

`DEFAULT_EXPR_PUSH_DOWN_BLACKLIST` 通过 `LazyLock` 首次访问初始化；读路径持有共享 `RwLock` 读锁，整体替换持有独占写锁，保证读者不会看到清空与重新填充之间的中间状态。版本号用 `AtomicI64::fetch_add(Ordering::SeqCst)`，为跨线程观察提供全序，但本文件不负责消费或重建计划缓存。

警告处理器通过 `Arc` 共享所有权并以 `Mutex` 串行追加，生命周期可跨多个克隆的 `PushDownContext`。表达式判定本身没有异步任务、通道、事务、文件或网络资源；所有递归均在调用线程同步完成。递归深度与表达式树深度一致，能力表查找是静态切片线性查找，黑名单查询是持读锁的哈希查找。

测试会修改进程级黑名单，因此新增并行测试必须在结束时恢复状态，并避免与其他修改该静态量的测试并发造成相互污染；现有 `pushdown_store_rules_blacklist_warnings_and_order_match_go` 在首尾调用 `clear_pushdown_blacklist`，但这不是跨测试隔离机制。

## 与 Go 版本的对应关系

直接对照是 `pkg/expression/infer_pushdown.go`。Rust 的 `can_func_be_pushed`、`can_expr_push_down`、三类存储能力函数、`can_enum_pushdown_preliminarily`、`IsPushDownEnabled` 掩码逻辑和四个批量 API 分别对应 Go 的同名 camelCase/PascalCase 实现。能力白名单与签名特例按 Go `switch` 展开；独立测试 `pkg/expression/infer_pushdown_test.rs` 特别锁定了 Go 明确允许和未允许的 TiFlash round/truncate/least/greatest 签名，并验证 `NullEQInt`。

主要表示差异如下：

- Go 黑名单是 `atomic.Pointer[map[string]uint32]`，Rust 使用 `RwLock<HashMap<...>>`；Go 时间戳记录“重载时间”，本文件的原子值是每次替换加一的序号。
- Go `PushDownContext` 还携带 `EvalContext`、`kv.Client` 并可创建 `PbConverter`；Rust 上下文没有这些字段，只保存本策略实际使用的状态。
- Go 通过 failpoint `PushDownTestSwitcher` 和 `PanicIfPbCodeUnspecified` 控制测试分支；Rust 用构建器字段表达相同的可控行为，不依赖 failpoint。
- Go 对 Constant/Column/CorrelatedColumn 调用 PB 转换器验证可编码性，并实际 `proto.Marshal(metadata)`；Rust 表达式模型没有独立 CorrelatedColumn，分别用“常量总可用”、`Column.encodable` 和 `metadata_serializable` 表达既成结果。
- Go 的警告是 `error` 交给 `WarnAppender`；Rust 收集格式化字符串。两者都只有在 normal 与 extra handler 同时存在时选择实际 handler，EXPLAIN 走 normal。
- Go 空批量显式返回两个 nil slice；Rust 返回两个空 `Vec`。在长度与“全部可下推”判断上语义一致，但容器的 nil/empty 表示不可直接等同。

## 扩展指南

新增或调整函数下推能力时，先确定目标引擎和 Go 依据：普通 TiKV/TiFlash 函数分别更新 `TIKV_FUNCTIONS`/`TIFLASH_FUNCTIONS`；依赖签名、参数、字符集或返回类型的规则应放进对应 `scalar_expr_supported_by_*` 分支，避免把条件函数放进无条件白名单。TiFlash CAST 规则集中修改 `flash_supports_cast`，ENUM 传播规则集中修改 `can_enum_pushdown_preliminarily`。

每次能力变化至少同步独立 Rust 测试，不把测试嵌入生产文件：优先扩展 `pkg/expression/infer_pushdown_test.rs`；涉及黑名单、警告、顺序、ENUM 或 FTS 时同步 `pkg/expression/fts_to_like_36_aster_unit_test.rs` / `fts_to_like_test.rs`；涉及规划器实际接线时增加或调整 `pkg/planner/core/integration_test.rs` 或最近的物理算子独立测试。应同时核对 `pkg/expression/infer_pushdown.go` 及其 Go 测试，防止 Rust 支持集合超前或遗漏 Go 行为。

新增存储类型时必须成组审查 `StoreType` 判定值、`mask()` 的 Unspecified 并集、`name()`、`can_func_be_pushed` 分派、`pkg/expression/expr_to_pb.rs` 和规划器中的类型映射，以及黑名单加载器的 store mask。枚举判别值参与持久黑名单位语义，不能随意重排。

动态黑名单扩展应保持两级键（函数名与 `函数.小写签名`）以及整体替换原子性。性能风险主要来自扩大静态切片后的线性查找、在热规划路径增加锁持有时间，或让递归重复构造字符串；正确性风险主要是错误放宽 TiFlash 类型/签名、绕过 metadata 可序列化要求，或使 Rust 与 Go/PB 协议支持集漂移。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、目标文件已索引且含 41 个符号；`explore 'pkg/expression/infer_pushdown.rs StoreType PushDownContext can_expr_push_down can_func_be_pushed replace_pushdown_blacklist'` 给出核心调用边和上下游候选；`query can_expr_push_down --kind function` 唯一命中本文件第 211 行；`node --file ...` 分段读取了目标 Rust 文件、Go 对照、独立测试及 `lib.rs` 装配。精确 callers/callees 查询因本地命令超过 30 秒窗口未返回，未将其当作额外完成证据。
- 源码与装配：`pkg/expression/infer_pushdown.rs`、`pkg/expression/fts_to_like.rs`、`pkg/expression/lib.rs:66-79,344-345,689-691`。
- crate 边界：`pkg/expression/Cargo.toml`（包名 `astersql-expression`，`lib.rs` 入口，`autotests = false`，并声明该 expression crate 的完整依赖边界）；本文件自身仅使用标准库及同 crate 类型。
- Go 对照：`pkg/expression/infer_pushdown.go:40-598`，覆盖全局状态、能力表、递归判定、上下文、批量接口与存储位掩码。
- Rust 测试：`pkg/expression/infer_pushdown_test.rs`；`pkg/expression/fts_to_like_36_aster_unit_test.rs:338-411`；`pkg/expression/fts_to_like_test.rs:233-251`；`pkg/expression/aggregation/window_func_test.rs`；`pkg/planner/core/integration_test.rs:1559-1588,3021-3044`。
- 直接接线：`pkg/planner/core/operator/physicalop/physical_projection.rs:148-161`、`pkg/expression/expr_to_pb.rs:303-310`、`pkg/executor/reload_expr_pushdown_blacklist.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档恰好具有 11 个固定二级章节，并人工复核本文没有把 Go 的完整 PB 转换器、缓存失效消费端或黑名单加载运行时误写成本文件现有实现。
