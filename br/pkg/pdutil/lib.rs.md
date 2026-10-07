# `br/pkg/pdutil/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-pdutil` 的 crate 根。`br/pkg/pdutil/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，仓库根 `Cargo.toml` 又把 `br/pkg/pdutil` 列为 workspace member；包元数据将其 Go 来源标为 `br/pkg/pdutil`、类型标为 `library`。

该文件本身不实现 PD 协议或调度算法，而是装配 [`pd.rs`](pd.rs) 与 [`utils.rs`](utils.rs)、平铺再导出两者的公开 API，并在测试构建中接入四个独立测试模块。仓库内其他 Cargo manifest 未声明对 `astersql-br-pkg-pdutil` 的依赖，因此当前 Rust 形态是可独立编译和验证的迁移 crate，尚不能据同名符号推断它已经进入 BR 的生产主链。

## 核心职责

- 用 `#[path = "pd.rs"] pub mod pd` 注册公开的 PD 控制面模块，其中包含客户端抽象、调度暂停/恢复、配置更新、版本门控和按 key range 设置 label rule 的逻辑。
- 用 `#[path = "utils.rs"] pub mod utils` 注册公开辅助模块，其中包含撤销闭包、placement rule HTTP 查询、规则匹配和 TiDB/PD 键编码辅助。
- 用 `pub use pd::*` 与 `pub use utils::*` 把两个模块的所有公开项再导出到 crate 根，使调用者可直接使用 `astersql_br_pkg_pdutil::PdController`、`GetPlacementRules` 等符号。
- 用 `#[cfg(test)]` 隔离 `parity_test.rs`、`pd_serial_test.rs`、`pd_test.rs`、`utils_test.rs`，保持测试逻辑不进入生产构建，也符合 Rust 源文件与测试文件分离的仓库约定。
- 用 crate 级 `#![allow(...)]` 容纳 Go 移植保留的驼峰命名、当前未使用的兼容 API 和导入；这些允许项只抑制编译告警，不改变类型、错误或并发语义。

## 主要符号

`lib.rs` 没有自定义常量、类型、trait、函数或 `impl`，其公开表面完全由模块声明和通配再导出组成：

- `pub mod pd`：公开实现模块。主要导出包括 `Context`、`PdHttpClient`、`PdClient`、`PdController`、`ClusterConfig`、`LabelRule`、`LabelRulePatch`、配置生成器、版本解析，以及暂停/恢复调度相关函数。
- `pub mod utils`：公开辅助模块。主要导出包括 `UndoFunc`、`Nop`、`PeerRoleType`、`Rule`、`PlacementHttpClient`、`GetPlacementRules`、`SearchPlacementRule` 和键编解码辅助。
- `pub use pd::*` / `pub use utils::*`：把上述公开项复制到 crate 根命名空间。两个子模块未来若新增同名公开项，可能在这里形成导入冲突或意外扩大公共 API。
- `mod parity_test`、`mod pd_serial_test`、`mod pd_test`、`mod utils_test`：私有且仅测试构建可见。测试可通过 `crate::...` 验证 crate 根再导出，也可通过 `crate::pd::...`、`crate::utils::...` 针对具体子模块。

## 执行流程

1. Cargo 从 `lib.rs` 建立 `astersql-br-pkg-pdutil` crate，并应用文件顶部的 lint 允许项。
2. 编译器按显式 `#[path]` 加载 `pd.rs` 与 `utils.rs`。两个模块互相依赖：`pd.rs` 使用 `utils::UndoFunc`/`nop_undo`，`utils.rs` 使用 `pd::Context`。
3. `pub use` 在 crate 根建立统一 API 门面；真正的运行流程仍从调用者选择的下游符号开始，`lib.rs` 不执行初始化、网络请求或后台任务。
4. 典型控制面路径是调用 `PdController`：通过注入的 `PdHttpClient`/`PdClient` 查询集群状态，按 PD 版本决定能力，暂停调度器和配置，并在后台以 TTL 周期刷新；恢复或关闭时唤醒刷新循环并恢复状态。
5. 典型 placement 路径是调用 `GetPlacementRules`：依据 TLS 选择 URL scheme，通过 `PlacementHttpClient` 获取 `/pd/api/v1/config/rules`，处理 200/412/其他状态并解析 JSON；随后 `SearchPlacementRule` 解码 `StartKeyHex`，按 table ID 和 peer role 查找规则。
6. 测试构建额外加载四个测试文件，分别覆盖跨文件 Go/Rust 契约、串行调度生命周期、时长格式和 placement 数据兼容性；非测试构建完全跳过这些模块。

