# `pkg/sessionctx/vardef/tidb_vars.rs`

## 文件定位

`tidb_vars.rs` 属于 `astersql-sessionctx-vardef` crate，是 TiDB 专有系统变量的底层定义表。它集中保存变量名、`Def*` 默认值、取值枚举、作用域/类型标志、进程级可变状态以及少量转换和原子访问函数。crate 入口 `pkg/sessionctx/vardef/lib.rs` 通过 `mod tidb_vars; pub use tidb_vars::*;` 将这些符号平铺重导出，因此调用方通常使用 `astersql_sessionctx_vardef::TiDB...` 或将整个 crate 别名为 `vardef`，而不会直接引用私有模块路径。

该文件本身不是完整的 SysVar 注册表，也不负责解析 SQL。系统变量的合法范围、SET/SHOW 行为及会话值保存主要由 `pkg/sessionctx/variable/variable.rs`、其 SysVar 定义和 `pkg/session/runtime/control.rs` 完成；本文件为这些上层流程提供稳定的名字、默认值和跨会话共享状态。RustCodeGraph 将该文件识别为被 20 个文件直接使用，实际消费面包括 session、DDL、executor、planner、server、domain、meta 与 infoschema。

crate 边界由 `pkg/sessionctx/vardef/Cargo.toml` 给出：库入口为 `lib.rs`，直接依赖 `chrono`、`sysinfo` 和路径依赖 `astersql-config-kerneltype`；`nextgen` feature 向 `kerneltype/nextgen` 透传。没有在这里声明 tipb、配置中心或慢日志规则 crate，相关 Go 类型因此在 Rust 文件中使用本地表示或简化边界。

## 核心职责

1. **声明 SQL 可见名称。** `TiDBSnapshot`、`TiDBMemQuotaQuery`、`TiDBEnableMDL` 等 `&str` 常量是注册、查找和 SET/SHOW 接线使用的键。源码按会话作用域（约从第 143 行）和实例/全局作用域（约从第 413 行）分组，但常量本身不携带运行时作用域检查。
2. **集中默认值及上下界。** 从 `DefTiDB...`、`Min...`、`Max...` 常量到 `On`/`Off`、断言级别、资源策略、TSO RPC 模式等规范字符串，供上层构造 SysVar 或初始化组件。声明常量不等同于上层已经注册或支持该变量，是否接线需继续检查消费者。
3. **保存进程级动态状态。** `ProcessGeneralLog`、`RunAutoAnalyze`、DDL 参数、统计缓存配额、TTL 参数、资源控制开关、连接属性统计、embedding 凭据等 `static` 值在进程内供多个会话和子系统共享。
4. **提供跨语言兼容转换。** `ExchangeCompressionMode`、`ScopeFlag`、`TypeFlag`、`ClusteredIndexDefMode` 及相应函数保持 Go 的数值顺序、规范字符串和回退规则。
5. **实现少量环境/内核相关默认逻辑。** `serverMemoryLimitDefaultValue` 根据可见物理内存选择 `"80%"` 或 `"0"`；`IsMDLEnabled` 和 `GetDefaultTxnAssertionLevel` 根据 Classic/NextGen 内核返回不同结果。

源码当前共有 968 个顶层 `pub const`、113 个顶层 `pub static` 和 19 个顶层 `pub fn`。绝大多数内容是声明性数据，真正有分支或状态转换的逻辑集中在文件开头的原子包装，以及约第 2227 行之后的辅助函数和类型实现。

## 主要符号

