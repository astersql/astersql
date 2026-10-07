# `pkg/expression/exprstatic/evalctx.rs`

## 文件定位

本文件实现 `astersql-expression-exprstatic` crate 的静态求值上下文。crate 入口
`pkg/expression/exprstatic/lib.rs` 以私有模块 `evalctx` 装载它，再通过
`pub use evalctx::*` 导出公开 API。这里的“静态”不是编译期常量，而是指上下文不依赖仍在
变化的会话对象：SQL 表达式可以持有一份字段基本不可变、必要资源通过 `Arc` 共享的快照。

`EvalContext` 位于表达式执行的基础层：上层 `ExprContext` 持有它；标量表达式、类型转换、
错误分级、计划缓存参数和需要语句级当前时间的函数通过 `exprctx::EvalContext` trait 使用它。
生产调用的直接例子包括 `pkg/expression/exprstatic/exprctx.rs` 中的
`NewExprContext`、`ExprContext::LoadSystemVars` 和 `MakeExprContextStatic`，以及
`pkg/session/runtime/planning.rs`、`pkg/session/runtime/system_session.rs`、
`pkg/session/runtime/modify_column_backfill.rs` 等静态执行场景。

`pkg/expression/exprstatic/Cargo.toml` 将本目录定义为独立 library crate，直接依赖
`astersql-expression-exprctx`、`astersql-expression-expropt`、`astersql-types`、
`astersql-errctx`、parser 的 MySQL/charset crate、sessionctx 的 vardef/variable crate，
以及 `chrono`/`chrono-tz`。因此本文件既是 trait 的实现者，也是这些较低层上下文和系统变量
组件的组合边界。

## 核心职责

本文件承担五组职责：

1. 用 `EvalCtxState` 汇集 SQL mode、`types::Context`、`errctx::Context`、时区、当前库、
   告警、语句时间、预处理参数、用户变量及可选求值属性。
2. 用 `EvalCtxOption` 和一组 `With*` 函数构造或派生上下文，并确保类型上下文和错误上下文
   始终绑定到最终选定的同一个告警处理器。
3. 通过 `TimeOnce` 提供语句级 `CurrentTime`：失败允许重试，首次成功后缓存，并按上下文
   时区规范化。
4. 实现 `contextutil::{WarnAppender, WarnHandler}`、`exprctx::{ParamValues,
   EvalContext, StaticConvertibleEvalContext}`，使静态对象可直接进入统一表达式接口；
   `MakeEvalContextStatic` 则把任意可静态化实现物化为独立快照。
5. `parse_system_vars` 与 `LoadSystemVars` 校验并归一化系统变量。解析结果
   `ParsedSystemVars` 也供相邻 `exprctx.rs` 使用，使 Eval/Expr 两层基于同一次解析结果更新。

它不负责执行具体表达式，也不保存完整 session。字符集、collation、加密模式、窗口精度等
属于 `ExprContext` 的字段虽然在 `ParsedSystemVars` 中解析，但不会由
`EvalContext::load_system_vars_internal` 写入 `EvalCtxState`。

## 主要符号

- `TimeOnce { lock, time, time_fn }`：内部语句时间缓存。`get_time(location)` 先无锁读取
  `OnceLock`，未命中时以 `Mutex` 串行调用回调；只有成功值才写入缓存。
- `WarnHandlerAdapter`：把共享的 `contextutil::WarnHandler` 适配成 `types::Context` 和
  `errctx::Context` 需要的 `WarnAppender`。
- `EmptyUserVarsReader`：默认用户变量读取器，所有变量均返回 `None`，并支持 trait 对象克隆。
- `EvalCtxState`：实际状态载体。手写 `Clone` 对字符串、参数 Datum、类型/错误上下文和用户变量
  做克隆，对告警处理器、当前时间缓存、可选属性注册表做 `Arc` 共享。
- `EvalCtxOption = Box<dyn FnOnce(&mut EvalCtxState)>`：构造选项。公开的 `WithWarnHandler`、
  `WithSQLMode`、`WithTypeFlags`、`WithLocation`、`WithErrLevelMap`、`WithCurrentDB`、
  `WithCurrentTime`、`WithMaxAllowedPacket`、`WithDefaultWeekFormatMode`、
  `WithDivPrecisionIncrement`、`WithOptionalProperty`、`WithParamList`、
  `WithEnableRedactLog`、`WithUserVarsReader` 分别替换一个状态维度。