## 数据与状态

`lib.rs` 自身不创建全局变量、堆对象或运行时状态。它暴露的状态均由下游模块定义：

- `PdController` 持有可选 `Arc<dyn PdClient>`、必需的 `Arc<dyn PdHttpClient>`、解析后的 `semver::Version`、暂停通道、两个 `AtomicBool` 和可覆盖的 `SchedulerPauseTTL`。
- `ClusterConfig` 保存原调度器列表、原 schedule 配置和可选 region label rule ID，作为后续恢复的快照。
- `Context` 以共享 `AtomicBool` 表示协作式取消；克隆共享同一取消标志。
- `Rule`、`LabelRule`、`LabelRulePatch` 等结构承载 PD HTTP JSON；`Rule` 还保留原始十六进制 key、角色、约束和版本字段。
- `UndoFunc` 是可在线程间共享的 `Arc<dyn Fn(Context) -> Result<(), SharedError> + Send + Sync>`，用于把恢复动作作为拥有所有权的闭包返回。

通配再导出不复制运行时数据，只增加符号解析路径；从 `crate::PdController` 与 `crate::pd::PdController` 取得的是同一个类型定义。

## 依赖与调用关系

crate 边界由 `br/pkg/pdutil/Cargo.toml` 给出：内部错误依赖为 `astersql-br-pkg-errors` 和 `astersql-errors`，外部依赖为 `hex`、`semver`、`serde`、`serde_json`、`uuid`。`lib.rs` 不直接引用这些库，具体使用发生在 `pd.rs` 和 `utils.rs`。

下游关系为：

- `lib.rs -> pd.rs`：模块装配和根级再导出；`pd.rs -> utils.rs::{UndoFunc, nop_undo}`。
- `lib.rs -> utils.rs`：模块装配和根级再导出；`utils.rs -> pd.rs::Context`。
- 测试构建下，`lib.rs` 分别加载四个 `*_test.rs`/`parity_test.rs` 文件；它们调用两个生产模块并以 mock trait 实现隔离真实 PD/TiKV 网络。

上游关系必须按 Cargo 事实解释：仓库 Cargo manifests 中只有本包的 `name`/porting 元数据和根 workspace member 记录，没有其他 crate 依赖 `astersql-br-pkg-pdutil`。RustCodeGraph 对 `pd.rs`、`utils.rs` 给出的若干其他目录“used by”结果包含常见 `crate::pd`/`crate::utils` 名称的跨 crate 匹配，不能作为生产接线证据。当前可确认的 Rust 上游是本 crate 的测试模块；Go 生产调用者仍直接导入 Go 包 `github.com/pingcap/tidb/br/pkg/pdutil`。

## 错误处理与边界

- `lib.rs` 没有可失败操作；错误类型与传播策略来自再导出的实现。网络、PD 返回和 JSON 解析错误通常以 `SharedError` 返回。
- `GetPlacementRules` 将 HTTP 412 解释为 placement rules 未启用并返回空列表；非 200/412 响应包装为 `ErrPDInvalidResponse`，200 响应的 JSON 解析错误继续上抛。
- `SearchPlacementRule` 对非法十六进制或非法 memcomparable key 选择跳过该规则，而不是终止整次搜索；无法识别的表键解码为 `0`。
- `PdController::ResetTS` 把包含 `Forbidden` 的错误视为旧版本 PD 不支持该 API并返回成功，其他错误不吞掉。
- 暂停配置能力受 PD `>= 4.0.8` 门控，按 key range 的 label TTL 能力受 PD `>= 6.1.0` 门控；解析非法版本时回退到 `0.0.0`，从而选择保守路径。
- 根级通配再导出是 API 边界：新增公开符号会自动暴露，重名会导致编译冲突；扩展时应显式检查，而不能依赖顶部的 `allow` 掩盖问题。

## 并发与资源生命周期

`lib.rs` 不直接启动线程或持有资源，但它决定哪些并发接口成为 crate 公共 API。`PdController` 的 client/通道通过 `Mutex` 管理，取消和关闭状态用原子变量管理；HTTP/client trait 与撤销闭包均要求 `Send + Sync`，便于后台线程共享。

暂停流程先同步写入调度器 delay 和可选配置，再创建刷新线程；刷新周期为暂停 TTL 的约三分之一，收到 `ResumeSchedulers` 信号、`Close` 丢弃 sender 或 context 取消后退出。按 key range 暂停会创建 `schedule=deny` label rule 并周期刷新 TTL，返回的等待闭包负责取消并等待清理完成。`PdController::Close` 以原子门闩保证幂等，依次关闭可选 PD client、HTTP client并断开暂停通道。

