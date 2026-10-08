# [`pkg/sessionctx/variable/statusvar.rs`](statusvar.rs)

## 文件定位

本文件属于 `astersql-sessionctx-variable` crate；crate 入口 `pkg/sessionctx/variable/lib.rs` 以公开模块 `pub mod statusvar` 暴露它，并把 `vardef` 与 `tls_dependency::tls` 分别再导出为本文件使用的作用域/原子计数器定义和 TLS 名称转换工具。`pkg/sessionctx/variable/Cargo.toml` 的 `[lib] path = "lib.rs"` 与依赖项 `vardef`、`tls_dependency` 确认了这一边界。

它实现的是状态变量的“提供者注册 + 快照聚合”层：调用者向全局注册表加入实现 `Statistics` 的提供者，再由 `GetStatusVars` 把各提供者的名称、动态值和作用域合并为一个快照。Go 完整应用中，`pkg/executor/show.go:ShowExec.fetchShowStatus` 调用同包 Go 版本的 `GetStatusVars`，因此该抽象服务于 `SHOW STATUS`。但是当前 Rust 生产代码搜索未发现对 `statusvar::GetStatusVars`、`RegisterStatistics` 或 `UnregisterStatistics` 的调用，且 `pkg/executor/show.rs:ShowExec.fetchShowStatus` 仍委托 `ShowOperation::Status`；因此本文件目前是可独立测试的迁移实现，不能据此声称 Rust SQL 主链已经接入。

## 核心职责

- `Statistics` 定义状态提供者协议：`Stats` 产生名称到值的映射，`GetScope` 为其中每个名称给出 `vardef::ScopeFlag`。
- `RegisterStatistics`、`UnregisterStatistics` 和 `STATISTICS_LIST` 管理进程内共享提供者；注册顺序会影响同名状态项的最终值。
- `GetStatusVars` 在一次读锁保护的遍历中生成 `HashMap<String, StatusVal>`；后出现的同名键覆盖先出现的键。
- `DefaultStatusStat` 始终作为首个提供者，提供四个 `Ssl_*` 状态、两个 Performance Schema 连接属性计数器和会话级 `tidb_keys_examined`。
- `StatusValue` 用 `Arc<dyn StoredStatusValue>` 对 Go `any` 做类型擦除，同时保留 `Debug`、`Display` 和只读下转型能力。

本文件不负责 SQL 层的全局/会话过滤、权限过滤或最终字符串转换；这些行为在 Go 主链中位于 `pkg/executor/show.go:ShowExec.fetchShowStatus`。Rust 当前也没有在本文件内实现这些展示策略。

## 主要符号

- `DefaultStatusVarScopeFlag: LazyLock<vardef::ScopeFlag>`：`ScopeGlobal | ScopeSession`，供默认 SSL 状态和一般提供者使用。
- `StoredStatusValue`：私有对象安全 trait，要求值为 `Send + Sync`，并暴露 `Any` 下转型及格式化入口。
- `StatusValue`：公开、可克隆的类型擦除值。`new<T>` 要求 `T: Any + Debug + Display + Send + Sync`；`downcast_ref<T>` 只借用内部值，不复制也不修改。
- `StatusVal`：把公开字段 `Scope` 与 `Value` 绑定为一个聚合结果项。
- `TLSConnectionState`、`SessionVars`：本文件所需的精简会话视图，只携带 TLS cipher/version 和 `KeysExamined`；源码注释明确它们不是完整会话对象。
- `StatisticsError`：`Box<dyn Error + Send + Sync + 'static>`，允许提供者返回可跨线程传递的异构错误。
- `Statistics`、`StatisticsHandle`：线程安全提供者接口及 `Arc<dyn Statistics>` 句柄。
- `RegisterStatistics` / `UnregisterStatistics` / `GetStatusVars`：注册、按指针身份注销、聚合三项公开操作。
- `TLS_CIPHERS` / `TLS_SUPPORTED_CIPHERS`：25 个 cipher 编号及按声明顺序生成的、以冒号分隔且保留末尾冒号的名称串。
- `DEFAULT_STATUS` / `DefaultStatusStat`：默认元数据和值模板及其唯一内建提供者。

## 执行流程

