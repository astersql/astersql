# `pkg/sessionctx/variable/sysvar.rs`

## 文件定位

本文件属于 `astersql-sessionctx-variable` crate，由 `pkg/sessionctx/variable/lib.rs` 以 `pub mod sysvar` 暴露。它移植了 Go `pkg/sessionctx/variable/sysvar.go` 中四组相对独立的逻辑：执行器并发变量的构造片段、安装时系统变量默认值选择、TiFlash 计算派发策略写入，以及流水线 DML 资源策略解析。

这里的 `sysvar::SysVar` 与 `sysvar::SessionVars` 是上述回调所需字段的精简模型，不是 crate 的完整系统变量注册表类型。完整 `SessionVars`、完整 `SysVar` 及钩子类型位于 `pkg/sessionctx/variable/variable.rs`，内建变量注册在 `pkg/sessionctx/variable/sysvar_builtins.rs`。因此，阅读本文件时不能把精简模型的 `SetSession` 直接等同于正式注册表的 `SetSessionFromHook` 链路。

当前可确认的生产接线是：`pkg/session/runtime/control.rs` 在执行 `SET <system-variable> = DEFAULT` 时调用 `sysvar::GlobalSystemVariableInitialValue`；`pkg/sessionctx/variable/nextgen.rs` 用显式的 next-gen `RuntimeEnvironment` 包装 `GlobalSystemVariableInitialValueWithRuntime`。源码检索未发现 `newExecConcurrencySysVar`、`setTiFlashComputeDispatchPolicy` 或 `setPipelinedDmlResourcePolicy` 的非测试 Rust 调用者，它们目前主要由独立测试验证。

## 核心职责

1. `newExecConcurrencySysVar` 统一构造全局与会话双作用域的整数并发变量元数据，并通过选项闭包覆盖 `AllowAutoValue` 或 `MinValue`。
2. `GlobalSystemVariableInitialValue` 从当前配置、测试编译状态和内核类型采样环境；`GlobalSystemVariableInitialValueWithRuntime` 根据变量名选择新安装默认值，未命中的变量保持调用者提供值。
3. `setTiFlashComputeDispatchPolicy` 委托 `tiflashcompute::GetDispatchPolicyByStr` 解析字符串，仅在解析成功后修改会话字段。
4. `setPipelinedDmlResourcePolicy` 支持 `standard`、`conservative` 和 `custom{...}` 三类策略，并以临时配置实现失败原子性。
5. `SysVarError` 为本文件的精简回调保存变量名、输入值和面向用户的错误文本。

这些职责都围绕“把字符串形式的系统变量安全地映射为会话侧强类型状态”；本文件不负责全局注册表、持久化、权限检查或 SQL 层作用域校验。

## 主要符号

- `SessionVars`：仅包含 `ExecutorConcurrency`、`TiFlashComputeDispatchPolicy` 和 `PipelinedDMLConfig`。其 `Default` 分别使用 `vardef::ConcurrencyUnset`、`DispatchPolicyInvalid` 和流水线默认配置。
- `PipelinedDMLConfig`：保存 flush 并发、resolve-lock 并发和写节流比例。默认值来自 `vardef::DefaultFlushConcurrency`（128）、`DefaultResolveConcurrency`（8）与 `0.0`。
- `SysVarError`：私有字段由 `wrong_value` 或 `dependency` 构造；公开的 `variable()`、`value()` 允许测试或调用者识别失败输入，`Display` 返回保存的消息。
- `ConcurrencySetter`：`Arc<dyn Fn(&mut SessionVars, i32) + Send + Sync>`，让构造出的变量可共享线程安全的状态写入回调。
- `ExecConcurrencySysVarOption`：一次性选项闭包；`withAllowAutoValue` 与 `withMinValue` 是当前两个构造器。
- `SysVar`：执行器并发变量的精简元数据，含作用域、名称、默认值、类型、上下界、auto 标志及私有 setter。`SetSession` 将正整数传给 setter，解析失败、零和负数统一回退到 `ConcurrencyUnset`，函数本身返回 `Ok(())`。
- `newExecConcurrencySysVar`：默认生成 `ScopeGlobal | ScopeSession`、`TypeInt`、最小值 1、最大值 `MaxConfigurableConcurrency`（256）、允许 auto 的定义，再按传入顺序应用选项。
- `RuntimeEnvironment`：将 `store_is_tikv`、`in_test`、`next_gen` 与事务断言默认值显式化。`current()` 从全局配置、`cfg!(test)`、`kerneltype::IsNextGen()` 和 vardef 读取快照。
- `GlobalSystemVariableInitialValue` / `GlobalSystemVariableInitialValueWithRuntime`：前者采样真实环境，后者是可确定性测试的纯分支函数。
- `setTiFlashComputeDispatchPolicy`：解析并提交 TiFlash 策略。
- `setPipelinedDmlResourcePolicy` / `parse_pipelined_concurrency`：解析预设或自定义流水线资源策略，并校验并发范围 `[1, 8192]`。

