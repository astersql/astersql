# `pkg/store/mockstore/tikv.rs`

## 文件定位

`tikv.rs` 是 `astersql-store-mockstore` crate 中 MockTiKV 后端的薄入口。crate 根在 [`pkg/store/mockstore/lib.rs`](lib.rs)，其中以 `pub mod tikv` 暴露本模块；crate 边界和直接依赖由 [`pkg/store/mockstore/Cargo.toml`](Cargo.toml) 定义。这个文件不实现 KV、MVCC、PD 或 RPC 协议，而是把已经汇总好的 `MockOptions` 交给 [`crate::unistore::build_embedded`](unistore.rs)，并用 `StoreType::MockTiKv` 保留调用方可观察的后端身份。

上游工厂 [`NewMockStore`](mockstore.rs) 在 `config.store_type == StoreType::MockTiKv` 时调用 `new_mock_tikv_store`。常见入口是 [`MockTiKVDriver::open`](mockstore.rs)：它解析 `mocktikv://` URI，填入路径、后端类型及全局事务本地闩配置，然后进入 `NewMockStore`。因此，本文件处于“测试存储选项已解析”与“共享嵌入式协议服务开始构建”之间。

## 核心职责

本文件只有两项职责：

1. `new_mock_tikv_store` 把 `&MockOptions` 和固定的 `StoreType::MockTiKv` 传给共享构建器，返回 `Result<MockStorage>`。
2. `newMockTikvStore` 提供 Go 风格命名的兼容别名，行为完全委托给 snake_case 入口。

这里刻意没有复制 [`build_embedded`](unistore.rs) 的集群创建、keyspace 转换、inspector/hijacker 调用和 `MockStorage` 组装逻辑。当前 Rust 工作区只有一套规范的进程内 TiKV 协议服务；MockTiKV 和 EmbedUnistore 共用构建流水线，仅以传入的 `StoreType` 区分门面身份。这个事实由 `tikv.rs::new_mock_tikv_store`、`unistore.rs::new_unistore` 对同一构建器的调用共同证明。

## 主要符号

- `pub fn new_mock_tikv_store(options: &MockOptions) -> Result<MockStorage>`：本模块的规范 Rust API。它不修改 `options`，也不持有其引用；函数体直接调用 `crate::unistore::build_embedded(options, StoreType::MockTiKv)`。
- `pub fn newMockTikvStore(options: &MockOptions) -> Result<MockStorage>`：Go 风格兼容 API，只调用 `new_mock_tikv_store`。crate 根允许 `non_snake_case`，所以该别名不需要局部 lint 豁免。
- `MockOptions`：由 [`mockstore.rs`](mockstore.rs) 定义的输入配置，包含路径、PD 地址、keyspace、集群检查器、客户端/PD 劫持器、事务本地闩容量等。本文件只透传它。
- `MockStorage`：由 [`mockstore.rs`](mockstore.rs) 定义的返回门面，持有 RPC 客户端、PD 客户端、集群、当前 keyspace、闩容量和后端标识。
- `StoreType::MockTiKv`：本文件唯一注入的固定策略值，确保共享构建器产出的 `MockStorage.backend` 仍标识为 MockTiKV。
- `Result<T>`：`mockstore.rs` 中 `std::result::Result<T, StoreError>` 的别名；本文件不新增错误类型或错误转换。

文件内没有常量、结构体、枚举、trait、`impl`、条件编译项或可变模块状态。

## 执行流程

规范调用链如下：

1. 调用方通过 `MockTiKVDriver::open` 或直接向 `NewMockStore` 提供 `WithStoreType(StoreType::MockTiKv)`；驱动路径还会加入 `WithPath`，并在全局配置启用时加入 `WithTxnLocalLatches`。
2. `NewMockStore` 依次应用函数式选项，并处理默认/显式 keyspace；随后按 `StoreType` 分派到 `tikv.rs::new_mock_tikv_store`。
3. `new_mock_tikv_store` 固定传入 `StoreType::MockTiKv`，调用 `unistore.rs::build_embedded`。
4. `build_embedded` 把测试侧 keyspace 元数据转换为嵌入式格式，调用 `embedded_unistore::New` 创建 RPC 客户端、PD 客户端和集群；然后依序运行 `cluster_inspector`、可选 `client_hijacker`、可选 `pd_client_hijacker`。
5. 构建器返回 `MockStorage`，其中 `backend` 为本文件传入的 `MockTiKv`，其余字段来自 `MockOptions` 或嵌入式服务。
6. `NewMockStore` 在需要时再执行 DDL checker 注入；该步骤发生在本文件返回之后，不属于 `tikv.rs` 的职责。

