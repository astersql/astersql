# `pkg/config/tiflash.rs`

## 文件定位

本文件属于 `astersql-config` crate；crate 根在 `pkg/config/lib.rs` 中以私有模块 `mod tiflash` 装入，再通过 `pub use tiflash::*` 将全部公开常量和函数重导出。因此调用方使用的是 `astersql_config::GetAutoScalerType`、`astersql_config::AWSASType` 等 crate 级路径，而不是直接依赖模块路径。`pkg/config/Cargo.toml` 没有为本文件声明专属 feature 或依赖；实现只使用 Rust 基本类型和模式匹配。

它是 TiFlash 存算分离场景中 AutoScaler 类型约定的配置边界：维护用户配置字符串、稳定的整数判别值、AWS 默认地址，并提供字符串到判别值的转换与用户配置合法性判断。实际拓扑获取、网络访问和全局 fetcher 生命周期位于 `pkg/util/tiflashcompute/topo_fetcher.rs`，不在本文件中。

## 核心职责

1. 用 `MockASStr`、`AWSASStr`、`GCPASStr`、`TestASStr` 和 `InvalidASStr` 固定配置文本协议。
2. 用 `MockASType` 至 `InvalidASType` 固定与 Go `iota` 顺序一致的 `i32` 判别值 `0..=4`。
3. 用 `GetAutoScalerType` 将输入字符串精确映射到判别值，未知输入统一降为 `InvalidASType`。
4. 用 `IsValidAutoScalerConfig` 将“可解析类型”进一步收窄为用户可配置的 `mock`、`aws`、`gcp`；内部测试类型 `test` 不被视为合法用户配置。
5. 提供 AWS AutoScaler 的默认类型 `DefASStr` 和集群内默认服务地址 `DefAWSAutoScalerAddr`。

本文件不读取配置文件、不保存全局配置、不创建 fetcher，也不判断是否启用了存算分离或 AutoScaler；这些属于上层配置和启动逻辑。

## 主要符号

- 字符串常量：`MockASStr = "mock"`、`AWSASStr = "aws"`、`GCPASStr = "gcp"`、`TestASStr = "test"`、`InvalidASStr = "invalid"`。匹配区分大小写且不裁剪空白。
- 默认值：`DefASStr` 是 `AWSASStr` 的别名；`DefAWSAutoScalerAddr` 是 `tiflash-autoscale-lb.tiflash-autoscale.svc.cluster.local:8081`。
- 整数常量：`MockASType = 0`、`AWSASType = 1`、`GCPASType = 2`、`TestASType = 3`、`InvalidASType = 4`。显式数值保持 `pkg/config/tiflash.go` 的 `iota` ABI/协议顺序。
- `pub fn GetAutoScalerType(typ: &str) -> i32`：对四个已知文本返回对应类型，其余任何文本（包括 `InvalidASStr`、空串、大小写变体）都返回 `InvalidASType`。
- `pub fn IsValidAutoScalerConfig(typ: &str) -> bool`：先调用 `GetAutoScalerType`，再只接受 `MockASType | AWSASType | GCPASType`。
- 模块属性 `#![allow(non_snake_case, non_upper_case_globals, dead_code)]`：允许保留 Go 迁移名称和暂未被所有 Rust 路径使用的公开协议符号。

## 执行流程

配置类型判定的完整局部流程是：调用方把 `&str` 传给 `IsValidAutoScalerConfig`；该函数调用 `GetAutoScalerType`；后者以精确 `match` 将 `mock/aws/gcp/test` 映射为 `0/1/2/3`，兜底映射为 `4`；前者仅对 `0/1/2` 返回 `true`。

运行时选择链由下游完成。`pkg/util/tiflashcompute/topo_fetcher.rs::InitGlobalTopoFetcher` 调用 `config::GetAutoScalerType(&typ)`，然后选择 mock、AWS 或测试 fetcher；`GCPASType` 当前返回 `topo fetch not implemented yet`，非法类型会清空全局 fetcher 并报错。也就是说，“`gcp` 是合法配置文本”只描述本文件的配置协议，不代表 Rust 下游已经实现 GCP 拓扑抓取。