- `EvalContext { id, state }`：公开静态求值上下文。`CtxID` 标识实例；访问器实现
  `exprctx::EvalContext` 的稳定接口。
- `NewEvalContext(options)`：按服务器默认值创建状态，顺序应用选项，再调用
  `bind_dependent_contexts` 重建类型/错误上下文，最后分配新 ID。
- `EvalContext::Apply(options)`：克隆现有状态形成新上下文；默认复用原语句时间的结果，允许
  选项覆盖字段，并产生新的 `CtxID`。
- `EvalContext::LoadSystemVars`：调用 `parse_system_vars` 完成校验，再仅对输入 map 中实际出现且
  属于 Eval 层的变量创建选项并 `Apply`。
- `MakeEvalContextStatic(context)`：从 `StaticConvertibleEvalContext` 读取稳定字段，冻结
  `CurrentTime` 的当前结果，克隆参数和用户变量；告警处理器尽可能复用原 `Arc`，可选属性
  有意清空。
- `ParsedSystemVars` / `parse_system_vars`：crate 内共享的系统变量解析结果与解析入口。
  辅助函数 `parse_bool`、`parse_clamped_unsigned`、`normalize_enum`、`parse_timestamp` 分别处理
  布尔、带范围无符号数、枚举和 Unix 时间戳。
- `StaticGlobalVarAccessor`：解析无关但已注册的系统变量时，为临时 `variable::SessionVars`
  提供只读默认全局值；TiDB 表存储接口明确返回 unknown-variable 错误。

本文件没有条件编译项；测试由 `lib.rs` 以 `#[cfg(test)]` 从独立文件
`evalctx_test.rs` 接入，符合生产代码与测试逻辑分离要求。

## 执行流程

默认构造流程如下：

1. `NewEvalContext` 创建容量为零限制的静态告警处理器，并生成对应 appender。
2. 状态采用默认 SQL mode、`types::StrictFlags`、UTC、严格错误级别、空数据库、懒取墙钟的
   `TimeOnce`、vardef 默认值、空参数、空用户变量和空可选属性。
3. 选项按传入顺序执行；同一字段被多次设置时，后一个选项生效。
4. `bind_dependent_contexts` 保留最终的 flags/location/level map，但重建 `type_ctx` 和
   `err_ctx`，保证它们把告警写入最终的 `warn_handler`。
5. `GenContextID` 为新对象分配 ID。

读取当前时间时，`TimeOnce::get_time` 先查询成功缓存；未命中后加锁并二次查询，以避免并发
重复执行回调。回调错误原样返回且不写 `OnceLock`，所以下次仍会重试。成功时间用调用时的
`location` 转换后写入缓存，之后同一 `TimeOnce` 始终返回该值。

`Apply` 的派生流程先克隆状态，再把新状态的 `current_time` 改成一个包装旧
`TimeOnce::get_time(previous_location)` 的新 `TimeOnce`。因此不覆盖时间时，派生上下文继承
相同瞬间；若只覆盖时区，派生缓存会把该瞬间转换到新时区；若传入 `WithCurrentTime`，则完全
替换回调。选项应用后再次绑定依赖上下文，原对象不被修改。

`LoadSystemVars` 先把 key 按 ASCII 小写匹配。`parse_system_vars` 从默认状态出发，收集 charset
和 collation 后按固定顺序处理，避免 `HashMap` 迭代顺序改变结果；`timestamp` 也延迟到最终
时区确定后解析。解析成功后，Eval 层只应用调用方实际提供的 `time_zone`、`sql_mode`、
`timestamp`、`max_allowed_packet`、`tidb_redact_log`、`default_week_format` 和
`div_precision_increment`。相邻 `ExprContext::LoadSystemVars` 使用同一 `ParsedSystemVars`，再应用
charset/collation、加密模式、SYSDate、noop 函数、窗口精度和 group-concat 长度等字段。

## 数据与状态

`EvalContext` 对外表现为不可变值对象，但内部包含有意共享的并发安全资源：

