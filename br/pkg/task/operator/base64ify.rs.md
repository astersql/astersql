# `br/pkg/task/operator/base64ify.rs`

## 文件定位

本文对应的生产源码是 [`base64ify.rs`](./base64ify.rs)。它属于 Cargo crate `astersql-br-pkg-task-operator`；该 crate 由 `br/pkg/task/operator/Cargo.toml` 定义，以同目录 `lib.rs` 为入口。`lib.rs` 将本文件声明为公开模块 `base64ify`，并通过 `pub use base64ify::*` 把公开入口平铺到 crate 根。

在应用调用链上，`br/cmd/br/operator.rs::newBase64ifyCommand` 注册 `base64ify [-r] -s <storage>` 子命令，解析 `Base64ifyConfig` 后把命令级取消标志转换成 `stubs::Context`，再调用本文件的 `Base64ify`。该命令生成可传给 `tikv-ctl compact-log-backup` 等工具的外部存储 protobuf Base64 字符串；它不读写备份对象。

## 核心职责

本文件承担三项相连但边界清楚的职责：

1. `Base64ify`/`runEncode` 实现命令业务流程：解析外部存储 URI、尝试构造并立即关闭存储以校验配置、在需要携带凭证时警告用户，最后输出编码结果。
2. `to_objstore_backend_options` 把 operator 层以字符串 map 保存的 S3 参数转换成 `astersql_objstore::parse::BackendOptions`。
3. `marshal_backend` 及其辅助函数手工生成与 Go `backuppb.StorageBackend.Marshal()` 一致的 protobuf wire bytes，再由 `base64` crate 的标准字母表编码。

这里的“试连”能力受 `pkg/objstore/storage.rs::New` 的实际接线约束：Local、HDFS、Noop 可由该函数直接构造；S3、GCS、Azure 等云后端需要 `StorageOptions.external_factory`。本文件创建选项时没有注入工厂，所以当前 Rust 完整命令路径会在云后端构造阶段返回 `storage ... is not supported yet`。`encode_backend_for_test` 只验证解析与序列化，不经过这一构造检查。

## 主要符号

- `pub fn Base64ify(ctx: Context, cfg: Base64ifyConfig) -> Result<()>`：公开 CLI 业务入口，只转发给 `runEncode`，保留 Go 同名 API 的形状。
- `pub fn runEncode(ctx: Context, cfg: Base64ifyConfig) -> Result<()>`：主流程实现。它把 operator 取消标志传给 objstore 上下文，解析 backend，调用 `New`，关闭存储，按需打印凭证警告，再输出 Base64。
- `fn objstore_error(...) -> Error`：把 objstore 的可显示错误压平为 operator 自有的字符串错误；转换后不保留原始错误类型。
- `fn parse_backend(...) -> Result<StorageBackend>`：调用 `astersql_objstore::parse::ParseBackend`，并额外拒绝 `StorageBackend::MemStore`，避免走 Rust 专用内存存储捷径。
- `fn to_objstore_backend_options(...)`：复制 S3 endpoint、region、加密、ACL、凭证、角色、provider/profile 等参数；`force-path-style` 解析失败或缺省时取 `true`。
- `push_varint`、`push_bytes`、`push_string`、`push_bool`：最小 protobuf wire 编码器。字符串空值和布尔 `false` 按 protobuf 默认值规则省略；嵌套消息使用 wire type 2。
- `marshal_s3`、`marshal_gcs`、`marshal_azure`：按对应 `backuppb` 消息字段号编码后端配置；Azure customer key 作为字段 10 的嵌套消息写入。
- `marshal_backend`：把 `StorageBackend` 枚举映射到外层 oneof 字段：Noop=1、Local=2、S3=3、GCS=4、HDFS=6、Azure=7。MemStore 已在解析阶段拒绝，因此该分支用 `unreachable!` 固化不变量。
- `pub(crate) fn encode_backend_for_test(uri: &str) -> Result<String>`：crate 内测试入口，只执行解析、wire 编码和 Base64，不创建实际存储。

## 执行流程

生产命令的顺序如下：

