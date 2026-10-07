# `pkg/domain/infosync/error.rs`

## 文件定位

本文件是 `astersql-domain-infosync` crate 的统一错误边界。crate 入口 `pkg/domain/infosync/lib.rs` 以私有模块 `mod error` 装配它，再通过 `pub use error::*` 对外重新导出 `Error` 与 `Result<T>`；因此 crate 内的 InfoSyncer、PD/etcd、标签、放置规则、资源组、Region 和 TiFlash 相关接口都可直接使用这两个名称。crate 归属及 `serde_json`、`thiserror` 依赖由 `pkg/domain/infosync/Cargo.toml` 确认。

文件只声明错误数据模型，不执行网络请求、重试或状态更新。具体错误何时产生、是否重试，由 `info.rs`、`region.rs`、各 manager 等调用方决定。

## 核心职责

- 用公开枚举 `Error` 汇总 infosync 当前可辨识的失败类别，使调用方可以按变体做控制流判断，而不必解析错误文本。
- 为所有变体提供 `std::error::Error`、`Display` 和 `Debug` 能力；其中 `Display` 文本由 `thiserror::Error` 派生及各 `#[error(...)]` 属性确定。
- 通过 `Json(#[from] serde_json::Error)` 建立 JSON 错误的自动转换，使 `serde_json::to_vec`、`from_slice` 等调用可以直接使用 `?`。
- 用 `Result<T>` 将 crate 内公开接口的成功类型与统一的 `Error` 绑定，减少重复签名并稳定错误边界。

它不是完整的 Go 错误码框架复刻：只有 `DomainService` 的展示文本内固定包含领域错误码 `8243`，其余变体主要保留 Rust 侧可匹配的类别与消息。

## 主要符号

### `pub enum Error`

`Error` 位于 `error.rs:23`，派生 `Debug` 与 `thiserror::Error`，包含六个变体：

- `NotInitialized`：无负载标记。`getGlobalInfoSyncer` 在全局 `InfoSyncer` 尚未设置时返回它；展示为 `infoSyncer is not initialized`。
- `PdHttpClientMissing`：无负载标记。需要 PD HTTP 客户端但当前同步器未配置该客户端时返回；展示为 `pd http cli is nil`。
- `PrometheusAddressNotSet`：无负载标记。PD 配置与 etcd 拓扑均没有 Prometheus 地址时返回；展示为 `prometheus address is not set`。
- `DomainService(String)`：保存领域服务返回的消息，展示为 `[domain:8243]{消息}`。`pdResponseHandler` 将非 `200/404/412` 状态的响应体放入该变体；放置规则重试逻辑据此识别不可重试错误。
- `External(String)`：保存无法进一步结构化的外部依赖、未支持操作或本地校验错误文本，展示时不添加前缀。它是有意保留的通用兜底类别。
- `Json(serde_json::Error)`：透明包装 JSON 编解码错误，`Display` 与错误链来源均委托给原始 `serde_json::Error`；`#[from]` 自动生成 `From<serde_json::Error>`。

### `pub type Result<T>`

`Result<T>` 位于 `error.rs:45`，等价于 `std::result::Result<T, Error>`。它不增加运行时包装；crate 内的 trait 方法、管理器和顶层便捷函数广泛用它统一返回类型。

## 执行流程

本文件自身没有函数调用流程。其运行时作用体现在错误构造、转换、传播和分类四步：

1. 下游操作发现失败。例如 `info.rs::getGlobalInfoSyncer` 发现全局单例为空，`region.rs` 发现 PD HTTP 客户端为空，或 `label_manager.rs` 的 JSON 编解码失败。
2. 调用点构造明确变体；JSON 错误则经 `From` 隐式转成 `Error::Json`。函数以 `Result<T>` 向上返回。
3. 大多数上层接口用 `?` 原样传播。最终日志或用户可见错误通过派生的 `Display` 输出稳定文本。
4. 需要策略判断的调用方匹配变体，而不是匹配文本。典型例子是 `info.rs::PutRuleBundlesWithRetry`：遇到 `DomainService` 立即返回；其他错误保存为最后一次错误并按配置重试。

PD 状态映射的边界由 `info.rs::pdResponseHandler` 给出：`200`、`404`、`412` 为成功，其他状态产生 `DomainService(String::from_utf8_lossy(body))`。因此无效 UTF-8 响应体会被有损替换后保存，而不会再产生单独的 UTF-8 错误。

