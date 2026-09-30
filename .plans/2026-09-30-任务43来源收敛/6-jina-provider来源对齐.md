# 任务 6: Jina provider Go 来源与 HTTP 行为

批次：【批次 6】依赖批次：1,5

状态：未开始

目的：核查 `pkg/inference/jina.rs` 是否逐项实现已有 Go Jina provider，而非凭文件名判断。

来源任务：43 的 Inference 依赖扩展；Go 对应文件 `pkg/inference/embedding/jina/jina.go`

预计会话范围：单个 provider 的配置、请求、响应和错误路径，不修改其它 provider。

## 文件

- 修改：`pkg/inference/jina.rs`、必要时 `pkg/inference/embed_fn.rs`。
- 测试：`pkg/inference/jina_test.rs`。
- Go 对照：`pkg/inference/embedding/jina/jina.go`、`jina_test.go` 和共享 `pkg/inference/embedding/base` HTTP helper。

## 上下文

- Go 包在 `embedding/jina`，Rust crate 把 provider 放在根目录；Go `CreateEmbeddings` 用动态 API key/base URL、base64 向量、受控响应大小及结构化错误。
- 当前 Rust 有 Jina HTTP 实现和本地测试，但需核对动态配置、401/非 2xx、取消及响应解码与 Go helper 的差异。原始任务 43 的 `inference.go` 本身不要求新增 Jina 语义。

## 测试计划

- 行为：相同配置和模拟 HTTP 响应下，Rust 与 Go provider 返回同形状向量或同类错误；API key/base URL 在每次调用时生效。
- 失败验证测试：扩展 `jina_test.rs` 中 `go_merge_43_jina_provider_posts_base64_and_rejects_multivector`，选择审查发现的一个具体不一致分支。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-inference cargo test --manifest-path pkg/inference/Cargo.toml --lib go_merge_43_jina`
- 预期失败原因：请求字段、配置刷新或状态码处理与 Go 不同。
- 模拟策略：仅模拟外部 HTTP 服务，检查请求路径、头、JSON 和响应；不访问真实 Jina 网络。

## 步骤

1. 阅读 Go Jina 和共享 base helper 的实际语义，逐分支对照 Rust。
2. 有偏差才添加失败回归并修正；没有偏差则以映射和现有测试完成。
3. 保留必要 Rust HTTP 适配，移除无法追溯的 provider 专属策略。

## 完成

报告 Go→Rust 函数映射、测试所覆盖的 HTTP 语义、保留/缩减的策略及精确命令。Ready 通过后删除本文件。