- `atomic_value!` 生成 `AtomicBoolValue`、`AtomicI32Value`、`AtomicI64Value`、`AtomicU32Value`、`AtomicU64Value`。每个包装只暴露 Go 风格的 `new`、`Load`、`Store`，读写统一使用 `Ordering::SeqCst`。
- `AtomicF64Value` 将 IEEE-754 位模式存入 `AtomicU64`；`Load`/`Store` 经 `to_bits`、`from_bits` 转换，避免为浮点值另加互斥锁。
- `AtomicStringValue(RwLock<String>)` 通过读锁克隆、写锁替换字符串；文件末尾另有 `Swap`，在一次写锁持有期内返回旧值。锁中毒会以 `expect("atomic string poisoned")` 触发 panic。
- `UnlimitedRateLimiter::Allow` 恒返回 `true`，作为 `GlobalSlowLogRateLimiter` 的无限速实现。它没有令牌桶状态。
- `GlobalSlowLogRulesValue` 目前只含 `RulesMap: HashMap<i64, String>`；`GlobalSlowLogRules` 再以 `LazyLock<RwLock<_>>` 包装。
- 变量名常量（`TiDB*` 等）定义 SQL/配置键；`Def*`、`Min*`、`Max*` 定义默认值与边界；`On`、`Off`、`Warn` 等定义规范文本。
- 进程全局 `static` 可按用途分为：日志与诊断（如 `ProcessGeneralLog`、`QueryLogMaxLen`）、统计分析（如 `RunAutoAnalyze`、`AnalyzeColumnOptions`）、DDL（如 `DDLReorgWorkerCounter`、`DDLErrorCountLimit`）、内存与计划缓存、TTL、资源控制、schema/事务限制、连接属性计数及 embedding 配置。
- `ExchangeCompressionMode(i32)` 保留 Go 枚举数值；`Name` 返回规范名，`ToTipbCompressionMode` 转为本地 `CompressionMode`，`ToExchangeCompressionMode` 对输入做 ASCII 大写后返回 `(模式, 是否识别)`。
- `ScopeFlag(u8)` 支持 `BitOr`；`ScopeNone`、`ScopeGlobal`、`ScopeSession`、`ScopeInstance` 是位标志。`String` 固定按 `SESSION,GLOBAL,INSTANCE` 顺序输出。
- `TypeFlag = u8` 的 `TypeStr` 到 `TypeDuration` 固定为 0 到 7，与 Go `iota` 顺序一致。
- `TiDBOptEnableClustered` 将精确的 `ON`/`OFF` 映射为聚簇索引 On/Off，其余输入回退到 `IntOnly`。
- 六组 DDL setter/getter（`Set/GetDDLReorgWorkerCounter` 等）以 `SeqCst` 访问裸 `AtomicI32`/`AtomicI64`，并假定上层 SysVar 已完成范围校验。
- `IsMDLEnabled`、`SetEnableMDL` 和 `GetDefaultTxnAssertionLevel` 封装内核差异；NextGen 下 MDL 强制为 true、事务断言默认 `STRICT`，Classic 则读取开关且默认 `OFF`。

## 执行流程

本文件没有单一入口，典型流程有四类：

1. **系统变量定义与赋值。** 上层以本文件的名称常量定位变量，以 `ScopeFlag`/`TypeFlag` 和默认值构造或校验 SysVar；例如 `pkg/sessionctx/variable/variable.rs` 使用 `ScopeSession` 判断赋值作用域、按 `TypeBool` 等分派规范化逻辑。`pkg/session/runtime/control.rs` 在 SET 路径完成校验后，将影响进程行为的值写入 `ProcessGeneralLog`、`AnalyzeColumnOptions` 或调用 `SetEnableMDL`。因此“字符串常量 → 上层注册/校验 → session 值或全局原子状态”才是完整链路。
2. **读取共享运行时配置。** 执行组件直接 `Load` 对应全局值。例如 `pkg/session/runtime/dispatch.rs` 读取 `ProcessGeneralLog` 决定日志路径，`pkg/domain/domain.rs` 更新 `AnalyzeColumnOptions`，`pkg/executor/adapter.rs` 读取 `QueryLogMaxLen`，DDL 与 meta 代码读取 DDL 计数器或功能开关。
3. **转换型辅助。** 压缩模式解析先对输入执行 `to_ascii_uppercase`，识别 `NONE`、`FAST`、`HIGH_COMPRESSION`、`UNSPECIFIED`；未知值返回 `(NONE, false)`，由调用方决定是否报错。作用域字符串化检查位集合；聚簇索引选项只接受规范大小写的 `ON`/`OFF`。
4. **内核相关分支。** Classic 的 `IsMDLEnabled` 读取私有 `enableMDL`；NextGen 在读取前直接返回 true，所以 `SetEnableMDL(false)` 只改变存储值，不会改变 NextGen 的有效行为。`pkg/session/runtime/normal_ddl_service.rs`、`pkg/session/runtime/session.rs`、`pkg/ddl/normal_policy.rs`、infoschema 等直接消费该结果。

初始化发生在首次装载 crate 或首次解引用 `LazyLock` 时。普通原子量以对应 `Def*` 常量初始化；字符串、规则表以及主机内存推导等不能用 const 构造的值由 `LazyLock` 延迟建立。TTL 时间窗口会经 `mustParseTime` 校验默认字符串，再以原文本存储。

