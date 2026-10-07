# `pkg/config/deploymode/mode.rs`

## 文件定位

本文件是 Rust crate `astersql-config-deploymode` 的核心实现，定义 TiDB 进程级部署模式及其配置编解码接口。crate 入口 `pkg/config/deploymode/lib.rs` 通过 `pub mod mode` 声明该模块，并用 `pub use mode::*` 将其公开 API 提升到 crate 根；同时还保留 `deploymode::deploymode::*` 兼容重导出路径。`pkg/config/deploymode/Cargo.toml` 指定 `lib.rs` 为库入口，并通过 `nextgen` feature 透传 `astersql-config-kerneltype/nextgen`。

部署模式不是 SQL 会话级状态，而是整个 TiDB 进程共享的配置。启动接线的直接例子是 `cmd/tidb-server/main.rs:555`：它把 `cfg.DeployMode` 交给 `deploymode::Set`；随后服务器启动流程以及 `pkg/session`、`pkg/ddl`、`pkg/expression`、`pkg/planner` 等下游通过 `IsStarter` 等查询函数选择行为。更完整的产品语义由同目录 `doc.go`/`doc.rs` 描述：这些模式仅对 NextGen 内核有实际判定意义，且不同实例若以不同组件配置启动，进程级值可能不一致。

## 核心职责

- 用 `Mode(i32)` 和三个常量 `Premium`、`PremiumReserved`、`Starter` 表示部署形态，并保留 Go `type Mode int32` 可以承载未知整数值的性质。
- 用全局原子量 `currentMode` 保存进程当前模式；默认值是 `Premium`，公开读取入口为 `Get`，受约束的写入入口为 `Set`。
- 通过 `IsPremiumReserved`、`IsStarter` 同时检查内核类型与部署模式，避免 classic 构建仅因原子值变化就表现为 NextGen 模式。
- 在规范字符串和 `Mode` 之间转换：`Parse` 负责解析，`Mode::String` 负责展示，`Mode::Valid` 负责合法性判断，`ModeList` 提供稳定的合法值顺序。
- 提供与 Go 自定义编解码方法对应的 `MarshalJSON`、`UnmarshalJSON` 和 `UnmarshalTOML`，供上层配置适配代码显式调用。

该文件只保存和解释模式，不实现各模式对应的业务策略；业务差异位于调用 `IsStarter`/`IsPremiumReserved` 的服务器、会话、DDL、规划器等模块。

## 主要符号

- `const premiumName`、`premiumReservedName`、`starterName`：三个规范化、小写的外部表示，分别为 `premium`、`premium_reserved`、`starter`；它们是文件内私有常量。
- `pub struct Mode(pub i32)`：可复制、可比较的公开新类型。字段公开使迁移代码能构造 `Mode(100)` 一类未知值，从而覆盖 Go 的错误与兜底分支，而不是由 Rust enum 在类型层面排除未知值。
- `pub const Premium/PremiumReserved/Starter`：合法值依次为 `0/1/2`。`Premium == Mode(0)` 与 `currentMode` 的静态初始化及 Go 原子整数零值一致。
- `static currentMode: AtomicI32`：私有的进程级状态；外部只能经 `Get`/`Set` 访问，同文件测试通过 `include!` 才能直接验证其防护逻辑。
- `pub fn Get() -> Mode`：以 `SeqCst` 顺序原子读取并包装为 `Mode`。
- `pub fn IsPremiumReserved() -> bool`、`pub fn IsStarter() -> bool`：先调用 `kerneltype::IsNextGen()`，再比较 `Get()` 的结果。短路求值使 classic 内核下恒为 `false`。
- `pub fn Set(mode: Mode) -> Result<(), String>`：先拒绝非 NextGen 内核，再拒绝 `!mode.Valid()`，最后以 `SeqCst` 写入。虽然注释规定“启动时设置且不可更改”，函数自身没有一次性写入锁；生命周期约束由调用约定承担，测试也会多次切换模式。
- `pub fn Parse(s: &str) -> Result<Mode, String>`：以 Unicode `to_lowercase` 做大小写归一化，但不裁剪空白；仅接受三个完整名称。
- `Mode::String(self) -> String`：合法值返回规范名，未知值返回 `unknown(N)`。
- `Mode::Valid(self) -> bool`：只承认三个常量。
- `Mode::MarshalJSON`：拒绝未知值，随后用 `serde_json::to_vec` 把规范名编码成 JSON 字符串字节。
- `Mode::UnmarshalJSON`：只把 JSON 字符串解到临时变量，再调用 `Parse`；全部成功后才覆盖 `self`。
- `Mode::UnmarshalTOML`：只接受 `toml::Value::String`，随后调用 `Parse`；全部成功后才覆盖 `self`。
- `pub fn ModeList() -> Vec<Mode>`：每次新建并返回 `[Premium, PremiumReserved, Starter]`，顺序与 Go 实现一致。

