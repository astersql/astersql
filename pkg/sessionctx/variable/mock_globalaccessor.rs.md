# `pkg/sessionctx/variable/mock_globalaccessor.rs` 逻辑说明

## 文件定位

`mock_globalaccessor.rs` 位于 `astersql-sessionctx-variable` crate；crate 边界由 `pkg/sessionctx/variable/Cargo.toml` 定义，入口 `pkg/sessionctx/variable/lib.rs` 以 `pub mod mock_globalaccessor` 公开本模块，并在 `cfg(test)` 下挂载独立测试 `mock_globalaccessor_test.rs`。它是系统变量访问行为的内存测试替身，不负责持久化，也不是会话生产路径当前使用的访问器。

文件名和公开方法沿用 Go `pkg/sessionctx/variable/mock_globalaccessor.go` 的 `MockGlobalAccessor` API 形状，但需要注意当前 Rust 接线边界：本文件只提供同名固有方法，没有为 `MockGlobalAccessor` 实现 `pkg/sessionctx/variable/variable.rs::GlobalVarAccessor` trait。会话对象持有的是 `Box<dyn GlobalVarAccessor>`，`pkg/sessionctx/variable/session.rs::SessionVars::new` 实际安装 `DefaultGlobalVarAccessor`。因此，本文件当前直接服务于测试，不在完整应用的生产系统变量读写主链中。

## 核心职责

- `MockGlobalAccessor` 用内存 `HashMap` 模拟全局系统变量读取和更新，便于测试错误、规范化和 hook 调用顺序。
- `NewMockGlobalAccessor` 创建“普通模式”：读取时依赖调用方传入的注册表，未知名称按 Go 行为返回空字符串。
- `NewMockGlobalAccessor4Tests` 创建“测试套件模式”：从传入迭代器建立内部注册表和可变值快照，未知名称返回错误。
- `SetGlobalSysVar`、`SetInstanceSysVar` 把真正的校验和副作用抽象为闭包，从而在不依赖完整 `SysVar`/`SessionVars` 上下文时验证控制流。
- `GetTiDBTableValue` 和 `SetTiDBTableValue` 只保留 Go mock 所需的极窄测试行为；它们不是通用的 `mysql.tidb` 表模拟器。

## 主要符号

- `MockGlobalAccessor`：唯一公开类型。字段均为私有：`vals` 保存测试套件模式下当前值；`registered_defaults` 保存构造时注册值的不可变快照；`testSuite` 选择两种读取语义。文件内没有模块级常量、trait、条件编译项或自定义 `Drop`。
- `NewMockGlobalAccessor() -> MockGlobalAccessor`：创建三个字段均为空、`testSuite = false` 的普通模式实例。
- `NewMockGlobalAccessor4Tests(defaults)`：接受任意产出 `(String, String)` 的迭代器，收集为 `registered_defaults`，克隆一份作为 `vals`，并开启测试套件模式。
- `GetGlobalSysVar(&self, name, registered)`：普通模式读取外部 `registered`；测试套件模式读取内部 `vals`。
- `SetGlobalSysVar(&mut self, name, value, validate, set_hook)`：检查名称，依次规范化、执行 hook、最后提交新值。
- `SetInstanceSysVar(&self, name, value, validate, set_hook)`：检查名称并调用校验与 hook，但不修改 `vals`。
- `SetGlobalSysVarOnly(&mut self, name, value, _skip_aliases)`：只检查名称并直接覆写 `vals`；保留但忽略 `_skip_aliases` 参数。
- `GetTiDBTableValue(&self, name)`：只支持字面名称 `tikv_gc_life_time`，并从 `registered_defaults` 而非 `vals` 读取。
- `SetTiDBTableValue(...)`：未实现，任何调用都会 panic。

所有构造函数和方法均为公开 API；内部状态不公开，调用方不能绕过方法直接改表。

## 执行流程

1. 普通模式由 `NewMockGlobalAccessor` 创建。调用 `GetGlobalSysVar` 时，如果传入注册表包含名称，就克隆对应值；没有注册表或缺少键都返回 `Ok("")`。此模式的空 `vals` 使三个设置方法都会把任何名称判为未知，因此它主要用于只读查找语义测试。
2. 测试套件模式由 `NewMockGlobalAccessor4Tests` 创建。构造器同时建立当前值表和注册默认值快照，之后普通全局变量读写只作用于 `vals`。
3. `SetGlobalSysVar` 先以 `vals.contains_key` 确认名称已注册，再把原始值交给 `validate`。只有校验成功才将规范化值传给 `set_hook`；只有 hook 成功才写入 `vals`。任一步失败都提前返回，旧值保持不变。
4. `SetInstanceSysVar` 复用“存在性检查 → 校验 → hook”的前两阶段，但接收 `&self` 且没有写回步骤，体现实例级设置只触发外部副作用的 Go mock 语义。
5. `SetGlobalSysVarOnly` 跳过校验和 hook，已注册时直接覆写当前值；布尔参数不参与分支。
6. `GetTiDBTableValue` 先限制名称必须为 `tikv_gc_life_time`，再从构造时快照读取。即便之后通过 `SetGlobalSysVarOnly` 修改同名 `vals`，表值读取仍保持原默认值；`mock_globalaccessor_test.rs::test_mock_api` 对此有专门断言。