## 数据与状态

静态名称和默认值是只读编译期数据；动态状态则是**进程级**而非会话级。同一进程中的会话、后台任务和服务组件共享这些原子量，另一个 TiDB/AsterSQL 进程不会自动看到更新。`ScopeGlobal` 描述 SQL 系统变量语义，不能误解为跨节点一致性协议；跨节点传播由本文件之外的全局变量缓存/配置流程负责。

整数、布尔和浮点包装均采用顺序一致原子访问。`AtomicF64Value` 保留全部位模式，包括 NaN 的具体表示；它只保证单值原子读写，不提供 compare-and-swap 或复合更新。`AtomicU64Value::Inc` 使用 `fetch_add(1) + 1` 返回递增后的值，溢出行为沿用 Rust 原子整数运算。字符串和慢日志规则使用 `RwLock`：读字符串会产生一份克隆，规则表的调用方必须自行遵循锁的读写生命周期。

DDL 的六个共享量使用标准库原子类型并通过函数封装；其他状态多使用 Go 风格包装的公开 `Load`/`Store`。测试通过 RAII guard 恢复修改过的全局状态，说明这些值会跨测试步骤持续存在，新增测试也必须避免泄漏状态。

`DefTiDBServerMemoryLimit` 是 `LazyLock<String>`，首次读取时根据 `sysinfo::System::total_memory()` 决定 `80%` 或禁用值 `0`。TTL 开始/结束时间当前保存为已校验的字符串，不是带时区的时间对象。若消费者需要比较窗口，仍须在其自身边界解析或转换。

## 依赖与调用关系

- **模块入口：** `pkg/sessionctx/vardef/lib.rs` 私有声明 `tidb_vars` 并公开重导出全部符号；同 crate 的 `sysvar.rs` 和 `runtime.rs` 分别覆盖 MySQL 通用名称与其他运行时租约。
- **直接依赖：** `std::sync::atomic` 提供标量原子；`RwLock`/`LazyLock` 提供字符串、Map 和延迟初始化；`chrono` 校验时间格式；`sysinfo` 探测主机总内存；`kerneltype` 提供 Classic/NextGen 判断。
- **系统变量层：** `pkg/sessionctx/variable/variable.rs` 使用作用域和类型标志完成校验；`pkg/session/runtime/control.rs` 读取或写入多个全局状态，并调用 `TiDBOptEnableClustered`、`SetEnableMDL`。
- **Session/DDL/元数据链：** session factory 将 `IsMDLEnabled` 同步到其他组件，normal DDL service 将其带入 DDL 上下文；`pkg/ddl/normal_policy.rs` 读取 DDL 错误上限和 MDL 状态；`pkg/meta/model/reorg.rs` 读取 reorg worker 数；`pkg/ddl/reorg.rs` 读取行格式。
- **执行与规划：** executor 读取日志长度、统计缓存等共享配置；planner、DDL 和 executor 的多个 crate 通过 Cargo 路径依赖 `astersql-sessionctx-vardef`，并直接引用默认值或变量名。
- **压缩模式：** `pkg/kv/version_test.rs` 验证 `ToExchangeCompressionMode`；生产消费者是否将本地 `CompressionMode` 接入真实 tipb 枚举，不能仅由本文件确认。

RustCodeGraph 对精确节点给出的下游边包括：`IsMDLEnabled → kerneltype::IsNextGen + enableMDL.load`、`ToExchangeCompressionMode → to_ascii_uppercase + 模式常量`、`SetDDLReorgWorkerCounter → DDLReorgWorkerCounter.store`。其 callers 查询在本次限定时间内未返回，因此上游位置由上述直接符号检索核对。

## 错误处理与边界

多数 API 不返回 `Result`：本文件假设上层已完成 SQL 值范围与类型校验。DDL setter 的注释明确说明范围由 SysVar validation 保证，所以直接调用这些函数可以写入任意同类型数值；新增调用者不能绕过上层验证。

`ToExchangeCompressionMode` 用布尔值报告未知名称，并安全回退到 `NONE`；`TiDBOptEnableClustered` 对未知或大小写不符的文本静默回退到 `IntOnly`。`ScopeFlag::String` 会忽略三个已知位以外的未知位；只有值恰好为 0 时返回 `NONE`。

