# `br/pkg/gluetikv/glue.rs`

## 文件定位

本文件属于 `astersql-br-pkg-gluetikv` library crate；crate 根 `br/pkg/gluetikv/lib.rs` 以 `#[path = "glue.rs"]` 挂载该模块，并把模块内符号重新导出。它实现 `br/pkg/glue/glue.rs` 定义的 `Glue` trait，为“不经过 TiDB SQL/Domain 层”的 BR 场景提供统一适配面。直接的上层组合者是 `br/pkg/gluetidb/glue.rs`：其中的 TiDB Glue 持有 `TikvGlue`，并把 `Open`、`StartProgress`、`Record`、`GetVersion` 委托到这里。

当前实现处于迁移期。`br/pkg/gluetikv/Cargo.toml` 明确为避开 arm64/grpc 依赖，将真实 TiKV driver、进度展示和 summary 收集替换为本地实现。因此，本文件是可编译、可测试的接口适配器，但 `default_open` **不会建立真实 PD/TiKV 连接**。

## 核心职责

1. 以 `Glue` 实现 `astersql_br_pkg_glue::Glue` 的完整对象安全接口，并通过 `AsConsoleGlue` 暴露内嵌的 `StdIOGlue`。
2. 在 `Open` 中复刻 Go 版本的 TLS 全局配置副作用：只有 `SecurityOption.CAPath` 非空时，才同步写入 CA、证书和私钥路径。
3. 为尚未接入的外部能力提供可观察替身：`NamedStorage` 表示已通过同步校验的 TiKV URL，`CounterProgress` 提供原子计数，`RECORDS` 保存 summary 等价记录。
4. 明确隔离 SQL 能力：`GetDomain`/`CreateSession` 返回占位对象；`NilSession` 的业务方法统一报错；`UseOneShotSession` 直接成功且不执行回调，对齐 Go 的空行为。
5. 生成 BR 版本文本，并报告存储所有权和 CLI 客户端类型。

## 主要符号

- `pub struct Glue { pub StdIOGlue: StdIOGlue }`：无业务状态的主适配器。`Glue::new()` 构造默认控制台实现；类型为 `Default + Clone + Copy`。
- `impl GlueTrait for Glue`：公开行为主体，包含 `GetDomain`、`CreateSession`、`Open`、`OwnsStorage`、`StartProgress`、`Record`、`GetVersion`、`UseOneShotSession`、`GetClient` 和 `AsConsoleGlue`。
- `pub type OpenFn`、`OPEN_HOOK`、`set_open_hook_for_test`：进程级可注入打开边界。hook 接收路径和 `SecurityOption`，返回 `Box<dyn Storage>` 或 `SharedError`。
- `default_open`、`validate_tikv_path`、`NamedStorage`：默认替身路径。校验 `tikv://` 前缀、非空 PD authority，以及 `disableGC` 查询参数是否能解析为布尔值；成功后只返回以原路径命名的 storage。
- `RECORDS`、`records_slot`、`take_records_for_test`：保存并排空 `(name, 1, value)`，模拟 `summary.CollectSuccessUnit` 的可观察结果。
- `CounterProgress`：使用 `AtomicI64` 实现 `Progress`；`Inc` 委托 `IncBy(1)`，`GetCurrent` 读取当前值，`Close` 为空操作。
- `NilSession`：满足 `Session` trait 的占位实现。除 `Close` 和返回擦除句柄的 `GetSessionCtx` 外，SQL、DDL、变量和元数据方法均返回 `gluetikv: session is nil`。

## 执行流程

`Open(path, option)` 的顺序是：先检查 `option.CAPath`；若非空，则克隆当前全局配置，写入三项 cluster SSL 字段并调用 `store_global_config`。随后锁住 `OPEN_HOOK`，克隆当前 hook 后释放锁；存在 hook 时把原始参数交给 hook 并原样返回其结果，否则进入 `default_open`。默认路径先由 `validate_tikv_path` 做本地语法校验，再构造 `NamedStorage`，不会进行网络 I/O。

`StartProgress` 每次创建独立的 `CounterProgress`，初值为零。调用者可通过 `Inc` 或 `IncBy` 并发累加，并以 `GetCurrent` 读取；传入的 context、命令名、总量和日志重定向标志当前均未参与展示逻辑。

