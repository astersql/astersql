# `br/pkg/backup/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-backup` 的 crate 根。`br/pkg/backup/Cargo.toml` 的 `[lib] path = "lib.rs"` 把它指定为库入口，`[package.metadata.porting]` 又把该 crate 对应到 Go 包 `br/pkg/backup`。根工作区 `Cargo.toml` 将此目录列为 workspace member；当前其他 Cargo manifest 未声明 `astersql-br-pkg-backup` 依赖，因此它目前主要是一个可独立构建和测试的迁移单元，而不是已经被 BR 上层 Rust crate 通过包依赖接入的完整生产链。

本文件是门面和装配层，不实现备份算法。它把 `stubs.rs`、`limit.rs`、`store.rs`、`schema.rs`、`client.rs` 注册成公开子模块，在测试构建中挂接六个独立测试模块，再把五个生产子模块的公开项重导出到 crate 根。

## 核心职责

1. **定义 crate 的生产模块边界**：五个 `#[path = "..."] pub mod ...` 声明明确了本 crate 当前纳入的实现集合。
2. **提供 Go 风格的扁平 API**：`pub use client::*`、`limit::*`、`schema::*`、`store::*`、`stubs::*` 使调用方既可使用 `crate::client::Client` 这样的分模块路径，也可从 crate 根取得公开符号。这模拟 Go 同一 `package backup` 中跨文件符号天然共享的使用体验。
3. **隔离测试代码**：`parity_test`、`limit_test`、`store_test`、`schema_test`、`schema_merge_option_test`、`client_test` 仅在 `#[cfg(test)]` 下编译，生产库不会挂入这些测试模块。
4. **容纳迁移期代码形态**：crate 级 `#![allow(...)]` 关闭 dead code、Go 风格命名、未使用项以及全部 Clippy lint。这允许移植代码保留 Go 的公开名字和暂未接线的接口，但也降低了编译器/Clippy 对无效代码和风格问题的提示强度。

## 主要符号

本文件没有常量、类型、trait、函数或 `impl`；其有效符号全部是模块声明和重导出：

- `pub mod stubs`：依赖边界桩。`stubs.rs` 提供 `Context`、`Error`、PD/TiKV/kvproto、对象存储、checkpoint 等轻量替身；文件注释明确说明许多实现是空操作、内存实现或固定返回，不能当作完整生产实现。
- `pub mod limit`：资源量限流器。主要公开项是 `ResourceConcurrentLimiter` 与 `NewResourceMemoryLimiter`，用 `Mutex`/`Condvar` 保持 Go `sync.Cond` 的宽松阈值语义。
- `pub mod store`：单 store 的备份请求拆分、流式发送/接收、超时看门狗和 store 拓扑观察。关键公开契约包括 `BackupSender`、`BackupRetryPolicy`、`ResponseAndStore`、`StartTimeoutRecv` 与 `SplitBackupReqRanges`。
- `pub mod schema`：schema 收集、checksum 校验、merge option 判断和元数据写出。关键入口包括 `Schemas`、`NewBackupSchemas`、`Schemas::BackupSchemas` 与 `DefaultSchemaConcurrency`。
- `pub mod client`：备份编排层。它通过 `ClientMgr` 注入连接和存储边界，由 `Client::BackupRanges`/`RunLoop` 协调进度树、store 请求、响应、锁解析、checkpoint 与元数据。
- 六个私有测试模块：只参与本 crate 的测试构建，不构成对外 API。
- 五条 glob re-export：把上述生产模块的全部公开项提升到 crate 根。新增同名公开项时可能出现重导出冲突或使调用路径来源变得含混，必须在扩展时检查。

## 执行流程

本文件本身没有运行时控制流；其作用发生在编译与名称解析阶段：

1. Cargo 依据 `br/pkg/backup/Cargo.toml` 载入 `lib.rs`。
2. 编译器应用 crate 级 lint allowance，然后按显式 `#[path]` 解析五个生产模块。
3. 普通构建跳过所有 `#[cfg(test)]` 模块；测试构建额外解析六个同目录独立测试文件。
4. `pub use ...::*` 将五个模块的公开项加入 crate 根命名空间。
5. 实际运行时由调用方选择公开入口。典型内部主链由 `client.rs` 体现：`Client::BackupRanges` 建立进度树并进入 `RunLoop`，`RunLoop` 通过 `store.rs` 的 `BackupSender::SendAsync`/`startBackup` 向各 store 发送请求并汇聚响应；schema 路径通过 `NewBackupSchemas`/`Schemas::BackupSchemas` 处理元数据；`limit.rs` 的限流器约束在途资源量。

