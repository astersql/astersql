# `br/pkg/operation/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是独立 Cargo 包 `astersql-br-pkg-operation` 的 crate 根。目录内 [`Cargo.toml`](Cargo.toml) 以 `[lib] path = "lib.rs"` 指向它，根 [`Cargo.toml`](../../../Cargo.toml) 又把 `br/pkg/operation` 列为 workspace member；`package.metadata.porting` 明确将该 crate 对应到 Go 包 `br/pkg/operation`，类型为 library。

本文件是 24 行的装配门面，不直接实现操作上下文。普通构建加载 [`context.rs`](context.rs) 并把其中的公开项平铺到 crate 根；测试构建再加载独立的 [`context_test.rs`](context_test.rs) 和 [`parity_test.rs`](parity_test.rs)。同目录没有 `doc.go`，包语义的最近权威 Go 来源是 [`context.go`](context.go)。

当前接线状态必须与 Go 区分：crate 已进入 workspace，但仓库其他 Cargo manifest 没有声明 `astersql-br-pkg-operation` 依赖。若干 Rust BR 文件使用名为 `operation` 的局部兼容类型，文本证据不能证明它们经过本 crate；Go 的 task、stream 与 restore 代码则直接导入同路径 Go 包。因此本文件目前是可独立编译和测试的迁移 crate 门面，尚不能称为完整 Rust BR 主链的统一操作上下文入口。

## 核心职责

- 用 `#[path = "context.rs"] pub mod context` 建立公开实现模块。
- 用 `pub use context::*` 重导出 `Context`、`HintField`、`LockResourceType`、四个锁资源常量、`LockMetaInput`、`NewContext` 以及日志/时间辅助符号，使调用方可以从 crate 根使用 Go 风格 API。
- 用两个 `#[cfg(test)]` 私有模块挂载独立测试文件，保证 Rust 源文件与测试逻辑不在同一文件。
- 用 crate 级 `#![allow(...)]` 接纳 `OperationID`、`StartedAt`、`NewContext`、`LockMeta` 等 Go 对齐命名，以及迁移阶段的未使用项。

它存在的价值是确定 crate 边界、公共可见性和测试边界；UUID、时间、hint、锁元数据和日志行为全部由 `context.rs` 执行。

## 主要符号

- `pub mod context`：唯一生产子模块，公开 `HintField`、`Context`、`LockResourceType`、`LockMetaInput` 和 `CapturedLog` 等类型，以及 `NewContext` 等函数。
- `pub use context::*`：通配重导出。当前不仅暴露 Go 生产契约，也暴露 `begin_log_capture`、`captured_logs`、`filter_captured_message`、`format_time_rfc3339`、`time_utc` 等 Rust 辅助 API；未来在 `context.rs` 新增 `pub` 项会自动扩大 crate 根 API。
- `mod context_test`：仅在 `cfg(test)` 下编译，覆盖构造、hint 变更、锁元数据校验、Go 零时间、字符串转义和生产日志路径。
- `mod parity_test`：仅在 `cfg(test)` 下编译，从 crate 根验证公开契约和重导出可用性。
- crate 级 `allow`：放宽 `dead_code`、三种命名 lint、未使用导入和未使用变量；它不改变运行期行为，也不证明被允许的符号已经被应用代码使用。

本文件自身没有常量、struct、enum、trait、函数或 `impl`。RustCodeGraph 对该文件只报告一个文件级符号，与纯门面职责一致。

## 执行流程

编译阶段的流程为：

1. Cargo 以 `lib.rs` 为 crate 根，先应用 crate 级 lint 例外。
2. 编译器按显式路径把 `context.rs` 解析为公开模块 `context`。
3. `pub use context::*` 把该模块的公开符号放到 crate 根命名空间。
4. 测试构建额外解析 `context_test.rs` 和 `parity_test.rs`；普通依赖构建不包含这两个模块。

典型运行期流程发生在导出的实现中：`NewContext(command)` 生成 UUID v4、记录当前 `SystemTime`、读取 OS hostname 并记录启动日志；调用方通过 `SetHintField` 添加、覆盖或删除诊断字段；需要对象存储锁时，`LockMeta(resource, detail)` 校验 operation ID、启动时间和资源类型，再生成 `OwnerID`、`LockType` 与由时间、hint、detail 拼接的 `Hint`。`lib.rs` 不参与上述分支，只决定这些入口能否从 crate 根被解析。

## 数据与状态

`lib.rs` 本身不持有数据或状态。它暴露的主要状态位于 `context.rs::Context`：