`mustParseTime` 对不支持的布局直接 panic，对格式不合法的值以断言 panic。它用于受源码控制的静态默认值初始化，不适合直接处理未经验证的用户输入。`AtomicStringValue` 和规则表的 `RwLock` 一旦中毒，后续访问会 panic；代码没有恢复中毒锁的分支。

`serverMemoryLimitDefaultValue` 将无法获得内存或总内存为零统一回退为 `"0"`，不向调用者暴露探测错误。`UnlimitedRateLimiter` 永不拒绝请求；若未来需要实际限流，替换它会改变调用方对无状态、无失败行为的假设。

变量名和默认值数量很大，最主要的维护边界是三者漂移：名称常量、默认值、上层 SysVar 注册/回调。只在本文件增加常量不会自动得到可用的 SQL 系统变量。

## 并发与资源生命周期

所有全局状态生命周期均为整个进程。标量读写使用 `SeqCst`，提供最强的单原子全序语义，但多个独立原子之间没有事务性快照；例如同时更新两个相关配置时，读者仍可能观察到新旧组合。需要跨字段不变量时，应在上层增加统一锁或版本化方案，不能仅依赖这里的两个 `Store`。

`LazyLock` 只初始化一次：`AnalyzeColumnOptions`、TTL 时间文本、规则表、字符串配置和默认内存限制都在首次使用时构造。`AtomicStringValue::Load` 在读锁内克隆后释放锁，返回值与后续写入隔离；`Store`/`Swap` 在写锁内替换所有权。不要在持有 `GlobalSlowLogRules` 锁时调用可能再次获取同一锁的代码。

`GlobalSlowLogRateLimiter` 是零状态、线程安全的 unit struct；独立 Rust 测试通过 `Barrier` 同步多个线程并发调用 `Allow`，验证调用次数和无阻塞接口，而非限流精度。DDL 原子访问测试、进程全局状态测试和 MDL 测试都会暂时修改静态状态，并用 `Drop` 守卫复原；后续测试应沿用独立测试文件与恢复模式，避免并行测试互相污染。