若调用 `newMockTikvStore`，只会在第 2、3 步之间多经过一次无状态转发，不改变分支、错误或返回值。

## 数据与状态

本文件自身不创建、缓存或修改状态。`&MockOptions` 是共享借用，函数内没有 clone、锁或全局写入；真正的资源创建在 `build_embedded` 中完成。

对返回状态最重要的不变量是：`MockStorage.backend == StoreType::MockTiKv`。其他字段的来源为：`client`、`pd_client`、`cluster` 来自 `embedded_unistore::New` 并可能经过 hijacker；`current_keyspace` 来自 `MockOptions::current_keyspace_meta()`；`txn_local_latches` 原样取自选项；`ddl_checked` 在共享构建器中初始化为 `false`。路径为空时是否采用内存/临时存储、keyspace ID 的含义以及集群 bootstrap 均由下游嵌入式实现决定，而不是本文件单独决定。

## 依赖与调用关系

上游调用边：

- RustCodeGraph 将 `mockstore.rs::NewMockStore` 标为 `new_mock_tikv_store` 的调用者；源码中的 `StoreType::MockTiKv` 分支也直接证明该边。
- RustCodeGraph 将 `tikv.rs::newMockTikvStore` 标为 `new_mock_tikv_store` 的调用者。当前索引未显示兼容别名的其他静态调用者。
- `MockTiKVDriver::open` 不直接调用本文件，而是通过 `NewMockStore` 间接进入。

下游调用边：

- RustCodeGraph 将 `unistore.rs::build_embedded` 标为 `new_mock_tikv_store` 的唯一被调用函数。
- `build_embedded` 继续依赖 `embedded_unistore::New`、`MockOptions::current_keyspace_meta` 及三个选项回调槽位；这些是共享构建器的依赖，不是本文件新增的依赖。

Cargo 层面，`astersql-store-mockstore` 直接依赖 `astersql-config` 和路径 crate `astersql-store-mockstore-unistore`。`lib.rs` 把后者再导出为 `embedded_unistore`，所以本文件通过 crate 内部 `unistore` 适配层间接使用它，没有外部第三方依赖或 feature 分支。

## 错误处理与边界

`new_mock_tikv_store` 和别名均使用 `?` 等价的直接 `Result` 传播语义：本文件不包装、不吞掉、也不重试错误。共享构建器只在 `embedded_unistore::New` 失败时把下游错误文本转换为 `StoreError`；成功创建资源后，inspector/hijacker 回调没有 `Result` 返回通道，其 panic 会按 Rust panic 语义向上传播。

边界条件包括：

- 本文件不验证路径、PD 地址、keyspace 组合或 latch 容量；这些约束属于选项层或下游实现。
- `MockOptions::current_keyspace_meta` 在非空 current ID 找不到对应元数据时会 panic；该检查发生在 `build_embedded` 组装返回值时。
- URI scheme 校验发生在 `MockTiKVDriver::open`；[`tikv_test.rs`](tikv_test.rs) 覆盖缺失 scheme 和非 `mocktikv` scheme 的错误，但直接调用本文件会绕过 URI 校验。
- `newMockTikvStore` 是完全等价的兼容别名，不能在其中加入与 `new_mock_tikv_store` 不一致的校验或默认值。

## 并发与资源生命周期

本文件不启动线程/任务，不创建通道，不获取锁，也不持有静态可变状态，因此没有独立的并发协议。并发安全边界来自输入和返回类型：`MockOptions` 中回调以 `Arc<dyn Fn ... + Send + Sync>` 保存，RPC/PD 客户端与集群也由共享所有权对象持有；本文件只在同步构建阶段借用配置。

资源生命周期从 `build_embedded` 调用 `embedded_unistore::New` 开始，所有权随后进入返回的 `MockStorage`。调用方应在使用结束时调用 `MockStorage::close`，它把关闭动作委托给底层 RPC 客户端并把错误转换为 `StoreError`。[`tikv_test.rs`](tikv_test.rs) 的 driver 测试在每次断言后显式关闭 store，说明这是预期的测试资源清理方式。本文件自身没有失败后的局部资源清理逻辑；初始化失败时的清理由 `embedded_unistore::New` 负责。

## 与 Go 版本的对应关系