- `OperationID: String` 是单次 BR 操作身份；`NewContext` 用 UUID v4 填充。
- `StartedAt: SystemTime` 是操作启动时刻；`Context::default()` 使用 Go `time.Time{}` 对应的公元 1 年零值，而不是 Unix epoch。
- 私有 `hintFields: Vec<HintField>` 按插入顺序保存键值；`HintFields()` 返回 clone，`SetHintField` 修改前也 clone，保证上下文副本和外部快照互不回写。
- `LockMetaInput` 是锁层输出值，仅包含 `OwnerID`、`LockType`、`Hint`；本 crate 不持有或续租真实分布式锁。
- 日志测试捕获槽 `LOG_CAPTURE` 是线程局部 `RefCell<Option<Vec<CapturedLog>>>`；`LogCaptureGuard::drop` 恢复先前槽位。

四个 `LockResourceType` 常量的字符串值与 Go 一致：`log-truncate-exclusive`、`migration-read`、`migration-write`、`migration-append`，这是跨语言锁元数据兼容边界。

## 依赖与调用关系

Cargo 层面，该 crate 只有 `uuid = { version = "1", features = ["v4"] }` 一个直接依赖；标准库提供时间、进程 ID、线程局部存储和 `hostname` 子进程调用。根 workspace 收录该 crate，但仓库 Cargo manifests 搜索没有发现其他 crate 依赖包名 `astersql-br-pkg-operation`。

文件内下游关系是 `lib.rs -> context.rs`，测试模式下再有 `lib.rs -> context_test.rs` 和 `lib.rs -> parity_test.rs`。实现内部的关键边为 `NewContext -> hostname -> hostname_from_command`、`NewContext -> emit_log`、`SetHintField -> isInitialized/hintFieldIndex/emit_log`、`LockMeta -> lockHint -> format_time_rfc3339/quote_go_string`。RustCodeGraph 能定位这些符号并在 `explore` 中报告实现内部边；对精确 `callers/callees` 的调用未返回稳定输出，因此没有据此虚构跨 crate 上游。

Rust 文本检索找到 `br/pkg/restore/log_client/client.rs`、`br/pkg/task/common.rs` 等处的 `operation::Context` 或 `OperationContext` 使用，但对应 Cargo manifests 不依赖本 crate，故它们只能作为相似迁移接口，不能列为本门面的已验证调用者。本 crate 已验证的 Rust 使用者是两个独立测试模块。

Go 的真实上游更明确：`br/pkg/task/common.go` 和 `br/pkg/task/operator/migrate_to.go` 创建上下文；`br/pkg/restore/log_client/client.go` 更新 `restore_id` hint；`br/pkg/stream/stream_metas.go` 为 migration read/append/write 锁构造元数据；`br/pkg/task/stream.go` 为日志截断独占锁构造元数据。这些调用解释包在完整 BR 应用中的设计位置，但不是 Rust 已完成接线的证据。

## 错误处理与边界

`lib.rs` 没有自己的错误分支。导出实现的边界如下：

- `NewContext(&str) -> Result<Context, String>` 的 Rust UUID 生成路径当前不会显式产生错误，但保留 `Result` 外形以对应 Go 的可失败构造接口；hostname 命令启动失败、非零退出或空输出时回退为 `"unknown"`。
- `SetHintField` 对空 key 直接忽略；未初始化上下文拒绝非空 value；空 value 表示删除。相同值重复设置仍记录 resolved 日志，值发生变化时额外记录 changed 警告。
- `LockMeta` 依次拒绝空 operation ID、Go 零启动时间和空资源类型，分别返回包含 `operation ID`、`operation started time`、`resource type` 的字符串错误。
- detail 采用 Go `strconv.Quote` 风格转义；时间输出固定为 UTC RFC3339、无亚秒。Unix epoch 和 epoch 前时间是有效值，只有公元 1 年占位值被视为未初始化。
- 未启用日志捕获时，当前 Rust 实现写结构化文本到 stderr，而 Go 使用 PingCAP logger/zap；消息和字段语义对齐，不代表日志后端完全等价。
- 通配重导出会把测试辅助性质的公开函数也形成外部 API；收窄可见性可能是兼容性变更，扩展时需显式审查。

## 并发与资源生命周期

入口不创建线程、异步任务、通道、事务、网络连接或文件锁。`Context` 是拥有所有权的可 clone 值；hint 向量修改采用 clone-before-mutate，worker 间复制后可独立更新，但类型本身没有内部锁，也不提供共享可变访问。

测试日志捕获使用线程局部槽，避免 Rust 测试并行运行时共享一个全局缓冲。`begin_log_capture` 保存旧值并安装空缓冲，`LogCaptureGuard` 在作用域结束或 unwind 时恢复旧值；嵌套捕获因此按 guard 生命周期还原。未捕获的生产日志同步写 stderr，并在创建上下文时同步执行一次 `hostname` 子进程，调用方应意识到这一小段进程 I/O。

`LockMetaInput` 只是元数据快照，不拥有锁资源，也没有 `Drop` 清理、续租或解锁动作。实际对象存储锁的获取、冲突处理和释放属于下游存储/stream 代码，不能从本门面推断。

## 与 Go 版本的对应关系