1. `br/cmd/br/operator.rs::newBase64ifyCommand` 用 `DefineFlagsForBase64ifyConfig` 注册并解析 `--storage/-s`、`--load-creds` 以及后端参数，构造 `Base64ifyConfig`。
2. `Base64ify` 调用 `runEncode`；后者通过 `StorageContext::from_cancellation_flag` 与命令层共享取消状态。
3. `to_objstore_backend_options` 转换 S3 参数，`parse_backend` 再按 URI scheme 生成 `StorageBackend`。空 URI、未知 scheme、缺少 bucket 等解析错误直接返回；`memstore://` 被本文件显式拒绝。
4. `New` 接收 `send_credentials = cfg.LoadCerd`、`check_s3_object_lock_options = true` 的选项来构造存储。成功后立即调用 `Close`，不执行对象读写。
5. 若 `LoadCerd` 为真，先向 stderr 输出高亮红色警告，提醒编码串含敏感凭证。
6. `marshal_backend` 生成 protobuf bytes，`STANDARD.encode` 使用带 `=` padding 的标准 Base64 编码，`println!` 将结果和换行写到 stdout。

测试辅助路径 `encode_backend_for_test` 从第 3 步直接跳到第 6 步，因此它能覆盖云后端 wire 格式，却不能证明云存储构造或凭证装载可用。

## 数据与状态

输入状态集中在 `Base64ifyConfig`（定义于 `config.rs`）：`StorageURI` 是待解析 URI，`BackendOptions` 保存后端 flags，`LoadCerd` 控制 `send_credentials` 和安全警告。历史拼写 `LoadCerd` 与 Go 字段保持一致，不应只在本文件单方面重命名。

解析后的 `StorageBackend` 是值类型枚举。本文件不缓存它、不持有全局状态；序列化过程只分配短生命周期的 `Vec<u8>`。protobuf 编码保持默认值省略规则，但外层 oneof 即使嵌套内容为空也会写入长度为零的字段，因此 `noop://` 得到 bytes `0a 00`，Base64 为 `CgA=`。

`Context` 内部是共享 `Arc<AtomicBool>`。本文件只把它桥接到 objstore `Context`，不自行轮询取消标志；实际是否观察取消取决于选中的存储构造路径。独立测试证实已取消 context 对 Noop 构造不产生错误。

## 依赖与调用关系

上游调用者：

- `br/cmd/br/operator.rs::newBase64ifyCommand` 是生产 CLI 入口；`br/pkg/task/operator/lib.rs` 负责模块声明与再导出。
- `br/pkg/task/operator/base64ify_test.rs` 直接调用 `Base64ify` 和 `encode_backend_for_test`。
- `br/pkg/task/operator/parity_test.rs::contract_normal_config_and_helpers` 通过 Noop backend 验证 operator 公共契约。

直接下游：

- `crate::config::Base64ifyConfig` 和 `crate::stubs::{Context, Error, Result, BackendOptions, color_hi_red}` 提供 operator 层配置、错误、取消状态与终端着色。
- `astersql_objstore::parse::{ParseBackend, StorageBackend, ...}` 负责 URI 解析和后端数据结构；`astersql_objstore::storage::New` 负责存储构造与可达性/配置检查边界。
- `base64::engine::general_purpose::STANDARD` 负责最终文本编码。

`Cargo.toml` 明确声明本文件所需的 `astersql-objstore` 路径依赖和 `base64 = "0.22"`；本文件没有条件编译项，也没有异步 runtime、网络 client 或 protobuf 生成 crate 的直接依赖。

## 错误处理与边界

- `ParseBackend` 的错误（空 URI、非法/未知 scheme、缺 bucket、非法参数等）经 `objstore_error` 转成 operator `Error` 并原样传播可显示文本。
- `memstore://` 虽是 objstore Rust 枚举成员，但本文件明确返回 `storage memstore not support yet`，以对齐 Go `ParseBackend` 路径而不是 Rust-only `NewFromURL` 捷径。
- `New` 失败时不会编码或打印 stdout；由于 `store` 尚未产生，也不存在关闭动作。当前未注入 `external_factory` 的云后端属于此分支。
- 成功构造后会立即 `Close`。`Close` 返回 `()`，因此关闭失败没有可传播通道；这与所依赖 trait 的签名一致。
- `LoadCerd` 警告只在 `New` 成功后打印。警告走 stderr，编码串走 stdout，便于脚本只捕获结果；但用户仍须把带凭证的输出视为秘密。
- 手工 wire 编码没有返回错误。其安全性依赖字段号和嵌套结构与上游 `backuppb` schema 保持同步；字段变化不会由 Rust 类型系统自动发现。
- `push_varint` 使用 `u64`，长度由 `usize` 转换；在现实配置字符串范围内可用，但本文件没有为极端超大分配设置显式上限。

## 并发与资源生命周期

所有函数同步执行，没有线程、异步任务、锁或通道。`runEncode` 在栈上持有配置、backend 和编码 buffer；函数返回后全部释放。