文件没有条件编译项；`#![allow(dead_code, non_snake_case, non_upper_case_globals)]` 允许暂未接线的移植符号和 Go 风格 API 命名保留。

## 执行流程

执行器并发构造与写入流程如下：

1. `newExecConcurrencySysVar` 用固定作用域、整数类型、范围和默认 auto 行为构造 `SysVar`。
2. 选项迭代器按顺序执行；同一字段被多次设置时，后一个选项覆盖前一个。
3. 调用 `SysVar::SetSession` 时尝试解析 `i32`，仅接受大于零的值；其他输入变为 `ConcurrencyUnset`。
4. 最终值交给构造时保存的 `ConcurrencySetter`，本层不再做范围截断，也不产生错误。

安装默认值流程如下：

1. 真实入口 `GlobalSystemVariableInitialValue` 调用 `RuntimeEnvironment::current()`。
2. `GlobalSystemVariableInitialValueWithRuntime` 按变量名匹配：TiKV 存储下为 async commit 和 1PC 返回 `ON`；测试环境将 OOM action 改为 `LOG`、auto analyze 改为 `OFF`。
3. 新安装固定覆盖 row format v2、mutation checker `ON`、adaptive LIMIT scan `ON`；事务断言级别及悲观事务公平锁按 classic/next-gen 分支选择。
4. 未列出的变量原样返回 `var_value`。`pkg/session/runtime/control.rs` 以正式注册表中的名称和默认值调用它，得到 `SET ... = DEFAULT` 的实际字符串。

流水线 DML 资源策略流程如下：

1. 输入先 `trim`，再转 ASCII 小写，所以策略名、键名大小写不敏感，错误对象保存的是去掉首尾空白后的值。
2. `standard` 直接提交默认配置；`conservative` 提交 2/2/0.0 配置。
3. 其他输入必须为 `custom{...}` 且花括号内容非空；内容按逗号拆成参数。
4. 每个参数以 `=` 或 `:` 分隔，并过滤空字段，以对齐 Go `strings.FieldsFunc`；因此测试覆盖了 `=concurrency==8` 和 `write_throttle_ratio=:0.25` 这类重复或边缘分隔符。
5. 仅接受 `concurrency`、`resolve_concurrency` 和 `write_throttle_ratio`。并发经 `parse_pipelined_concurrency` 校验为 1 到 8192；比例要求 `0 <= ratio < 1`。
6. 所有参数先写入 `new_config`，全部成功后才一次性替换 `vars.PipelinedDMLConfig`，任何中途错误都不改变原配置。

## 数据与状态

本文件修改的都是调用者提供的 `&mut SessionVars`，没有在模块内维护注册表或可变静态量。三个状态区域彼此独立：执行器并发是 `i32`，TiFlash 策略是 `DispatchPolicy`，流水线策略是三字段结构体。

`RuntimeEnvironment::current()` 会读取进程级全局配置和内核类型，但把结果复制为一个普通值对象；后续默认值计算不持有配置引用。显式的 `GlobalSystemVariableInitialValueWithRuntime` 不读写全局状态，便于覆盖 classic、next-gen、TiKV 和测试环境组合。

流水线自定义策略没有要求三个键全部出现：临时配置先取标准默认值，只覆盖输入中出现的字段。重复键也没有被拒绝，后一次赋值覆盖前一次。比例使用 Rust `f64::parse` 后只检查 `< 0` 或 `>= 1`；与 Go 的比较方式一致，`NaN` 不会被这两个比较拒绝，这是现有代码边界而不是额外保证。

## 依赖与调用关系