## 数据与状态

`Error` 本身不持有共享状态，也不读写全局变量。三个无负载变体只表达类别；`DomainService` 与 `External` 拥有各自的 `String`；`Json` 拥有原始 `serde_json::Error`。错误值随 `Result<T>` 按所有权移动，离开作用域后由 Rust 正常释放。

`Result<T>` 是编译期类型别名，不产生额外字段、分配、缓存或序列化格式。错误枚举也未派生 `Clone`、`PartialEq`、`Serialize` 或 `Deserialize`，测试和业务逻辑应使用模式匹配检查类别，而不能依赖值相等或将其直接持久化。

## 依赖与调用关系

直接依赖只有两项：

- `thiserror`：生成 `Display`、`std::error::Error` 及透明来源链；由 `Cargo.toml` 的 `thiserror = "2"` 提供。
- `serde_json`：提供 `Json` 变体负载及自动 `From` 转换；由 `Cargo.toml` 的 `serde_json = "1"` 提供。

上游使用面由 `lib.rs` 的重新导出和 RustCodeGraph 的文件关系确认。图谱报告本文件被 13 个文件使用；直接源码检索显示主要生产调用者包括：

- `info.rs`：产生全部专用变体，并以 `Result<T>` 贯穿全局同步器、拓扑、放置规则、资源组及 TiFlash 门面。
- `region.rs`：四个需要 PD HTTP 客户端的 API 产生 `PdHttpClientMissing`。
- `label_manager.rs`：JSON 序列化/反序列化通过 `?` 转为 `Json`。
- `types.rs`：默认 PD HTTP trait 实现用 `External` 表示未支持能力。
- `placement_manager.rs`、`resource_manager_client.rs`、`tiflash_manager.rs`、`mock_info.rs`：用 `External` 表示输入、查找或模拟器状态错误。
- `schedule_manager.rs` 及其他 manager trait：以 `Result<T>` 作为统一接口返回类型。

本文件没有可调用函数，因而没有有意义的下游函数调用边；RustCodeGraph 对精确枚举执行 `callers` 未返回边，变体级引用以 `rg` 的精确匹配补证。

## 错误处理与边界

- `NotInitialized` 与 `PdHttpClientMissing` 必须区分：前者表示全局同步器不存在，后者表示同步器存在但缺少完成特定操作所需的 PD HTTP 客户端。
- `PrometheusAddressNotSet` 只表达配置源均未提供地址；PD/etcd 访问失败或 JSON 无效会沿其他错误路径返回，不能归并为“未设置”。
- `DomainService` 是当前唯一被业务重试逻辑显式分类的变体。`PutRuleBundlesWithRetry` 将其视为不可重试，而 `External` 等错误可重试直到次数耗尽。新增服务错误时若误用 `External`，会改变重试次数、等待时间和外部请求量。
- `External` 不保留结构化来源链，只保留调用点给出的字符串；不应在可用专用变体或可透明包装源错误时滥用它。
- `Json` 保留 `serde_json::Error` 的类别、位置和来源；当前源码没有显式写 `Error::Json`，但 `label_manager.rs`、`info.rs` 中的 JSON 操作通过 `?` 触发自动转换。
- `DomainService` 的 `String` 来自 HTTP 响应体且没有长度限制；本文件不负责截断、脱敏或状态码保存，记录该错误时应由更高层考虑日志敏感性和体积。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、网络连接或取消句柄，也没有自定义 `Drop`。错误值均为拥有型数据，可随普通返回路径跨函数传播；是否能跨线程取决于其负载类型的自动 trait，文件没有手写 `Send`/`Sync` 实现。

并发相关资源由调用方管理。例如全局 `InfoSyncer` 的锁定与克隆发生在 `info.rs`，PD 客户端锁是否在回调前释放由 `region_test.rs` 验证。错误枚举不延长这些锁或客户端的生命周期。重试等待也位于 `PutRuleBundlesWithRetry` 的 `thread::sleep`，不属于本文件。

## 与 Go 版本的对应关系

Go 对照不是单文件逐项映射，而是分散定义：

