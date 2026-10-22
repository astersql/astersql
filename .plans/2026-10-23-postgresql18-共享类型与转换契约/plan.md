# PostgreSQL 18 共享类型与转换契约计划

目标：将蓝图任务 3 分成独立文档块，明确规范类型、转换与 wire 元数据契约，供用户随后逐块拆分实现。

范围：只补充 `docs/postgresql-18-core-compatibility-roadmap.md` 第 3 章。覆盖映射、布尔、整数、numeric、浮点、文本、bytea、date、time、timestamp、timestamptz、interval、UUID、JSON、JSONB、基础数组、转换和元数据一致性。

范围外：不实现生产代码、测试平台或数据库环境，不修改其他章节，不生成逐 codec 实现子任务，不扩建存储、优化器、执行器或扩展生态。

假设：来源为 `.plans/2026-10-22-postgresql18-核心兼容蓝图/3-共享类型与转换契约.md`；当前第 1、2 章已有正文，第 3 章为空骨架。18.6 是来源指定目标，本轮未核实发布状态或实际兼容能力。

## 设计决策

- 按用户要求每项保留一个文档块，类型内部表示、转换、参数与结果 codec 不继续拆实现。共享正文增量编辑优于独立草稿合并，避免术语漂移。
- 比较仅扩展 wire 映射与补充规范类型信息，按保真证据选择；cast 比较直接复用与方言先决策再复用，具体结论在对应块记录。

## 架构说明

- `Datum`/`FieldType` 与 `NativeType`/`PreparedMetadata` 承接规范值和元数据；PG 参数与结果编解码在 server 边界。目录专用类型不证明通用支持。
- 共写第 3 章，全部串行；语法归工作流 2、运算归 4、会话状态归 5、目录归 6、协议状态机归 7。

## 开发策略

- 本轮只创建计划；后续编号任务只写路线图文档。当前代码、目标行为、已运行证据与未验证能力分别记录。
- 先核对第 1、2 章和来源契约，再取证代码与独立测试；PG 外部事实须查官方 PostgreSQL 18 文档，精确二进制格式必要时核对对应官方源码并记录版本。
- 已检查 Cargo workspace 及 server/types/expression manifests；文档任务不运行 Cargo，失败测试、槽位、代码构建及 lint 不适用。
- 本计划不是复杂功能实现 ExecPlan。`plan.md` 生成后只读；进度、发现、决策、阻塞及验证记录写编号任务文件。后续实现另建遵守 `PLANS.md` 的计划。
- Ready 采用文档范围检查；当前 `.agents/skills/tidb-verify-profile/SKILL.md` 缺失，使用明确的结构、引用、需求覆盖与差异检查，报告该限制。