## 执行流程

启动设置流程如下：

1. 上层读取配置得到 `Mode`；`cmd/tidb-server/main.rs:555` 调用 `deploymode::Set(cfg.DeployMode)`。
2. `Set` 先查询 `kerneltype::IsNextGen()`。classic 内核直接返回 `deploy mode can only be set for nextgen TiDB`，不写全局状态。
3. NextGen 内核下调用 `Mode::Valid`；未知整数返回 `invalid deploy mode N`，也不写状态。
4. 两项检查均通过后，`currentMode.store(..., Ordering::SeqCst)` 发布新值。
5. 下游调用 `Get` 获取原值，或调用 `IsStarter`/`IsPremiumReserved` 获取带内核门控的布尔判断。例如 `cmd/tidb-server/main.rs:703`、`pkg/session/starter_bootstrap_file.rs:218`、`pkg/ddl/index.rs:83` 和 `pkg/expression/builtin_inference.rs:64` 都以 Starter 判定控制各自流程。

配置文本转换流程分两类：

1. 编码时，`MarshalJSON` 先做合法性检查，再经 `String` 得到规范名并交给 `serde_json`；因此未知整数不能被悄悄编码为 `"unknown(N)"`。
2. 解码时，JSON/TOML 方法先验证输入类型，再调用 `Parse` 做大小写不敏感的名称匹配；只有解析成功才赋值给接收者，所以失败不会留下部分更新。

`Parse`、`String`、`Valid` 和 `ModeList` 不访问全局状态，可独立用于配置校验、展示和枚举合法选项。

## 数据与状态

`Mode` 的数据域是一个 `i32`。合法集合严格是 `{0, 1, 2}`，但类型允许集合外的值；这使错误处理与 Go 版本保持一致，也是 `Valid` 不可省略的原因。规范字符串与数值的映射是一一对应的：`0 -> premium`、`1 -> premium_reserved`、`2 -> starter`。

唯一可变生产状态是 `currentMode: AtomicI32`。它在进程装载时初始化为 `Premium.0`，不需要惰性初始化或堆分配。所有公开读写均复制一个 `i32`，不会借出引用，也没有所有权传播。`ModeList` 返回新 `Vec`，调用者修改列表不会影响模块内部状态。

“模式设置后不可更改”是启动生命周期不变量，而不是由 `Set` 的实现强制的一次性不变量：`Set` 可以在 NextGen 构建中被重复调用。`pkg/config/deploymode/mode_test.rs` 与 `migration_aster_unit_test.rs` 也依靠重复设置及恢复来隔离用例。因此新增运行期调用时必须避免把测试便利误解为支持热切换。

## 依赖与调用关系

下游依赖只有三个：标准库 `AtomicI32/Ordering`、本 crate 经 `lib.rs` 重导出的 `kerneltype`、以及 Cargo 中声明的 `serde_json = "1"` 与 `toml = "0.8"`。`nextgen` feature 不在本文件内形成条件编译分支，而是改变 `astersql-config-kerneltype` 的实现；本文件始终调用统一的 `kerneltype::IsNextGen()`。