- `id` 每次 `NewEvalContext`、`Apply` 或 `MakeEvalContextStatic` 都重新生成，不能用内容相等
  推断 ID 相等。
- `warn_handler: Arc<dyn WarnHandler + Send + Sync>` 在普通 `Apply` 中共享，所以派生对象追加
  告警会反映到同一列表；`WithWarnHandler` 可切换到独立列表。
- `type_ctx` 和 `err_ctx` 是值式克隆，但其中的 appender 始终在构造末尾重新指向当前告警
  handler，避免 option 顺序留下旧绑定。
- `current_time: Arc<TimeOnce>` 在状态克隆时共享；`Apply` 再包一层，以支持派生时区，同时
  不让派生对象改变原对象的缓存表示。
- `param_list` 在 `WithParamList` 和 `AllParamValues` 中按 `Vec<Datum>` 克隆；构造完成后与调用方
  的原 vector 解耦，`GetParamValue` 也返回克隆的 Datum。
- `user_vars` 通过 `UserVarsReader::Clone` 复制 trait 对象，默认读取器不保存变量。
- `props` 用 `Arc<OptionalEvalPropProviders>` 共享；`WithOptionalProperty` 构造全新注册表，语义
  是整体替换而非增量合并。

`ParsedSystemVars` 是一次解析的完整中间状态。即使某字段仅供 `ExprContext` 使用，也在这里统一
保存，以避免两个上下文分别解析同一输入而出现顺序或归一化差异。

## 依赖与调用关系

上游关系经 RustCodeGraph 核验：

- `NewEvalContext` 被 `exprctx.rs::NewExprContext` 直接调用，也用于 planner/session runtime、
  aggregation descriptor、meta build 和 table partition expression 等需要脱离完整 session 的场景。
- `MakeEvalContextStatic` 的生产调用者是 `exprctx.rs::MakeExprContextStatic`；
  `pkg/expression/sessionexpr/sessionctx.rs` 通过它把会话求值上下文转换成静态实现。
- `parse_system_vars` 被本文件的 `EvalContext::LoadSystemVars` 和相邻文件的
  `ExprContext::LoadSystemVars` 调用。

下游依赖分工为：

- `contextutil` 提供上下文 ID、共享错误、告警接口与静态告警存储。
- `types` 提供 Datum、FieldType、类型 flags、时区和类型转换告警入口。
- `errctx` 提供错误组到 Error/Warn/Ignore 的映射。
- `exprctx` 定义被本文件实现的接口、参数越界错误、用户变量和可选属性 trait。
- `expropt` 保存 key 到 provider 的注册表。
- `mysql` 解析 SQL mode 并提供字符集默认值；`charset` 校验字符集/校对名称。
- `vardef` 定义系统变量名称、默认值和范围上限；`variable::SessionVars` 校验本文件不专门处理
  但已注册的系统变量。
- `chrono`/`chrono-tz` 表示绝对时间与命名时区。

RustCodeGraph 将本文件标记为被 73 个文件使用；这个数字包含测试和间接符号引用，文档不把它
解释为 73 条运行时调用边。上面的主链只列出由图查询及生产源码直接确认的关系。

## 错误处理与边界

- `TimeOnce::get_time` 传播时间回调的 `SharedError`，失败不缓存。其互斥锁若 poisoned 会以
  `expect("timeOnce mutex poisoned")` panic；这被视为进程内同步不变量破坏，而不是 SQL 错误。
- 默认 SQL mode 在 `OnceLock` 初始化时解析；服务器内置默认字符串若无效会 panic，因为该值
  是编译期/配置常量不变量。
- `GetParamValue(index)` 的 Rust 参数为 `usize`，因此不存在 Go 版的负索引分支；越过 vector
  长度时返回 `exprctx::ErrParamIndexExceedParamCounts`。
- `parse_bool` 只接受大小写不敏感的 ON/OFF 和精确的 1/0。
- `parse_clamped_unsigned` 对合法负整数裁剪到下限，对正整数裁剪到闭区间；不能解析的文本报
  `invalid value ... for system variable ...`。`max_allowed_packet` 另向下对齐到 1024 字节。