`Record(name, value)` 获取全局记录缓冲锁并追加 `(name.to_string(), 1, value)`；`take_records_for_test` 在同一把锁下用 `mem::take` 原子式取走整个向量并将缓冲恢复为空。

其余 trait 流程为直接映射：`GetVersion` 返回 `"BR\n" + BuildInfo()`；`OwnsStorage` 恒为 `true`；`GetClient` 恒为 `ClientCLP`；`AsConsoleGlue` 返回内嵌 `StdIOGlue` 的新 `Arc`；`UseOneShotSession` 忽略 storage、关闭标志和回调并返回 `Ok(())`。

## 数据与状态

`Glue` 自身只含零尺寸的 `StdIOGlue`，可复制且不拥有连接。可变状态都在进程级静态槽中：`OPEN_HOOK: OnceLock<Mutex<Option<OpenFn>>>` 保存可选 opener，`RECORDS: OnceLock<Mutex<Vec<(String, i32, u64)>>>` 保存记录。两者首次访问才初始化，因此创建 `Glue` 不会分配这些缓冲。

`NamedStorage.name` 保存完整输入路径；它仅覆盖 `Storage::name`，其他能力沿用 `Storage` trait 默认值。`CounterProgress.current` 是有符号 64 位原子计数，没有总量、完成态或溢出保护。`NilSession` 不保存状态；`GetSessionCtx` 每次返回一个新的 `Arc<()>` 擦除句柄，并不代表真实 session context。

## 依赖与调用关系

crate 的直接依赖由 `br/pkg/gluetikv/Cargo.toml` 限定为 `astersql-br-pkg-glue`、版本构建信息、全局配置和错误库。关键下游分别是：接口/占位类型来自 `br/pkg/glue/glue.rs`，版本内容来自 `astersql_br_pkg_version_build::Info`，TLS 状态经 `astersql_config::{get_global_config, store_global_config}` 读写，所有可失败路径使用 `astersql_errors::{New, SharedError}`。

RustCodeGraph 对目标文件列出 38 个符号，并显示文件级直接使用者为 `br/pkg/gluetikv/parity_test.rs`。由于 trait 调用和字段委托不一定形成静态直接边，还需结合源码确认生产关系：`br/pkg/gluetidb/glue.rs` 导入本 crate 的 `Glue as TikvGlue`，在 `New()` 中构造它，并在自身 `Open`、`StartProgress`、`Record`、`GetVersion` 中委托。本仓库的 `br/cmd/br/*.rs` 当前使用另一套 `br/cmd/br/stubs.rs::TikvGlue`，不能把 Go CLI 对 `gluetikv.Glue{}` 的直接使用误写为 Rust CLI 已直接接入本 crate。

## 错误处理与边界

`validate_tikv_path` 拒绝三类已实现边界：缺少 `tikv://` 前缀、scheme 后首个 path 分段中没有任何非空逗号分隔 PD 地址、以及 `disableGC` 值无法解析为 Rust `bool`。其他查询参数被忽略；它也不验证主机/端口格式、重复参数、percent encoding 或 keyspace 参数语义。通过校验仅表示可以到达当前替身边界，不表示集群可访问。

hook 返回的错误由 `Open` 原样传播。TLS 全局配置写入发生在调用 hook/default opener 之前，因此后续打开失败不会回滚配置。所有 `Mutex::lock()` 都用 `expect` 或 `unwrap` 风格处理：若持锁线程 panic 导致 mutex poisoned，后续访问会 panic，而不是返回 `SharedError`。`NilSession` 的业务方法返回固定错误，避免把 Go 的 `nil session` 在 Rust 中误当作可用对象；但 `GetDomain` 返回占位 `Arc<Domain>`、`CreateSession` 返回 `NilSession`，只是因 Rust trait 返回类型不能表达 Go 的 `(nil, nil)`。

## 并发与资源生命周期

`OPEN_HOOK` 和 `RECORDS` 各自由独立互斥锁串行保护；hook 在锁内克隆、锁外执行，避免用户回调期间长期占锁或产生直接重入死锁。测试必须在结束时调用 `set_open_hook_for_test(None)` 并恢复全局配置，否则并行用例或后续调用会观察到污染。记录缓冲同样跨所有 `Glue` 实例共享，`take_records_for_test` 会消费所有尚未读取的记录。