应用启动侧的直接证据是 `cmd/tidb-server/main.rs`：兼容启动配置同时满足 `DisaggregatedTiFlash && UseAutoScaler` 时，才调用 `InitGlobalTopoFetcher`。该启动配置目前来自 `cmd/tidb-server/stubs.rs::config::Config`；`pkg/config/config.rs::Config` 只能直接看到 `use_auto_scaler`，尚未承载 Go 版本的全部 TiFlash AutoScaler 字段。因此不能把 Go `Config.Valid` 的完整校验链视为 Rust 主配置已经接通。

## 数据与状态

本文件的数据全是进程只读的编译期常量。函数只读取借用字符串并返回 `bool` 或 `i32`，不分配、不缓存、不修改输入，也不访问环境变量、文件、网络或全局配置。

字符串常量是外部配置协议；整数常量是模块间分派协议。尤其不能随意调整 `Mock/AWS/GCP/Test/Invalid` 的整数顺序，否则会偏离 Go `iota` 对照，并可能改变下游 `match` 的分派结果。`InvalidASStr` 是供调用者表达非法类型的文本常量，但 `GetAutoScalerType` 不需要为它单列分支：它与任意未知文本一样落入 `InvalidASType`。

## 依赖与调用关系

- 模块装配：`pkg/config/lib.rs` 声明 `mod tiflash` 并 `pub use tiflash::*`。
- crate 边界：`pkg/config/Cargo.toml` 定义 crate 名 `astersql-config`，本文件本身不使用该清单中的第三方依赖。
- 本文件内部调用边：`IsValidAutoScalerConfig -> GetAutoScalerType`；`GetAutoScalerType` 没有函数级下游调用。
- Rust 生产消费边：`pkg/util/tiflashcompute/topo_fetcher.rs::InitGlobalTopoFetcher -> astersql_config::GetAutoScalerType`，并读取类型与字符串常量构造具体 fetcher 或错误消息。`pkg/util/tiflashcompute/Cargo.toml` 通过路径依赖 `../../config` 引入本 crate。
- 启动边：`cmd/tidb-server/main.rs` 在存算分离与 AutoScaler 同时开启时调用 `InitGlobalTopoFetcher`；这条链最终消费本文件的映射协议。
- Rust 校验边：`pkg/config/const_3_aster_unit_test.rs::constants_and_tiflash_mapping_match_go` 覆盖默认值、全部四个已知映射、未知值和用户合法性；`pkg/config/config_test.rs::test_auto_scaler_config` 再覆盖 `mock` 合法、`test` 非法以及配置开关更新。
- 下游行为测试：`pkg/util/tiflashcompute/migration_aster_unit_test.rs::global_fetcher_initialization_matches_go_type_switch` 间接验证 `test/gcp/invalid` 分派及空地址边界。

RustCodeGraph 对目标文件报告两个直接使用文件为 `pkg/config/config_test.rs` 与 `pkg/config/const_3_aster_unit_test.rs`；跨 crate 的生产消费边由索引源码和仓库文本引用共同确认。

## 错误处理与边界

这两个函数均不返回 `Result`，不会主动报错或 panic。未知输入通过 `InvalidASType` 显式归一化；合法性函数则返回 `false`。空串、前后带空格、`AWS` 等大小写变体、`invalid` 和任意新字符串当前都属于未知/非法输入。

`TestASStr` 是可解析但不可作为用户配置的特殊边界：`GetAutoScalerType("test") == TestASType`，而 `IsValidAutoScalerConfig("test") == false`。下游测试可直接用它创建测试 fetcher，但生产配置校验不应放行。

`GCPASStr` 是另一个需要区分层次的边界：本文件认定其用户配置合法，但当前 Rust 下游 fetcher 返回未实现错误。Go `pkg/config/config.go::Config.Valid` 在存算分离且启用 AutoScaler 时调用合法性函数并拒绝空地址；Rust `pkg/config/config.rs` 当前没有同等完整字段与调用证据，故这部分属于尚未完成的迁移接线，而不是本文件的错误处理。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、句柄或需要释放的资源。所有常量具有静态生命周期；两个纯函数只在调用栈上进行比较，天然可并发调用。