## 数据与状态

`vals` 与 `registered_defaults` 都拥有 `String` 键和值，不借用调用方数据。测试套件构造时的克隆刻意形成两个状态层：前者可变，代表当前全局值；后者在对象生命周期中不再变化，代表注册表默认值。重复键经 `collect::<HashMap<_, _>>()` 后只保留迭代器中最后出现的值；文件没有额外检测重复项。

`testSuite` 是构造后不变的模式位。普通模式不会把 `registered` 参数缓存到对象内；测试套件模式则忽略该参数。名称匹配是区分大小写的 `String`/`str` 哈希查找，本文件不负责别名、大小写规范化或注册新变量。

## 依赖与调用关系

直接运行时依赖只有标准库 `std::collections::HashMap`；本文件没有使用 `Cargo.toml` 中的外部 crate。`lib.rs::mock_globalaccessor` 提供模块入口，`lib.rs` 还把 `mock_globalaccessor_test.rs` 作为独立测试模块挂载，满足测试逻辑不内嵌生产文件的仓库约束。

RustCodeGraph 对目标文件报告的直接使用文件包括 `mock_globalaccessor_test.rs`、`error_1_aster_unit_test.rs`、`slow_log_test.rs` 和 `variable_test.rs`；其中可核实的直接构造与方法调用集中在前两者：

- `error_1_aster_unit_test.rs::mock_accessor_matches_normal_and_testsuite_lookup_and_set_order` 同时构造普通/测试套件模式，验证外部注册表查找、缺失处理以及 validate → hook → 写入顺序。
- `mock_globalaccessor_test.rs::test_mock_api` 验证未知变量、校验失败、合法写入、绕过校验写入及 GC 生命周期快照。
- `mock_globalaccessor_test.rs::missing_gc_lifetime_registry_entry_panics` 验证缺少指定注册项时的 panic。
- `mock_globalaccessor_test.rs::mock_global_accessor_remains_send_and_sync` 在编译期证明该纯 `HashMap<String, String>` 类型满足 `Send + Sync`。

RustCodeGraph 能识别本文件的构造器和六个方法节点，但对精确 Rust 方法节点执行 `callers`/`callees` 未返回静态边；因此上述调用关系由索引的 file-use 结果和直接调用点交叉确认。方法下游是 `HashMap::{contains_key,get,insert}`、迭代器 `collect`、字符串克隆/分配以及调用方提供的闭包，没有网络、存储或异步调用。

## 错误处理与边界

- 测试套件模式读取未知名称，以及三个设置方法收到未知名称时，返回文本错误 `Unknown system variable '<name>'`。这只是 `Result<_, String>`，不保留 Rust 正式路径 `VariableError` 的错误种类或 Go `ErrUnknownSystemVar` 的堆栈信息。
- 普通模式读取未知名称不是错误，而是 `Ok("")`；调用方无法据此区分“注册值为空”和“名称不存在”。
- `SetGlobalSysVar` 对 validate 或 hook 错误原样传播，而且写入在两者之后，所以失败是原子性的：本对象的旧值不变。`SetInstanceSysVar` 同样传播错误，但即使成功也不改变本对象状态。
- `GetTiDBTableValue` 对任何非 `tikv_gc_life_time` 名称直接 `panic!("not supported")`；指定名称缺少注册默认值时 `panic!("Get SysVar Failed")`。
- `SetTiDBTableValue` 对所有输入直接 panic。其返回类型虽然是 `Result<(), String>`，当前不存在返回 `Ok` 或 `Err` 的路径。
- 文件不检查空名称、空值、重复默认项或别名；需要这些语义时应由调用方构造的注册数据和校验闭包承担。

## 并发与资源生命周期

对象完全拥有两个 map，没有引用、句柄、锁、任务、通道、事务或显式资源清理。离开作用域时依靠 Rust 自动释放字符串与 `HashMap`。`SetGlobalSysVar` 和 `SetGlobalSysVarOnly` 需要 `&mut self`，Rust 借用规则阻止同一实例上的无同步并发写；只读方法可通过共享引用并发调用。

`mock_globalaccessor_test.rs::mock_global_accessor_remains_send_and_sync` 证明类型当前可跨线程转移或共享，但类型本身不提供内部同步。若通过 `Arc` 在多线程中共享，只读访问可直接进行；要执行写操作，调用方仍需 `Mutex`、`RwLock` 等外部同步。validate 和 hook 均在借用方法期间同步执行，文件不捕获或延长闭包生命周期。

## 与 Go 版本的对应关系

Rust 与 Go `mock_globalaccessor.go` 保留了双模式读取、未知变量分支、全局设置的“验证 → hook → 写入”、实例设置不写 map、`SetGlobalSysVarOnly` 跳过校验，以及 TiDB 表读写的受限/panic 行为。独立的 Go `mock_globalaccessor_test.go::TestMockAPI` 与 Rust `mock_globalaccessor_test.rs::test_mock_api` 覆盖相同的主要成功和失败路径。

