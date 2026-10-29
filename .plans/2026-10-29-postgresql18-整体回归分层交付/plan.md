# PostgreSQL18整体回归分层交付计划

目标：将已承诺能力汇入可追溯、可复现的分层交付证据，保留缺口，阻止历史记录或拒绝结果被误报为兼容通过。

范围：承接后续整体实施任务21，复核能力/case、18.6参考构建、白名单、唯一有效计数、单元与差分、各真实客户端、同进程双方言、必要RealTiKV及冻结资源预算；只补局部验收接线。

范围外：新增SQL子系统、递归依赖移植、通用CI/压测平台；本次不编码、不运行Cargo/数据库/客户端，不批准发布。

假设：当前HEAD为de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3。已有证据实现与独立测试；源码存在不等于当前通过。18.6、上游18—20接口及预算必须在实施时证实。

## 设计决策

采用逐能力复核和窄回归，避免整包广扫或重建验证平台。运行任务串行以避免端口、源码和采样干扰。上游缺口保留为依赖。

## 架构说明

复用pg_core_evidence与独立Rust测试；薄探针只消费既有夹具。报告保存原始记录路径、摘要和版本；各编号ExecPlan独立维护进度。

## 开发策略

对新增行为断言先取得真实失败再作最小接线；已有行为正确只补覆盖并采当前证据。每项源码修复须另明确最小范围和相邻回归，不在回归任务内吞并完整上游。

当前pkg/server/Cargo.toml定义astersql-server，lib.rs挂载独立测试，autotests=false，无需新增feature。pkg/server/doc.go不存在；后续触及其他包先读其doc.go，触及DDL先读docs/agents/ddl/README.md。遵守根与pkg/server/AGENTS.md，保留版权；Rust修复可用后添加AsterSQL版权行，先cargo fmt --all。外部依赖只使用上游已发布Git tag。

本指南只读。任务文件按PLANS.md记录Progress、Surprises & Discoveries、Decision Log、Outcomes & Retrospective。执行顺序和资源依赖见prompt.md及各编号文件。证据持久产物置于docs/postgresql18-delivery/；原始日志须脱敏并附摘要，不能仅留可能被清理的target路径。