RustCodeGraph 对关键链路的核对结果包括：`NewTableBackupClient -> NewBackupClient`，`BackupRanges -> GetBackupClient/ResetBackupClient/BuildProgressRangeTree/RunLoop`，`RunLoop -> SendAsync/CollectStoreBackupsAsync/OnBackupResponse`；`BuildBackupRangeAndInitSchema -> BuildBackupSchemas`。这些边属于被本门面导出的子模块，而不是 `lib.rs` 自身的函数调用。

## 数据与状态

`lib.rs` 不持有运行时数据、全局状态或配置值。它唯一改变的是 crate 的编译结构和公开名称集合。

实际状态归属如下：

- `client.rs` 的 `MainBackupLoop` 持有请求模板、全局进度树、重试通道、限流器与取客户端回调；`Client` 持有存储、checkpoint、加密、GC 等备份配置和依赖。
- `store.rs` 的 `timeoutRecv` 管理超时线程、取消句柄和刷新通道，`ResponseAndStore` 把响应与 store ID 绑定。
- `schema.rs` 的 `Schemas` 持有 DB/表迭代器、规模估计与可选 checkpoint checksum。
- `limit.rs` 的 `ResourceConcurrentLimiter` 以 `Mutex<isize>` 保存当前资源占用，以 `Condvar` 唤醒等待者。
- `stubs.rs` 包含大量内存态替身和测试开关；它们通过 glob re-export 同样暴露在 crate 根，因此调用者不能仅凭根路径判断某符号是否为生产级实现。

## 依赖与调用关系

直接编译依赖只有两类：

- **同目录模块**：`client` 依赖 `limit`、`schema`、`store` 和 `stubs`；`store` 依赖 `limit` 与 `stubs`；`schema` 依赖 `stubs`。模块声明顺序本身不是 Rust 的运行时初始化顺序，但当前顺序先放置基础桩和限流，再放 store/schema/client，清楚表达依赖层次。
- **Cargo 外部依赖**：`br/pkg/backup/Cargo.toml` 仅声明 `serde`（启用 `derive`）和 `serde_json`。`lib.rs` 没有直接 `use` 它们，具体使用发生在子模块或桩边界。

上游方面，RustCodeGraph 将目标文件标为被 `tools/tazel/parity_test.rs` 使用；该边用于仓库结构/对齐检查，不代表备份运行时调用。仓库搜索没有发现其他 manifest 对包名 `astersql-br-pkg-backup` 的依赖声明。crate 内测试通过 `crate::client`、`crate::store` 等模块路径验证公开契约；上层 BR Rust 代码中出现的 `crate::backup::*` 多属于各自 crate 的本地 `backup` 模块，不能据此认定其依赖本 Cargo 包。

## 错误处理与边界

本文件不产生或传播运行时错误：模块解析、重复重导出或缺失路径会直接成为编译错误。

需要特别注意的边界有：

- `#![allow(clippy::all)]` 和其他 allowance 只屏蔽 lint，不改变类型检查、借用检查或运行时错误语义；但它们可能掩盖新代码中的未使用项和风格退化。
- glob re-export 扩大了公开面。两个模块新增同名公开符号时，可能导致 crate 根重导出冲突；删除或改名公开项也会直接改变根 API。
- `stubs` 是有意缩减的依赖边界。其 `Error` 主要保存字符串，`Context` 用共享内存近似取消，多个外部系统模块只提供调用方所需子集。文档与扩展不能把这种对齐解释为真实 PD/TiKV、gRPC、对象存储或完整错误链已经接入。
- 各业务错误由子模块负责：例如 `client.rs` 校验 checkpoint 哈希、存储锁和上下文取消，`store.rs` 处理 RPC/接收超时与重试通知，`schema.rs` 传播迭代、checksum 和元数据写入错误。

## 并发与资源生命周期

`lib.rs` 不启动线程、任务或通道，也没有初始化/销毁顺序。`#[cfg(test)]` 只改变哪些模块参与编译，不创建运行时隔离。

被它导出的并发与资源生命周期主要位于：

- `ResourceConcurrentLimiter::Acquire/Release`：以条件变量阻塞与广播唤醒；其契约是“进入临界区前检查当前值”，因此单次申请可令占用暂时超过 threshold。
- `store.rs`：`StartTimeoutRecv` 启动超时和父取消观察线程；`timeoutRecv::Stop` 幂等关闭刷新通道、等待线程并取消派生 context；备份流应在收尾路径调用 `CloseSend`。
- `client.rs`：`MainBackupSender::SendAsync` 启动后台发送线程，以通道汇聚响应和重试策略；`RunLoop` 应在取消或完成时等待在途工作收尾，`ClientMgr::Close` 是连接资源释放边界。
- `schema.rs`：`Schemas::BackupSchemas` 通过有界工作池并行计算，但按迭代顺序串行发送元数据，以维持确定性。