当前迁移并非接口级等价，主要差异如下：

- Go 类型显式实现 `GlobalVarAccessor` 接口并持有可替换的 `SessionVars`；Rust 类型没有实现 Rust trait，也没有 `SessionVars` 字段，而是由调用方闭包注入校验与 hook。
- Go `NewMockGlobalAccessor4Tests` 自行遍历 `GetSysVars()` 并创建 `SessionVars`；Rust 构造器要求调用方显式提供默认项。这降低了耦合，但完整性由调用方负责。
- Go 普通模式直接查询包级 `sysVars`；Rust 普通模式通过 `registered: Option<&HashMap<...>>` 参数查询外部表。
- Go 使用结构化的 `error`；Rust mock 使用 `String`。正式 Rust trait 使用 snake_case 方法和 `VariableError`，签名与本文件的 Go 风格固有方法不兼容。
- Go 的 GC 生命周期查询调用 `GetSysVar(vardef.TiDBGCLifetime)`；Rust 用字面键查询构造时快照。两者都不读取可变 `vals`，但 Rust 测试数据必须显式包含该键。

因此，“对应 Go mock 行为”是本文件的测试目标，而“可作为 Rust `SessionVars` 的 trait object”目前不是已支持能力。

## 扩展指南

- 新增普通系统变量行为时，优先修改 `MockGlobalAccessor` 对应方法，并同步扩展 `mock_globalaccessor_test.rs`；涉及双模式差异或调用顺序时，也应更新 `error_1_aster_unit_test.rs::mock_accessor_matches_normal_and_testsuite_lookup_and_set_order`。
- 若要让该类型进入正式 Rust `SessionVars` 接线，最可能的入口是实现 `variable.rs::GlobalVarAccessor`。这不是简单改名：需要把 `String` 错误转换为 `VariableError`，适配 `Context` 参数、snake_case trait 方法和可变性签名，并决定校验/hook 从何处取得；完成前不得把它装入 `Box<dyn GlobalVarAccessor>`。
- 新增 `mysql.tidb` 表键时，应扩展 `GetTiDBTableValue` 的支持集合并为每个键补独立成功、缺失和不支持边界测试；实现 `SetTiDBTableValue` 时还要明确写入 `vals`、`registered_defaults` 还是单独的表状态，不能破坏 GC 生命周期“注册默认值为真源”的现有测试契约。
- 若引入内部并发可变性，应重新评估 `Send + Sync` 测试、锁中执行用户闭包导致的死锁风险以及失败后的状态回滚。当前同步、先 hook 后提交的顺序是重要不变量。
- 性能风险主要来自测试套件构造时完整 map 克隆和每次读取的字符串克隆；当前用途为小范围测试辅助。若复制 Go 的全注册表且高频构造，应先量化成本，不要把 Go 注释所警告的测试套件模式带入生产热路径。

## 验证依据

- 源码全貌：`pkg/sessionctx/variable/mock_globalaccessor.rs`，RustCodeGraph 文件节点显示 141 行、11 个符号；逐项核对一个 struct、两个构造函数和六个方法，无常量、trait impl 或条件编译。
- crate 与模块边界：`pkg/sessionctx/variable/Cargo.toml` 的包名、`[lib] path = "lib.rs"`、`autotests = false`，以及 `pkg/sessionctx/variable/lib.rs` 的公开模块和独立测试挂载；目标目录不存在 `doc.go`。
- 正式接口/生产实现对照：`pkg/sessionctx/variable/variable.rs::GlobalVarAccessor`、`SessionVars::GlobalVarsAccessor`，以及 `pkg/sessionctx/variable/session.rs::DefaultGlobalVarAccessor` 和 `SessionVars::new`。
- Go 对照：`pkg/sessionctx/variable/mock_globalaccessor.go::{MockGlobalAccessor, NewMockGlobalAccessor, NewMockGlobalAccessor4Tests}` 及其六个接口方法。
- 测试证据：`pkg/sessionctx/variable/mock_globalaccessor_test.rs::{test_mock_api, mock_global_accessor_remains_send_and_sync, missing_gc_lifetime_registry_entry_panics}`、`pkg/sessionctx/variable/error_1_aster_unit_test.rs::mock_accessor_matches_normal_and_testsuite_lookup_and_set_order`、`pkg/sessionctx/variable/mock_globalaccessor_test.go::TestMockAPI`。
- RustCodeGraph 查询：执行了 `status`、目标文件 `explore`、文件 `node`、主要符号 `query`，并尝试对精确 Rust 节点执行 `callers`/`callees`；索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边。精确方法调用边未由图返回，故使用索引 file-use 与直接调用点补证，没有据此虚构生产调用链。
- 本任务只新增说明文档，没有修改或运行 Rust/Go 代码；按任务约束不运行 Cargo。交付结构由任务规定的 11 标题命令验证，并人工复核“文件为何存在、如何运行、如何安全扩展”三类问题。