- crate 边界：`pkg/sessionctx/variable/Cargo.toml` 声明包名 `astersql-sessionctx-variable`，本文件直接使用其中的 `config`、`kerneltype`、`tiflashcompute` 与 `vardef` 路径依赖；没有 feature 条件。
- 上游生产调用：`pkg/session/runtime/control.rs` 的系统变量 `DEFAULT` 求值分支调用 `GlobalSystemVariableInitialValue`；`pkg/sessionctx/variable/nextgen.rs::GlobalSystemVariableInitialValue` 调用显式运行时版本。
- 下游默认值依赖：`config::get_global_config().store`、`kerneltype::IsNextGen()`、`vardef::GetDefaultTxnAssertionLevel()` 以及 vardef 中的变量名、字符串和数值常量。
- 下游 TiFlash 依赖：`tiflashcompute::GetDispatchPolicyByStr` 完成字符串到 `DispatchPolicy` 的校验和转换。
- 测试调用：`sysvar_3_aster_unit_test.rs` 直接覆盖全部四组逻辑；`sysvar_test.rs` 覆盖动态默认值和 Go `FieldsFunc` 分隔语义；`varsutil_test.rs` 还覆盖并发默认状态与 setter。
- RustCodeGraph 将本文件标为被 `stmtctx.rs`、`session.rs`、`sysvar_builtins.rs` 和相关测试文件使用，但精确源码检索没有找到前三者对本文件核心 API 的直接调用；因此不将这类文件级图边解释为已验证的函数调用边。

## 错误处理与边界

`SysVar::SetSession` 有意把无效整数视为“未设置”，不会返回 `SysVarError`；它也只检查正数，不在此处应用 `MinValue`、`MaxValue` 或 `AllowAutoValue`。正式系统变量链通常应在调用 setter 前完成类型和范围归一化，直接调用该精简 API 时需自行理解这一前置条件。

`setTiFlashComputeDispatchPolicy` 以 `?` 提前返回解析错误，并通过 `SysVarError::dependency` 保留下游错误文本；字段赋值发生在解析完成之后，所以失败不改变原策略。

流水线解析的所有格式错误、未知键、整数解析错误、并发越界和比例越界都变为 `wrong_value`。该错误文本与 MySQL/TiDB 风格一致，并保存变量名 `TiDBPipelinedDmlResourcePolicy`。临时配置保证失败原子性，但函数不诊断重复键，也不记录 Go 实现中的警告日志。

`RuntimeEnvironment::current()` 没有错误返回；若全局环境在采样后变化，本次计算仍使用已取得的快照。未命中的变量不被验证，只原样返回输入默认值。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务、文件句柄或网络连接。`ConcurrencySetter` 使用 `Arc` 且要求闭包 `Send + Sync`，允许多个 `SysVar` 持有者安全共享回调对象；真正的状态修改仍要求独占的 `&mut SessionVars`，因此本层无需锁。

`ExecConcurrencySysVarOption` 是 `FnOnce`，在构造阶段被消费，不会留到运行期。`RuntimeEnvironment`、`PipelinedDMLConfig` 和错误对象都拥有其字符串或数值数据，不借用外部生命周期。流水线配置先在栈上构造，成功时整体移动到会话；错误路径自然丢弃临时值。