全局 fetcher 的并发与生命周期由 `pkg/util/tiflashcompute/topo_fetcher.rs` 管理：它把 `Arc<dyn TopoFetcher>` 写入全局读写锁。该事实解释了本文件输出值的用途，但修改锁、fetcher 替换策略或网络资源生命周期不应落在本文件中。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/config/tiflash.go`。Rust 保留了同名字符串常量、默认值、整数常量和两个函数；`&str -> i32` 对应 Go `string -> int`，显式 `0..4` 对应 Go `iota`，Rust `match` 对应 Go `switch`。四个已知类型、未知类型兜底以及用户合法集合均逐项一致。

Go 侧还在 `pkg/config/config.go` 中使用 `DefASStr`、`DefAWSAutoScalerAddr` 填充默认配置，并在 `Config.Valid` 中按 `DisaggregatedTiFlash && UseAutoScaler` 条件校验类型和地址。当前 Rust 主配置 `pkg/config/config.rs` 仅包含 `use_auto_scaler`；启动路径使用 `cmd/tidb-server/stubs.rs` 的 Go 风格兼容字段。故本文件的常量/函数移植已对齐，但它与 Rust 主配置的完整默认值和校验接线不能据此宣称完成。

Go 的 `pkg/config/config_test.go::TestAutoScalerConfig` 主要验证开关默认值及全局更新；Rust 对应覆盖分散在 `pkg/config/config_test.rs::test_auto_scaler_config` 和更细的 `pkg/config/const_3_aster_unit_test.rs::constants_and_tiflash_mapping_match_go`。Rust 测试位于独立文件，没有内嵌到生产源文件。

## 扩展指南

- 新增 AutoScaler 类型时，应同时增加字符串常量和稳定判别值，更新 `GetAutoScalerType`；若允许用户配置，还要更新 `IsValidAutoScalerConfig`。必须同步 Go `pkg/config/tiflash.go`，避免两种实现协议漂移。
- 同步更新下游 `pkg/util/tiflashcompute/topo_fetcher.rs::InitGlobalTopoFetcher` 的分派和实现；只让合法性校验接受新类型而没有 fetcher 会造成“配置通过、启动失败”。
- 在独立测试 `pkg/config/const_3_aster_unit_test.rs` 中补齐已知、未知、内部测试类型和大小写/空白边界；若影响 fetcher，更新 `pkg/util/tiflashcompute/migration_aster_unit_test.rs`，不要把测试写入 `tiflash.rs`。
- 修改默认类型或地址时，同时核对 Go `pkg/config/config.go` 的默认配置、部署环境中的服务发现地址及启动配置接线。默认地址变化有部署兼容风险；整数重排有分派兼容风险；新增分支本身性能影响近似常数级，但下游网络与资源行为必须另行评估。
- 若补齐 Rust 主配置校验，应在 `pkg/config/config.rs` 的默认值、反序列化和校验路径中接线，并添加独立回归测试；不要把配置加载或全局状态逻辑塞入本文件。

## 验证依据

- RustCodeGraph `status`：索引包含 `pkg/config/tiflash.rs`，目标文件识别为 3 个图符号（文件及两个函数）。
- RustCodeGraph `explore/node`：完整读取 `pkg/config/tiflash.rs`，确认 12 个常量、`IsValidAutoScalerConfig`、`GetAutoScalerType` 及文件的两个直接测试使用者。
- RustCodeGraph `callees IsValidAutoScalerConfig`：确认唯一函数调用边为 `IsValidAutoScalerConfig -> GetAutoScalerType`；`callees GetAutoScalerType` 无下游函数。
- RustCodeGraph 源码节点：核对 `pkg/config/tiflash.go`、`pkg/util/tiflashcompute/topo_fetcher.rs::InitGlobalTopoFetcher`、`cmd/tidb-server/main.rs`、`pkg/config/config.rs`、`pkg/config/config.go::Config.Valid` 以及三个相关 Rust 测试文件。
- 配置与装配文件：读取 `pkg/config/Cargo.toml`、`pkg/config/lib.rs`、`pkg/util/tiflashcompute/Cargo.toml`；确认 crate 名、重导出和跨 crate 路径依赖。
- 文本引用复核：用 `rg` 确认 Rust 生产消费点、Go 默认值/校验点、兼容 stub 字段和测试引用；未发现 `pkg/config/doc.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。最终以固定 11 个二级标题的结构检查和人工事实复核作为交付验证。