`CounterProgress` 使用 `Ordering::Relaxed`，只保证计数本身的原子性，不建立与其他内存状态的 happens-before 关系；这足以支持独立计数读取，但不能作为任务完成同步原语。`Close` 不释放外部资源，也不阻止后续累加。`NamedStorage` 没有连接或显式关闭逻辑，drop 只释放路径字符串。`AsConsoleGlue` 每次返回新的 `Arc<StdIOGlue>`；`Glue` 本身不持有该 `Arc`。

## 与 Go 版本的对应关系

Go 对照文件是 `br/pkg/gluetikv/glue.go`。两端一致的公开意图包括：仅 TiKV 的 Glue、拥有打开的 storage、`ClientCLP`、`"BR\n" + build.Info()`、CA 非空时同步三项 cluster SSL 配置、`UseOneShotSession` 不调用回调，以及通过 `StdIOGlue` 提供控制台能力。

迁移差异必须显式保留：Go `Open` 调用真实 `driver.TiKVDriver{}.Open(path)`，Rust `default_open` 仅验证部分 URL 并返回 `NamedStorage`；Go `StartProgress` 调用 `utils.StartProgress`，Rust 只有内存计数；Go `Record` 写 summary 聚合器，Rust 写测试可排空的全局向量；Go 的 `GetDomain`/`CreateSession` 返回 `nil, nil`，Rust 用 `Domain`/`NilSession` 占位以满足非可空 trait 返回类型。`br/pkg/gluetikv/Cargo.toml` 将真实 driver/summary 接线明确列为 grpc 环境健康后的待办，因此这些差异不是已完成能力。

`br/pkg/gluetikv/glue_test.go::TestGetVersion` 只约束版本字符串顺序。Rust 的 `glue_test.rs::test_get_version` 对齐该断言；`parity_test.rs::go_rust_public_contract_matches` 进一步固定存储所有权、客户端类型、空会话行为、记录单位数、进度计数、TLS 条件副作用、hook 错误透传、默认路径校验和控制台暴露。

## 扩展指南

接入真实 TiKV driver 时，应替换 `default_open`/`NamedStorage`，保持 `Open` 中 TLS 配置先行、hook 优先和错误透传顺序；同时更新 `Cargo.toml` 依赖与说明，并在独立的 `glue_test.rs` 或 `parity_test.rs` 中覆盖有效连接边界和失败映射。不要把测试写回本生产文件。

恢复真实进度或 summary 时，修改 `StartProgress`/`Record`，同时确认并发、关闭语义、输出重定向和聚合单位仍与 Go 一致；如果保留测试 hook，应避免进程级测试状态在并行执行时串扰。若要改变 URL 解析，优先复用真实 driver 的解析实现，而不是继续扩充一套可能漂移的局部解析器。

扩展 session/domain 能力不应直接让 `NilSession` 静默成功；应先调整 `br/pkg/glue/glue.rs` 的抽象和上层调用契约，再与 `br/pkg/gluetidb/glue.rs` 的真实 Domain/session 路径划清职责。任何公开方法语义变化都应同步核对 `br/pkg/gluetikv/glue.go`、`glue_test.go`、`glue_test.rs` 和 `parity_test.rs`，并评估全局配置兼容性、mutex 争用以及真实网络 I/O 带来的性能和生命周期风险。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/gluetikv` 确认目标及 Go/测试邻接文件；`node --file br/pkg/gluetikv/glue.rs` 读取 309 行实现并报告 38 个符号；`explore` 确认 `set_open_hook_for_test -> parity_test`、`default_open -> validate_tikv_path`、`Record -> records_slot` 等调用边。
- Rust 源与接口：`br/pkg/gluetikv/glue.rs`、`br/pkg/gluetikv/lib.rs`、`br/pkg/glue/glue.rs`、`br/pkg/gluetidb/glue.rs`。
- crate 边界：`br/pkg/gluetikv/Cargo.toml`；其 porting metadata 指向 Go 包 `br/pkg/gluetikv`，依赖注释记录了当前替身范围。
- Go 对照：`br/pkg/gluetikv/glue.go`、`br/pkg/gluetikv/glue_test.go`；Go CLI 入口还可在 `br/cmd/br/backup.go`、`br/cmd/br/restore.go` 看到 `gluetikv.Glue{}` 的使用。
- 独立 Rust 测试：`br/pkg/gluetikv/glue_test.rs`、`br/pkg/gluetikv/parity_test.rs`。本任务为纯文档分析，按计划未运行 Cargo；结论来自结构查询和源码/测试对照，不声称执行过测试。