全局配置读取发生在 `RuntimeEnvironment::current()` 调用瞬间；本文件不负责同步配置更新。正式完整会话类型的线程安全声明位于其他文件，不应由这里的精简 `SessionVars` 推导。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/sessionctx/variable/sysvar.go`：

- Rust `withAllowAutoValue`、`withMinValue` 和 `newExecConcurrencySysVar` 对应 Go 同名函数（约 102--166 行）。默认元数据与 Go 一致，setter 的正整数解析/回退语义也对应 `tidbOptPositiveInt32`。
- 差异是 Go 构造出的完整 `SysVar` 还安装 `Validation` 钩子，用于追加旧并发变量指向 `tidb_executor_concurrency` 的弃用警告；本文件精简 `SysVar` 没有 `Validation` 字段，因此没有承载这段告警语义。
- `GlobalSystemVariableInitialValueWithRuntime` 对应 Go `GlobalSystemVariableInitialValue`（约 4333--4372 行），通过显式环境参数把 Go 的全局配置、`intest.InTest` 和内核检测变为可测试输入。变量分支及“未命中原样返回”保持一致。
- `setTiFlashComputeDispatchPolicy` 对应 Go 4374--4381 行，均为先解析、后赋值。
- `setPipelinedDmlResourcePolicy` 对应 Go 4383--4487 行：大小写归一化、三种策略、`FieldsFunc` 式分隔、数值范围和临时配置提交保持一致。Rust 没有移植 Go 对无效数值写后台 warning 日志的副作用，但返回错误与状态不变语义一致。
- Go 使用完整 `SessionVars` 和完整注册表 `SysVar`；Rust 本文件使用局部精简结构，正式 Rust 注册表位于 `variable.rs`/`sysvar_builtins.rs`。这是当前迁移/接线状态的边界。

Go 测试 `pkg/sessionctx/variable/sysvar_test.go::TestGlobalSystemVariableInitialValue` 验证安装默认值。Rust 的 `sysvar_test.rs::TestGlobalSystemVariableInitialValue` 增加了显式 classic、TiKV 与 next-gen 环境覆盖；`TestPipelinedDmlResourcePolicyFieldsFuncSeparators` 固化 Go 分隔行为。更聚焦的 Rust 测试位于独立文件 `sysvar_3_aster_unit_test.rs`，没有把测试内嵌到生产源文件。

## 扩展指南

- 新增动态安装默认值时，在 `GlobalSystemVariableInitialValueWithRuntime` 增加最窄变量分支，并同步 `sysvar_3_aster_unit_test.rs`、`sysvar_test.rs` 及 Go 对照测试；确认它确实只适用于新安装，避免改变升级集群既有值。
- 新增运行时判定条件时，先把输入加入 `RuntimeEnvironment`，由 `current()` 集中采样，再在纯函数中分支；这样可避免测试直接修改进程全局配置。
- 新增流水线自定义键时，应先扩展 `PipelinedDMLConfig`，在临时配置的 `match` 中解析并校验，再补充成功、边界、未知键和“失败不部分提交”测试。若改变重复键、空字段或非有限浮点数规则，必须核对 Go `strings.FieldsFunc`/`strconv.ParseFloat` 语义及兼容风险。
- 接入 `newExecConcurrencySysVar` 到正式注册表前，必须处理精简 `SysVar` 与 `variable.rs::SysVar` 的类型差异，并补齐 Go `Validation` 弃用告警；不要用当前测试模型替换完整注册链。
- 接入 TiFlash/流水线 setter 到正式会话时，应在 `sysvar_builtins.rs` 的正式 `SetSessionHook` 中完成适配，并验证正式 `SessionVars` 字段映射。相关测试继续放在独立 `*_test.rs` 文件。
- 性能上，默认值匹配为常数规模；流水线自定义解析会分配小写字符串和每项 `Vec`。若该路径变成高频路径再考虑消除分配，优化不得改变 Go 兼容的分隔行为和失败原子性。

## 验证依据

- 源码全貌：RustCodeGraph `node --file pkg/sessionctx/variable/sysvar.rs --offset 1 --limit 260` 与 `--offset 261 --limit 260`，确认文件共 405 行、28 个索引符号及全部实现。
- 符号检索：RustCodeGraph `query` 确认 Rust/Go 的 `SysVar`、`newExecConcurrencySysVar`、`GlobalSystemVariableInitialValue`、`setTiFlashComputeDispatchPolicy`、`setPipelinedDmlResourcePolicy` 对应位置。
- 调用证据：RustCodeGraph 文件边报告 5 个使用文件；函数级 `callers/callees` 查询在当前索引上超时未返回，因此又以精确源码检索和 RustCodeGraph 文件节点核对了 `pkg/session/runtime/control.rs` 与 `pkg/sessionctx/variable/nextgen.rs` 的真实调用。未把无法落到具体调用表达式的图边当作结论。
- crate 与模块：读取 `pkg/sessionctx/variable/Cargo.toml`、`pkg/sessionctx/variable/lib.rs`、`pkg/sessionctx/variable/variable.rs` 和 `pkg/sessionctx/variable/sysvar_builtins.rs`，确认包依赖、公开模块及精简/正式类型边界；目标包没有 `doc.go`。
- Go 对照：读取 RustCodeGraph 中 `pkg/sessionctx/variable/sysvar.go` 100--199、4310--4487 行，以及 `pkg/sessionctx/variable/sysvar_test.go` 的 `TestGlobalSystemVariableInitialValue`。
- Rust 测试：读取 `pkg/sessionctx/variable/sysvar_3_aster_unit_test.rs`、`sysvar_test.rs` 1720--1860 行和 `varsutil_test.rs` 的并发变量用例；它们覆盖选项顺序、无效并发回退、运行时默认值、TiFlash 失败不更新、三类流水线策略、范围错误与失败原子性。
- 常量核对：`pkg/sessionctx/vardef/tidb_vars.rs` 定义 `MaxConfigurableConcurrency=256`、流水线并发范围 1--8192、默认 128/8、保守 2/2 及三种策略字符串。
- 本任务仅生成文档，按计划不运行 Cargo。最终以任务给定命令确认目标文件存在且恰有十一个固定二级章节，并人工复核链接、当前接线边界和未验证项。