跨模块回归 `parity_test.rs::go_rust_public_contract_matches` 使用原子变量、互斥量、通道和可控流覆盖正常、边界、失败与资源清理路径；更细粒度测试分别在各独立 `*_test.rs` 文件中。

## 与 Go 版本的对应关系

Go 目录没有对应的 `lib.go`：`client.go`、`limit.go`、`store.go`、`schema.go` 都声明 `package backup`，Go 工具链天然把这些文件合并为同一个包。Rust 必须显式声明模块并决定可见性，因此 `lib.rs` 是 Rust 独有的适配门面，五条 `pub use` 用来近似 Go 包的平铺符号空间。

映射关系为：

- `client.rs` 对应 `client.go`，负责备份客户端与主循环编排。
- `limit.rs` 对应 `limit.go`，保持 `sync.Cond` 限流语义。
- `store.rs` 对应 `store.go`，保持请求拆分、单 store 流和超时/重试语义。
- `schema.rs` 对应 `schema.go`，保持 schema/checksum/meta 写出语义。
- `stubs.rs` 没有等价的单一 Go 文件；它聚合 Go 版本从大量真实包导入的外部边界，是当前 Rust 迁移为轻量编译与测试建立的适配层。

Go 测试位于 `client_test.go`、`limit_test.go`、`store_test.go`、`schema_test.go`、`schema_merge_option_test.go` 和 `main_test.go`。Rust 对应测试保持为独立文件，并由本 crate 根在 `#[cfg(test)]` 下挂载，符合“源文件与测试逻辑分离”的仓库约定。`parity_test.rs` 额外作为跨 `client/limit/store/schema` 公共契约的集中回归。

## 扩展指南

- 新增生产模块时，在本文件增加明确的 `#[path] pub mod`；只有确实需要模拟 Go 平铺 API 时才增加 glob 或精确重导出。优先考虑 `pub use module::{Type, function}`，以减少命名碰撞和无意扩大 API。
- 新增测试必须放在独立的 `*_test.rs` 文件，并在此处用 `#[cfg(test)]` 挂载；不要把测试逻辑内嵌进 `lib.rs`。同时同步核对对应 Go 测试或记录 Rust 独有场景的理由。
- 修改 `client`/`store`/`schema`/`limit` 的公开项时，同时检查 crate 根重导出、`parity_test.rs::go_rust_public_contract_matches` 及各自独立测试。并发或清理行为还应覆盖取消、超时、通道关闭、线程 join 和错误传播。
- 若要把该 crate 接入真实 BR Rust 主链，必须在上游 Cargo manifest 添加带明确依赖关系的包引用，并系统替换/收窄 `stubs`；不能把当前内存桩直接视为生产依赖实现。
- 收窄 crate 级 allowance 时应分模块逐项处理，避免一次性改名破坏与 Go 对齐的公开 API。若保留 Go 风格名字，建议只允许必要 lint，而不是长期维持 `clippy::all`。
- 扩展后的最低文档风险检查包括：模块路径是否真实存在、根 API 是否发生冲突、Cargo 依赖是否仍可复现、Go/Rust 语义差异是否被测试覆盖，以及桩与真实实现的边界是否仍清晰。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；目标区域 `files --filter br/pkg/backup` 返回 36 个 Go/Rust 文件。
- RustCodeGraph `node --file br/pkg/backup/lib.rs --offset 1 --limit 120`：确认文件共 68 行、五个公开生产模块、六个条件测试模块、五条 glob re-export，并显示文件级使用边 `tools/tazel/parity_test.rs`。
- RustCodeGraph `explore`：确认 `BackupRanges -> RunLoop`、`RunLoop -> SendAsync/CollectStoreBackupsAsync/OnBackupResponse`、`BuildBackupRangeAndInitSchema -> BuildBackupSchemas` 等关键子模块调用边。
- RustCodeGraph 读取并核对：`client.rs`、`limit.rs`、`store.rs`、`schema.rs`、`stubs.rs`、`parity_test.rs`、`client_test.rs`、`limit_test.rs`。
- Cargo 核对：`br/pkg/backup/Cargo.toml` 与根 `Cargo.toml`；确认 crate 名、`lib.rs` 入口、Go 包映射、workspace 成员关系以及 `serde`/`serde_json` 依赖。
- Go 对照核对：`client.go`、`limit.go`、`store.go`、`schema.go` 的包声明，以及 `client_test.go`、`limit_test.go`、`store_test.go`、`schema_test.go`、`schema_merge_option_test.go`、`main_test.go` 的测试分布。
- Rust 测试核对：`parity_test.rs` 的跨模块公共契约测试，以及 `client_test.rs`、`limit_test.rs`、`store_test.rs`、`schema_test.rs`、`schema_merge_option_test.rs` 中的独立 `#[test]`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有十一个固定二级章节。