测试生命周期也被 crate 根明确控制：四个测试模块只在 `cfg(test)` 下编译。`pd_serial_test.rs` 和 `parity_test.rs` 使用 `Arc`、`Mutex`、channel 和 mock client 验证暂停刷新、恢复、关闭、错误分类及 label TTL 清理，不需要真实 PD/TiKV。

## 与 Go 版本的对应关系

Go 包由同目录 `pd.go` 与 `utils.go` 共同组成，天然共享 `package pdutil` 命名空间；Rust 没有 Go 式目录包，因此 `lib.rs` 用两个显式模块加两条通配再导出模拟相同的包级使用体验。

- Go `pd.go` 对应 Rust `pd.rs`：`PdController`、调度器/配置暂停恢复、版本判断、ResetTS 兼容和 region label TTL 是主要对照面。Rust 以 `PdHttpClient`/`PdClient` trait 注入外部边界，而 Go 直接使用 PD client 类型。
- Go `utils.go` 对应 Rust `utils.rs`：`UndoFunc`、`GetPlacementRules`、`SearchPlacementRule` 及 table key 语义保持对应。Rust 把 HTTP GET 抽象为 `PlacementHttpClient`，并在本文件内提供所需的 memcomparable/table ID 编解码。
- Go `pd_serial_test.go` 对应 Rust `pd_serial_test.rs`；Rust `parity_test.rs` 还聚合配置、placement、暂停生命周期和错误类别的对等契约；`pd_test.rs` 与 `utils_test.rs` 补充时长精度和 PD JSON/API v2 key 边界。
- Go `main_test.go` 通过 `goleak.VerifyTestMain` 做包级 goroutine 泄漏检查；Rust 当前没有等价的全局测试入口，主要以显式等待、取消和关闭断言验证生命周期。这一差异不能表述为已经具备完整泄漏检测。

## 扩展指南

- 新增 PD 控制面能力应放入 `pd.rs`，新增 placement/key 辅助应放入 `utils.rs`；只有新增第三个职责清晰的模块时才修改 `lib.rs` 的模块装配。
- 新增或修改生产逻辑时同步扩展独立测试文件，不要把 `#[cfg(test)]` 测试实现内嵌进 `lib.rs`、`pd.rs` 或 `utils.rs`。调度生命周期优先放入 `pd_serial_test.rs`，跨 Go/Rust 契约放入 `parity_test.rs`，局部边界放入相应 `*_test.rs`。
- 新增 `pub` 项前检查两次导出路径和名称冲突；若符号只供模块内部使用，应降低可见性，避免被 `pub use ...::*` 自动纳入 crate API。
- 接入生产 Rust BR 时必须在消费方 Cargo manifest 添加明确依赖并替换相应本地桩/同名模块导入，同时验证 trait 实现、错误类型和关闭责任；workspace membership 本身不代表已接线。
- 修改暂停/恢复流程时重点保持“首次写入成功后才启动刷新、TTL 周期刷新、恢复时清理规则和配置、Close 幂等”的不变量。兼容风险集中在 PD 版本门控、HTTP 状态特殊处理和 Go 风格公开名称；性能风险集中在后台刷新频率、顺序 HTTP 请求和锁持有范围。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter br/pkg/pdutil` 确认 crate 根、两个生产模块、四个 Rust 测试及 Go 对照文件；`node --file` 核对 `lib.rs` 全文、`PdController`/trait/恢复路径和 placement 查询/键解码实现；`query` 区分 Rust/Go 同名符号。
- crate 与接线：`br/pkg/pdutil/Cargo.toml`、根 `Cargo.toml`、`Cargo.lock`；对全部 Cargo manifests 搜索包名与路径，未发现其他 Rust crate 声明依赖。`br/pkg/pdutil/BUILD.bazel` 仅描述同目录 Go library/test，不是 Rust 消费证据。
- Rust 源码：`br/pkg/pdutil/lib.rs`、`pd.rs`、`utils.rs`；独立测试：`parity_test.rs`、`pd_serial_test.rs`、`pd_test.rs`、`utils_test.rs`。
- Go 对照：`br/pkg/pdutil/pd.go`、`utils.go`、`pd_serial_test.go`、`main_test.go`，用于核对包级导出、调度/版本/ResetTS/label TTL、placement rule 和测试生命周期意图。
- 本任务只新增说明文档，按计划不运行 Cargo。交付前执行任务指定的结构命令，确认文档存在且恰有 11 个固定二级章节，并人工复核 `lib.rs` 本身是无运行时逻辑的 crate 门面、生产接线状态未被夸大。