[`context.go`](context.go) 是直接语义来源。两侧都保留 `Context`、`HintField`、`LockResourceType`、四个锁类型、`NewContext`、`HintFields`、`SetHintField` 和 `LockMeta` 的核心外形；都要求上下文有非空 ID 和非零启动时间，并按插入序生成诊断 hint。

关键差异包括：

- Go `Context` 使用 `time.Time`，Rust 使用 `SystemTime` 并自行实现 Go 零时间和 RFC3339 UTC 格式化。
- Go `LockMeta` 返回 `objstore.LockMetaInput`，Rust 定义本地 `LockMetaInput` 值类型；尚无 Cargo 依赖证明它已接到对象存储 crate。
- Go `NewContext` 的 `uuid.NewRandom` 可返回 error 并由 PingCAP errors 注释；Rust `Uuid::new_v4` 当前直接成功，但签名仍返回 `Result<_, String>`。
- Go 通过 `os.Hostname` 获取主机名，Rust 启动系统 `hostname` 命令；两者失败都回退 `unknown`，但平台可用性和开销不同。
- Go 使用全局 zap logger，Rust 无日志框架依赖，生产路径写 stderr，测试路径使用线程局部捕获。
- Go slice 复制语义由 `slices.Clone` 明确实现；Rust `Vec`/`Context` clone 天然深拷贝字符串，同时实现仍在写入前 clone 以保持迁移意图清晰。

`context_test.rs` 基本复刻 [`context_test.go`](context_test.go) 的构造、hint 副本、更新告警、删除和 LockMeta 校验，并额外覆盖 Go 零时间、控制字符 Quote 和真实 stderr/hostname 路径；`parity_test.rs` 从 crate 根验证门面再导出的公开契约。

## 扩展指南

1. 仅当模块布局、crate 根公开 API 或测试挂载改变时修改 `lib.rs`；上下文业务行为应继续放在 `context.rs`。
2. 新增锁资源类型时同步 Go `context.go`、Rust 常量、锁消费方和独立测试，确认字符串在跨语言锁文件中完全一致。
3. 新增 hint 或锁元数据行为时维持“空 key 忽略、空 value 删除、未初始化拒绝写入、返回快照不可回写”的不变量，并同步 `context_test.rs` 与 Go 测试意图。
4. 测试继续放在独立 `context_test.rs` 或 `parity_test.rs`，不要嵌入 `lib.rs`/`context.rs`。应用接线后，还应在最近调用方增加独立 Rust 测试，验证真实锁存储类型与错误传播。
5. 若要接入完整 Rust BR 主链，应在消费 crate 的 `Cargo.toml` 显式依赖本包，并替换局部兼容 `operation` 类型；同时核对 `LockMetaInput` 是否需要与对象存储 canonical 类型统一。不能仅凭同名类型假定兼容。
6. 调整日志后端、hostname 探测或时间/转义实现时，需保留 Go 可观测字段和字节级 hint 格式；正确性风险集中在锁类型/OwnerID/hint 漂移，兼容风险集中在 glob 重导出和本地 `LockMetaInput`，性能风险主要是每次 `NewContext` 启动 `hostname` 子进程及 hint 更新的 Vec/String clone。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7032 个 Rust 文件；`files --filter br/pkg/operation` 确认 Rust/Go 源与测试集合；`node --file` 完整读取 `lib.rs`、`context.rs`、`context_test.rs`、`parity_test.rs`、`context.go` 和 `context_test.go`；`query NewContext` 区分本包与仓库其他同名函数；精确 `explore` 用于核对实现内部调用关系。
- crate 边界：读取 `br/pkg/operation/Cargo.toml` 和根 `Cargo.toml`，确认包名、`lib.rs` 路径、Go package 元数据、workspace 成员与唯一 `uuid` 依赖；Cargo manifest 文本搜索未发现其他 crate 依赖本包。
- Rust 调用面：读取 `br/pkg/restore/log_client/client.rs`、`br/pkg/task/common.rs` 的相似 operation 接口，并以对应 Cargo manifest 缺少本包依赖为边界，没有把同名符号误判为本 crate 调用。
- Go 应用证据：文本检索 `br/pkg/task/common.go`、`br/pkg/task/operator/migrate_to.go`、`br/pkg/restore/log_client/client.go`、`br/pkg/stream/stream_metas.go`、`br/pkg/task/stream.go`，核对创建、hint 传播及四类锁资源的实际用途。
- 测试证据：Rust `context_test.rs` 覆盖初始化、hint clone/update/delete、日志、Go 零时间、Quote 与 LockMeta 三个错误分支；`parity_test.rs` 验证 crate 根导出。Go `context_test.go` 提供对应测试意图。
- 本任务是纯文档分析，按计划不运行 Cargo。交付使用任务指定命令验证本文存在且恰有 11 个固定二级标题，并人工复核唯一新增生产物是本说明文件；仓库要求的 `.agents/skills/tidb-verify-profile` 当前不存在，故无法加载其 Ready 命令集，采用任务明确规定的文档结构检查作为 Ready 范围证据。