文件内调用边为：`IsPremiumReserved -> kerneltype::IsNextGen + Get`，`IsStarter -> kerneltype::IsNextGen + Get`，`Set -> kerneltype::IsNextGen + Mode::Valid`，`MarshalJSON -> Mode::Valid + Mode::String + serde_json::to_vec`，`UnmarshalJSON -> serde_json::from_slice + Parse`，`UnmarshalTOML -> Parse`。`ModeList`、`String` 和 `Valid` 是叶子逻辑。

RustCodeGraph 将 `mode.rs` 标记为被 123 个文件使用；其调用图摘要列出 `IsStarter` 的生产调用者，包括 `pkg/session/runtime/session.rs`、`pkg/ddl/index.rs`、`pkg/expression/builtin_inference.rs`、`pkg/dxf/importinto/job.rs`、`pkg/planner/core/planbuilder.rs` 和 `pkg/domain/domain.rs`。源码搜索进一步确认服务器入口在 `cmd/tidb-server/main.rs:555` 写入模式，并在同文件多个位置读取 Starter 状态。Cargo 清单显示 session、DDL、domain、executor、planner、standby 等多个 crate 直接依赖 `astersql-config-deploymode`，所以该 API 是跨子系统配置边界。

## 错误处理与边界

- 所有错误以 `String` 返回，没有自定义错误类型或错误源链。调用者若需要分类，只能基于调用阶段或文本；修改错误文本可能影响与 Go 对齐的测试。
- `Set` 的检查顺序固定：非 NextGen 错误优先于模式合法性错误。在 classic 内核中调用 `Set(Mode(100))` 仍首先报告内核限制。
- `Parse` 大小写不敏感但不 `trim`；`"Starter"` 成功，`" starter "` 失败。错误使用调试字符串格式，保留引号，如 `invalid deploy mode "unknown"`。
- `String` 对未知整数不会报错，而是返回 `unknown(N)`；`MarshalJSON` 则先调用 `Valid` 并报错，避免未知值进入配置输出。
- `UnmarshalJSON` 仅接受 JSON 字符串；数字、对象、数组和无效 JSON 由 `serde_json` 拒绝。`UnmarshalTOML` 仅接受 TOML 字符串值，其余类型返回 `invalid deploy mode <值>`。
- 两种反序列化均先解析临时值，成功后才写 `self`，失败时接收者保持原值。
- 本文件没有验证跨 TiDB 实例的一致性；`doc.go` 明确指出不同实例配置可能不同，这是组件配置方案的既知边界。

## 并发与资源生命周期

`currentMode` 使用 `AtomicI32`，因此并发读取和写入不存在数据竞争。`Get` 与 `Set` 都使用最强的 `Ordering::SeqCst`，所有线程观察到同一个全序；这一选择比只保证单变量原子性更保守，并直接对齐 Go `atomic.Int32.Load/Store` 的顺序意图。该状态没有锁、中间缓存、后台任务、通道或显式释放步骤。

原子性只保证单次读写安全，不保证“只能设置一次”，也不把部署模式与其他启动配置组成事务。生产生命周期依赖启动阶段先调用 `Set`、再启动读取者。若未来需要真正的不可变初始化，应同时评估启动错误恢复、测试隔离和所有现有重复设置用例，不能仅把原子变量替换为一次性单元而不调整调用方。

测试中的资源收尾位于独立文件：`mode_test.rs` 用 `CurrentModeCleanup::drop` 恢复原值；`migration_aster_unit_test.rs` 在 NextGen 分支末尾显式恢复 `Premium`。由于状态是进程全局的，新增并行测试也必须恢复状态，并注意共享状态可能造成用例间干扰。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/config/deploymode/mode.go`，相关测试是 `mode_test.go`。Rust 实现保留了 Go 的主要结构和顺序：`Mode int32` 对应 `Mode(pub i32)`，三个 `iota` 值对应 `Mode(0..=2)`，`atomic.Int32` 对应 `AtomicI32`，`Load/Store` 对应 `SeqCst` 原子操作，所有公开函数和方法仍使用 Go 风格名称。

行为对齐点包括：默认值为 Premium；部署模式判定受 `kerneltype.IsNextGen` 门控；`Set` 先检查内核再检查合法值；`Parse` 只做大小写归一化而不去空白；未知值可被构造、可显示为 `unknown(N)`、但不可 JSON 编码；JSON/TOML 解码只接受字符串；`ModeList` 顺序固定。

语言适配差异是：Go 的 `error` 变为 `Result<_, String>`；Go 的接口自动分派改为显式方法调用；Go `UnmarshalTOML(any)` 的运行时类型断言改为匹配 `&toml::Value`；Rust `Parse` 错误分支没有 Go 所需的占位返回模式，只返回 `Err`。这些差异不改变成功路径或测试覆盖的错误语义。

Rust 独立测试 `pkg/config/deploymode/mode_test.rs` 对照 Go 的 `TestModeJSON`、`TestModeTOML`、`TestCurrentMode`；`migration_aster_unit_test.rs` 额外验证大小写、不裁剪空白、未知整数、列表顺序和 JSON/TOML 错误路径。测试不嵌入生产文件，而是由 `lib.rs` 以 `#[path]` 声明独立测试模块；`mode_test.rs` 内部的 `include!("mode.rs")` 是测试访问私有原子的现有机制。