1. 首次访问 `STATISTICS_LIST` 时，`LazyLock` 创建 `RwLock<Vec<StatisticsHandle>>`，其中已经有一个 `Arc<DefaultStatusStat>`；这对应 Go `init` 中注册默认提供者的效果。
2. 外部组件可把 `Arc<dyn Statistics>` 传给 `RegisterStatistics`。函数取得写锁并追加句柄，不去重。
3. 聚合时，`GetStatusVars(vars)` 取得注册表读锁，按向量顺序调用每个提供者的 `Stats(vars)`。
4. 对 `Stats` 返回的每个 `(name, value)`，聚合器再调用同一提供者的 `GetScope(&name)`，构造 `StatusVal` 并写入结果 `HashMap`。若多个提供者返回同名键，后写入者覆盖先写入者。
5. `DefaultStatusStat::Stats` 先克隆 `DEFAULT_STATUS` 的值模板，再以 `vardef::ConnectAttrsLongestSeen.Load()` 和 `ConnectAttrsLost.Load()` 覆盖两个实时全局计数器。
6. 若传入 `Some(SessionVars)`，它覆盖 `tidb_keys_examined`；仅当 `TLSConnectionState` 也是 `Some` 时，才通过 `tlsutil::CipherSuiteName`、`TLS_SUPPORTED_CIPHERS`、常量 `0x01 | 0x04` 和 `tlsutil::VersionName` 覆盖四个 `Ssl_*` 项。
7. 注销时，`UnregisterStatistics` 取得写锁，按 `Arc::ptr_eq` 从后向前寻找相同句柄，并用 `swap_remove` 删除最后一次注册；若不存在则保持注册表不变。

`pkg/sessionctx/variable/statusvar_2_aster_unit_test.rs` 验证了以上注册/注销、重复注册逐次移除、默认值覆盖和错误传播流程；`statusvar_test.rs` 则保留了与 Go `TestStatusVar` 对齐的基础路径。

## 数据与状态

全局可变状态只有 `STATISTICS_LIST` 及 `vardef` 中由默认提供者读取的两个原子计数器。注册表持有 `Arc`，因此注册会增加提供者生命周期；注销只移除一个槽位，只有当其他 `Arc` 也释放后对象才会析构。默认提供者不会被专门保护：虽然 API 不导出其句柄，未来若内部代码取得相同句柄，通用注销逻辑仍会按身份处理。

`DEFAULT_STATUS` 是不可变模板，不直接承载一次查询的实时状态。每次 `DefaultStatusStat::Stats` 都创建新 `HashMap<String, StatusValue>`，但每个默认 `StatusValue` 的内部数据通过 `Arc` 克隆共享；实时覆盖项则创建新的 `StatusValue`。`GetStatusVars` 又创建一层 `StatusVal` 结果映射，所以返回值是本次聚合的快照，不借用注册表或会话。

作用域不变量来自 `DEFAULT_STATUS`：四个 SSL 项同时属于 Global/Session，两个连接属性计数器仅为 Global，`tidb_keys_examined` 仅为 Session。`ScopeFlag` 的实际位定义位于 `pkg/sessionctx/vardef/tidb_vars.rs`。

`None` 会话不是错误：SSL 项保持空字符串、verify mode 保持 `0_i32`，`tidb_keys_examined` 保持 `0_u64`；两个原子计数器仍始终读取实时值。测试还固定了 TLS 1.3 示例 `0x1301 -> TLS_AES_128_GCM_SHA256`、`0x0304 -> TLSv1.3`，以及支持列表包含 25 个以冒号终止的名称。

## 依赖与调用关系

直接下游依赖如下：

- `vardef::ScopeFlag`、`ScopeGlobal`、`ScopeSession` 决定可见范围；`vardef::ConnectAttrsLongestSeen` 和 `ConnectAttrsLost` 提供原子计数。
- `tlsutil::CipherSuiteName` 与 `VersionName` 位于 `pkg/util/tls/tls.rs`；未知 cipher 返回空字符串，未知版本返回格式化十六进制文本，这一行为会原样进入状态快照。
- 标准库 `Any`、`Arc`、`LazyLock`、`RwLock` 和 `HashMap` 分别承担类型擦除、共享所有权、惰性初始化、并发保护和结果聚合。

当前 Rust 图与文本证据表明：`lib.rs` 暴露模块，两个独立测试文件直接调用公开 API；生产 Rust 中只有 `DefaultStatusStat` 实现本文件的 `Statistics`，没有找到其他实现或三个注册/聚合 API 的直接调用。RustCodeGraph 对目标文件报告“used by 33 files”，但精确 callers/callees 查询没有产出可归属到这些 API 的边，因此这里不把文件级依赖误当作函数级接线。

