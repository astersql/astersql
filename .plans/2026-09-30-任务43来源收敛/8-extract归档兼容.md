# 任务 8: Extract 普通表归档格式

批次：【批次 8】依赖批次：1,7

状态：未开始

目的：让 Server 生产 Extract ZIP 的普通表元数据和统计内容符合 Go replayer 格式。

来源任务：43 当前任务文件的 Extract 生产验证；超出原始 `extract.go` 单行差异

预计会话范围：只修正普通表归档内容，不处理视图和分区；不更换 HTTP Handler。

## 文件

- 修改：`pkg/server/extract_runtime.rs`、必要时 `pkg/domain/plan_replayer_dump.rs`。
- 测试：`pkg/server/runtime_test.rs`；必要时 `pkg/domain/plan_replayer_dump_test.rs`。
- Go 对照：`pkg/domain/extract.go` 的 `dumpExtractPlanPackage`、`pkg/domain/plan_replayer_dump.go` 的 config/meta/schema/stats/variables/bindings helper。

## 上下文

- 当前生产路径已有非空语句摘要 ZIP 测试，但 `stats/*.json` 仍是 SHOW STATS 行数组，不能据“ZIP 可解码”断言 Go replayer 能导入。
- Go 视图、分区统计、配置、SQL 记录各有固定文件名/内容；必须逐条核对而非只检查条目存在。
- `tests/mysqlcompat/compatibility-cases.json` 在当前工作树缺失，Server 测试编译时会报 include_str 错误；不得把临时空清单作为兼容验证通过的证据。

## 测试计划

- 行为：Go replayer 对非空普通表 ZIP 的 config/meta/SQL/stats 内容解析成功；`SkipStats` 分支不写统计。
- 失败验证测试：在 `runtime_test.rs` 扩展 `go_merge_43_canonical_server_domain_serves_extract_archive`，实际解析具体 config/meta/SQL/stats JSON；先展示格式不符。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-server cargo test --manifest-path pkg/server/Cargo.toml --lib go_merge_43_canonical_server_domain_serves_extract_archive`
- 预期失败原因：统计 JSON 结构不符合 Go helper 输出。
- 模拟策略：本地 SQL/InfoSchema 和 ExtStorage 真实执行；只在外部存储不可用时用仓库已有对象存储测试后端。

## 步骤

1. 先核对 Go helper 和消费端预期，列出每个 ZIP 条目的精确结构。
2. 为一个不兼容条目写失败验证，修正后按普通表归档条目逐段复验。
3. 如果缺失 mysqlcompat 清单阻止编译，先定位其来源并在本任务文件记录具体阻塞；不要提交假清单。
4. 检查产物可由 Go/Rust replayer 读取，清理测试对象。

## 完成

提供普通表与跳过统计两类产物的实际文件/JSON 证据及失败→通过命令；无法交叉读取时明确未验证，不得宣称格式兼容。Ready 通过后删除本文件。
