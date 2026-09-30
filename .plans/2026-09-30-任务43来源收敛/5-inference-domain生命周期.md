# 任务 5: Domain inference 生命周期

批次：【批次 5】依赖批次：1,2

状态：未开始

目的：只验证 Go `inference.go` 新增的 provider 初始化、关闭和 getter 生命周期。

来源任务：43，`pkg/domain/inference.go`、`domain.go`

预计会话范围：Domain 内的一次注册/关闭行为；各 provider HTTP 细节由下一任务核查。

## 文件

- 修改：`pkg/domain/domain.rs`；需要时 `pkg/inference/embed_fn.rs`。
- 测试：`pkg/domain/canonical_domain_test.rs`。
- Go 对照：`pkg/domain/inference.go` 及 `domain.go` 的 `Start`/`Close` 调用点。

## 上下文

- Go `initInferenceProviders` 存入新 `EmbedFn`；`closeInferenceProviders` 原子交换为 nil 后调用 Close；`GetEmbedFn` 返回当前指针。
- 现有 Rust `init_inference_providers`/`close_inference_providers` 已接入 Domain，核对关闭后的可观察状态与重复关闭。

## 测试计划

- 行为：启动后 getter 返回 provider，关闭后不再返回；重复关闭不重复释放。
- 失败验证测试：若现有 `go_merge_43_domain_owns_inference_provider_lifecycle` 缺少分支，在 `canonical_domain_test.rs` 添加失败断言。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-domain cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43_domain_owns_inference_provider_lifecycle`
- 预期失败原因：缺失的原子替换或关闭顺序让 getter/资源状态错误。
- 模拟策略：用现有本地 provider/mock 观测 Close；不调用外网。

## 步骤

1. 读取 Go 生命周期代码与 Rust Domain 相邻测试。
2. 缺口存在时先写失败回归，随后做最小修复。
3. 聚焦测试通过后检查关闭顺序和并发读 getter 的风险。

## 完成

提供启动、getter、关闭、重复关闭的对照证据；无代码缺口可用现有测试与源码审查完成。Ready 通过后删除本文件。