- `pkg/domain/infosync/error.go` 只定义 `ErrHTTPServiceError = dbterror.ClassDomain.NewStd(errno.ErrHTTPServiceError)`。Rust 用 `DomainService(String)` 承担相应 HTTP 服务错误类别，并在展示文本中保留 `[domain:8243]`；Go 侧则保留 `dbterror` 的标准错误类和 `Equal` 判断能力。
- Go `info.go::getGlobalInfoSyncer` 直接构造文本 `infoSyncer is not initialized`；Rust 将其结构化为 `NotInitialized`，但保持展示文本一致。
- Go `info.go` 与 `region.go` 通过 PD client 的 `ErrClientGetLeader.FastGenByArgs("pd http cli is nil")` 表达客户端缺失；Rust 将其收敛为 `PdHttpClientMissing`，保留消息但不保留 Go 的 PD 错误类型。
- Go `ErrPrometheusAddrIsNotSet` 是 `dbterror` 标准错误；Rust 对应 `PrometheusAddressNotSet`，保留语义和消息，不携带 Go 错误码对象。
- Go 使用 `errors.Trace` 传播 JSON 等一般错误；Rust 用专用 `Json` 透明包装 `serde_json::Error`。`External` 则覆盖当前尚无专用 Rust 类型的其他字符串错误。

控制流对齐比错误表示完全同构更重要：Go `PutRuleBundlesWithRetry` 用 `ErrHTTPServiceError.Equal(err)` 识别后立即返回，Rust 对应地匹配 `Error::DomainService(_)`；Go 与 Rust 的 PD 状态处理都接受 `200/404/412`，其余状态进入领域服务错误。

## 扩展指南

- 新增可由调用方采取不同策略的失败类别时，在 `Error` 中增加专用变体，并同步核对所有按变体分类的逻辑，尤其是 `info.rs::PutRuleBundlesWithRetry`；不要仅把消息塞进 `External`。
- 包装新的第三方错误时，优先使用携带原始错误的透明变体和 `#[from]`，但必须确认同一来源类型只有一个自动 `From` 路径，避免转换冲突。
- 修改 `#[error(...)]` 文本或 `[domain:8243]` 前缀属于外部可见兼容性变化，应与 Go 的 `dbterror` 消息/错误码契约一起核对。
- 如果新增变体携带非线程安全资源，会改变整个 `Error` 的自动 `Send`/`Sync` 能力；错误负载宜保持拥有型、轻量且不持锁。
- 测试逻辑必须继续放在独立文件。变体产生与控制流测试应扩展同目录 `info_test.rs`、`region_test.rs` 或对应 manager 的 `*_test.rs`；若要验证纯展示文本、错误来源链和 JSON 自动转换，应新建独立 `error_test.rs` 并在 `lib.rs` 以 `#[cfg(test)]` 接入，而不是把测试嵌入 `error.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件并覆盖 `pkg/domain/infosync/error.rs`；`files --filter pkg/domain/infosync` 确认目标及相邻生产/测试文件；`node --file pkg/domain/infosync/error.rs --offset 1 --limit 240` 核对完整 45 行源码和 13 个使用文件；`node pkg/domain/infosync/error.rs::Error` 核对枚举源码。精确 `callers` 没有返回变体引用，`callees` 因同名 `Error` 发生歧义，故未把该错误图输出当作调用事实。
- Rust 源与 crate 边界：`pkg/domain/infosync/error.rs`、`pkg/domain/infosync/lib.rs`、`pkg/domain/infosync/Cargo.toml`。
- 直接调用证据：`pkg/domain/infosync/info.rs`、`region.rs`、`label_manager.rs`、`types.rs`、`placement_manager.rs`、`resource_manager_client.rs`、`tiflash_manager.rs`、`mock_info.rs`、`schedule_manager.rs`；使用 `rg` 精确检索六个 `Error` 变体和 `Result<`。
- Rust 独立测试：`pkg/domain/infosync/info_test.rs::test_put_bundles_retry` 验证 `DomainService` 不重试而其他错误重试；`test_set_keyspace_config_without_pdhttp_client` 验证客户端缺失；`pd_status_and_tiflash_rule_contract_match_go` 验证状态码与响应体映射。`pkg/domain/infosync/region_test.rs::replication_state_and_missing_client_match_go_fallbacks` 验证四个 Region API 的缺失客户端边界。未发现直接覆盖 `NotInitialized`、`PrometheusAddressNotSet`、`Json` 展示/来源链的专项 Rust 测试。
- Go 对照：`pkg/domain/infosync/error.go`、`info.go`、`region.go`、`info_test.go`，用于核对错误类、消息、状态码边界和重试分类。
- 本任务为纯文档分析，按计划不运行 Cargo；结构检查用于确认目标文档存在且恰有 11 个固定二级章节。