Go 对照提供了完整应用位置：`pkg/executor/show.go` 在 `SHOW STATUS` 时聚合；`pkg/server/server.go`、`pkg/server/conn.go`、`pkg/ddl/ddl.go`、`pkg/bindinfo/binding_handle.go` 和 `pkg/store/gcworker/gc_worker.go` 注册提供者；DDL 与 domain 生命周期还会注销提供者。它们是迁移语义和未来 Rust 接线的证据，不代表同名 Rust 组件当前已经完成接入。

## 错误处理与边界

`GetStatusVars` 使用 `statistics.Stats(vars)?`：任意提供者出错会立即停止后续遍历并把原错误返回，局部构建的 `HashMap` 被丢弃，调用方拿不到部分结果。`provider_errors_are_returned_without_being_silenced` 明确覆盖该约定。`GetScope` 本身不能返回错误；默认实现遇到不在 `DEFAULT_STATUS` 的名称会 `panic!("unknown default status variable: ...")`，所以 `DefaultStatusStat::Stats` 返回的键必须与模板保持一致。

读写锁中毒不会继续 panic：三个公开操作都用 `PoisonError::into_inner` 取回 guard。这提高了注册表的可恢复性，但也意味着锁中毒之后的数据一致性由调用者和已有写入状态承担，本文件不做回滚或审计。

边界还包括：注册不去重；注销只依据 `Arc` 分配身份而非提供者值相等；注销不存在的句柄是无操作；同名状态值遵循最后写入覆盖；`StatusValue` 只接受 `'static` 的 `Any` 值，不能包装借用当前栈帧的非静态引用。`GetStatusVars` 在持有读锁期间执行第三方提供者代码，提供者若同步尝试注册或注销会等待同一锁并可能形成自死锁，因此提供者的 `Stats`/`GetScope` 不应回调注册表写操作。

## 并发与资源生命周期

`Statistics: Send + Sync`、`StatisticsHandle = Arc<dyn Statistics>` 和 `StoredStatusValue: Send + Sync` 使注册表、提供者与结果值可在线程间共享。`RwLock` 允许多个聚合并发读取，但注册/注销与所有聚合互斥；由于 `GetStatusVars` 将读锁保持到所有提供者的 `Stats` 和 `GetScope` 调用结束，慢提供者会延迟写操作，多个慢查询也会延长提供者的有效生命周期。

`LazyLock` 保证默认作用域、注册表、TLS 列表和默认模板各初始化一次。`TLS_SUPPORTED_CIPHERS` 仅在首次需要时构造，此后克隆的是 `String` 本身（插入 `StatusValue::new(TLS_SUPPORTED_CIPHERS.clone())` 时产生拥有所有权的新字符串），不会借用惰性静态值。

原子计数器的内存序封装在 `vardef::AtomicI64Value.Load/Store` 中，本文件只读取，不管理计数器重置。测试修改计数器时使用 `serial_test::serial` 并在结束前恢复旧值，说明这些计数器是跨测试/跨会话共享资源；新增测试必须保持同样的隔离策略。生产提供者资源的启动和停止由各自组件负责，本文件只持有 `Arc`，不会调用显式关闭方法。

## 与 Go 版本的对应关系

Rust 主要逐项对应 `pkg/sessionctx/variable/statusvar.go`：`Statistics`、`StatusVal`、默认作用域、注册表锁、追加注册、从后查找并 swap 删除、聚合时错误短路、25 个 TLS cipher、默认状态集合及会话覆盖规则均保持一致。`pkg/sessionctx/variable/statusvar_test.go:TestStatusVar` 的 mock 注册和作用域断言由 `statusvar_test.rs:test_status_var` 对齐。

语言层差异包括：

- Go 的 `any` 在 Rust 中成为受 `Debug + Display + Send + Sync + Any` 约束的 `StatusValue`，读取具体类型需 `downcast_ref`。
- Go 用接口值相等查找提供者；Rust 只接受 `Arc` 句柄并以 `Arc::ptr_eq` 比较身份。对重复注册同一句柄，两边都删除最后一个匹配项；两个内容相同但分配不同的 Rust 提供者不会互相注销。
- Go 在 `init()` 中先生成 cipher 字符串并调用 `RegisterStatistics(defaultStatusStat)`；Rust 分别通过 `TLS_SUPPORTED_CIPHERS: LazyLock` 和 `STATISTICS_LIST` 初值实现惰性等效初始化。
- Go `GetStatusVars` 返回 `map[string]*StatusVal`，Rust 返回拥有值的 `HashMap<String, StatusVal>`；空会话由 Go `nil` 对应 Rust `None`。
- Go 的真实 `SessionVars` 和 `tls.ConnectionState` 是完整运行时对象；Rust 文件目前定义精简的 `SessionVars`/`TLSConnectionState` 边界类型，源码明确将完整会话映射留给包集成任务。
- Go 已有多个生产提供者和 `SHOW STATUS` 调用链；Rust 当前搜索未发现相应生产接线。这是迁移完成度差异，而不是本文件内部算法差异。