- `normalize_enum` 接受大小写不敏感名称或从零开始的枚举索引。
- `parse_timestamp` 拒绝非数字、NaN/Infinity 和 chrono 无法表示的时间；负小数通过借一秒并补
  纳秒保持正确瞬间。`timestamp=vardef::DefTimestamp` 不固定解析时的墙钟，而在装入上下文后
  由 `CurrentTime` 回调懒求值。
- `time_zone` 当前使用 `chrono_tz::Tz::from_str`，只接受命名时区数据库中的值；本文件没有
  实现 Go `SessionVars` 可能支持的其他时区文本形式，扩展时需先确认兼容目标。
- 未知或非 Eval/Expr 专属变量交给临时 `SessionVars::SetSystemVar` 校验：已注册且值合法则忽略，
  未知或非法值返回错误。`StaticGlobalVarAccessor` 不读写 TiDB 表。
- `MakeEvalContextStatic` 会先取得 `CurrentTime` 的 `Result` 并把该结果封入回调：若源上下文当时
  返回错误，静态对象会稳定地重复该错误，而不是重新访问已变化的 session。

## 并发与资源生命周期

`TimeOnce` 是本文件最关键的并发结构。`OnceLock<DateTime<Tz>>` 支持成功结果的无锁读取，
`Mutex<()>` 只覆盖首次求值窗口；双重检查保证多个线程竞争时通常只执行一次成功回调。错误不写
缓存，因此连续失败可能由不同调用依次重试。锁在回调执行期间持有，故时间回调应短小且不应
反向等待同一个 `TimeOnce`。

能够跨线程共享的回调和告警对象均要求 `Send + Sync`。告警的实际并发正确性由
`contextutil::WarnHandler` 实现保证；本文件只通过 `Arc` 和 adapter 维持身份一致性。
`OptionalEvalPropProviders` 与时间缓存通过 `Arc` 管理生命周期，`EvalContext` 销毁时只有最后一个
引用释放资源。用户变量读取器依赖其 `Clone` 契约，是否深拷贝内部数据由具体实现负责。