Go 对照文件是 [`pkg/store/mockstore/tikv.go`](tikv.go)。两版入口都接收 mock 选项并返回测试存储，也都要求集群 inspector、client/PD hijacker、事务本地闩、附加 TiKV 选项和当前 keyspace 最终进入构建结果，但实现形态不同：

- Go `newMockTikvStore` 直接调用 `testutils.NewMockTiKV` 创建 client/cluster/PD client，再调用 `tikv.NewTestTiKVStore` 和 `mockstorage.NewMockStorage`。
- Rust `new_mock_tikv_store` 不逐项复刻上述 client-go 装配，而是以 `StoreType::MockTiKv` 调用与 Rust `new_unistore` 共用的 `build_embedded`。因此“MockTiKV 身份”得以保留，但底层实际是工作区规范的进程内协议服务。
- Go 函数会用 `errors.Trace` 包装首次创建错误；Rust 的对应错误文本转换位于共享构建器，入口本身直接返回其 `StoreError`。
- Go 只有未导出的 camelCase 入口；Rust 同时提供规范 snake_case 公共函数和 Go 风格公共别名，以兼顾 Rust 调用和迁移期命名兼容。

Go [`tikv_test.go`](tikv_test.go) 与 Rust [`tikv_test.rs`](tikv_test.rs) 都通过 driver 验证全局 latch 开关和 URI 错误。Rust 测试没有直接断言两个入口别名等价，也没有在此文件单独测试 inspector/hijacker/keyspace；这些行为由共享工厂及其独立测试承担。

## 扩展指南

新增 MockTiKV 专属行为前，先判断它属于“后端选择”还是“共享嵌入式构建”。只有必须在 MockTiKV 身份下发生、且不应影响 EmbedUnistore 的策略，才适合接入 `new_mock_tikv_store`；集群创建、keyspace、客户端/PD 劫持、闩或公共资源组装应继续修改 `unistore.rs::build_embedded` 或选项层，避免两条后端路径漂移。

修改时应保持以下约束：

- `newMockTikvStore` 继续只委托规范入口，避免两套实现。
- `StoreType::MockTiKv` 必须传入最终 `MockStorage.backend`，否则上层按后端身份判断的行为会失真。
- 不要在入口吞掉下游错误、隐式改变 keyspace 或绕开 inspector/hijacker 顺序。
- 若增加异步任务或额外资源，必须定义失败回滚和 `MockStorage::close` 的对称清理路径。

测试逻辑应放在独立的 [`pkg/store/mockstore/tikv_test.rs`](tikv_test.rs)，不要嵌入生产文件。入口分派或通用选项变化还应同步检查 [`mockstore_test.rs`](mockstore_test.rs)；Go 语义变化则对照 `tikv.go`、`tikv_test.go`。高风险点是与 Go 测试后端语义不一致、回调调用顺序变化、资源泄漏和把 MockTiKV 特有逻辑意外施加到 EmbedUnistore；本入口本身只增加一次函数转发，当前没有额外性能成本。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，目标 `pkg/store/mockstore/tikv.rs` 已索引并报告 3 个符号。
- RustCodeGraph `node pkg/store/mockstore/tikv.rs::new_mock_tikv_store`：确认调用 `unistore.rs::build_embedded`，调用者为 `mockstore.rs::NewMockStore` 和 `tikv.rs::newMockTikvStore`。
- RustCodeGraph `node pkg/store/mockstore/tikv.rs::newMockTikvStore`：确认别名只调用 `new_mock_tikv_store`。
- RustCodeGraph `node pkg/store/mockstore/unistore.rs::build_embedded`：确认其调用者包括 `new_mock_tikv_store` 与 `new_unistore`，并核对 keyspace 转换、嵌入式创建、回调顺序和 `MockStorage` 字段来源。
- 已读取生产路径：`pkg/store/mockstore/tikv.rs`、`lib.rs`、`Cargo.toml`、`mockstore.rs`、`unistore.rs`、`unistore/Cargo.toml`。
- 已读取 Go 对照：`pkg/store/mockstore/tikv.go`、`mockstore.go`、`unistore.go`。
- 已读取独立测试：`pkg/store/mockstore/tikv_test.rs`、`tikv_test.go`、`mockstore_test.rs`。
- 本任务只新增说明文档；按计划不运行 Cargo。交付前使用任务指定命令验证恰有 11 个固定二级章节，并用限定范围的 diff 人工复核链接、符号名和无越界修改。