唯一共享状态是由命令层传入的原子取消标志。它被克隆进 objstore context，生命周期至少覆盖 `New` 调用。Noop/Local/HDFS 的具体构造并不保证检查取消；`base64ify_cancelled_context_matches_go_noop_behavior` 明确要求已取消的 Noop 调用仍成功。

存储资源的生命周期是“构造成功后立即关闭”。当前代码不是 RAII guard：若未来在 `New` 与 `Close` 之间加入可失败操作，必须确保所有提前返回路径仍关闭资源；现状两者之间没有 `?` 或其他失败点。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/task/operator/base64ify.go`：Rust 的 `Base64ify → runEncode → ParseBackend → New → Close → 可选警告 → Marshal → StdEncoding` 保持相同阶段和顺序，警告文本也保留了 Go 中的历史拼写 `Credientials`。

主要实现差异有三点：

1. Go 直接调用生成的 protobuf `s.Marshal()`；Rust 当前没有在本 crate 中依赖对应生成类型，而是由 `marshal_backend` 手工写 wire format。`base64ify_uses_backuppb_protobuf_wire_format` 用 Noop、Local、HDFS、S3、GCS、Azure 的固定结果约束兼容性。
2. Go 的 objstore 生产接线能按后端构造真实云存储；当前 Rust `New` 需要调用方提供 `external_factory`，本文件没有提供，所以云后端完整命令路径尚未达到 Go 的可用范围。
3. Rust 显式拒绝 `MemStore`，注释说明这是为了避免 Rust `NewFromURL` 才有的内存后端捷径；Go 对照流程本来也不提供该命令语义。

已有测试没有直接捕获 stdout/stderr，也没有验证 `LoadCerd=true` 的真实凭证回填；这些能力不能仅由 wire-format 测试推断为已完整对齐。

## 扩展指南

- 新增或调整后端字段时，应先核对上游 `backuppb.StorageBackend` 及嵌套消息的字段号，再修改对应 `marshal_*` 和 `marshal_backend`；同步扩展 `base64ify_test.rs` 的固定 Base64 向量，避免只验证“能编码”而未验证 wire 兼容。
- 若要让 S3/GCS/Azure 生产路径可用，应在 objstore 集成边界提供真实 `external_factory`，并保持 `send_credentials`、Object Lock 检查和错误传播语义；不要通过跳过 `New` 来伪造成功，因为 Go 流程把构造作为配置/可达性验证。
- 若增加 GCS/Azure 的 flag 到 operator `BackendOptions`，还必须扩展 `to_objstore_backend_options`。当前转换函数只显式复制 S3 map，不能假设其他后端命令行选项已接入。
- 若改变输出或警告，需保持 stdout 只有机器可消费的 Base64、stderr 承载安全提示，并补充独立测试；涉及敏感信息时避免在错误或日志中回显 secret。
- 测试逻辑继续放在同目录独立的 `base64ify_test.rs`，不要内嵌到生产文件。至少覆盖解析失败、各 oneof/字段号、默认值省略、凭证开关、资源关闭和云工厂接线。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/task/operator/base64ify.rs` 定位到目标文件；`explore "br/pkg/task/operator/base64ify.rs Base64ify Base64ifyReader base64ify reader operator"` 给出 `Base64ify → runEncode`、`runEncode → parse_backend/to_objstore_backend_options/marshal_backend` 以及测试调用边；`node --file ... --offset 1 --limit 260` 核对了目标文件全部 216 行和 15 个符号。
- 生产源码：`br/pkg/task/operator/base64ify.rs`、`br/pkg/task/operator/config.rs`、`br/pkg/task/operator/stubs.rs`、`br/pkg/task/operator/lib.rs`、`br/cmd/br/operator.rs`。
- crate/下游边界：`br/pkg/task/operator/Cargo.toml`、`pkg/objstore/parse.rs::{StorageBackend, ParseBackend}`、`pkg/objstore/storage.rs::{Context, Options, New}`。
- Go 对照：`br/pkg/task/operator/base64ify.go`、`br/cmd/br/operator.go::newBase64ifyCommand`。
- 独立 Rust 测试：`br/pkg/task/operator/base64ify_test.rs` 覆盖取消的 Noop 路径、未知 scheme 和六类 backend 的 protobuf Base64；`br/pkg/task/operator/parity_test.rs::contract_normal_config_and_helpers` 覆盖 flag 解析后的 Noop 公共入口。
- 本任务是纯文档分析，按总计划不运行 Cargo；交付前使用任务指定命令验证本文恰有 11 个固定二级章节。