普通 `Apply` 共享告警状态是设计语义，不是隔离边界。若调用方需要独立告警生命周期，必须显式
传入 `WithWarnHandler`；若需要新的语句时间，必须显式传入 `WithCurrentTime`。参数和字符串字段
则是拥有式副本，不依赖调用方容器生命周期。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/exprstatic/evalctx.go`，测试对照为同目录
`evalctx_test.go`。Rust 保留了 Go 的核心结构：`timeOnce`/`TimeOnce`、私有 state、函数式 option、
`EvalContext`、`Apply`、参数快照、系统变量加载和 `MakeEvalContextStatic`；Rust 测试名称也基本与
Go 测试一一对应。

主要语言适配和当前差异如下：

- Go 用嵌入 `evalCtxState` 和接口值；Rust 用显式 `state`、trait object、`Box`/`Arc`，并通过
  trait impl 转发公开方法。
- Go 的 `timeOnce` 使用 `atomic.Pointer` 加 mutex；Rust 使用 `OnceLock` 加 mutex，均只缓存首次
  成功值。
- Go 构造时先让 `typeCtx`/`errCtx` 以 `ctx` 自身作为告警 appender；Rust 通过
  `WarnHandlerAdapter` 和 `bind_dependent_contexts` 达到同一共享告警效果。
- Go `Apply` 是 state 浅拷贝后重建依赖上下文；Rust 的 `Clone` 明确规定每个字段是复制、克隆
  还是 `Arc` 共享。两者都分配新 ID、默认继承时间并允许 option 覆盖。
- Go 的 `WithParamList` 从 `PlanCacheParamList` 抽取 Datum；Rust API 直接接收 `Vec<Datum>`。
- Go `MakeEvalContextStatic` 复用 `GetWarnHandler`；Rust trait 新增可选的
  `GetWarnHandlerArc`，有 Arc 时保留同一 handler，否则以静态 handler 包装原读取结果。
- 两版都明确不复制 optional properties。Rust 用空 vector 表达该 TODO/边界。
- Go 借助完整 `variable.SessionVars` 执行系统变量规范化；Rust 对关键变量显式实现解析，并对
  其余已注册变量调用临时 `SessionVars` 校验。Rust 独立测试额外固定了数值裁剪、
  `max_allowed_packet` 对齐、redact 原值保留、无关注册变量校验和默认 timestamp 懒求值。
- Rust `default_sql_mode` 先调用 `FormatSQLModeStr` 再解析；Go 直接解析默认 mode 字符串。两者
  当前测试断言相同默认结果。

因此后续修改不能只让 Rust 测试通过；应先确认 Go 的实际语义，并保留 Rust 因所有权、线程安全
和现有 trait 契约所需的局部适配。

## 扩展指南

新增一个 Eval 层字段时，至少检查以下接入点：

1. 在 `EvalCtxState` 增加字段并更新手写 `Clone`。
2. 为外部构造提供 `With*` option，在 `NewEvalContext` 设置与 Go/vardef 一致的默认值，并增加
   只读访问器。
3. 若该字段属于 `exprctx::EvalContext` 或静态转换契约，同步对应 trait crate 及本文件 trait
   impl；外部依赖必须按仓库规则在其独立上游仓库发布 tag，不能本地 vendor 或 patch。
4. 在 `Apply`、`MakeEvalContextStatic` 中明确该字段应复制、共享、重建还是清空。
5. 若来自系统变量，扩展 `ParsedSystemVars`、`parse_system_vars` 和
   `load_system_vars_internal`；若同时影响 Expr 层，还要同步
   `pkg/expression/exprstatic/exprctx.rs`，并保持 charset/collation 等顺序不变量。
6. 在独立的 `pkg/expression/exprstatic/evalctx_test.rs` 增加默认值、option、Apply 隔离、静态
   转换、合法/非法系统变量和边界归一化测试；不要把测试嵌入生产文件。若 Go 版同时演进，核对
   `evalctx.go` 与 `evalctx_test.go` 的新增行为。

修改告警、时间或可选属性时风险较高：告警 handler 更换后必须重新绑定 `type_ctx`/`err_ctx`；
时间逻辑必须保留“错误可重试、成功只缓存一次、时区转换不改变瞬间”；optional providers 当前
是整体替换且静态化清空。系统变量解析应优先复用 variable/charset/mysql 的权威定义，避免在此
复制一套会漂移的名称、默认值或枚举表。

性能方面，热路径访问器应继续保持简单克隆或 `Arc` 借用；不要让每次表达式求值重新解析系统
变量、读取 session 或加全局锁。若扩展 `TimeOnce` 回调，还需评估首次调用持锁期间的阻塞。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引可用，覆盖 11,467 个文件；`files --filter
  pkg/expression/exprstatic/evalctx.rs` 确认目标文件已索引并含 96 个符号。
- RustCodeGraph `node --file pkg/expression/exprstatic/evalctx.rs`：完整读取 891 行生产源码，核对
  `TimeOnce`、`EvalCtxState`、全部 option、`EvalContext`、trait impl、
  `MakeEvalContextStatic`、`ParsedSystemVars` 和解析辅助函数。
- RustCodeGraph 精确节点：`NewEvalContext` 的调用轨迹包含
  `MakeEvalContextStatic` 及独立测试；`MakeEvalContextStatic` 的生产调用者为
  `MakeExprContextStatic`；`parse_system_vars` 的调用者为 Eval/Expr 两层 `LoadSystemVars`。
- crate 与模块证据：`pkg/expression/exprstatic/Cargo.toml`、
  `pkg/expression/exprstatic/lib.rs`、`pkg/expression/exprstatic/exprctx.rs`。
- Go 对照证据：`pkg/expression/exprstatic/evalctx.go` 和
  `pkg/expression/exprstatic/evalctx_test.go`。
- Rust 独立测试证据：`pkg/expression/exprstatic/evalctx_test.rs`，覆盖默认/全选项构造、时间
  重试与缓存、告警共享与替换、optional properties 整体替换、Apply、参数快照、静态物化、
  系统变量加载、默认 timestamp 懒求值、无关注册变量校验和 Go 归一化规则。

本任务是纯文档分析，按计划未运行 Cargo，也未据此声称代码测试在当前环境通过。文档结构另以
任务指定命令验证必须恰有 11 个固定二级章节；关键关系同时经图索引和上述相邻源码人工复核。