本文件不创建线程、任务、通道、事务或网络资源，也不负责跨节点广播。它只提供可被这些长生命周期组件并发读取的本地状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/sessionctx/vardef/tidb_vars.go`，Rust 保留了变量名、声明分组、默认值、枚举数值、作用域字符串顺序、聚簇索引回退、DDL 原子访问，以及 NextGen 的 MDL/事务断言分支。Go 测试 `pkg/sessionctx/vardef/tidb_vars_test.go` 的 `TestIsMDLEnabledInNextGen` 和慢日志 limiter 并发 benchmark，在 Rust 独立测试 `tidb_vars_test.rs` 中有相应行为覆盖；更完整的迁移测试位于 `tidb_vars_2_aster_unit_test.rs`。

需要明确的表示或接线差异如下：

- Go 使用 `go.uber.org/atomic` 和 `sync/atomic`；Rust 以本地包装统一采用 `SeqCst`。Go 的具体库操作内存序不应仅凭本文件推断为完全相同，但公开 Load/Store 语义保持一致。
- Go `GlobalSlowLogRateLimiter` 是 `rate.NewLimiter(rate.Inf, 1)`，Rust 用恒允许的 `UnlimitedRateLimiter` 表达同一全局“不限速”结果；Go benchmark 另行构造有限速 limiter，而 Rust 测试只对全局无限速对象做并发调用，因此不是性能等价验证。
- Go 慢日志规则值是指向结构化规则的原子指针；Rust 当前是 `RwLock<HashMap<i64, String>>`。规则文本的解析和原子整表替换语义并未在此文件中完整复刻，消费者需按当前 Rust 类型处理。
- Go 压缩模式直接映射 `tipb.CompressionMode`；Rust 因 Cargo 未依赖 tipb，定义了本地 `CompressionMode`。名称和数值分支有测试，但 protobuf 边界仍需由实际接线处验证。
- Go TTL 窗口是 `atomic.Time`，Rust 保存经过 `chrono` 校验的 `String`；Go duration 原子在 Rust 中多以 `i64` 表示。数值单位由各 `Def*` 注释和消费者约定维持，类型系统没有编码单位。
- Go 的 `GlobalLogMaxDays` 和 `DDLSlowOprThreshold` 从全局配置初始化；Rust 当前分别初始化为 `0` 和 `300`。这属于当前源码事实，不能描述为已与运行时配置动态一致。
- Go `serverMemoryLimitDefaultValue` 通过 `memory.MemTotal()` 返回错误；Rust 使用 `sysinfo` 并只判断总内存是否非零，两者都以 `80%`/`0` 表达成功与回退，但探测来源不同。

因此本文件是高覆盖度迁移，而不是所有外部类型和初始化来源的一比一替换。判断某变量是否“完全迁移”时还必须检查上层注册、回调和生产消费者。

## 扩展指南

新增 TiDB 专有系统变量时，至少同步检查以下位置：

1. 在本文件相应作用域区域增加名称常量，并在默认值区域增加 `Def*` 及必要的 Min/Max/规范取值；保持 Go 对照的名称、数值和单位。
2. 在 `pkg/sessionctx/variable` 的 SysVar 注册/校验位置接入名称、作用域和类型；若用户赋值会影响进程行为，在 `pkg/session/runtime/control.rs` 或对应缓存重建路径更新共享状态。不要把“增加常量”当成完整接线。
3. 若需要进程级动态值，优先复用合适的原子包装；涉及多个字段一致性或复杂结构时设计单一锁/不可变快照。敏感字符串（如 API key）不得写入日志、错误或缓存键；变更 embedding 配置时应同步维护 `EmbeddingConfigVersion` 的失效语义。
4. 若新增枚举或转换，保持未知输入的处理契约、Go 数值顺序和外部协议映射，并在独立测试文件中覆盖规范值、大小写、未知值和回退路径。
5. 修改 DDL setter/getter 时保留上层范围校验前提和原子访问；修改 MDL/断言逻辑时同时验证 Classic 与 `nextgen` feature，尤其不能让 `SetEnableMDL(false)` 绕过 NextGen 强制开启规则。
6. 测试必须继续放在独立文件。优先扩展 `pkg/sessionctx/vardef/tidb_vars_2_aster_unit_test.rs`（类型、默认值、原子访问、时间和内核分支）或 `pkg/sessionctx/vardef/tidb_vars_test.rs`（Go 同名测试/并发行为），并用恢复守卫还原所有被修改的静态状态；Go 语义变化时同步核对 `tidb_vars.go` 与 `tidb_vars_test.go`。

主要兼容风险是 SQL 名称/默认值漂移、Go iota 数值或协议枚举不一致、作用域错误以及 Classic/NextGen 分支倒置；主要正确性风险是绕过校验写入全局状态或多原子更新破坏不变量；主要性能风险是高频读取字符串产生克隆、扩大 `SeqCst` 原子使用，以及在全局 `RwLock` 内执行重工作。

## 验证依据

- 源文件全貌：`pkg/sessionctx/vardef/tidb_vars.rs`（2685 行）；符号盘点确认 968 个 `pub const`、113 个 `pub static`、19 个 `pub fn`，并检查了所有结构体、枚举、类型别名、impl 和函数区域。
- crate 与模块边界：`pkg/sessionctx/vardef/Cargo.toml`、`pkg/sessionctx/vardef/lib.rs`。
- Go 对照：`pkg/sessionctx/vardef/tidb_vars.go`，重点核对进程全局 var block、内存/时间辅助、压缩模式、作用域/类型标志、聚簇索引转换、DDL 原子访问及内核分支。
- 独立测试：`pkg/sessionctx/vardef/tidb_vars_test.rs`、`pkg/sessionctx/vardef/tidb_vars_2_aster_unit_test.rs`；Go 对照测试：`pkg/sessionctx/vardef/tidb_vars_test.go`。
- RustCodeGraph：`status` 确认索引覆盖目标文件；文件节点报告该文件被 20 个文件使用；精确 `node` 查询验证 `IsMDLEnabled`、`ToExchangeCompressionMode`、`SetDDLReorgWorkerCounter` 的源码与下游边。callers 查询未在限定时间返回，直接上游改用符号检索核对。
- 直接调用证据包括：`pkg/session/runtime/control.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/domain/domain.rs`、`pkg/ddl/normal_policy.rs`、`pkg/ddl/reorg.rs`、`pkg/meta/model/reorg.rs`、`pkg/infoschema/issyncer/syncer.rs`、`pkg/sessionctx/variable/variable.rs`、`pkg/executor/adapter.rs` 和 `pkg/kv/version_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档恰含 11 个固定二级章节，并人工复核未把声明常量误述为完整 SysVar 接线。