## 扩展指南

新增默认状态变量时，应同时修改 `DEFAULT_STATUS` 和 `DefaultStatusStat::Stats`（若值是动态的），确保 `Stats` 返回的每个名称都能被 `GetScope` 查到；否则聚合会 panic。若新增 TLS 派生项，还应核对 `pkg/util/tls/tls.rs` 的未知值行为。新增值类型必须满足 `StatusValue::new` 的完整 trait 约束，并在独立测试文件中用 `downcast_ref` 或 `Display` 验证实际类型，避免只验证字符串表象。

新增提供者应在拥有其生命周期的组件中创建一个稳定的 `StatisticsHandle`，注册与注销必须使用同一个 `Arc`；不要临时重新构造等价对象去注销。提供者应让 `Stats` 和 `GetScope` 快速、无副作用，尤其不能在这两个回调中同步调用注册/注销。名称冲突需显式决定覆盖顺序，因为注册顺序就是优先级且 `swap_remove` 会改变后续遍历顺序。

测试逻辑必须继续放在独立文件：基础 Go 对齐用例在 `pkg/sessionctx/variable/statusvar_test.rs`，Rust 补充边界在 `statusvar_2_aster_unit_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] #[path = ...]` 挂载。涉及全局注册表或原子计数器的测试应使用 `#[serial]`，在所有退出路径清理已注册句柄并恢复计数器。未来接入 Rust `SHOW STATUS` 时，还需在 executor 独立测试中覆盖作用域过滤、权限和字符串转换；不要把这些展示策略塞回本文件。

兼容风险主要是状态名、具体值类型、作用域和 Go 的 TLS 命名；性能风险主要是每次聚合分配两个映射以及在读锁内调用所有提供者；并发风险主要是回调重入写锁和遗漏注销导致 `Arc` 长期存活。优化分配或缩短锁区间时必须先保持提供者快照、覆盖顺序和错误时不返回部分结果这三项语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/sessionctx/variable/statusvar.rs --offset 1 --limit 400` 返回目标文件 331 行全貌并报告文件级 `used by 33 files`；`query GetStatusVars`、`query RegisterStatistics`、`query UnregisterStatistics` 定位到 Rust/Go 定义及同名符号。精确 `callers/callees` 查询未返回可用函数边，因此未以它证明生产调用。
- Rust 源与模块边界：`pkg/sessionctx/variable/statusvar.rs`、`pkg/sessionctx/variable/lib.rs`、`pkg/sessionctx/variable/Cargo.toml`。
- 直接依赖：`pkg/sessionctx/vardef/tidb_vars.rs` 的 `ScopeFlag`、`ScopeGlobal`、`ScopeSession`、`ConnectAttrsLongestSeen`、`ConnectAttrsLost`；`pkg/util/tls/tls.rs` 的 `CipherSuiteName`、`VersionName`。
- 独立 Rust 测试：`pkg/sessionctx/variable/statusvar_test.rs` 与 `pkg/sessionctx/variable/statusvar_2_aster_unit_test.rs`，覆盖基础 Go 对齐、重复注册、动态默认状态、空会话和提供者错误。
- Go 对照与应用接线：`pkg/sessionctx/variable/statusvar.go`、`statusvar_test.go`、`pkg/executor/show.go`、`pkg/server/server.go`、`pkg/server/conn.go`、`pkg/ddl/ddl.go`、`pkg/domain/domain.go`、`pkg/bindinfo/binding_handle.go`、`pkg/store/gcworker/gc_worker.go`。
- Rust 接线边界复核：`rg` 仅找到 `pkg/executor/show.rs:ShowExec.fetchShowStatus -> ShowOperation::Status`，且生产 Rust 中没有找到本文件三个公开操作的直接调用或额外 `Statistics` 实现；因此文档把完整应用链标为 Go 现状、Rust 待接线事实。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行任务指定的 11 章节结构验证，并人工复核所有“已实现/已接线”表述均有上述路径或符号支持。仓库说明所指的 `.agents/skills/tidb-verify-profile` 在当前工作区不存在，故无法执行其 Ready 文档检查；这不影响任务明确要求且已可运行的结构验证。