## 扩展指南

新增部署模式时，至少应同步修改本文件的名称常量、公开 `Mode` 常量、`Parse`、`Mode::String`、`Mode::Valid` 和 `ModeList`；还要检查 `Set` 的通用校验是否足够，并搜索所有穷举或直接比较现有模式的调用方。对应 Go 文件 `mode.go` 与两侧独立测试也必须同步，避免 Rust/Go 配置格式分叉。

新增或改变模式语义时，优先在具体业务模块增加 `IsXxx` 调用或策略分派；不要把 DDL、会话或执行器行为塞进本配置文件。若新增 `IsXxx`，应保留 `kerneltype::IsNextGen() && Get() == Xxx` 的门控，除非产品明确允许 classic 内核使用该模式。

修改文本解析时要评估配置兼容性：增加别名可以兼容旧配置，改变规范输出或开始裁剪空白则会偏离 Go 语义。修改序列化 API 时要注意当前方法不是 Rust `Serialize/Deserialize` trait 实现；若引入 trait，应确认上层配置框架的调用方式，并保留现有显式方法兼容面。

修改全局状态模型时风险最高。真正强制“一次设置”会破坏当前测试恢复模式，也可能影响启动失败重试；降低内存序则需证明调用方不依赖发布顺序。对应回归应继续放在 `pkg/config/deploymode/mode_test.rs` 或 `migration_aster_unit_test.rs`，不要写回生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示本地索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/config/deploymode` 确认目标源、Go 对照和两份 Rust 测试均已索引；`node --file pkg/config/deploymode/mode.rs` 读取了完整 165 行与 13 个符号。`explore` 的调用图摘要确认 `Parse -> UnmarshalJSON/UnmarshalTOML`，以及 `IsStarter` 在 session、DDL、expression、DXF、planner、domain 等模块的调用关系。
- Rust 实现与边界：`pkg/config/deploymode/mode.rs`；crate 入口与测试装配：`pkg/config/deploymode/lib.rs`；产品级包契约：`pkg/config/deploymode/doc.go` 与 `doc.rs`。
- crate 边界：`pkg/config/deploymode/Cargo.toml`，确认库名、`nextgen` feature、kerneltype 路径依赖及 `serde_json`/`toml` 依赖。
- Go 对照：`pkg/config/deploymode/mode.go`；Go 测试：`pkg/config/deploymode/mode_test.go`。
- Rust 测试：`pkg/config/deploymode/mode_test.rs` 与 `pkg/config/deploymode/migration_aster_unit_test.rs`，覆盖 JSON/TOML、大小写、空白、未知值、模式列表、classic/NextGen 门控和全局状态恢复。
- 直接接线抽查：`cmd/tidb-server/main.rs`、`pkg/session/starter_bootstrap_file.rs`、`pkg/ddl/index.rs`、`pkg/expression/builtin_inference.rs`、`pkg/planner/core/planbuilder.rs`。这些证据只用于说明调用边，未把相邻模块实现扩展为本任务范围。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅运行任务指定的 11 章节结构验证，并人工复核上述结论均有源码、调用图、Cargo、Go 对照或测试依据。
