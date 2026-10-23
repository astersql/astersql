# PostgreSQL 18 核心兼容路线图

本路线图用于定义 AsterSQL 的 PostgreSQL 核心兼容目标及后续验收契约，组织为下列八个工作流。路线图是文档交付，不表示相应兼容功能、差分平台或运行环境已经实现。

总范围涵盖兼容契约、方言与解析、类型与转换、函数与运算符、SQL 会话与事务、系统目录、线协议以及客户端和双方言共存验收。各项均为待定义和验证的目标，不构成当前支持声明；本轮交付八章骨架和第 1 章的兼容契约与差分验证要求。

## 1. 兼容契约与差分验证

### 模块

本章建立契约及取证要求，模块定位从 [历史 PG 边界说明](postgresql-protocol-first-phase.md)、[连接模块说明](../pkg/server/pg_conn.rs.md) 和 [真实客户端测试入口](../pkg/server/pg_client_integration_test.rs) 开始。PG 专属连接与协议边界入口为 `pkg/server/pg_conn.rs`；共享会话和执行接入的调研入口为 `pkg/server/conn.rs`、`pkg/server/driver.rs`，沿现有内核继续定位，不另建存储、优化器或执行器。模块说明仅供导航，后续取证须以能力 ID 关联实际源码符号、独立测试和提交 SHA；这些入口及其中的历史测试记录不作为本轮当前运行证据。

### 目标行为

#### 范围与非目标

核心兼容目标是为应用使用 PostgreSQL SQL 与客户端接口时可观察的行为建立明确契约，并在后续任务中定义差分验证要求。契约应区分共享语义与方言专属语义，覆盖查询结果、类型及元数据、NULL、错误、会话与事务状态和客户端交互；本小节划定范围，能力矩阵、证据规则和用例格式分别由下列同名小节定义。

PostgreSQL **18.6** 是来源计划指定的目标基线；本轮未核实该补丁版本的发布状态，也未运行真实 PostgreSQL 对照。握手中报告的版本、协议协商成功或客户端可连接，都不能据此宣称当前具备 PostgreSQL 18.6 的 SQL、事务或完整功能兼容性。

兼容工作共享现有规范执行内核，通过明确的方言边界承接 PostgreSQL 目标行为，并保持 MySQL 方言既有契约。这里的共享内核指复用现有执行能力，不另建存储、优化器或执行器；共享实现本身也不证明两种方言的可观察语义相同。

本路线图明确排除扩展加载生态：CREATE EXTENSION、PostGIS、FDW、自定义过程语言、C 扩展、自定义类型/运算符类及第三方扩展加载生态均为非目标，不列为核心兼容交付或验收承诺。

本轮文档任务不实现生产代码、差分执行器、数据库环境或 CI；不扩展兼容能力，不撰写第 2–8 章正文。后续目标必须有相应实现和当前验证证据后，才能成为当前支持结论；既有说明或历史测试不能替代该证据。

#### 能力矩阵

矩阵以可独立验收的方言契约为单位，先记录目标，再逐项补齐当前证据。能力 ID 稳定标识契约，不随测试名称或实现位置改变；示例 ID 仅用于说明模板，不代表已经登记的支持能力。

| 字段 | 填写约定 |
| --- | --- |
| 能力 ID | 唯一标识场景与方言契约；同一场景有双方言契约时，用 `/pg`、`/mysql` 分别标识并关联。 |
| SQL/客户端场景 | 写明方言、输入 SQL 或客户端操作、参数、编码及必要会话前提，避免仅用函数名表达范围。 |
| 所属工作流 | 使用本路线图第 2–8 章的编号与标题指定责任归属；跨工作流时列主责及协作方。 |
| 分类 | 仅取共享语义、PG 专属、MySQL 专属、明确不支持；表示目标归属，不表示当前已经支持。 |
| 当前证据 | 未采集当前运行证据时填“未验证”；历史文档与测试路径只能注明为调研入口，不能当作实测结论。 |
| 目标结果 | 分方言记录期望的值、行或拒绝行为及适用边界；成功与拒绝均可成为验收目标。 |
| 类型与元数据 | 分方言记录预期结果类型、列名、可空性及协议元数据；PG OID 与 MySQL 类型标志各自验收，不要求相等。 |
| SQLSTATE | 记录各方言预期错误码及触发条件；成功场景填“不适用（成功）”，不能用无错误码表示未验证。 |
| 依赖 | 写明完成契约所需的工作流、能力 ID 或环境前提；无前置依赖时明确填“无”。 |
| 验收入口 | 标明后续用例的定位入口、双方对照或拒绝检查方式；计划入口须注明“待新增”，不能冒充已存在的测试。 |

分类中的“共享语义”要求列明范围内的可观察行为一致，仍分别核对两种方言的类型及协议元数据。“PG 专属”和“MySQL 专属”各自保留方言契约，不因共用执行内核就合并；“明确不支持”要求记录拒绝范围及错误目标，不能把尚未实现或未验证的目标自动归入这一类。

“同名异义分流”不是第五种分类：同一名字在两种方言下语义不同时，分别建立 PG 专属与 MySQL 专属契约及验收条件，并关联两个能力 ID。下表示意将这两个契约并列在一行以便比较；后续登记与验收分别追踪，不将其标为共享语义，也不以共享实现推导结果一致。

字段不得以空白或裸 `null` 隐去信息。“待定义”表示目标尚未细化，“未验证”表示缺少当前运行证据，“不适用（原因）”表示该字段在此场景无含义；这些状态不能相互替代。若目标要求返回 SQL NULL，应明确写“SQL NULL”及对应的类型、可空性；它是结果值，不是矩阵缺项。

以下仅有四条说明性示例，分别演示共享函数、同名异义、PG 专属与明确拒绝。所有结果和 SQLSTATE 都是待验收目标；类型细节标为待定义的条目须在后续实施前补齐。四条当前证据均为未验证，本轮没有运行 AsterSQL、PostgreSQL 或 MySQL 对照，示意行不能充当支持证明。

| 能力 ID | SQL/客户端场景 | 所属工作流 | 分类 | 当前证据 | 目标结果 | 类型与元数据 | SQLSTATE | 依赖 | 验收入口 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 示例 ABS-POS | PG 与 MySQL：`SELECT abs(1) AS v`，限定正整数字面量 | 4. 方言函数与运算符契约；协作 3. 共享类型与转换契约 | 共享语义 | 未验证 | 两方言均返回一行值 1；不由此推广到溢出、NULL 或其他重载 | 列名 v、整数结果；精确类型、可空性、PG OID 与 MySQL 类型标志待定义并分别核对 | 不适用（成功） | 工作流 2、3 的字面量解析与类型契约 | 待新增：工作流 4 的双方言函数用例，分别对照参考端的值及元数据 |
| 示例 LENGTH/pg；示例 LENGTH/mysql | 各方言 `SELECT length('é') AS v`，输入编码均为 UTF-8 | 4. 方言函数与运算符契约；协作 3. 共享类型与转换契约 | PG 专属；MySQL 专属（两个契约） | 两个契约均未验证 | PG 契约按字符数，目标 1；MySQL 契约按字节数，目标 2；后续分别采集对照证据 | 两端列名 v、整数结果；精确类型、可空性及各自协议元数据待定义 | 两个契约均不适用（成功） | 工作流 2、3 的方言分流与字符串编码契约 | 待新增：工作流 4 的两条关联用例，分别对照对应方言参考端，禁止跨方言要求值相等 |
| 示例 SEARCH-PATH/pg | PG 同一会话依次 `SET search_path TO public`、`SHOW search_path` | 5. SQL会话与事务语义；协作 2. 方言身份与解析边界 | PG 专属 | 未验证；历史说明仅为调研入口 | 设置后 SHOW 反映 public 搜索路径；此示例不承诺完整 schema 解析或 MySQL 同名操作 | SET 无结果列；SHOW 的文本结果类型、列名、可空性及 PG 元数据待定义 | 不适用（成功） | 工作流 2 的 PG 会话与名称边界 | 待新增：工作流 5 的同会话设置与读取用例，对照 PG 参考端 |
| 示例 EXTENSION-REJECT/pg | PG：`CREATE EXTENSION postgis`，假定连接及 SQL 传输有效 | 2. 方言身份与解析边界 | 明确不支持 | 未验证 | 按扩展加载非目标拒绝；不得创建扩展对象，不扩展为扩展生态实现任务 | 不适用（拒绝命令，无成功结果集） | PG 目标 0A000（不支持功能）；当前映射未验证 | 工作流 2 的拒绝边界及工作流 7 的错误响应 | 待新增：工作流 2 的拒绝用例，核对错误码与无对象副作用；不要求和可加载扩展的 PG 参考端结果相等 |

#### 证据规则

兼容结论必须绑定能力 ID、具体场景及证据范围。证据按用途分级，不能仅按文档数量或测试名称升级结论；下表定义后续取证要求，本轮没有采集当前客户端或差分运行记录。

| 证据类别 | 可证明的范围 | 结论边界 |
| --- | --- | --- |
| 官方 PostgreSQL 18 行为来源 | 为目标行为提供规范依据，记录官方手册的具体版本、章节与链接；存在歧义时补充对应版本的参考端复现 | 定义预期，不能证明 AsterSQL 已实现；大版本手册不能单独证明某个补丁版本的实测结果。 |
| 源码/独立测试定位 | 以仓库提交 SHA、源码路径及符号、独立测试路径及用例名定位实现与断言意图；入口包括 [连接模块说明](../pkg/server/pg_conn.rs.md) 和 [客户端测试文件](../pkg/server/pg_client_integration_test.rs) | 阅读源码或找到测试不等于测试已执行；模块说明只作导航，运行结论仍需真实源码、独立测试及当前记录相互对应。 |
| 历史测试记录 | 追溯过去在已知版本、环境和命令下报告的行为；[历史 PG 边界说明](postgresql-protocol-first-phase.md) 是调研入口 | 没有本轮复现时统一标注“历史记录，未复测”；缺失字段照实标明，不能据此填入矩阵的当前通过证据。 |
| 当前真实客户端记录 | 证明本轮指定服务端、客户端及配置下，实际执行的连接或 SQL 场景得到所记录结果 | 仅覆盖执行及断言的内容；连接成功不证明 SQL，SQL 执行成功也不证明与 PostgreSQL 语义相同。 |
| 当前差分记录 | 对同一能力契约，在 AsterSQL 与指定 PostgreSQL 参考端上执行相同场景，逐项比较预期与实际结果 | 仅覆盖所运行的输入、类型、元数据、NULL、SQLSTATE、会话/事务状态及副作用等适用断言；需列出未比较项与规范化规则，不能推广为全部 PostgreSQL 兼容。 |

每条证据记录至少包含下列可追溯字段；不适用项写明原因，缺失项不得用空白掩盖：

| 字段 | 记录要求 |
| --- | --- |
| 关联与来源位置 | 能力 ID、场景/用例 ID、记录日期、证据类别、官方来源版本/章节/链接、源码及独立测试路径/符号、日志或产物位置；本地来源绑定仓库提交 SHA 及影响结果的未提交改动。 |
| 版本 | AsterSQL 构建与提交、PostgreSQL 参考端精确版本、客户端/驱动及依赖版本；未使用参考端须明示，不得由 server_version 推断实际构建或参考端版本。 |
| 环境 | 操作系统、部署方式、连接与协议模式、数据库初始状态、编码、时区、会话参数、必要配置及数据准备/清理步骤；记录不得暴露凭证。 |
| 命令与退出码 | 可复现的完整命令、筛选条件、执行时间及每个必要步骤的实际退出码；注明跳过、忽略、失败和环境缺项，命令退出 0 不代替断言结果。 |
| 有效用例数量 | 实际执行并完成适用断言的用例数，另列发现、跳过、忽略与失败数量；重试不得重复计入有效覆盖，零测试不得记为通过。 |
| 预期与实际结果 | 预期行为及依据、双方实际值与适用类型/元数据、错误和状态等断言、比较结果；保留原始输出位置，解释规范化或排除项及其影响，不能只写“通过”。 |
| 结论与限制 | 可连接、可解析、SQL 子集或语义兼容中的具体结论，覆盖条件及未验证项；缺证、版本不明或有效用例数量为零时记录“未验证”及原因。 |

结论用词有以下不同的验收条件，不构成自动递进关系：

- **可连接**：当前客户端完成所声明的握手、认证及连接就绪断言；只覆盖指定客户端、协议与配置。
- **可解析**：具体输入通过指定解析入口及 AST/拒绝边界断言；不能由握手或命令返回成功推定，也不说明执行结果正确。
- **SQL 子集**：列明实际执行并验收的语句、参数、数据和边界用例，说明检查了哪些结果；不能将少量成功语句概括为整个语法或功能兼容。
- **语义兼容**：仅对明确的能力契约和覆盖条件作结论，要求当前差分记录及官方行为依据，适用的值、类型、元数据、NULL、错误、状态与副作用断言完整通过；未覆盖重载、边界或其他客户端必须单列为未验证。明确不支持项按能力矩阵的拒绝目标验收，不因参考端可执行而要求结果相同，也不称为该功能已支持。

反例（仅示意，非本轮实测）：某记录只证明一个指定客户端完成握手，收到 server_version 和 ReadyForQuery，未发送 SQL。错误表述是“PostgreSQL 18 SQL 及事务语义兼容”；正确表述是“在该记录的版本、环境和配置下可连接；SQL 解析、执行、事务及语义兼容未验证”。若该记录来自历史说明，还必须加注“历史记录，未复测”，不能写作当前可连接结论。server_version 是服务端报告值，模块说明文件是调研资料，两者均不能单独证明功能通过。

证据缺失、版本不明、零测试或缺少适用断言的条目，相关当前结论统一标“未验证”，注明缺口；历史记录继续保留其历史身份。新证据只能支撑明确覆盖的契约，经核对来源、运行记录和断言后再更新对应当前证据，不得自动升级支持状态，也不得覆盖既有记录的限制。本轮只建立这些规则，不变更已有支持结论。

#### 差分用例清单

清单使用 JSON 对象描述单个用例，以 `case_id` 关联能力矩阵中的 `capability_id`。`case_id` 在整个清单内唯一，`capability_id` 引用唯一的能力契约，允许多个用例覆盖同一能力；两者均不随文件排列、执行顺序或重试变化。双方言不同契约分别建用例，通过 `related_case_ids` 关联。以下沿用矩阵的说明性能力 ID，不登记新的支持能力。

本轮选择文档内 JSON 示例：仅用 Markdown 表格无法直接作为结构化运行输入；独立 JSON Schema 会引入额外文件、维护和实现约束，因此留待运行器任务决定。这里仅定义格式契约，没有新增清单文件、校验器、执行器或 CI。

| 字段 | 含义与约束 |
| --- | --- |
| `format_version`、`case_id`、`capability_id`、`related_case_ids` | 格式版本与稳定关联；关联列表可为空，表示没有关联用例。格式演进不能重用旧 ID 表示另一契约。 |
| `dialects.pg`、`dialects.mysql` | 两个方言分别声明适用状态和原因。适用项各自记录 `sql`、`setup_sql`（准备）、`cleanup_sql`（清理）、`parameters`、`session`、`expected`。SQL 可不同，但须说明映射和覆盖边界；不能直接把 PG SQL 发送给 MySQL 并忽略错误。 |
| `parameters` | 按绑定位置记录参数值、逻辑类型、方言类型及文本/二进制格式；PG OID、MySQL 类型标志分别记录。参数值使用下述带标签的值表示。无参数用空数组；需要但尚未知的类型填“待定义”，不能执行时猜测。 |
| `session`、`setup_sql`、`cleanup_sql` | 分方言明确编码、时区、会话参数、连接/协议模式、初始数据和事务条件；准备/清理数组按顺序执行，空数组表示无需 SQL 操作。清理失败、准备失败及会话不满足须进入运行记录，不能算有效通过。 |
| `expected` | 目标依据、成功或预期拒绝、结果值及顺序、类型与元数据、SQLSTATE、会话/事务状态与副作用断言。`output`、`sqlstate`、`state_and_effects` 分别承载结果、错误及状态/副作用断言；`output` 沿用观测结果的行、列及顺序结构。目标未细化时标“待定义”；真实参考端的观测结果独立存放，禁止用目标值填充观测值。 |
| `comparison` | 每方言 AsterSQL 与对应真实参考端分别比较；规定行顺序、重复行、列顺序和规范化。无 ORDER BY 时若契约不要求行序可按多重集合比较，仍保留重复行及原始输出；列序和类型不得顺便忽略。每个排除项需说明理由和限制，未知规则标“待定义”。 |
| `observations` | 分别保存 `aster_pg`、`reference_pg`、`aster_mysql`、`reference_mysql` 的实际版本、客户端信息、输出、SQLSTATE、状态及副作用和证据引用。每个必需观测缺失时显式标“未采集”，不允许以空数组冒充零行成功。 |
| `evidence_refs`、`verdict` | 引用上一节证据记录的稳定 ID 或可定位产物（附提交 SHA），并能追溯日期、环境、完整命令及退出码、有效/跳过/失败数量、原始输出和限制。`verdict` 仅在全部适用断言及证据齐全后可记通过；否则记“未验证”或实际失败，缺证不能通过。 |

输入及采集后的每个结果单元用 `{"kind":"value","value":"1"}` 表示非 NULL 值，用 `{"kind":"sql_null"}` 表示 SQL NULL；具体类型由参数或结果列元数据提供。字符串 `"NULL"`、空字符串、零行和 SQL NULL 必须区分。JSON 的裸 `null` 不用作缺证或 SQL NULL。采集前整个字段用 `{"status":"未采集"}`，目标字段未细化用 `{"status":"待定义"}`，不适用用 `{"status":"不适用","reason":"具体原因"}`；这些状态均不等于空的真实结果。

`observations` 中每个采集完成的对象至少包含 `status`、`version`（实际服务端精确版本及 AsterSQL 构建/提交）、`client`（客户端/驱动及依赖版本、协议模式）、`output`、`sqlstate`、`state_and_effects`、`evidence_refs`。`output` 内含 `rows`（行数组，单元按上述标签编码）、`columns`（按列序记录列名、类型、可空性及 PG OID/MySQL 类型标志等元数据）和 `row_order`；命令无结果集需明确说明原因。成功的 `sqlstate` 记“不适用（成功）”，错误时保留实际码和原始错误输出。实际状态、副作用和客户端信息同样不得由预期推断。版本字段中另记录计划基线及参考端实际版本，二者不同需说明对结论的限制；不能把握手报告版本当作构建版本。

下面是唯一的格式示例，SQL 和参数只是待执行输入。PG/MySQL SQL 分别使用自身的参数占位符；示例限定正整数输入，尚不覆盖 NULL 输入或其他重载。两个方言均需采集 AsterSQL 与真实参考端，所有真实对照值、类型、SQLSTATE 和版本尚未采集，目标断言也须在后续实施前细化，不构成有效用例或支持证据。PostgreSQL 18.6 仅沿用来源计划的目标，本轮未核实其发布状态。

```json
{
  "format_version": "1",
  "case_id": "example.abs-pos.bound-int",
  "capability_id": "示例 ABS-POS",
  "related_case_ids": [],
  "mapping": "分别绑定正整数 1；仅覆盖此输入，不推广到其他重载",
  "dialects": {
    "pg": {
      "status": "适用",
      "sql": "SELECT abs($1) AS v",
      "setup_sql": [],
      "cleanup_sql": [],
      "parameters": [
        {
          "position": 1,
          "logical_type": "有符号 32 位整数",
          "dialect_type": "int4",
          "protocol_type": {"status": "待定义"},
          "format": "text",
          "value": {"kind": "value", "value": "1"}
        }
      ],
      "session": {
        "encoding": "UTF-8",
        "timezone": "UTC",
        "protocol": "扩展查询",
        "initial_state": "独立空闲会话，无需表数据",
        "settings": {"status": "待定义"}
      },
      "expected": {
        "outcome": "成功",
        "basis": "能力矩阵示例 ABS-POS；官方行为来源待补齐",
        "output": {"status": "待定义"},
        "sqlstate": {"status": "待定义"},
        "state_and_effects": {"status": "待定义"}
      }
    },
    "mysql": {
      "status": "适用",
      "sql": "SELECT abs(?) AS v",
      "setup_sql": [],
      "cleanup_sql": [],
      "parameters": [
        {
          "position": 1,
          "logical_type": "有符号 32 位整数",
          "dialect_type": {"status": "待定义"},
          "protocol_type": {"status": "待定义"},
          "format": "binary",
          "value": {"kind": "value", "value": "1"}
        }
      ],
      "session": {
        "encoding": "UTF-8",
        "timezone": "UTC",
        "protocol": "预处理语句",
        "initial_state": "独立空闲会话，无需表数据",
        "settings": {"status": "待定义"}
      },
      "expected": {
        "outcome": "成功",
        "basis": "能力矩阵示例 ABS-POS；参考端行为依据待补齐",
        "output": {"status": "待定义"},
        "sqlstate": {"status": "待定义"},
        "state_and_effects": {"status": "待定义"}
      }
    }
  },
  "comparison": {
    "pairs": [["aster_pg", "reference_pg"], ["aster_mysql", "reference_mysql"]],
    "row_order": "按返回顺序逐行比较",
    "column_order": "按返回顺序逐列比较",
    "normalization": [],
    "excluded_assertions": []
  },
  "observations": {
    "aster_pg": {"status": "未采集"},
    "reference_pg": {"status": "未采集"},
    "aster_mysql": {"status": "未采集"},
    "reference_mysql": {"status": "未采集"}
  },
  "planned_reference_versions": {"pg": "18.6（来源计划目标，未核实）", "mysql": "待定义"},
  "evidence_refs": {"status": "未采集"},
  "verdict": {"status": "未验证", "reason": "目标及参数协议类型待细化，尚无任何真实对照记录"}
}
```

PG 专属或 MySQL 专属场景中，另一方言对象用 `status: 不适用` 并记录原因；这是覆盖边界，不能计为该方言通过。明确不支持项使用 `expected.outcome: 预期拒绝`，记录拒绝目标 SQLSTATE、状态恢复及无副作用要求；目标码未知时标“待定义”。若参考端允许操作，保留其真实成功结果，并将比较方式明确为 AsterSQL 的拒绝契约验收，不能强求成功与拒绝相等，也不能忽略任一错误。普通成功用例遇到“不支持”错误是失败或阻塞证据，不能临时转成预期拒绝使其通过。类型、元数据或其他适用断言未采集时仍为未验证，只有完整符合拒绝契约的当前证据才能支持“拒绝边界已验收”，不代表功能已支持。

#### 依赖与验收交接

本轮八个工作流的文档交接顺序为：契约→方言→类型→函数→SQL/会话/事务→目录→协议→客户端验收。下表将此依赖图对应到章节和交接内容；它是路线图编写顺序，实际实现须按真实源码、行为依赖、测试及资源约束重新核实批次，不能据此假定类型或 SQL 实现必须等待协议实现。

| 工作流 | 前序输入与交接产物 |
| --- | --- |
| 1. 兼容契约与差分验证 | 建立范围、能力矩阵、证据规则和 JSON 用例契约，为后续工作流提供统一验收语言。 |
| 2. 方言身份与解析边界 | 读取工作流 1，明确方言身份、语法及名称解析责任，给类型与后续工作流提供可定位的输入边界。 |
| 3. 共享类型与转换契约 | 读取工作流 2 的解析边界，明确类型、转换、NULL 与元数据目标，交给函数及 SQL 语义契约。 |
| 4. 方言函数与运算符契约 | 读取工作流 3，区分共享语义与同名异义契约，为 SQL 表达式验收提供边界。 |
| 5. SQL会话与事务语义 | 读取工作流 4 及方言、类型契约，描述会话/事务状态与副作用目标，交给目录可见性和协议状态验收。 |
| 6. 系统目录与内省契约 | 读取工作流 5 及名称、类型契约，明确目录对象、可见性和内省输出目标，交给协议及客户端用例。 |
| 7. 线协议认证与数据通道 | 读取工作流 6 及类型、错误、会话契约，明确协议承载和错误恢复目标，交给真实客户端验收。 |
| 8. 客户端与双方言共存验收 | 汇合前七项契约，定义客户端场景、方言隔离和当前运行证据要求；只对覆盖的能力作结论。 |

模块定位使用本章“模块”的 PG 边界、共享内核和独立客户端测试入口；模块路径存在或历史测试被记录，均不等于该能力已有当前证据。验收按“能力矩阵”的能力 ID 与目标，关联“差分用例清单”的 `case_id`、`capability_id` 和“证据规则”的版本、环境、命令、退出码、有效用例数量、预期与实际结果等字段；每项适用断言均须能回溯来源，缺项保持未验证。

交接给源蓝图任务 2“方言身份与解析边界”的只读输入是本路线图第 1 章及第 2–8 章标题骨架、四类矩阵示例、证据分级与唯一 JSON 格式示例，以及本章列出的调研入口。任务 2 的定义和范围见 `.plans/2026-10-22-postgresql18-核心兼容蓝图/2-方言身份与解析边界.md`；此处只定义交接，不执行该任务。

文档交接通过条件：范围排除完整，八章顺序及第一章五个责任标题齐全；矩阵的四类示例明确目标与未验证状态；JSON 的能力 ID 可对应矩阵，用例 ID 唯一，SQL NULL、未采集、待定义与不适用含义一致；证据规则不将历史记录或可连接升级为语义兼容；引用路径存在且计划产物标明待新增；任务 2 可据此继续描述方言责任，无需假定兼容能力或运行器已实现。目标中的待定义字段由对应后续能力任务细化，不能视作已验收。

后续细分只按单一能力的可观察契约、单一用例族或独立差分运行器拆分。各任务须声明输入、依赖、适用断言、证据缺口与通过条件；运行器单独立项，本轮不创建执行平台、环境或 CI。文档交付验收只表示契约可交接；所有尚无当前运行记录的能力仍为未验证。

### 范围外

核心非目标统一以“范围与非目标”小节为准。本章不实现生产行为、运行器、参考数据库环境或 CI，不撰写第 2–8 章正文；交接顺序不扩展为各子系统实现计划。

### 验收

文档验收以“依赖与验收交接”的通过条件为准，逐项检查范围、矩阵、证据字段、JSON 关联、章节结构和路径。行为验收则须按矩阵目标及证据规则另行采集当前记录，不能用本章完成、JSON 语法通过或零测试替代。

### 后续细分

按“依赖与验收交接”先交给源蓝图任务 2，再沿后续工作流细化；实施任务限定单一能力、单一用例族或独立运行器。各项在实现前补齐目标及验证入口，按实际依赖重新排批次；本章不展开这些实现任务。

## 2. 方言身份与解析边界

### 模块

[章节验收与后续交接](#章节验收与后续交接)汇总本章来源映射；复核入口为 pg_name.rs、pg_session.rs、pg_extended.rs 及独立的 pg_name_test.rs、pg_query_test.rs，仅作静态证据。

[解析承载方案决策](#解析承载方案决策)比较共享 AST 扩展与独立受控 PG 解析入口；取证入口为 yy_parser.rs、parser.y、ast/expressions.rs、ast/dml.rs、ast/lib.rs、[optimize.rs](../pkg/planner/optimize.rs) 与 pg_extended.rs，独立测试入口为 yy_parser_test.rs、pg_extended_test.rs。只核对必要局部交接。

[方言身份传递](#方言身份传递)负责连接到共享内核的身份边界。实际源码入口为 [pg_conn.rs](../pkg/server/pg_conn.rs) 的 `PgService::start` / `negotiate`、[pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::new` / `handle` / `process`、[pg_session.rs](../pkg/server/pg_session.rs) 的 `PgSession` / `SessionQuery::parse`，以及 [conn.rs](../pkg/server/conn.rs) 的 `ClientConn::dispatch`、`SessionDriver::open_ctx` 和 `TiDBContext`。另核对 [driver.rs](../pkg/server/driver.rs) 的 `IDriver::OpenCtx`：它是另一层生命周期抽象，不能冒充 PG 当前调用的 `SessionDriver`。本块不指定尚未取证的内核类型或接口名。

[类型转换语法边界](#类型转换语法边界)的调研入口为 [yy_parser.rs](../pkg/parser/yy_parser.rs)、[expressions.rs](../pkg/parser/ast/expressions.rs) 和上述扩展查询入口；实际共享表达式转换载体另定位到 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `ExprKind::Cast`。只核对本块必要的 AST 与参数接线，不展开类型执行子系统。

[大小写匹配语法边界](#大小写匹配语法边界)的源码入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `PatternLikeOrIlikeExpr`、[parser.y](../pkg/parser/parser.y) 的 `PredicateExpr` / `IlikeOrNotOp` / `LikeOrIlikeEscapeOpt` 与 [pg_name.rs](../pkg/server/pg_name.rs) 的 `rewrite` / `adapt`。语法文件与 Rust 载体分别取证，不以 Go 语义动作推断 Rust 运行接线。

[返回子句语法边界](#返回子句语法边界)的指定入口为 [ast/dml.rs](../pkg/parser/ast/dml.rs)、[parser.y](../pkg/parser/parser.y) 与 [pg_result.rs](../pkg/server/pg_result.rs)；实际共享 DML/投影载体在 [ast/lib.rs](../pkg/parser/ast/lib.rs)，以 `Returning`、`SelectField`、`command` / `encode_formats` 为必要局部交接点。

[冲突子句语法边界](#冲突子句语法边界)沿用 [ast/dml.rs](../pkg/parser/ast/dml.rs)、[parser.y](../pkg/parser/parser.y) 与 [pg_extended.rs](../pkg/server/pg_extended.rs)；实际 INSERT 载体为 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `InsertStmt` / `Assignment`，只补充本块必要的局部取证。

[数组构造语法边界](#数组构造语法边界)的入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `Expr` / `RowExpr` / `ParamMarkerExpr`、[parser.y](../pkg/parser/parser.y) 的 `SimpleExpr` / `ArrayKwdOpt` 和 [pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::process` / `markers`，仅取证构造与类型/参数交接。

[行构造语法边界](#行构造语法边界)的入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `RowExpr` / `Expr::restore`、[parser.y](../pkg/parser/parser.y) 的 `SimpleExpr`、[pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::close` / `scope` / `relation`；独立测试入口为 parser_test.rs 与 pg_name_test.rs，只作静态取证。

[引号与大小写规则](#引号与大小写规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的名称扫描/改写与 [expressions.rs](../pkg/parser/ast/expressions.rs) 的名称载体为入口；独立测试为 pg_name_test.rs、pg_query_test.rs，仅作静态证据。

[限定名称解析规则](#限定名称解析规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `validate_relations` 与 [ast/dml.rs](../pkg/parser/ast/dml.rs) 为入口，实际共享 TableName 另核对 [ast/lib.rs](../pkg/parser/ast/lib.rs)；独立证据入口为 pg_name_test.rs、pg_query_test.rs。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)以 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::scope` / `relation` 为入口；[ast/dml.rs](../pkg/parser/ast/dml.rs) 为指定导航，实际 CTE 载体在 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `WithClause` / `CommonTableExpression`。独立测试入口为 pg_name_test.rs、parser_test.rs，仅静态阅读。

[模式搜索路径解析契约](#模式搜索路径解析契约)以 [pg_session.rs](../pkg/server/pg_session.rs) 的路径访问器、[pg_name.rs](../pkg/server/pg_name.rs) 的关系门禁和 [pg_catalog.rs](../pkg/server/pg_catalog.rs) 的目录遮蔽接线为入口；独立测试入口为 pg_session_test.rs、pg_query_test.rs，仅静态阅读。

### 目标行为

[章节验收与后续交接](#章节验收与后续交接)以来源四步与完成条件的映射收束本章，给工作流 3 提供解析结构、名称绑定及类型判定边界。

#### 方言身份传递

本块承接第 1 章的共享内核、双方言独立契约与证据规则，对应源工作流 2 步骤 1；只定义身份传递，语法与名称规则留给本章后续文档块。连接建立时应由所属入口确定 PG 或 MySQL 身份，由该连接的会话持有，贯穿简单查询、扩展查询及后续执行上下文。身份在连接生命周期内稳定；会话设置、事务结束、预处理语句关闭和错误恢复不得把另一连接或另一入口切换方言。可变进程全局开关不能承担此职责。以下“应”均为目标契约，当前源码定位不等于全链路实现或运行通过。

责任链为 `listener → session → parser → resolver（名称绑定阶段） → planner → expression context`；箭头表示需要交接的身份责任，不表示已存在同名字段或静态调用边。

| 阶段 | 当前入口与实际边界 | 状态拥有者、生命周期与目标缺口 |
| --- | --- | --- |
| listener | PG 的 `PgService::start` 接受连接，`negotiate` 调用 `driver.open_ctx(id, 0, 45, "", None)`；MySQL 的 `ClientConn::dispatch` 经 `openSession` 取得上下文并分派 Query / Stmt 命令。 | PG service 拥有监听与工作线程，`ClientConn` 拥有 MySQL 连接状态；目标由连接所属入口确定身份，不能把 service 级共享对象变成可变当前方言。启动协议版本和握手版本字符串不是 SQL 方言身份或兼容证据。 |
| session | PG 每个连接循环创建一个 `Extended`，其 `session: PgSession` 被 Q 与扩展查询路径共用；`PgSession` 当前仅有 `path`、`failed_transaction`。`SessionDriver::open_ctx` 返回共享 `TiDBContext`。 | `Extended`、其语句及 portal 映射属于当前连接，连接循环结束后释放；`PgSession` 是真实局部状态入口，但没有全链路方言标记。目标将连接身份与可变 search_path/事务状态分开，关闭连接后不得被后续连接继承。 |
| parser | Q 先尝试 `SessionQuery::parse` 与 `CatalogQuery::parse_session`，普通路径经 `pg_name::adapt`、`pg_sql::adapt_with_context` 等适配后交给 `TiDBContext::execute_query`；扩展 Parse 在 `Extended::process` 做相应识别、适配与 `markers` 映射，再进入 `prepare_statement`。 | 当前受控识别及文本适配位于 PG 连接边界，不是完整 PG parser。目标每次解析显式获得本连接身份，生成的语句与预处理对象保留身份来源；解析器重用不能继承上一连接身份。共享 parser 内部的实际承载接口尚未在本块定位，后续需局部取证，不能从 SQL 文本或占位符反推身份。 |
| resolver | 当前可定位的名称入口是普通 SQL 的 `pg_name::adapt` 与目录查询的 `parse_session`，均收到当前 `PgSession`；共享 `TiDBContext` 接口没有显式方言参数。 | resolver 表示名称绑定阶段，不假定存在同名模块。目标名称绑定读取语句所属会话身份及适用的名称环境；该环境在解析/绑定/执行间的生效时点由后续规则定义。本块没有证明身份已传入共享名称绑定阶段，也不把边界名称适配当作该证明。 |
| planner | 普通 Q 的可定位共享入口为 `TiDBContext::execute_query`；普通扩展查询为 `prepare_statement`、`execute_prepared_statement`，参数是 SQL 或 statement ID/绑定值等，没有显式方言参数。 | 目标规划及计划重用保留产生该语句的方言语义；跨连接缓存不得混用两方言语义。当前只核实上述交接入口，未核实 planner 内部接线、缓存归属或键设计，不强定新增字段、类型及缓存接口。 |
| expression context | 普通执行仍由上述 `TiDBContext` 接口承接；`driver.rs::Expression` 是协议参数/结果值枚举，不是内核表达式求值上下文。 | 目标每条语句及其执行/重执行期间获得所属连接身份，函数分流和求值不得读取其他连接的可变状态。尚无共享表达式上下文接收 PG 身份的取证；具体落点交给后续局部实现调研，函数、转换的语义契约归工作流 3、4。 |

扩展查询目标要求 Parse、Bind、Execute、Describe、Close 与 Sync 始终属于同一连接身份；命名语句与 portal 不得跨连接解析、绑定或执行。简单查询也必须使用同一身份源。身份稳定不意味着冻结所有会话设置：现有 `pg_introspection_namespace_session_state` 的断言意图包括 Parse 后 SET，再执行 `current_schema()` 时观察当前会话值；search_path 状态执行和完整时点规则由工作流 5 承接，目录可见性由工作流 6 承接，本块不推广这条断言到全部名称绑定或计划失效行为。

当前证据固定为源码/独立测试阅读（基于仓库提交 `d55446b4a3`，本轮无行为执行）：[pg_session_test.rs](../pkg/server/pg_session_test.rs) 的 `pg_introspection_namespace_parser_boundaries` 断言有界会话语句的接受/拒绝；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `pg_introspection_namespace_session_state` 断言两 PG 连接 search_path 交错隔离及简单/扩展查询局部状态承接，`postgres_create_table_simple_query` 提供普通 Q 接入的调研入口。这些测试没有证明 PG/MySQL 同时连接时的 parser→resolver→planner→expression context 身份隔离，本轮也没有运行它们。`pkg/server/doc.go` 不存在，已读包内模块说明及源码；不以历史通过记录补当前证据。

以下为待新增的隔离验收用例族，沿第 1 章分别登记能力 ID、case ID、结果元数据和观测记录；表内是目标输入及结果，不是执行结果。成功项 SQLSTATE 均记“不适用（成功）”；没有身份专用 SQLSTATE，不凭空创造错误码。

| 场景与正反例 | PG 适用性、目标结果 | MySQL 适用性、目标结果 | 错误与证据责任 |
| --- | --- | --- | --- |
| DIALECT-IDENTITY-COEXIST：同时保持 PG A、MySQL M 活跃，交错执行 `SELECT 1 AS v`；PG 分别经 Q 及零参数 Parse/Bind/Execute，M 经普通查询及自身预处理协议。反例是 PG 查询改变 M 的后续解析身份。 | 适用；每次返回一行 v=1，所属连接身份始终 PG。 | 适用；每次返回一行 v=1，所属连接身份始终 MySQL；精确类型与协议元数据分别按工作流 3、7 补齐。 | 无错误位置（成功）；相同值本身不足证明身份隔离，后续待新增连接/语句上下文断言，覆盖六段责任链且不引入产品可见调试接口。全部未验证。 |
| DIALECT-IDENTITY-PG-PAIR：A、B 两条 PG 连接；A 设置 `search_path TO pg_catalog, public`，B 显式设置 `search_path TO public`，交错 `SHOW search_path`，Q 和扩展协议交叉使用，再各自关闭并重连。反例是 A 的设置、Sync 或断开改变 B。 | 适用；各自 SHOW 反映本连接显式设置，稳定身份均 PG；重连重新初始化，不能继承 A 的状态。具体默认值另按工作流 5 契约核对，不以现有 public 默认值代表 PG 完整默认行为。 | 不适用这些 PG 设置语句；保持 M 的 `SELECT 1 AS v` 作为旁路隔离检查，不向 M 投递 PG 设置并忽略错误。 | 成功项无错误位置；状态取值归工作流 5，身份隔离归本块。已有测试仅有局部断言意图；完整场景未验证。 |
| DIALECT-IDENTITY-ERROR：A 通过扩展 Parse 发送 `SELECT )`，随后 Sync，再执行 `SELECT 1 AS v`；B、M 同时继续各自正例。反例是 A 的失败恢复令 B/M 切换身份或执行 A 的语句。 | 适用；A 语法错误目标 42601，恢复后可继续，B 保持自己的状态与身份。 | 不向 M 发送此 PG 协议负例；M 的正例应继续返回 1，MySQL 语法错误码及恢复不由本块猜定。 | parser 负责原始 SQL 的定位，PG 协议边界输出 SQLSTATE 和错误字段；位置目标指向 `)`，精确参考端位置须补采集。`write_error` 当前只写 S/V/C/M，没有 P 字段；不能宣称已支持错误位置。恢复契约协作工作流 7，全场景未验证。 |

外部目标依据为 PostgreSQL **18** 官方手册：§54.2.2/§54.2.3 [简单与扩展查询消息流](https://www.postgresql.org/docs/18/protocol-flow.html)，用于区分查询生命周期、预处理语句与 portal 的责任；§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)，规定 P 若提供须基于原始查询、从 1 起按字符计数，不能直接返回改写后字节偏移；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42601 的语法错误分类。文档于 2026-10-23 查阅，未运行参考端；18.6 仍仅为来源指定目标，无该补丁版本实测结论。连接私有身份及禁止可变全局切换是 AsterSQL 双入口架构目标，不把它表述为 PostgreSQL 手册中的内部接口要求。

#### 类型转换语法边界

本块对应源工作流 2 步骤 2 的 `::` 分类，继承[方言身份传递](#方言身份传递)的连接私有身份契约。目标只确定表达式输入、优先级、嵌套、参数位置和诊断保真；类型解析后的转换可行性、舍入、溢出、NULL、结果类型与元数据由工作流 3 定义。本轮仅交付文档，以下用例均未运行，不能据此宣称已经支持。

当前源码证据基于提交 `a19b8cba62`：`Parser::ParseSQL` 在 `yy_parser.rs` 中让 `Scanner` / `yyParse` 决定语法接受与错误，`ParseOneStmt` 限定单语句，`yySetOffset` 向表达式记录原始偏移；现有 MySQL SQL mode 不能当作已存在的 PG 身份入口。`expressions.rs` 的 `ParenthesesExpr`、`BinaryOperationExpr`、`ParamMarkerExpr` 提供括号、运算树和参数顺序/偏移的调研入口；不能把该文件名当成 PG cast 节点定义。实际 `ast/lib.rs::ExprKind::Cast` 已有 `Expr`、`Tp: FieldType`、`FunctionType`、`ExplicitCharSet`，同一枚举有 `Parentheses` 和带 `Offset` 的 `ParamMarker`；这些字段说明可复用候选，不证明已能保留 PG 原始类型拼写、引用规则或 `::` 源位置。

`Extended::process` 的普通 Parse 路径先做有界适配，再调用 `markers` 和共享 `prepare_statement`；`markers` 仅将引号/注释之外的 `$n` 映射到 `?` 并返回零基参数索引序列，不解析转换语法。当前普通扩展路径在参数 OID 缺省/为零时返回 0A000，要求显式 OID；因此 `($1)::int4` 的语法目标不能自动升级为已支持从转换推断参数类型。目录/会话专用识别分支也不能证明通用 SELECT 转换支持。已阅读独立测试 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_parser_err_msg`（MySQL near/line 与注释诊断），以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `marker_mapping_preserves_literals_comments_and_index_order`（字面量、注释、重复/乱序参数保护及拒绝边界）、`canonical_prepare_parameter_syntax_probe`（共享入口 `?` 与 `$1` 的断言意图）；本轮未执行测试，所读断言不构成 `::` 的通过证据。`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

承载方案留待后续最小语法取证：优先评估复用现有 Cast；若需要独立受控 PG 解析入口，其输出仍须可交给共享内核。两种方案都应保留以下信息，未定位的新接口不在此命名或假定存在：

- 操作数表达式树、显式括号及每层转换的顺序；`::` 左结合，不能把链式转换合并为最后一个目标类型，也不能把 `1 + 2::int4` 解析成 `(1 + 2)::int4`。
- 目标类型的原始拼写、限定名分段、引号与大小写信息，以及出现时的类型修饰参数、数组维度等语法信息；这是保真需求，不承诺本块实现完整类型语法、数组或自定义类型。支持集合及类型绑定交工作流 3，不能直接将任意 PG 类型名映射为 MySQL `FieldType` 后丢弃来源。
- 原始 SQL 中操作数、`::`、类型及参数的跨度、原始参数编号与出现顺序，并继承所属连接身份；改写时需有可回溯原 SQL 的位置映射。重复 `$1` 仍绑定同一个外部参数，不能因转换嵌套重排、复制绑定值或将值拼入 SQL。
- 词法层保护字符串、带引号标识符及注释中的 `::` / `$1`。不能用全局文本替换把 `::` 通用翻译为 CAST，也不能删除字符串/注释中的片段；现有 `markers` 拒绝 dollar quote 且不承诺 PG 嵌套块注释，后续要单独取证并明确拒绝边界，不能静默按 MySQL 词法解释。

下表是待新增验收用例族的输入契约，后续按第 1 章登记能力 ID 与 case ID。PG 成功项 SQLSTATE 为“不适用（成功）”，无错误位置；结果值只是目标，精确类型/OID及列元数据交工作流 3、7 补齐。所有实际输出与参考端错误位置均待采集。

| 输入与正反例 | PG 目标 | MySQL 对照与适用性 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| 正例 `SELECT 1::int4` | 解析为整数常量的显式转换，目标值 1；类型名 int4 是 PG 既有名称。 | 原 SQL 作为方言隔离负例，应拒绝 `::`；自身正例 `SELECT CAST(1 AS SIGNED)` 目标值 1，不要求与 PG 类型元数据相同。 | PG 成功；MySQL 负例定位冒号附近，精确诊断/码待参考端及本地取证；双方未验证。 |
| 正例 `SELECT ($1)::int4`，扩展 Parse/Bind/Execute，绑定整数 1 | 保留括号、参数编号 1、转换目标，目标值 1。分别覆盖显式 OID 与省略 OID 后由类型上下文推断；后者为目标缺口。 | 原 SQL 不作 MySQL 正例；自身预处理正例 `SELECT CAST((?) AS SIGNED)` 绑定 1，目标值 1。 | 参数不在普通 Q 文本中直接绑定；PG Parse 语法与类型推断分责，Bind 值解码另验收。当前 OID 限制如上，两协议均未运行。 |
| 优先级正例 `SELECT 1 + 2::int4` 与 `SELECT (1 + 2)::int4` | 分别为右操作数转换与括号和式转换，值均目标为 3；必须检查不同树形，不能仅比较相同结果。 | `::` 均为负例；自身正例分别为 `SELECT 1 + CAST(2 AS SIGNED)`、`SELECT CAST((1 + 2) AS SIGNED)`。 | 语法树责任归本块，求值归工作流 3、4；无当前 AST 或运行证据。 |
| 嵌套正例 `SELECT 1::int4::int8` 与 `SELECT ((1)::int4)::int8` | 前者按左结合，均保留两层转换，目标值 1；不能直接消去内层转换。 | `::` 均为负例；自身正例 `SELECT CAST(CAST(1 AS SIGNED) AS SIGNED)` 仅对照嵌套形态，不宣称两层类型等价。 | 类型链执行及元数据归工作流 3；树形与位置待新增断言，未验证。 |
| 参数位置正例 `SELECT ($2)::int4, ($1)::int4, ($2)::int4`，绑定 1、2 | 目标行 `(2,1,2)`，映射顺序为 `[1,0,1]`，各转换保留来源位置。 | 自身正例 `SELECT CAST(? AS SIGNED), CAST(? AS SIGNED), CAST(? AS SIGNED)` 按出现顺序绑定 2、1、2；不把 MySQL 的三个位置冒充 PG 两个参数。 | 现有 marker 测试仅覆盖一般映射意图，带转换的完整 Parse/Bind 仍未验证。 |
| 缺目标类型负例 `SELECT 1::`、`SELECT ($1)::`（后者走扩展 Parse） | 拒绝不完整转换，语法错误目标 42601；不能忽略 `::` 或退化为普通表达式。 | 同样不是 MySQL 合法转换；自身缺类型负例 `SELECT CAST(1 AS)` 应拒绝。 | PG 定位缺失类型的输入末尾，精确字符位置待参考端采集；MySQL 可更早在冒号处拒绝，不能强求错误位置相等。parser 保留原 SQL 定位，PG 协议边界输出错误字段。 |
| 词法保护正例 `SELECT '1::int4 $1' /* :: $2 */`，以及扩展输入 `SELECT ($1) /* :: $2 */ ::int4` 绑定 1 | 前者字符串值原样保留且无参数；后者仅绑定一个参数，目标值 1，注释不产生转换/参数节点。 | 前者可作 MySQL 自身正例；后者 `::` 是负例，对照用 `SELECT CAST((?) /* :: $2 */ AS SIGNED)`。 | PG/MySQL 仅按各自词法解释；成功无位置。dollar quote、嵌套注释另建边界负例/支持用例前须取证，本轮未验证。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §4.2.9 [Type Casts](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-TYPE-CASTS)（CAST 与 `::` 两种形式）、§4.2.2 [位置参数](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-EXPRESSIONS-PARAMETERS-POSITIONAL)、§4.1.6 [运算优先级](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-PRECEDENCE)（`::` 左结合且高于加法）、§8.1 [数值类型](https://www.postgresql.org/docs/18/datatype-numeric.html)（int4/int8）、§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)（P 基于原查询、从 1 起按字符定位）与[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)（42601）。MySQL 对照采用 **8.4** §14.10 [Cast Functions and Operators](https://dev.mysql.com/doc/refman/8.4/en/cast-functions.html)，仅作自身 CAST 形式的文档依据，不替代实际参考版本选择或运行记录。语法错误、未知类型和转换失败须按阶段区分，不能全映射为 42601；后两者具体目标码由工作流 3 按能力取证。本轮未运行真实数据库，18.6 仍为来源指定目标，无该补丁版本实测结论。

#### 大小写匹配语法边界

本块对应源工作流 2 步骤 2 的 ILIKE 分类，继承[方言身份传递](#方言身份传递)的连接身份契约；只定义 `ILIKE`、`NOT ILIKE`、`ESCAPE` 的语法承载、优先级和诊断责任。匹配算法、locale/collation、NULL、类型转换及运算符重载交工作流 3、4；名称大小写折叠由后续名称规则块定义。以下目标与当前静态证据分开登记，所有运行结果均未验证。

当前源码证据基于提交 `5780b7041ac54cf696b55a9a43fd170e47a4cfc0`：Rust `PatternLikeOrIlikeExpr` 已有 `expr`、`pattern`、`not`、`is_like`、`escape: u8`、`escape_explicit` 及模式缓存字段；这是可复用载体，不证明 PG 输入已完整接线。未纳入图索引的 `parser.y` 有 `ilike` token、`IlikeOrNotOp` 和 `PredicateExpr` 分支，以 `IsLike: false`、`Not` 区分操作类别和否定；`LikeOrIlikeEscapeOpt` 只接收字符串字面量，缺省为反斜杠且 explicit=false，显式空串产生零 escape，超过一个字节时报 ESCAPE 参数错误。该文件含 Go 语义动作，不能冒充 Rust parser 的当前通过证据；字节转义载体也不能直接宣称覆盖 PG 的字符/表达式转义边界。优先级声明将 like/ilike 与比较操作符放在同层，而 PG 18 表 4.2 将模式匹配置于比较之上，因此不能照搬共享声明即宣称 PG 优先级已对齐。

`pg_name::source_tokens` / `rewrite` 保留非编辑区的原 SQL，主要改写关系名和引号；`adapt` 无原生关系时可直接返回改写结果，不是 ILIKE 的完整解析器或求值器。已阅读独立测试 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_high_not_precedence_mode`（用 BETWEEN 检查 MySQL SQL mode 下 NOT 树形）和 `test_parser_err_msg`（near/line 与注释诊断），以及 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（字面量/注释保护及名称改写）、`pg_introspection_names_rejection_boundaries`（引号、嵌套注释等拒绝边界）。这些用例没有给本块 ILIKE 提供专项通过证据，本轮未执行测试；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

承载选择留待最小局部取证：优先比较复用共享 AST 与增加受控 PG 解析入口；前者须核对 Rust parser 实际构造、访问/恢复及共享消费者是否保真，后者须将相同信息交给共享内核，并保持原 SQL 位置。两方案都必须保留操作符类别、左操作数、右模式表达式、否定标志、ESCAPE 是否显式出现及其原始表达式/文本与跨度；省略 ESCAPE 与 `ESCAPE ''` 不得合并。现有 u8 字段不足时只规划必要的表示扩展，不在此规定新接口或补建匹配系统。不能把 ILIKE 无条件替换为 LIKE，也不能全局改写字符串/注释中的关键字，不能用 lower() 包装替代 PG 匹配契约。

PG 目标按模式匹配高于比较、IS、NOT、AND、OR 的顺序保留树形；`NOT ILIKE` 的 NOT 属于该谓词，前置逻辑 NOT 仍需保留其外层位置，括号可显式改变组合。MySQL SQL mode 的 NOT 优先级不能泄漏到 PG 连接。ESCAPE 只关联所属匹配谓词，表达式转义的完整允许集合由后续对照取证；当前字面量规则不能被当成完整 PG 规则。`ILIKE ANY`、符号别名 `~~*` / `!~~*` 及其他匹配家族不纳入本块承诺，后续单独登记，不能借本小节推导已支持。

下表为待新增用例族，按第 1 章分别登记 PG/MySQL 能力 ID 与 case ID。PG 成功项 SQLSTATE 均为“不适用（成功）”，无错误位置；PG 布尔类型与协议元数据交工作流 3、7。MySQL 原 ILIKE 输入作为隔离负例，目标拒绝；共享语法已有扩展候选，实施前须核对现有 MySQL 入口基线并显式解决差异，不在此改动它。MySQL 自身 LIKE 对照不视为 ILIKE 的等价翻译，大小写结果依赖所选 collation，未指定时不猜值或错误码。所有场景均未实测。

| 输入与正反例 | PG 目标 | MySQL 适用性与目标 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| 正例 `SELECT 'Ab' ILIKE 'a%'` | 在支持 ASCII 大小写折叠的确定性 locale/collation 前提下，接受匹配谓词，目标 true；前提需在后续参考环境明确记录。 | 原 SQL 为隔离负例；自身正例 `SELECT 'Ab' LIKE 'a%'` 接受，值待选定 collation 后按工作流 4 取证。 | PG 成功；MySQL 拒绝目标在 ILIKE 附近，精确 SQLSTATE/位置待参考端及本地核实。双方未验证。 |
| 正例 `SELECT 'Ab' NOT ILIKE 'a%'` 与 `SELECT NOT ('Ab' ILIKE 'a%')` | 同上述前提，均目标 false；前者保留谓词否定，后者保留外层 NOT 与括号，不能只凭相同结果证明树形。 | 原 SQL 均为负例；自身 `SELECT 'Ab' NOT LIKE 'a%'` 与 `SELECT NOT ('Ab' LIKE 'a%')` 接受，结果按自身 collation 核实。 | PG 成功；MySQL 原输入的精确拒绝码/位置未验证。 |
| 优先级正例 `SELECT 'Ab' ILIKE 'a%' = TRUE`、`SELECT NOT 'Ab' ILIKE 'a%' AND TRUE` | 分别为匹配结果再比较、外层 NOT 作用于匹配后再 AND；上述前提下目标 true、false。需树形断言，不能把比较纳入模式操作数或将 NOT 作用于左字符串。 | 原 SQL 为负例；MySQL 自身组合以 LIKE 取代关键字单独登记，保留其 SQL mode 语义，不强求 PG 树形。 | 语法组合归本块，求值归工作流 4；当前没有专项 AST 或运行证据。 |
| ESCAPE 正例 `SELECT 'A_b' ILIKE 'a!_b' ESCAPE '!'`，及 `SELECT 'Ab' NOT ILIKE 'a%' ESCAPE '!'` | 保留显式转义及否定；上述前提下目标 true、false。 | 原 SQL 为负例；自身 LIKE / NOT LIKE 配合 `ESCAPE '!'` 接受，值按自身 collation 验收。 | PG 成功；两方言分别核对转义原文与结果，未验证。 |
| 区分省略与空串：`SELECT 'a_b' ILIKE 'a!_b'` 和 `SELECT 'a_b' ILIKE 'a!_b' ESCAPE ''` | 两者都接受但 ESCAPE 出现状态不同；显式空串表示禁用转义。本行仅验收语法信息，不扩写匹配算法。 | 原 SQL 为负例；自身 LIKE 的省略/空串受 MySQL SQL mode 约束，需记录 NO_BACKSLASH_ESCAPES 后另取目标，不直接套用 PG 默认值。 | PG 成功，MySQL 自身形式的具体值/拒绝码待取证；均未验证。 |
| 缺右操作数负例 `SELECT 'Ab' ILIKE`、`SELECT 'Ab' NOT ILIKE`；缺转义负例 `SELECT 'Ab' ILIKE 'a%' ESCAPE` | 拒绝不完整谓词，目标语法错误 42601；不得吞掉 ILIKE、NOT 或 ESCAPE 后执行剩余表达式。 | 原 SQL 应拒绝，可早在 ILIKE 附近失败；自身负例 `SELECT 'Ab' LIKE`、`SELECT 'Ab' LIKE 'a%' ESCAPE` 同样拒绝。 | PG 定位缺失表达式的输入末尾，精确字符位置待参考端采集；MySQL 不要求同位置/同码。parser 保留原 SQL 跨度，协议工作流 7 输出 SQLSTATE/P，当前未验证。 |
| 词法保护正例 `SELECT 'ILIKE NOT ILIKE ESCAPE' /* ILIKE */` | 返回原字符串，字符串/注释不产生匹配节点。 | 自身正例，返回原字符串；无 ILIKE 运算符。 | 双方言成功、SQLSTATE 不适用；现有名称测试只覆盖类似保护意图，本输入仍未验证。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §9.7.1 [LIKE 与 ILIKE](https://www.postgresql.org/docs/18/functions-matching.html#FUNCTIONS-LIKE)（ILIKE、否定及 ESCAPE 规则）、§4.1.6 [操作符优先级](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-PRECEDENCE)、§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)（P 从 1 起按原始 SQL 字符定位）与[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)（42601）。MySQL 自身形式参考 **8.4** §14.8.1 [字符串比较操作符](https://dev.mysql.com/doc/refman/8.4/en/string-comparison-functions.html)，不以其替代本地扩展基线取证。多字符或非字面量 ESCAPE 的合法性、类型/求值错误及其精确 SQLSTATE 尚未取证，由工作流 3、4 补齐；不能统一按 42601 处理。本轮没有数据库或该输入的行为执行证据，18.6 仍仅为来源指定目标，无补丁版本实测结论。

#### 返回子句语法边界

本块对应源工作流 2 步骤 2 的 RETURNING 分类，继承[方言身份传递](#方言身份传递)的连接身份；只定义 INSERT/UPDATE/DELETE 的子句位置、投影保真、语法拒绝及 planner/结果描述交接。真实写入、返回行求值与事务副作用归工作流 5，类型归工作流 3，wire 编码归工作流 7。以下是待实现、待验收契约，不能以删除 RETURNING 后执行原语句、追加 SELECT 或只返回受影响行数冒充成功。

当前静态证据基于提交 `2ca07d8705eeaba2fda8b2f7823468147b7c2e1d`：指定入口 `ast/dml.rs` 是 DML 基础类型，实际共享 `InsertStmt`、`UpdateStmt`、`DeleteStmt` 在 [ast/lib.rs](../pkg/parser/ast/lib.rs)，三者均有 `Returning: Vec<SelectField>`。`SelectField` 提供 `Expr`、`WildCard`、`AsName`、`Offset`、`OriginalText`，可作为复用候选，但不能由这些字段推导 PG 原文、跨度或所有消费者已保真。未索引的 `parser.y` 中 `ReturningClause` 为省略或 `RETURNING FieldList`，`InsertIntoStmt` 将其放在 `OnDuplicateKeyUpdate` 后，`UpdateStmtNoWith` 与单表 `DeleteWithoutUsingStmt` 在尾部承接；这些是带 Go 动作的共享语法，不能直接证明 PG 的 ON CONFLICT、UPDATE FROM、DELETE USING 或 Rust 接线已支持。

`pg_result::command` 在共享 ParseSQL 后，对上述三类 AST 的非空 Returning 明确返回 `0A000`；语法解析错误在该入口映射为 `42601`。这表示现有 PG 命令门禁有拒绝分支，不表示本轮已运行验证。`encode_formats` 以 `QueryResult.columns` / `native_types` 描述列并编码行，DML completion 使用 affected_rows；目前源表 OID/属性号填零、typmod 填 -1，不能据此声称完整 RETURNING 元数据已实现。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_parser_err_msg`（near/line 诊断）、[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `simple_query_roundtrip`（普通 DML completion 与 SELECT 元数据）和 `simple_query_encoding_rejects_unrepresentable_results`（编码及命令拒绝意图）；它们不是本块三类 RETURNING 的专项通过证据。局部补读 [parser_actions/dml_test.rs](../pkg/parser/parser_actions/dml_test.rs) 的 `dml_delete_single_table_keeps_go_modifiers_and_returning`、`dml_insert_and_replace_keep_go_hints_and_returning`，仅有 Returning 长度等断言，未覆盖本契约的完整投影、UPDATE、PG 隔离及副作用。本轮未运行任何测试；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

PG 目标位置为 INSERT 的数据源及可选 ON CONFLICT 之后，UPDATE 的 SET、可选 FROM/WHERE 之后，DELETE 的可选 USING/WHERE 之后；不能照搬共享 MySQL ORDER BY/LIMIT 等扩展顺序。ON CONFLICT 本身由下一块定义，本块只保留衔接位置。投影按顺序保留表达式树、`*`/限定星号、显式或省略 AS 的输出别名及其原始拼写/引号、原 SQL 字符跨度；区分没有子句和出现 RETURNING 但投影为空。字符串和注释中的关键字不产生子句，不以文本截断或全局替换接入。

承载方案留待后续最小取证，比较共享 AST 与受控 PG 入口，不能凭候选字段定新接口。解析负责子句及字段保真；resolver 负责列、限定名、星号绑定，planner 负责保留投影及列顺序，绑定/类型阶段产生输出列名、类型、可空性及可用的来源信息，交工作流 3、7 描述。Describe 不得为获取元数据而执行写入；零行命中也有已确定的列结构。执行阶段由工作流 5 验证 INSERT/UPDATE 默认返回新行、DELETE 默认返回删除前行，并分别验证提交/回滚与受影响行数。PG 18 还含 OLD/NEW 引用及 `RETURNING WITH (OLD AS o, NEW AS n)`；本块不承诺其实现，后续须单独取证及明确支持集合，未支持合法形式以 0A000 在副作用之前拒绝，不能误称 PG 语法非法。MERGE、触发器和规则的实现不在本块范围。

下表为待新增验收用例族；各行独立初始化普通表 `t(id integer)`，无触发器、规则或默认值。UPDATE/DELETE 初始仅一行 id=1，INSERT 初始为空；具体 schema/连接前提由后续用例记录。PG 正例成功时 SQLSTATE 不适用、无错误位置；精确 OID/类型修饰等交工作流 3、7。MySQL 原 RETURNING 输入均作为隔离负例，拒绝目标在 RETURNING 附近，精确码/位置待参考端采集；自身去掉子句的 DML 是合法对照，仅验证自身副作用，不能作为 PG 返回行验收。共享语法已有 RETURNING 扩展候选，实施前须取证现有 MySQL 入口基线并解决差异，不能从 MySQL 官方语法推导本地已拒绝。所有行为均未实测。

| 输入与正反例 | PG 解析与列描述目标 | 执行交接及 MySQL 适用性 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| INSERT 正例 `INSERT INTO t(id) VALUES (1) RETURNING id` | 保留一个列投影，输出列名 id，类型取自绑定列。 | 工作流 5 目标返回 `(1)` 且仅插入一行；MySQL 原输入为负例，自身 INSERT 无返回投影。 | PG 成功目标；当前存在 0A000 门禁，未验证。 |
| UPDATE 正例 `UPDATE t SET id = id + 1 WHERE id = 1 RETURNING id + 10 AS next_id` | 保留算术表达式与别名 next_id，不能误取 SET 表达式或丢弃 WHERE。 | 工作流 5 目标返回 `(12)`，存储为 id=2；MySQL 原输入为负例，自身 UPDATE 后独立查询验证修改。 | PG 成功目标；列名、绑定类型及真实写入分别验收，未验证。 |
| DELETE 正例 `DELETE FROM t WHERE id = 1 RETURNING *` | 保留星号，由绑定阶段按表列顺序展开为 id；不得由返回行推断列数。 | 工作流 5 目标返回删除前 `(1)` 且表为空；MySQL 原输入为负例，自身 DELETE 验证删除。 | PG 成功目标；当前无全链路通过证据。 |
| 多投影/别名正例 `INSERT INTO t(id) VALUES (1) RETURNING id, id + 1 AS next_id, id label` | 三项有序，保留表达式、显式 AS 与省略 AS；输出列名 id、next_id、label。 | 工作流 5 目标行 `(1,2,1)` 且仅写入一次；MySQL 原输入为负例。 | PG 成功目标；树形、列顺序及副作用须独立断言，未验证。 |
| 零行正例 `UPDATE t SET id = 2 WHERE id = 9 RETURNING id` | 仍有 id 列结构，不能退化成无列命令。 | 工作流 5 目标返回零行、原表不变；MySQL 原输入为负例，自身 UPDATE 为零行命中对照。 | PG 成功目标；Describe/Execute 一致性待取证。 |
| 空投影负例 `INSERT INTO t(id) VALUES (1) RETURNING`、`UPDATE t SET id = 2 RETURNING`、`DELETE FROM t RETURNING` | 拒绝缺失投影，PG 目标 42601；不得当作省略子句。 | 解析失败不得执行写入；MySQL 原输入同为负例，可更早在 RETURNING 拒绝。 | PG 指向缺失投影的输入末尾，精确 P 待参考端采集；parser 保留原 SQL 定位，工作流 7 输出错误字段。 |
| 错位负例 `UPDATE t SET id = 2 RETURNING id WHERE id = 1` | 拒绝 RETURNING 后错置的 WHERE，PG 目标 42601。 | 不得删去尾部或子句后执行 UPDATE；MySQL 原输入为负例。 | PG 定位 WHERE 附近，精确字符位置待取证；未验证。 |
| 词法保护正例 `INSERT INTO t(id) VALUES (1) /* RETURNING */` 与 `SELECT 'RETURNING'` | 前者无返回投影，后者普通字符串投影；不制造 Returning 节点。 | 两方言自身正例；前者只插入，后者返回原字符串。 | 成功时 SQLSTATE 不适用；专项保护与无误改写证据待新增。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §6.4 [Returning Data from Modified Rows](https://www.postgresql.org/docs/18/dml-returning.html) 及 SQL 命令 [INSERT](https://www.postgresql.org/docs/18/sql-insert.html)、[UPDATE](https://www.postgresql.org/docs/18/sql-update.html)、[DELETE](https://www.postgresql.org/docs/18/sql-delete.html) 的 Synopsis/输出参数；错误依据为 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 的 42601/0A000 与 §54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的原查询字符位置 P（从 1 起）。MySQL 自身语法依据为 **8.4** §15.2.7 [INSERT](https://dev.mysql.com/doc/refman/8.4/en/insert.html)、§15.2.17 [UPDATE](https://dev.mysql.com/doc/refman/8.4/en/update.html)、§15.2.2 [DELETE](https://dev.mysql.com/doc/refman/8.4/en/delete.html)，其语法未列 RETURNING，不替代本地扩展及实际参考端取证。合法但未支持、语法非法、列绑定/类型失败必须分阶段，列绑定/类型失败的具体错误码待相关工作流采集，不能全部映射为 42601。18.6 仍仅为来源指定目标，本轮无该补丁版本或真实数据库实测结论。

#### 冲突子句语法边界

本块对应源工作流 2 步骤 2 的 ON CONFLICT 分类，只定义语法信息与名称绑定交接。PG 的冲突目标、动作及谓词不能直接等同于 MySQL `ON DUPLICATE KEY UPDATE`，也不能改写为 `INSERT IGNORE`、`REPLACE` 或空赋值列表。唯一性推断、并发、锁、更新执行及事务副作用归工作流 5；本块没有开放 upsert 能力。

当前证据为基于提交 `391d3895b8` 的源码和独立测试阅读，本轮没有运行行为验证。`ast/dml.rs` 的 `TableName`、`Join::restore` 是共享表/连接基础类型；实际 `InsertStmt` 位于 `ast/lib.rs`，其 `OnDuplicate: Vec<Assignment>` 仅提供赋值列表，`Assignment` 包含 `Column` / `Expr`，不能区分“无冲突子句”与 DO NOTHING，更没有 PG 冲突目标、目标谓词和更新条件的独立字段。`parser.y::InsertIntoStmt` / `OnDuplicateKeyUpdate` 将 MySQL 子句写入 OnDuplicate；这是 Go 语义动作的语法证据，不是 Rust PG 解析接线或成功证据。该语法文件未入图，直接读取核对。`pg_extended.rs::Extended::process` 的 Parse 分支经会话/目录识别、名称和 SQL 适配、`markers` 后调用 `prepare_statement`，未在此提供 PG 冲突 AST 或原 SQL 跨度映射；`markers` 的参数映射不能替代诊断位置映射。

已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_insert_statement_memory_allocation`，断言普通 INSERT 的 1001 行及分配上限；已读 [pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `simple_query_roundtrip` 普通 INSERT/UPDATE/DELETE 命令完成及结果断言、`postgres_create_table_simple_query` 的建表入口。这些断言不覆盖 ON CONFLICT，不能作为本块通过证据。`pkg/parser/doc.go`、`pkg/server/doc.go` 均不存在；当前未证明 PG 冲突子句的接受、拒绝、错误码或位置，不从缺字段推断实际报错。

目标承载方案仍待本章解析方案任务比较共享 AST 扩展与受控 PG 入口；不预设新增类型或方法名。无论选择哪条路径，交接至少保留：

| 语法信息 | 目标保真及交接责任 |
| --- | --- |
| 子句及动作 | 区分未出现、DO NOTHING、DO UPDATE；保留 ON CONFLICT、DO、动作和 SET 的原 SQL 跨度。不能用空 OnDuplicate 隐去 DO NOTHING。 |
| 冲突目标 | 区分省略、索引推断列表、ON CONSTRAINT 名称；列表保留列与带括号表达式、顺序、显式 COLLATE/opclass 及各自跨度。DO NOTHING 可省略目标，DO UPDATE 必须指定目标；目标可解析不表示约束存在或可用。 |
| 两类谓词 | 推断列表后的 WHERE 属于目标谓词，SET 后的 WHERE 属于更新条件，分别保留表达式树、出现性、括号及跨度；不能合并为 INSERT/SELECT 的过滤条件。 |
| 更新与名称 | 保留有序赋值、单列/多列目标、DEFAULT/表达式/子查询形态；保留 `excluded.v` 与目标表或别名引用的限定名和引号，交名称绑定区分拟插入行与现有行，不能文本替换成 MySQL VALUES 或普通表。目标表名为 excluded 时显式别名的遮蔽关系须交后续名称任务核对。 |
| 原查询与协议 | Q 与扩展 Parse 使用同一 PG 契约；保留 `$n` 的身份、重复/乱序映射及原查询字符跨度。合法但暂不支持的形态须明确拒绝，不能丢字段后执行；错误传输和恢复交工作流 7。 |

下列均为待新增的 `CONFLICT-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID；不是已实现测试。各例假设 `t(id, v, active)` 可绑定；需要执行冲突时再由工作流 5 创建合适唯一约束/索引和初始行。所有成功项 SQLSTATE 为“不适用（成功）”、无错误位置，均尚未运行。

| 正反例输入 | PG 目标结果与 AST 验收 | MySQL 适用性、错误及证据责任 |
| --- | --- | --- |
| 正例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT DO NOTHING`；再比较显式 `(id)` 与 `ON CONSTRAINT t_pkey` 目标 | 接受相应语法；分别保留省略、索引列、约束名及 DO NOTHING，不能等同于无子句。执行时忽略何种冲突由工作流 5 验收。 | PG 写法在 MySQL 契约下应拒绝；具体 MySQL SQLSTATE/位置待实际参考端与本地基线取证，不套用 PG 码。 |
| 正例：`INSERT INTO t AS dst(id,v,active) VALUES (1,2,true) ON CONFLICT (id) WHERE active DO UPDATE SET v=excluded.v WHERE dst.v<>excluded.v` | 接受语法；目标 WHERE 和更新 WHERE 分属两棵树，SET 中 excluded 与 dst 引用保持可绑定身份；冲突条件成立/不成立的真实写入与不更新结果另交工作流 5。 | 此 PG 形式应拒绝；MySQL 诊断仍待取证。不能仅删除目标或 WHERE 后转成 MySQL 更新。 |
| 正例形态：`ON CONFLICT ((lower(v)) COLLATE "C" text_ops) WHERE active DO NOTHING`（接在相应 INSERT 后） | 保留表达式括号、collation/opclass 名称及目标谓词；只验收解析保真，名称是否存在、类型及索引可用性另行取证，不承诺执行成功。 | PG 目标形式不适用 MySQL；目标名字有效性和相关错误码由名称/类型及工作流 5 补齐。 |
| 反例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT (id)`；以及尾部 `DO`、`DO UPDATE SET` | 缺少动作或赋值，PG 语法拒绝目标 42601；定位缺失项后的结束位置（或实际非法 token），解析器记录本子句跨度，精确 P 值待参考端采集。 | 同样不是合法 MySQL 冲突更新形式；MySQL 码与位置独立取证。禁止静默退化为普通 INSERT。 |
| 反例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT DO UPDATE SET v=excluded.v` | 缺少 DO UPDATE 所需冲突目标，应拒绝；具体参考端诊断阶段、SQLSTATE 与 P 是否提供须后续采集，不能只凭 Synopsis 的可选目标猜码。 | 不适用 MySQL；拒绝和定位单独验收，不作为 MySQL upsert 正例。 |
| MySQL 正例/PG 反例：`INSERT INTO t(id,v) VALUES (1,2) ON DUPLICATE KEY UPDATE v=3` | PG 方言目标语法拒绝 42601，错误跨度指向不属于 PG 子句的关键字，精确 P 待采集。 | MySQL 原生语法接受目标，OnDuplicate 保留赋值；真实冲突更新与 affected rows 交工作流 5，不能要求与 PG DO UPDATE 的结果天然一致。 |
| 词法及参数保护：字符串/注释中出现 `ON CONFLICT`、`excluded.v`；扩展 Parse 中 VALUES/SET/两类谓词含重复或乱序 `$1`、`$2` | 字符串/注释不产生冲突节点，实际参数绑定位置不因子句改动错位；按合法上下文构造完整 SQL，断言树、参数身份与原 SQL 跨度。 | MySQL 仅使用自身合法 INSERT/冲突更新及 `?` 参数，交错连接不继承 PG 解析身份；双方均未验证。 |

官方规则（2026-10-23 查阅）依据 PostgreSQL **18** SQL 命令 [INSERT 的 Synopsis 与 ON CONFLICT Clause](https://www.postgresql.org/docs/18/sql-insert.html)：目标、动作、两类谓词及 excluded 的责任分别保留。错误类别依据 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)，错误位置依据 §54.8 [Error and Notice Message Fields](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的 P：原查询从 1 起按字符计数，不直接使用改写后字节偏移。合法未支持路径的目标拒绝码为 0A000，不能冒充参考 PG 语法错误；本轮未证明当前门禁。MySQL 对照依据 **8.4** §15.2.7.2 [INSERT ... ON DUPLICATE KEY UPDATE](https://dev.mysql.com/doc/refman/8.4/en/insert-on-duplicate.html)，独立保留原生契约。18.6 仍仅为来源目标，本轮没有该补丁版本的实测证据。

#### 数组构造语法边界

本块对应源工作流 2 步骤 2 的数组构造分类，只定义 `ARRAY[...]` 的解析结构与类型交接。当前证据基于提交 `59c3c7c0535b3b54a1764401d0fccb244bfbcd8a` 的源码/独立测试阅读，没有行为实测。`expressions.rs::Expr` 有 Value、Raw、Row 等变体，`RowExpr::values` 是行字段列表，该枚举没有专门的数组构造变体；不能把 Raw 文本或 Row 列表当作数组结构保真的证据。未入图的 `parser.y` 已直接核对 `SimpleExpr` / `ArrayKwdOpt`：现有 ARRAY 关键字用于 CAST 等路径，`SetArray` 标志不等于 PG 的方括号构造语法，也不能从这些 Go 语义动作推断 Rust 实际接线或当前拒绝结果。

`pg_extended.rs::Extended::process` 的 Parse 分支经过会话/目录识别、名称及 SQL 适配和 `markers` 后准备语句；普通非目录路径的 OID 白名单没有数组 OID，未知参数类型另有 0A000 门禁。这只说明该参数入口的局部限制，不证明无参数数组 SQL 的接受/拒绝。`markers` 保存重复/乱序参数的映射，不保存原 SQL 诊断跨度。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_expression` 注册项和 `run_contract_table_with_options`：它们检查用例表接受性与顶层 AST 类型，不能证明数组嵌套树和类型保真；已读 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `parse_bind_execute_sync` 中 current_catalog 的 Parse/Bind/Describe/Execute/Sync 断言，该段不是数组用例。本轮未执行这些测试，亦未证明其他用例是否覆盖数组。`pkg/parser/doc.go`、`pkg/server/doc.go` 均不存在。

承载方案交本章后续解析方案任务比较共享 AST 扩展与受控 PG 入口，不预设新接口。解析层必须保留 ARRAY 构造类别、有序元素表达式树、每层方括号及逗号跨度、空列表、内层 ARRAY 的显式性、显式转换的类型名/数组标记及其作用层级、参数身份与原 SQL 字符位置。不能扁平化嵌套、按文本逗号切割或替换为 ROW、JSON_ARRAY、字符串字面量。语法嵌套不等于最终值维数：解析保留树，矩形形状约束和表达式求值得到的维度交后续语义验收；数组类型、共同元素类型、空数组定型、转换、NULL 与 OID 身份归工作流 3，参数/结果文本和二进制编码、错误字段和恢复归工作流 7。Q 与扩展 Parse 使用同一 PG 语法契约，不以 Parse 成功代表可 Bind 或可返回数组。

下表为待新增 `ARRAY-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID。无表依赖；参数场景另记录参数类型、值及编码。所有行为均未验证，成功目标 SQLSTATE 为“不适用（成功）”、无错误位置。表中数组值用逻辑元素/形状表达，不承诺 wire 文本格式、列名或 OID；结果类型由工作流 3 细化，协议描述及编码由工作流 7 验收。

| 正反例输入 | PG 目标结构与类型边界 | MySQL 适用性、诊断及证据责任 |
| --- | --- | --- |
| 正例 `SELECT ARRAY[1,2]` | 保留两项整数表达式及顺序，目标逻辑数组元素为 1、2；共同元素类型交工作流 3，不能用两列行值替代。 | PG 写法在 MySQL 为隔离负例，应按自身语法拒绝；MySQL SQLSTATE/位置须采集自身参考端和本地基线，不套用 PG 码。 |
| 正例 `SELECT ARRAY[ARRAY[1,2],ARRAY[3,4]]` 与 `SELECT ARRAY[[1,2],[3,4]]` | 保留两种原始写法及相同嵌套分组；目标为 2×2 矩形、元素按上述顺序。不能因省略内层 ARRAY 而丢层级。 | 两种 PG 写法均为 MySQL 隔离负例；诊断仍待取证。矩形形状和值的运行验证另行实施。 |
| 语法正例/定型反例 `SELECT ARRAY[]`；带类型正例 `SELECT ARRAY[]::integer[]` | 空列表须可表示且不等于 NULL、零列或语法残缺；独立无类型上下文的前者不能得到无类型数组成功结果，后者交工作流 3 定型为空整数数组。裸空数组的参考 SQLSTATE、阶段及精确位置待 PG 18 参考端采集，不猜码；解析层不得擅自默认 text[]。 | 原 PG 输入在 MySQL 为负例；自身空值/JSON 语法按自身契约验收，不能要求得到 PG 空整数数组。 |
| 混合元素边界 `SELECT ARRAY[1,2.5]`、`SELECT ARRAY[1,true]` | 均保留元素树和各自类型信息；前者目标共同数值类型，后者无法按共同类型规则统一，交工作流 3 判定并拒绝。精确类型、转换结果及类型失败 SQLSTATE/位置由该工作流补参考端证据，不能一律当 42601。 | MySQL 对原输入按语法拒绝，不能先按 PG 元素共同类型规则求值；当前未采集诊断。 |
| 形状反例 `SELECT ARRAY[[1,2],[3]]` | 保留不等长内层结构，不伪造缺失元素；矩形约束失败由后续语义阶段拒绝，错误阶段、SQLSTATE 与 P 是否出现待参考端取证，不能因方括号闭合就声称执行成功。 | 原输入不是 MySQL 数组构造正例；其自身 JSON 嵌套不等长规则不参与 PG 矩形验收。 |
| 语法反例 `SELECT ARRAY[1,2` | 未闭合方括号，PG 拒绝目标 42601；解析器定位输入末尾的缺失闭合处，精确 P 值待参考端采集，禁止补括号后执行。 | MySQL 可更早拒绝该 PG 构造；具体错误与位置独立采集。工作流 7 传输诊断，不把改写后字节偏移当原 SQL 字符位置。 |
| 参数/词法保护 `SELECT ARRAY[$2,$1,$2]`；`SELECT 'ARRAY[1,2]' /* ARRAY[] */` | 前者保留重复/乱序身份与元素位置，类型及 Bind 另验收；后者仅为普通字符串，不能制造构造节点。前者有充分类型上下文后才是完整成功用例；后者为双方言自身正例，目标原字符串。 | MySQL 不接受前者 PG 构造及 $n 参数契约，自身参数用 ?；交错连接验证不泄露 PG 解析身份。后者成功时无错误位置，双方均未实测。 |
| MySQL 自身正例 `SELECT JSON_ARRAY(1,2)` | 不作为 PG 数组构造正例或等价翻译；该函数在 PG 下的存在性/行为另行取证。 | 按 MySQL JSON 构造规则目标为 JSON 数组，成功时无错误码/位置；其 JSON 类型及编码按自身契约，不比作 PG integer[]。本轮未实测。 |

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2.12 [Array Constructors](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-ARRAY-CONSTRUCTORS) 定义方括号、嵌套、省略内层 ARRAY、矩形约束及空数组显式定型；§10.5 [共同类型判定](https://www.postgresql.org/docs/18/typeconv-union-case.html) 交接工作流 3。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分语法错误 42601 与功能未支持 0A000；合法但暂未支持的形态目标明确以 0A000 拒绝，不能冒充参考 PG 语法非法，本轮未证明当前门禁。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 规定 P 指向原查询、从 1 起按字符计数。MySQL **8.4** §11.5 [Expressions](https://dev.mysql.com/doc/refman/8.4/en/expressions.html) 与 §14.17.2 [JSON_ARRAY](https://dev.mysql.com/doc/refman/8.4/en/json-creation-functions.html#function_json-array) 仅定义自身语法对照，不证明本地入口已拒绝 PG 构造。18.6 仍仅为来源目标，无该补丁版本实测结论。

#### 行构造语法边界

本块对应源工作流 2 步骤 2 的行构造分类，只定义 `ROW(...)`、多字段括号行和单表达式括号的解析边界。当前静态证据基于提交 `d3995a985309c6393975d051cb90b98be5176bb9`，不是运行通过证据。已核对 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `RowExpr::values: Vec<Expr>` 与 `Expr::restore` 的 Row 分支：可保存有序字段树，恢复时统一输出 ROW，载体本身没有显式 ROW 与隐式括号的来源标记或逐项跨度。共享载体是候选，不能据其存在推断 PG 解析已接线或匿名行类型已可执行。未入图的 [parser.y](../pkg/parser/parser.y) 的 `SimpleExpr` 分别将 `(Expression)` 建为 ParenthesesExpr，将 `(ExpressionList, Expression)` 和 `ROW(ExpressionList, Expression)` 建为 RowExpr；后两条要求至少两项，且含 Go 语义动作，不证明 Rust 当前接受范围。

已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::close` / `scope` / `relation`：close 配对括号，scope 对扫描遇到的括号递归，并不建立行表达式树；relation 在关系位置只允许以 SELECT/WITH 开头的括号，否则返回 0A000 的关系分组拒绝。这是局部名称适配限制，不能推广到 SELECT 列表中的行构造，更不能把所有括号当子查询或关系。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_expression` 注册与 `run_contract_table_with_options`，其接受性及顶层类型对照没有直接断言本块的字段嵌套、原始括号和错误位置；已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（含子查询/CTE、extract 内 FROM 及字面量/注释）和 `pg_introspection_names_rejection_boundaries`（引号、注释及名称拒绝）。这些不是本块新增专项断言，亦未执行；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 不存在。

目标由共享 AST 扩展或受控 PG 解析入口承载，选择交后续解析方案任务取证，不预设新接口。两种方案都须区分行构造、分组表达式、标量子查询和 FROM 关系括号；保留显式 ROW 的出现性、字段数量与顺序、逐字段表达式树、嵌套层级、括号/逗号及原 SQL 字符跨度，不能按逗号文本切分、扁平化或改写成 SELECT 列表。字段顺序不承诺求值顺序；字符串、注释及引号内的 ROW/括号不制造节点。行类型、字段类型、转换及参数定型交工作流 3，行比较、NULL 及运算符语义交工作流 3、4；工作流 7 负责复合值元数据/编码、错误传输与恢复。Q 与扩展 Parse 共用 PG 契约，保留 `$n` 身份；解析接受不能替代 Bind、Describe 或结果编码证据。

下表是待新增 `ROW-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID；除关系例外无表依赖，所有行为均未验证。PG 成功目标 SQLSTATE 为“不适用（成功）”、无错误位置；行值以逻辑字段描述，不预定 wire 文本、列名或 OID。类型可判定是目标分析结论，不表示本地实现已具备该能力。

| 输入与正反例 | PG 语法、类型与执行目标 | MySQL 适用性及诊断责任 |
| --- | --- | --- |
| 正例 `SELECT ROW(1,2)` | 语法合法；保留显式 ROW 和有序两字段；目标可定为匿名 record，字段为两个整数，精确字段类型/OID 交工作流 3。目标执行返回一列复合值，字段依次为 1、2，不能返回两列。 | ROW 多字段也是 MySQL 行语法；不能标为 PG 专属负例。但该 SELECT 投影的合法语法不保证 MySQL 可作为单列复合值返回；适用上下文、标量限制及 SQLSTATE/位置须 MySQL 参考端取证，不承诺同结果。 |
| 正例 `SELECT (1,2)` | 语法合法的隐式两字段行，与前例同逻辑字段和匿名行类型目标；保留隐式形式及跨度，不当子查询。执行目标仍是一列复合值，尚未验证。 | 多字段括号行也是 MySQL 语法，投影执行限制独立验收，同样不能仅因语法接受声称可执行。 |
| 分组正例/单字段行反例 `SELECT (1)`；对比 `SELECT ROW(1)` | 前者语法合法、类型为标量整数、目标值 1，不能建成单字段行；后者合法且为含整数的单字段匿名 record，目标一列复合值。两者类型可区分，执行和协议仍待证。 | `(1)` 是自身分组正例，目标标量 1；MySQL ROW 形式要求至少两项，`ROW(1)` 为语法负例。具体 MySQL 错误码/位置另取证，不套 PG 码。 |
| 语法反例 `SELECT ROW(1,` | 缺少下一字段及闭合括号，拒绝目标 42601；无合法完整树，不进入类型判定或执行，不能自动补齐。定位输入末尾，精确 P 待 PG 18 参考端采集。 | 同样是不完整输入，应拒绝；MySQL 码和位置独立取证。解析层负责原 SQL 定位，PG 协议层传递错误字段。 |
| 嵌套正例 `SELECT ROW(1,ROW(2,3))` | 语法合法；外层两字段，第二字段仍为两字段行，不能扁平化成三字段。匿名行及内层字段类型交工作流 3，目标执行保持嵌套值；复合结果编码须工作流 7 补证。 | 只作为自身行表达式上下文候选；嵌套可执行范围、类型及诊断须取证，不推导与 PG 复合值等价。 |
| 参数正例 `SELECT ROW($2,$1,$2)`（扩展 Parse，显式整数参数类型，绑定 1、2） | 保留参数编号、重复身份及字段位置，目标逻辑字段 2、1、2；语法完整，依给定参数类型定型，Parse/Bind/结果仍须分别验证。无类型上下文的参数不能直接声称可定型。 | MySQL 使用自身 `?` 和行语法上下文，不能继承 `$n` 或 PG record 类型；具体执行用例待 MySQL 基线取证。 |
| 名称交互正例 `SELECT ROW(t.id,(SELECT 2)) FROM public.t AS t`；对比关系分组 `SELECT * FROM (public.t) AS t` | 假定 public.t 有整数 id；前者行字段含标量子查询，外层关系名照常绑定，类型依赖绑定/子查询结果，执行待证。后者属于关系括号，当前名称适配有独立拒绝边界，不能用该拒绝否定表达式 ROW；关系组支持不由本块扩写。 | MySQL 用自身已建 t 和合法限定名验证，public 不自动视为 PG schema；关系组诊断独立取证，不要求双方言名称改写相同。 |
| 词法保护正例 `SELECT 'ROW(1,2)' /* (1,2) */` | 只产生普通字符串，目标原字符串值；没有行节点、无行类型需求，语法/定型/执行目标明确但未验证。 | 自身正例，目标相同字符串；双方言交错连接的行/分组正反例验证身份不泄露，仍须分别记录类型与诊断。 |

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2.13 [Row Constructors](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-ROW-CONSTRUCTORS) 定义显式 ROW 与多字段省略形式及匿名 record；§4.2 的表达式分类及 §4.2.11 区分分组与标量子查询。同页还定义零字段 ROW 和 `rowvalue.*` 形式，它们应由后续任务独立定义支持集合，不在本块四个核心输入之外擅自宣称支持。MySQL **8.4** §11.5 [Expressions](https://dev.mysql.com/doc/refman/8.4/en/expressions.html) 提供分组、ROW 至少两项及子查询的自身语法依据，投影/嵌套的实际诊断仍缺参考端证据。[PG 附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分 42601 与合法未支持目标拒绝码 0A000；不能把类型/上下文错误一律作为语法错误，本轮未证明当前门禁。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 要求 P 若提供须基于原查询、从 1 起按字符定位，精确值及是否提供由参考端补证。18.6 仍仅为来源目标，没有该补丁版本或本地数据库实测。

#### 引号与大小写规则

本块对应源工作流 2 步骤 3 的名称词法契约，区分名称拼写、绑定键和后端存储约束。静态证据基于提交 `3bd789433dee85dcd9b2037ee631bdcf13a29429`：已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Token` / `tokens` / `source_tokens` / `quote` / `rewrite` / `Resolver::relation`。tokens 调用非 native 模式；source_tokens 将未引号名称转为小写 name，双引号名称保留大小写、解码成对双引号，并保存 quoted 及原输入字节 start/end。rewrite 为双引号名称生成反引号文本，关系位置再替换成当前数据库限定名；未被编辑覆盖的文本直接复制。因此 token 中小写 name 不等于整个 SQL 已按 PG 规则折叠，尤其不能据此证明表达式列名/别名已正确绑定。native=true 仅用于 stored_view_select 的存储源提取，不是允许 PG 输入反引号的开关。

当前扫描跳过普通单引号字符串、行注释及普通块注释，保留未编辑原文；未闭合引号/空名称返回 42601。PG 输入反引号、美元引号、可执行 MySQL 注释及嵌套块注释有 0A000 拒绝边界；其中美元字符串与嵌套注释是合法 PG 形式，不能把当前拒绝解释为 PG 语法非法。单引号扫描沿用 native 反斜杠转义，不足以证明 standard_conforming_strings、E 字符串或全部 PG 词法保护正确。错误返回只含 SQLSTATE 和消息，rewrite 返回 String，未见本块原 SQL 到转换后文本的持久位置映射。

已读 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `CiString`、`ColumnName::matches` / `restore` 与 `RestoreCtx::write_name`：original/lowercase 双拼写、按小写匹配及反引号还原不能表达完整 PG 引号敏感绑定，也不保留逐名称 quoted 和原 SQL 跨度。已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（双引号、`"a""b"`、字符串/注释）和 `pg_introspection_names_rejection_boundaries`（空/未闭合引号、`"PUBLIC"` 等拒绝）；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `postgres_create_table_simple_query` 使用双引号混合大小写表名，但按 schema.name.lower / table.name.lower 查找，未断言大小写不同对象共存或误绑定拒绝。这些仅为静态测试意图，本轮未执行。`pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 均不存在。

目标在 PG 词法阶段区分标识符、关键字、字符串和注释，保存原拼写、是否引号、解码名称、规范绑定键和原 SQL 跨度；未引号按 PG 规则折叠，双引号保真且 `""` 解码为一个名称字符。resolver 对表、列和别名使用该语义身份，不能统一用 lowercase 或依靠反引号转换制造大小写敏感。共享 AST 扩展与受控 PG 入口的选择交后续解析方案块取证，不预设新接口。若后端存储/目录无法区分 foo 与 Foo，应显式登记能力缺口、阻止静默合并/误读；命名映射或存储改造另行细分，本块不指定方案。限定名、CTE 遮蔽及 search_path 在后续块定义；目录可见性归工作流 6，会话设置归工作流 5。

以下为待新增 `NAME-QUOTING` 用例族，所有运行行为均未验证。成功目标 SQLSTATE 为“不适用（成功）”、无错误位置；类型和元数据按第 1 章分别采集，不把拼写一致当运行通过。MySQL 基线为 8.4，明确记录 sql_mode、lower_case_table_names、对象类别及平台；双引号默认字符串模式与 ANSI_QUOTES 分开验收。

| 输入与正反例/前提 | PG 目标及诊断责任 | MySQL 自身目标及诊断责任 |
| --- | --- | --- |
| 正例：预建 public.foo，列 id=1；分别 `SELECT id FROM foo`、`SELECT id FROM Foo`、`SELECT id FROM "foo"` | 三者均绑定 foo，目标值 1；不将 Foo 作为独立对象。名称折叠发生于词法/绑定交接，非字符串全文转小写。 | 未引号 foo/Foo 的表绑定取决于 lower_case_table_names 与平台，不预定一致；双引号表名在默认模式是负例，ANSI_QUOTES 下按自身表名规则处理，不自动获得 PG 大小写敏感。MySQL 错误码与位置须参考端采集。 |
| 保真正例/误绑定反例：同一 schema 预建 foo(id=1) 和 "Foo"(id=2)；`SELECT id FROM "Foo"` | 目标只返回 2；foo/Foo/"foo" 仍返回 1。再单独以仅有 foo 的夹具查询 "Foo"，应为未定义关系 42P01，不能降级绑定 foo。是否给出 P 及精确位置待 PG 18 参考端取证。 | 不要求其可创建这两个表；必须先按自身配置确认可区分性。以单个表的列名 foo/Foo 做自身不区分大小写正例；不将后端原有规则改成 PG 规则。 |
| 转义正例：预建 `public."a""b"`，id=3；`SELECT id FROM public."a""b"` | 解码名称为 a 后接一个双引号再接 b，目标 3；原拼写与解码值分别保存，不把引号当字符串或分隔两名称。 | 反引号包围 a、双引号、b 为自身标识符正例；ANSI_QUOTES 下同一双引号输入也是名称候选，默认模式该 FROM 输入是负例。结果/诊断独立取证，不把 SQL 文本改写结果当执行证据。 |
| 词法反例：`SELECT * FROM "Foo`；空名称 `SELECT * FROM ""` | 均拒绝目标 42601，不补齐引号、不进入对象查找。解析层保留错误起始引号及输入末尾跨度，P 的准确选择/数值待参考端取证，工作流 7 只传递诊断。 | 同一 FROM 输入在默认模式及 ANSI_QUOTES 下均须拒绝；错误码、阶段、位置独立采集，不套 PG 42601 或 P。自身未闭合反引号名称另作负例。 |
| 方言隔离：反引号包围的 foo 作 FROM 表名；另对比 `SELECT "Foo"` | 反引号输入为 PG 语法负例，目标 42601；当前适配返回 0A000 是已知诊断差异。后者是名称引用，在无 FROM 且无该列时目标 42703，不能返回字符串。 | 反引号表名是自身正例；默认模式 SELECT 双引号 Foo 返回字符串 Foo，ANSI_QUOTES 下是列引用并按自身缺列规则拒绝。隔离回归交错 PG/MySQL 连接，禁止 PG 改写或绑定键泄露。 |
| 保护正例：`SELECT 'Foo "a""b"' /* "Foo" */`，另加行注释中同样名称 | 字符串值保持原大小写及双引号，注释不制造名称/关系节点；目标只返回该字符串。不同 PG 字符串模式与合法嵌套注释须另取证，不据此推广保护集合。 | 普通单引号和普通注释是自身正例，目标相同文本；MySQL 特有可执行注释另按自身规则，不被 PG 扫描门禁改写。 |

位置责任为：解析/适配保留原查询和每次替换区间，后续绑定/解析诊断若基于转换后文本，必须先映射回原 SQL；字符计数与 UTF-8 字节偏移分别处理。PG 的 P 若提供须按原输入从 1 起按字符计数，不能直接使用 Token.start/end 或反引号文本偏移。待新增用例需包含 `"a""b"` 长度变化、public 到当前数据库限定名变化，以及错误前有中文的输入；精确 P、MySQL 自身位置和简单 Q/扩展 Parse 一致性均缺运行证据。合法但尚未支持的形态按明确能力门禁拒绝，不把词法错误、绑定缺失与 0A000 混为同类。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.1.1 [标识符](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-SYNTAX-IDENTIFIERS)、§4.1.2 与 §4.1.5 定义名称、字符串、注释边界；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 提供 42601、42P01、42703 和 0A000 分类；§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §11.2 [对象名称](https://dev.mysql.com/doc/refman/8.4/en/identifiers.html) 和 §11.2.3 [名称大小写](https://dev.mysql.com/doc/refman/8.4/en/identifier-case-sensitivity.html) 仅为自身目标依据。U& 引号、编码、名称长度/截断与非 ASCII 折叠的完整契约另需专项官方取证及参考端验证，不由 Rust to_lowercase 推导。18.6 仅沿用来源目标，没有该补丁版本或本地数据库实测结论。

#### 限定名称解析规则

本块对应源工作流 2 步骤 3 的关系名与列/别名限定责任，承接[引号与大小写规则](#引号与大小写规则)。静态证据基于提交 `c05789991a5462588cd20c8c9dedace2f233014f`：已核对 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `scope`、`rewrite`、`adapt` 与 `validate_relations`。relation 按点号收集关系分段：单段非 CTE 名在 public 不在路径时返回 42P01；public.t 与当前库 db.public.t 可进入改写，其他 schema、其他库及更多段返回 0A000；无当前数据库时返回 3D000。限定 public 不受 has_public 布尔值限制。adapt 先以空数据库探测，再按需读取 SELECT DATABASE() 并重写：首遍允许三段库名只是探测，不能脱离第二遍当前库检查解释为允许跨库。最终将关系跨度替换为后端当前库和表名，不表示 public 已成为完整 PG schema。

validate_relations 对成功解析的共享 AST 遍历 TableName，拒绝非空且与当前库 lowercase 不同的 Schema（0A000）；解析失败时返回 Ok，后续命令门禁仍须拒绝错误，不能把此 guard 当完整语法/权限检查或保证所有节点可见。指定 [ast/dml.rs](../pkg/parser/ast/dml.rs) 的 `TableName::new` / `restore` 仅有 schema/name 两级及 MySQL 引用还原；实际 guard 使用 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `TableName`（Schema、Name），均不能单独承载 PG 数据库/schema/关系三层、逐段引号与原跨度。scope 只在关系位置调用 relation，a.id 不因此成为 schema.table；列与别名绑定仍需共享解析和绑定层的专项证据，不能由改写成功推定。

已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`：t、public.u、db.public.v 的混合 FROM/JOIN、双引号 a.id 及 DML 关系改写；`pg_introspection_names_rejection_boundaries`：other.public.t、other.t、db.private.t、public.t.extra、空当前库和路径无 public 的拒绝断言。二者只调用 rewrite，不覆盖 adapt 的数据库读取或 AST guard。已读 [pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `postgres_create_table_simple_query`，public 的混合大小写建表落到 test 库是局部意图，不是跨库拒绝、别名或完整 schema 的证明；本轮均未运行。目标包 `pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 不存在。

目标按语法上下文区分关系限定与表达式限定，保留有序分段、每段 quoted/解码名称及原 SQL 跨度。关系一段交给当前数据库内的名称搜索，两段为显式 schema/关系，三段为数据库/schema/关系且数据库必须匹配连接数据库；不得删掉 foreign 前缀后落到同名本地表，也不得用后端 database 名直接接受为 PG schema。resolver 在执行前完成隔离、对象绑定及权限交接；即便跨库用户有后端权限也不能放开 PG 跨库访问。非 public schema 的目标绑定依赖工作流 6 的目录模型，尚未开放时以明确能力门禁 0A000 拒绝，不静默回退 public。CTE 遮蔽由下一块定义；search_path 的完整搜索、状态执行分别由后续名称块和工作流 5 定义，本块只固定查找责任，不展开实现。

下列为待新增 `NAME-QUALIFIED` 用例族，均无当前运行证据。PG 夹具为连接数据库 db、public.t(id=1)、路径包含 public、具备必要权限且无同名 CTE；other 数据库中的同名表取 id=2 以检测误读。成功目标 SQLSTATE 不适用（成功）、无错误位置；结果列类型/元数据按第 1 章另行采集。MySQL 8.4 使用独立夹具和自身权限，默认数据库 db 的 t(id=1)、数据库 public 的 t(id=2)，不能把 PG 的 public 自动映射过去。

| 输入/正反例 | 当前静态边界 | PG 目标与诊断责任 | MySQL 自身目标与诊断责任 |
| --- | --- | --- | --- |
| 正例 `SELECT id FROM t` | public 可搜索且当前库非空时改写到后端 db.t；无 public 时 rewrite 返回 42P01。 | 给定夹具返回 1；未限定名由 resolver 搜索，不硬编码总是 public；路径不含可见 t 时目标 42P01，完整路径规则另块定义。 | 绑定默认库 db.t，目标 1；不读取 PG search_path。 |
| 正例 `SELECT id FROM public.t` | 显式 public 可在 public=false 时改写 db.t；数据库为空返回 3D000。 | 绑定当前库显式 public.t，目标 1，不受 public 是否列入路径影响；对象不存在目标 42P01。 | public 是数据库限定，按独立夹具返回 2；无该库/表或无权限时独立采集自身诊断，不套 PG schema 语义。 |
| 正例 `SELECT id FROM db.public.t` | db 匹配当前库时改写 db.t；不是三个后端名称层次。 | db 必须是连接数据库，目标同 public.t 返回 1。数据库身份在绑定前核对，不能依文本删前缀通过。 | 三段 FROM 关系名是自身语法负例；MySQL 的三段列名 db.t.id 是另一上下文。错误码/位置待参考端采集。 |
| 跨库反例 `SELECT id FROM other.public.t`；另对比 `SELECT id FROM other.t` | 在当前库 db 下均拒绝 0A000。 | 前者拒绝，目标 0A000，不读 other 或本地同名表；后者是当前库 schema other 的关系，不是跨库语法：无该关系目标 42P01，合法但未开放的 schema 按能力门禁 0A000，不伪称 PG 禁止该 schema。 | 前者是三段关系名语法负例；other.t 可作为自身跨库正例，预建且授权时读取 other 库 t，结果与权限独立验收，不受 PG 门禁影响。 |
| 列/别名正例 `SELECT a.id FROM public.t AS a`；反例 `SELECT t.id FROM public.t AS a`；缺列 `SELECT a.missing FROM public.t AS a` | relation 只改 FROM 表；token_scopes 有别名保留意图，无列绑定断言。 | 正例返回 1；显式别名后原表名不再可引用，第二条目标 42P01；第三条目标 42703。binder 按查询作用域处理别名与列，不把 a 当 schema 去跨库查表。 | 用 `FROM db.t AS a` 作自身正例返回 1；原名/缺列负例按自身规则拒绝，诊断独立采集，不套 PG SQLSTATE。 |
| 形态反例 `SELECT * FROM public.`；多段负例 `SELECT * FROM db.public.t.extra` | 尾点缺名称返回 42601；多段关系返回 0A000。 | 不丢空段或截断多余段；前者目标 42601，后者精确 PG 诊断须参考端取证，不能由适配层 0A000 推定。 | 分别按自身关系语法拒绝；多段列引用与关系引用分开，精确错误码/位置待参考端取证。 |

错误责任：parser 负责分段合法性及上下文，resolver/binder 负责数据库隔离、可见关系/别名/列与权限，工作流 7 传递原诊断。所有拒绝必须在读写目标关系前完成，不能借转换后 AST 绕过检查。P 若提供须映射回原 SQL、从 1 起按字符计数；限定名长度变化、逐段引号和错误前中文均纳入待新增位置断言，不能使用改写后的字节偏移。上述目标 SQLSTATE 的精确触发、P 是否提供及数值、错误优先级、简单 Q/扩展 Parse 一致性和双方言交错连接隔离均待运行取证；无当前数据库的 3D000 是现适配边界，连接建立/切库契约由工作流 5、7 另证，不伪造 PG 同场景对照。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §5.10.1 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html#DDL-SCHEMAS-CREATE) 定义 schema.table 与只允许当前数据库的 database.schema.table；§5.10.3 将未限定关系交给搜索路径。§7.2.1.2 [表和列别名](https://www.postgresql.org/docs/18/queries-table-expressions.html#QUERIES-TABLE-ALIASES) 定义别名替代原关系名；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 0A000、42601、42P01、42703 分类，具体错误位置仍需参考端补证。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P；MySQL **8.4** §11.2.2 [Identifier Qualifiers](https://dev.mysql.com/doc/refman/8.4/en/identifier-qualifiers.html) 区分关系的 database.table 与列的 database.table.column。18.6 仅为来源目标，本轮没有该补丁版本或本地数据库实测。

#### 公共表表达式遮蔽规则

本块对应源工作流 2 步骤 3 的 CTE 名称责任，承接[引号与大小写规则](#引号与大小写规则)及[限定名称解析规则](#限定名称解析规则)。静态取证基于提交 `b5ae432d5a2c6682d13b9fe2b3813bd72c766482`：已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::scope` / `relation`、`rewrite` / `adapt`。scope 克隆继承的名称集合，在扫描每个定义体后才插入该 CTE 名，再向后续定义和主查询传递；括号递归使用克隆集合，内层新增名称不会泄漏出去。relation 只在单段名命中集合时保留引用；显式 public.t 仍改写到当前后端库的表，即使 public 不在路径中。WITH 后紧接 RECURSIVE 一律返回 0A000，包括没有实际自引用的输入；这只是当前入口门禁，不能称为 PG 的递归规则。

已读独立测试 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`：覆盖 `WITH t AS (SELECT 1) SELECT * FROM t` 在 public=false 时不改写、同名 CTE/public.t 区分及内层 CTE 不泄漏到外层；`pg_introspection_names_rejection_boundaries` 覆盖 RECURSIVE 的 0A000。未执行这些测试；现有断言没有证明嵌套同名的绑定对象、前向引用、自引用、重复定义或错误位置。已读 [ast/dml.rs](../pkg/parser/ast/dml.rs) 的基础 `TableName`，CTE 实际共享载体另定位到 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `WithClause`（IsRecursive、顺序 CTEs）、`CommonTableExpression`（Name、ColNameList、Query、IsRecursive）及 `WithClauseRef`；这些字段存在不等于 PG 绑定接线完整。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_dml_stmt` / `test_set_operator` 注册和 `run_contract_table_with_options`：接受性及顶层节点类型检查不能替代 CTE 作用域断言。`pkg/server/doc.go`、`pkg/parser/doc.go`、`pkg/parser/ast/doc.go` 不存在。

目标由 parser 保留定义顺序、查询块嵌套、递归标记、列名列表及名称的 quoted/规范身份和原 SQL 跨度；resolver/binder 拥有每个查询块的 CTE 名称空间与具体绑定。普通 WITH 的定义体只见先前同级定义及合法外层名称，主查询见本层全部定义；内层同名定义在其可见区域遮蔽外层 CTE，退出内层后恢复外层，不能用无绑定身份的字符串集合证明实现完成。未限定关系先查可见 CTE，再交关系搜索；显式 schema.t 绕过 CTE，按真实关系绑定。非递归定义自身尚未登记时，同名引用仍可绑定合法外层 CTE 或真实表，不能仅凭拼写相同认定递归或无条件拒绝。会话后续语句不保留 CTE 名称，CTE 不成为目录对象；search_path 状态执行及目录可见性分别交工作流 5、6。

PG 的 RECURSIVE 还允许无环前向引用，不等于每个 CTE 都自递归；自引用是否合法由递归项位置等结构约束决定，互递归不属于 PG 已实现能力。本块只登记解析/绑定入口责任，不开放递归执行；能力未开放时保持明确 0A000 门禁，并将参考 PG 的合法结果与 AsterSQL 暂时拒绝分别记录。物化、工作表迭代、优化、SEARCH/CYCLE 与数据修改型 CTE 的执行另行细分，不以本块补建完整 WITH RECURSIVE。

以下为待新增 `NAME-CTE` 用例族，全部运行行为未验证。PG 夹具为当前库 db、可见 public.t(v=9)、不存在关系 a/b/r，普通表搜索路径含 public；MySQL 8.4 独立夹具为默认库 db 的 t(v=9)，显式限定对照使用 db.t，PG 的 public.t 不直接充当 MySQL schema 语义。成功项 SQLSTATE 不适用（成功）、无错误位置；类型、列名及协议元数据按第 1 章另采集。

| 输入/正反例 | PG 目标与当前边界 | MySQL 自身目标 | 诊断责任与证据缺口 |
| --- | --- | --- | --- |
| 基本正例 `WITH t AS (SELECT 1) SELECT * FROM t` | 目标单行 1，遮蔽 public.t 的 9；现有静态测试仅断言 rewrite 保留，public=false 也应命中 CTE。 | 同一 SQL 目标 1，遮蔽默认库真实表。 | binder 绑定 CTE，不能先因搜索路径缺表拒绝；无执行/元数据证据。 |
| 嵌套同名正例 `WITH t AS (SELECT 1 AS v) SELECT t.v, q.v FROM t CROSS JOIN (WITH t AS (SELECT 2 AS v) SELECT v FROM t) AS q` | 目标一行 (1,2)，内层绑定内层 t，外层仍为外层 t。当前集合扫描保留同名引用，不能辨别绑定对象。 | 同一 SQL 目标 (1,2)，分别验证查询块的同名解析。 | binder 记录层级与对象身份；静态改写相同不证明遮蔽正确。 |
| 显式表正例 `WITH t AS (SELECT 1 AS v) SELECT v FROM public.t` | 目标 9；即使移除路径中的 public 仍指显式表，当前 relation 将它改写到 db.t。 | 使用 `WITH t AS (SELECT 1 AS v) SELECT v FROM db.t`，目标 9；db 是数据库限定。 | resolver 绕过 CTE，目录/权限交工作流 6；不存在真实表时 PG 目标 42P01，MySQL 自身诊断另采集。 |
| 顺序正例 `WITH a AS (SELECT 1 AS v), b AS (SELECT v FROM a) SELECT v FROM b`；前向反例 `WITH a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a` | 前者目标 1；后者在无真实 b 时拒绝，目标 42P01，不提前注册未来 b。当前扫描后插入次序吻合局部意图，但不会自行查目录；有真实同名 b 时应另验真实表绑定，不能一概拒绝。 | 前者目标 1；后者同样不可引用后定义的 b，无真实 b 时拒绝，精确码/位置待采集。 | binder 区分未来 CTE 与可见真实表；错误位置应关联定义体中的 b，P 是否提供及数值待证。 |
| 递归关键字边界 `WITH RECURSIVE t AS (SELECT 1) SELECT * FROM t`；无环前向 `WITH RECURSIVE a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a` | PG 合法，目标分别为 1、2；当前适配器两者均在 RECURSIVE 处拒绝 0A000。 | 第一条目标 1；第二条仍不能前向引用，无真实 b 时拒绝；不能复制 PG 放宽规则。 | parser/binder 区分关键字、依赖与实际递归；当前门禁与参考成功分别验收，MySQL 精确诊断待取证。 |
| 自引用边界 `WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<2) SELECT n FROM r ORDER BY n`；去掉 RECURSIVE 的反例 | 合法形式参考目标两行 1、2，当前入口拒绝 0A000；去掉关键字且无外层/真实 r 时目标 42P01，不自动升级递归。 | 合法形式目标 1、2；去掉关键字且无真实 r 时拒绝，官方示例给出 1146/42S02，仍需此输入实测。 | binder 负责自引用可见性，执行另交工作流 5；不据扫描通过宣称递归可用。 |
| 非法递归反例 `WITH RECURSIVE r(n) AS (SELECT n FROM r UNION ALL SELECT 1) SELECT * FROM r`；同层重复 `WITH t AS (SELECT 1), t AS (SELECT 2) SELECT * FROM t` | 第一条自引用位于非递归项，应拒绝，精确码待参考端取证；第二条应拒绝重复定义，目标 42712。当前分别为 RECURSIVE 门禁与集合去重，后者未显式检查重复。 | 两条分别按自身递归结构与名称唯一性规则拒绝；精确码/位置待采集。 | binder 负责结构及重复名诊断；不能让 HashSet 去重隐藏错误，也不能用 0A000 代替 PG 本来无效输入的真实诊断。 |
| 名称不泄漏反例：先执行基本正例，再单独执行 `SELECT * FROM t`；另将内层 WITH 放入派生表后在外层引用 t | 下一语句/外层均不得使用已结束的 CTE；给定真实表夹具应返回 9，而非 1，删除真实 t 的独立夹具下目标 42P01。现有测试只覆盖内层新增名不泄漏的文本。 | 独立连接/夹具同样绑定真实表 9；无表时按自身诊断拒绝。 | resolver 负责查询块生命周期；跨语句、简单 Q/扩展 Parse 及双方言交错连接均待新增实际断言。 |

诊断按 parser 的形态/跨度、binder 的可见性/定义顺序/重复与递归结构、工作流 7 的错误传递分责。P 如提供，须指向原 SQL，从 1 起按字符计数；定义体引用、第二个重复名及递归项的定位、引号转换和错误前中文均须独立取证，不能用改写后字节偏移。未列精确码的目标需先查官方对应源码或真实参考端，再补矩阵；当前拒绝不是未来功能成功，所有适用断言仍按第 1 章保留“未验证”。

官方依据（2026-10-23 查阅）：PostgreSQL **18** [SELECT 的 WITH/FROM](https://www.postgresql.org/docs/18/sql-select.html#SQL-WITH) 定义普通/递归 WITH 的同级引用顺序、真实表被遮蔽及显式 schema 绕过；§7.8 [WITH Queries](https://www.postgresql.org/docs/18/queries-with.html) 定义单查询作用域及递归项结构。嵌套同名按这些作用域规则制定上述目标，仍需参考端验证具体输入；[18 分支 parse_cte.c](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/parser/parse_cte.c) 是重复名与递归诊断的后续取证入口，分支可变，不作固定补丁实测证据。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42P01/42712/0A000 分类，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §15.2.20 [WITH](https://dev.mysql.com/doc/refman/8.4/en/with.html) 定义查询块搜索、同级仅向前序引用及自递归关键字要求。18.6 仍仅为来源指定目标，没有本轮补丁版本、AsterSQL、PG 或 MySQL 的实测结果。

#### 模式搜索路径解析契约

本块对应源工作流 2 步骤 3 的 schema/search_path 责任，承接[引号与大小写规则](#引号与大小写规则)、[限定名称解析规则](#限定名称解析规则)及[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)。静态证据基于提交 `d0e11420ba7c3e452418375ce494b5769bae811f`：已读 [pg_session.rs](../pkg/server/pg_session.rs) 的 `PgSession::has_public` / `schema` / `public_precedes_catalog` / `execute` 与 `SessionQuery::parse`。当前默认路径仅 public；has_public 只判断成员，schema 只取原始列表首项，public_precedes_catalog 仅在 public 与 pg_catalog 都显式出现且 public 在前时为真。parse 只接受这两个名称、无重复列表及空字符串路径，其他 schema（包括 missing）、重复项返回 0A000；这不是 PostgreSQL 完整路径模型，也没有证明首项是存在且有 USAGE 的有效 schema。

[pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::relation` / `adapt` 将未限定非 CTE 关系交给 has_public 门禁；public 缺席返回 42P01，显式 public.t 绕过该门禁并改写到当前后端库，非 public/跨库拒绝沿前块契约。[pg_catalog.rs](../pkg/server/pg_catalog.rs) 的 `CatalogQuery::parse_session_with_types` 在 public_precedes_catalog 为真时，通过当前库快照检查隐式目录关系是否被同名用户表遮蔽，再交给有界目录解析；`current_schema` 字段取自 session.schema()。目录行生成中 pg_namespace 仅映射当前库为 public，并提供 pg_catalog，不因路径排除而删除目录行。上述接线不是任意 schema 的 resolver，也不能由路径成员推导对象存在或用户可见。

独立测试只阅读、未执行：[pg_session_test.rs](../pkg/server/pg_session_test.rs) 的 `pg_introspection_namespace_parser_boundaries` 覆盖有界 SET/RESET/SHOW 接受与缺失 schema、重复名拒绝；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `pg_introspection_namespace_session_state` 有两连接不同路径、空路径 current_schema 为 NULL、隐式 catalog 可查询、RESET 和 Parse 后 SET 的 current_schema 断言；`pg_introspection_names_live_tables` 有路径仅 pg_catalog 时未限定用户表 42P01、显式 public 表仍读取的断言。未据此声称同名 catalog 完整优先级、权限过滤或缓存重绑定已通过。`pkg/server/doc.go` 不存在，本块源码和测试阅读均不替代当前运行证据。

目标由工作流 5 拥有连接私有的路径设置、SHOW、RESET、SET LOCAL 及事务恢复；本块只定义 resolver 消费状态的责任，不指定新类型或方法。交接信息须能区分原始设置与有效有序 schema、连接数据库/角色、名称身份与对象类别，并关联工作流 6 提供的真实对象、schema 存在性及权限可见性；不能用 has_public 或首项字符串取代目录查询。未限定关系先按前块查可见 CTE，再按有效关系搜索路径查首个匹配对象；显式 schema.t 绕过 CTE 和路径成员筛选，仍须执行目录存在性、权限及当前数据库隔离检查，失败不能静默回落 public。

PG 目标规则：省略 pg_catalog 时隐式优先于显式路径；显式列出则遵从所在位置。空显式路径不禁用隐式 catalog，普通 public 表不能因此可见；存在临时 schema 时，关系/类型还须按 PG 的临时 schema 优先级处理，不能将关系顺序直接用于函数/运算符。不存在或无 USAGE 的路径项按官方规则忽略，特殊项 "$user" 按当前用户解析；这些完整规则是工作流 5、6 的后续依赖，当前入口不支持任意 schema、用户路径或临时 schema 的部分须显式登记门禁 0A000，不能误称 PG 要求拒绝。对象创建默认 schema、函数/运算符重载及临时对象实现不在本块展开。current_schema 的目标是首个有效显式 schema（没有则 SQL NULL），不能因隐式 pg_catalog 总可搜索便返回 pg_catalog；SHOW 原值、有效路径及目录可见对象分别验收。

以下为待新增验收入口，所有运行行为均“未验证”。夹具：同一 PG 数据库仅有 public.t(id) 一行 1，未建临时对象，角色具有所需 USAGE/SELECT；目录同名场景另建 public.pg_namespace(nspname) 一行 'user_marker'，参考端 pg_catalog.pg_namespace 含 pg_catalog 行。MySQL 独立默认库 db 中准备 t(id)=1，仅按自身名称规则对照，不使用 PG 设置。

| 场景与正反例 | PG 目标结果与当前边界 | MySQL 适用性与目标 | SQLSTATE、定位及交接 |
| --- | --- | --- | --- |
| public 在路径：`SET search_path TO public` 后 `SELECT id FROM t`；反例改为不存在的 missing_t | 返回 1；隐式 catalog 仍先搜索。missing_t 目标 42P01，不能以路径含 public 作为存在证明。 | `SELECT id FROM t` 在 db 返回 1；PG SET 不适用。缺表按自身诊断取证。 | 成功不适用；缺表由 resolver 诊断原关系名，工作流 6 负责对象存在性。 |
| public 不在路径：`SET search_path TO pg_catalog` 后分别 `SELECT id FROM t`、`SELECT id FROM public.t` | 前者目标 42P01；后者仍返回 1。当前有对应有界测试意图，显式名不受路径成员限制不等于免权限检查。 | t 仍绑定默认库；对应显式写法为 db.t。public.t 在 MySQL 是数据库限定，不能直接当 PG schema 对照。 | resolver 对未找到的 t 定位，显式名按前块权限/目录交接；不得转而读取其他数据库。 |
| catalog 同名与顺序：分别设置 public、pg_catalog, public、public, pg_catalog，执行 `SELECT nspname FROM pg_namespace WHERE nspname = 'pg_catalog'`；再显式查 pg_catalog.pg_namespace 与 public.pg_namespace | 前两种隐式查询返回 pg_catalog 行；第三种绑定用户表返回零行。显式 catalog 查询总返回该行；显式 public 查询在此谓词下零行。顺序控制的是对象绑定，不能靠 UNION 两个对象模拟。 | PG 路径和目录 SQL 不适用；旁路 `SELECT id FROM db.t` 始终返回 1，不受 PG 连接设置影响。 | 各成功项 SQLSTATE/位置不适用；同名夹具、绑定身份、列元数据及隐式/显式一致性由工作流 6 协作验收，当前未验证。 |
| 空路径：`SET search_path TO ''` 后查询 t、public.t、pg_catalog.pg_namespace，以及 `SELECT current_schema()` | t 目标 42P01；public.t 返回 1；显式 catalog 可查，隐式 pg_namespace 也仍绑定 catalog；current_schema 目标 SQL NULL。空路径不等于禁用所有对象。 | PG SET/current_schema/catalog 不适用；旁路查询 db.t 返回 1。 | 缺表归 resolver；空路径设置和内省值归工作流 5，目录对象归工作流 6；其余成功无错误位置。 |
| 两 PG 会话 A=public、B=pg_catalog，交错查询 t/public.t，随后仅 A 改为空路径；MySQL M 同时查 db.t | 初始 A 的 t 返回 1，B 的 t 为 42P01，两者 public.t 都返回 1；A 改路径后仅 A 的未限定查找改变，B 状态保持。简单 Q 与扩展查询分别采证。 | M 始终返回 1；不向 M 注入 PG SET 语句并忽略错误。 | 工作流 5 拥有状态隔离，resolver 消费所属连接的有效状态；不能从 current_schema 的两连接测试推广为全链路绑定隔离。 |
| 能力边界反例：`SET search_path TO missing, public`、`SET search_path TO "$user", public` | PG 参考端按存在性/USAGE 展开或忽略相应项；当前入口返回 0A000，作为有界能力差异登记，不作为 PG 语法错误或未来成功证据。 | PG 路径语句不适用，MySQL 自身默认库逻辑独立。 | 本轮不猜造任意角色/权限组合结果；需工作流 5、6 的夹具和参考端证据后细分开放条件。 |

诊断责任分开：路径设置语法与拒绝归工作流 5，名称无匹配的 42P01 归 resolver，目录/权限诊断归工作流 6，错误传输归工作流 7。0A000 为当前有界能力门禁，不能替代 PG 合法输入的完整目标。P 如提供须指向原 SQL，从 1 起按字符计数；双引号、错误前中文和改写跨度均待独立取证，不从后端改写文本计算。Parse/Bind/Execute 间改变路径时，既有 current_schema 断言不能决定关系绑定时点、重新分析及缓存失效规则；须后续与工作流 5 单独取证再定义，不承诺所有已准备语句固定或立即重绑定。类型、OID、NULL 元数据、精确诊断位置及参考端结果均须按第 1 章补证。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §5.10.3/5.10.5 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html) 说明路径和系统目录；§19.11.1 [search_path](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-SEARCH-PATH) 给出隐式/显式 catalog、有效路径、临时 schema 及用户项规则；§9.27.1 [Session Information Functions](https://www.postgresql.org/docs/18/functions-info.html#FUNCTIONS-INFO-SESSION) 定义 current_schema/current_schemas。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42P01/0A000 分类，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 定义 P。MySQL **8.4** §11.2.2 [Identifier Qualifiers](https://dev.mysql.com/doc/refman/8.4/en/identifier-qualifiers.html) 支持上述默认库/数据库限定对照。18.6 仅为来源指定目标，本轮未运行该补丁版本、AsterSQL、PG 或 MySQL，不将大版本规则当作补丁实测。

#### 解析承载方案决策

本块对应源工作流 2 步骤 4，汇总此前六类语法和名称规则，决策为：**先做最小语法原型，再选择最终接入方式**。目前只确定共享规范执行内核、连接私有方言身份、原 SQL 保真及显式能力门禁；不认定共享 AST 扩展或独立受控 PG parser 已胜出。候选 B 的“独立”仅指词法/语法入口和必要的语法表示，不包含第二套 resolver、优化器、执行器或存储。现有文本适配保留为有界过渡，不作为第三条通用翻译路线。

证据基于提交 `a2d37b4477ba9feb4492d32c780a38b2560f64ff`，本轮仅阅读源码和独立测试。已核对 [yy_parser.rs](../pkg/parser/yy_parser.rs) 的 `Parser::ParseSQL` / `ParseOneStmt` / `yySetOffset`：Scanner/goyacc 决定接受和诊断，入口返回共享 Node，单语句入口检查数量，偏移写入不等于完整字符跨度映射。[parser.y](../pkg/parser/parser.y) 有 Go 语义动作的 `IlikeOrNotOp`、`ReturningClause`、`OnDuplicateKeyUpdate` 及 RowExpr 分支，不能作为 Rust PG 接线证明。[expressions.rs](../pkg/parser/ast/expressions.rs) 的 `PatternLikeOrIlikeExpr` 以 u8 保存 escape，`RowExpr` 只列 values；[dml.rs](../pkg/parser/ast/dml.rs) 是基础类型入口，其 `TableName`/恢复逻辑不能代替 PG 分段引号身份。实际共享 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `ExprKind::Cast` 使用 FieldType，`InsertStmt` 有 OnDuplicate 赋值列表和 Returning 投影，却不足以表达 PG 冲突目标及两类谓词；候选扩展仍须逐消费者取证。

[optimize.rs](../pkg/planner/optimize.rs) 的 `Optimize`、`OptimizeAstNode`、`OptimizeAstNodeNoCache` 接收 `resolve::NodeW` 及共享会话/InfoSchema；Optimize 区分 EXECUTE 与普通缓存路径。这是共享规划边界证据，不证明任意新 PG 语法可直接交给它，更不能通过绕开正常入口跳过只读准入、缓存或成功后效应。[pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::process` 对普通 Parse 依次执行名称/SQL 适配、`markers`、结果适配并调用 `prepare_statement`，会话/目录分支另行处理；普通参数 OID 缺省/为零时仍返回 0A000。这里没有已取证的接收 PG AST 的准备接口。`markers` 的重复/乱序参数映射也不是原查询诊断映射。

独立测试已读 [yy_parser_test.rs](../pkg/parser/yy_parser_test.rs) 的 `failure_returns_warnings_and_parser_reuse_clears_them`、`hint_uses_live_scanner_mode_and_position`（警告生命周期、scanner mode/位置），以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `marker_mapping_preserves_literals_comments_and_index_order`、`canonical_prepare_parameter_syntax_probe`（词法保护、参数顺序及共享准备入口的语法边界断言意图）。它们没有覆盖本块两条路线的端到端保真，且本轮未执行。`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/planner/doc.go`、`pkg/server/doc.go` 不存在；不把模块说明或历史通过记录当作当前行为证据。

| 比较维度 | 候选 A：扩展共享 AST / 现有解析链 | 候选 B：独立受控 PG 解析入口，再交共享内核 |
| --- | --- | --- |
| 接线成本 | 可复用 ParseSQL、Node 与既有访问链；仍需连接方言接入、Rust 规约动作及缺失字段，不能只改 parser.y。 | 需新增有限 PG 词法/语法及保真转换，并取证共享准备入口如何接收节点；当前 prepare_statement 只见 SQL，接口成本未知。 |
| 共享内核复用 | 载体较近，但每个新增字段仍须穿过 visitor、resolver、planner、Describe；字段存在不等于消费者支持。 | 必须转换到共享表示及正常规划入口；无法表达的语义先明确拒绝，不能建 PG 专用执行内核绕过。 |
| MySQL 回归面 | 修改共享词法、优先级、SQL mode 或 AST 恢复可能影响现有入口；ILIKE/RETURNING 已有扩展候选，须先记录真实 MySQL 基线，不能直接删去。 | 独立入口可隔离语法接受面，但共享表示/消费者与缓存仍可能回归；必须验证交错连接和重连，不能据入口独立即宣称隔离。 |
| 源码跨度/错误定位 | yySetOffset 是起点，仍需补原拼写、quoted、逐节点跨度和改写映射；现有 near/line 不能冒充 PG P。 | 可从原 SQL 记录跨度，但转换及内核诊断必须回溯原查询；字节与字符位置、多字节前缀及 EOF 都要验证。 |
| 参数绑定 | 复用现有准备路径需保留 `$n` 身份和出现顺序，不能让每个 `?` 变成独立 PG 参数；OID 推断仍为缺口。 | PG 参数节点可保留编号，但转换/准备元数据须证明重复绑定一致；不能用参数值拼 SQL 或暗中沿用文本替换。 |
| 维护成本 | 一套主要语法链，但双方言分支与字段语义须持续回归；受共享语法限制的成本尚未测量。 | 增加语法与转换层的双向维护、版本跟踪及拒绝集合；必须逐能力登记，不能把外部 parser 的接受范围当成支持集合。 |

六类语法的最小保真清单沿用对应小节，不重定语义：`::` 保留转换链、目标名/修饰及参数；ILIKE 保留操作类别、NOT、ESCAPE 出现性/表达式和优先级；RETURNING 保留三类 DML 的有序投影/别名及 Describe 交接；ON CONFLICT 保留省略/推断/约束目标、两种动作、两类谓词及 excluded 绑定；ARRAY 保留构造类别、维度嵌套与空列表；ROW 保留显式性、字段顺序/嵌套并区分分组和子查询。另须携带标识符分段、原拼写/引号、CTE 定义顺序和查询块作用域，供 resolver 消费；schema/search_path 的状态执行归工作流 5、目录可见性归工作流 6。不得把这些信息提前压成 MySQL SQL 后再声称仍然保真。

后续原型是待新建的独立任务，限定同一组原始 SQL、连接身份、编码、显式参数 OID 与参数编号作为输入，两候选使用相同用例清单。候选接口只描述数据交接，不命名新 API：A 从带连接身份的解析入口产生扩展共享节点；B 从受控 PG 入口产生保真语法表示，再转换为共享节点。两者均输出原 SQL/节点跨度、参数身份、诊断及共享消费者交接结果。先取证节点到 `resolve::NodeW` 和准备元数据的必要局部接线，不能假定现有 prepare_statement 接收 AST；本轮不实现此原型，也不生成接口定义。

下表是原型最小输入集，完整行为用例仍引用前面小节，所有树形、绑定、SQLSTATE 与位置均未实测。成功的语法项 SQLSTATE 为“不适用（成功）”，不代表可执行；涉及数据修改的输入只解析/描述，不发出写入。

| 输入、正反例 | PG 目标及责任 | MySQL 对照及证据边界 |
| --- | --- | --- |
| `SELECT ($2)::int4, ($1)::int4, ($2)::int4`，参数显式 int4；反例 `SELECT 1::` | 正例有三个独立转换节点及编号序列 2/1/2；反例目标 42601，parser 定位原输入末尾，工作流 7 传输 P。精确 P 待参考端采集。 | `::` 是隔离负例；自身正例用三个 CAST(? AS SIGNED)，绑定顺序 2/1/2。MySQL 精确诊断码/位置仍待取证。 |
| `SELECT 'Ab' ILIKE 'a%' ESCAPE '!'`；`SELECT ':: $1 ILIKE' /* $2 */` | 前者保留谓词与显式 ESCAPE；后者只有字符串和注释，不制造参数或转换。匹配值/locale 交工作流 4，成功无错误位置。 | 前者是目标隔离负例，但现有共享扩展基线须先核实；后者为自身字符串正例。不能用 LIKE 替换证明 PG 语义。 |
| `INSERT INTO t(id) VALUES ($1) ON CONFLICT (id) DO NOTHING RETURNING id`；反例尾部只有 RETURNING | 保留冲突目标、动作及投影；缺投影目标 42601，定位 EOF。绑定需后续 t 夹具，写入/返回行交工作流 5；合法未开放路径目标先于副作用拒绝 0A000。 | PG 子句为目标负例；自身 INSERT/ON DUPLICATE KEY UPDATE 用例单独验收，不认为动作等价。本地拒绝码/位置仍缺证。 |
| `SELECT ARRAY[1,2], ROW(1,2), (1)`；反例 `SELECT ROW(1,` | 区分数组、两字段行与标量分组；未闭合输入目标 42601，精确末尾位置待采集。数组/record 类型及编码不由原型补齐。 | ARRAY 构造不作为自身正例；ROW/分组按自身合法上下文检查，不能要求 MySQL 返回 PG 复合值。精确诊断待双方参考取证。 |

以上诊断输入再加入合法多字节字符串/注释前缀、带引号分段名和嵌套 WITH，核对跨度及作用域保真，不扩写执行功能。原型成功标准是：两路线分别给出这些输入的树形/拒绝、原 SQL 字符定位、参数映射及共享消费者最小交接证据，并记录修改面、不能承载字段和维护代价；双方言基线、并发连接身份及缓存隔离须有独立证据。可暂以明确门禁拒绝未开放合法语法，但拒绝本身不能算该语法交接成功。若必须丢信息、通用文本替换、用全局可变方言开关、执行写入获取 Describe 或补建第二套执行内核，该路线淘汰；若只是消费者尚未支持，应记录共同缺口和下一项局部取证，不能凭两路线均未编译/零测试选胜者。证据仍不足时保持未决，明确缺少哪个输入/交接结果，不递归补建子系统；原型完成后单独评审承载决策，再按能力细分实现。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2 [Value Expressions](https://www.postgresql.org/docs/18/sql-expressions.html) 的参数、转换、数组与行分类，以及 [INSERT](https://www.postgresql.org/docs/18/sql-insert.html) 的冲突/返回子句；协议 §54.2.3 [Extended Query](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-EXT-QUERY) 区分 Parse、Bind 与 Execute，§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 规定 P 为原查询从 1 起的字符位置；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分 42601 与 0A000。其他语法及 MySQL 规则沿用前面各块的官方来源和缺证说明。18.6 仅为来源指定目标，本轮没有该补丁版本或真实数据库的运行证据。

#### 章节验收与后续交接

本块累计审阅本章十二个前序文档块，对应来源任务四个步骤及完成条件；只确认文档契约可交接。前序交付可由提交 `a19b8cba62` 至 `00c684fb49` 的十二条文档提交和本章小节核对，不依赖已删除的编号任务文件。方案结论仍是先做最小原型，未选定最终承载方式；所有功能和双方言行为仍为未验证。

下表将来源步骤映射到小节、后续独立实施边界与验收入口。入口中的用例族均待新增或细化，沿对应小节的 PG/MySQL 正反例、夹具、目标结果与诊断责任登记，不代表现有通过测试；本表不重复 SQL，也不展开完整 parser 实施清单。

| 来源需求 | 本章小节及交接内容 | 后续独立实施块 | 验收入口与所需证据 |
| --- | --- | --- | --- |
| 步骤 1：连接私有身份链 | [方言身份传递](#方言身份传递)：listener→session→parser→resolver→planner→expression context | 先取证身份拥有权与关闭/重连，再逐交接点细分 | 待新增 DIALECT-IDENTITY 三类场景；简单/扩展查询、两 PG 连接与 MySQL 交错、缓存隔离均需当前证据，不能只比 SELECT 1 的值。 |
| 步骤 2：`::` | [类型转换语法边界](#类型转换语法边界)：转换链、类型名称与参数来源 | 单独取证转换节点保真，类型可行性交工作流 3 | 待新增该小节的优先级/嵌套、重复参数、缺类型与词法保护断言；区分树形、类型失败及原 SQL 位置。 |
| 步骤 2：ILIKE | [大小写匹配语法边界](#大小写匹配语法边界)：操作类别、否定与 ESCAPE | 单独细分匹配语法，求值交工作流 3、4 | 待新增该小节的谓词/NOT/优先级/ESCAPE 正反例；先采集 MySQL 既有扩展基线，不能将 LIKE 当等价翻译。 |
| 步骤 2：RETURNING | [返回子句语法边界](#返回子句语法边界)：三类 DML 的投影与 Describe 交接 | 单独细分投影保真与描述接线，副作用交工作流 5 | 待新增三类 DML、零行仍有列、空投影/错位拒绝；Describe 不写入，工作流 3、7 分别验证类型和元数据，0A000 门禁不算功能成功。 |
| 步骤 2：ON CONFLICT | [冲突子句语法边界](#冲突子句语法边界)：目标、动作、双谓词与 excluded | 单独细分冲突语法及绑定，执行交工作流 5 | 待新增 CONFLICT-SYNTAX；省略/推断/约束目标及反例逐项检查，不以 MySQL 冲突更新代替 PG 契约。 |
| 步骤 2：数组构造 | [数组构造语法边界](#数组构造语法边界)：构造类别、嵌套与空列表 | 单独细分数组树形，定型/形状与编码交工作流 3、7 | 待新增 ARRAY-SYNTAX；区分语法完整、空数组定型、混合类型及非矩形拒绝，不把数组改成行或 JSON。 |
| 步骤 2：行构造 | [行构造语法边界](#行构造语法边界)：显式/隐式行、标量分组与嵌套 | 单独细分行/分组保真，复合类型交工作流 3 | 待新增 ROW-SYNTAX；ROW(1,2)、(1,2)、(1)、ROW(1, 分别验收树形、定型/拒绝与执行边界，不强求 MySQL 可返回 PG record。 |
| 步骤 3：引号与大小写 | [引号与大小写规则](#引号与大小写规则)：原拼写、quoted 与绑定身份 | 单独细分名称保真与精确绑定，后端可区分性另取证 | 待新增 NAME-QUOTING；foo/Foo/"Foo"、双引号转义及缺对象反例，MySQL 配置分别记录，不能让 lowercase 合并对象。 |
| 步骤 3：限定名 | [限定名称解析规则](#限定名称解析规则)：关系分段、列/别名及当前数据库隔离 | 单独细分关系限定与列作用域，目录协作工作流 6 | 待新增 NAME-QUALIFIED；t/public.t/db.public.t、跨库拒绝及 a.id，不截断前缀后误读本地表。 |
| 步骤 3：CTE 遮蔽 | [公共表表达式遮蔽规则](#公共表表达式遮蔽规则)：定义顺序、查询块与绑定对象 | 单独细分普通 CTE 绑定与递归门禁，不补建递归执行 | 待新增 NAME-CTE；嵌套同名、显式表绕过、前向/自引用/重复名及退出作用域，字符串集合保留不等于绑定通过。 |
| 步骤 3：schema/search_path | [模式搜索路径解析契约](#模式搜索路径解析契约)：有效有序路径与显式名 | 单独细分 resolver 消费路径，状态/目录分别交工作流 5、6 | 待新增该小节的 public 在/不在、catalog 顺序、空路径与两会话隔离；绑定时点及缓存重分析另取参考端证据。 |
| 步骤 4：两条方案比较 | [解析承载方案决策](#解析承载方案决策)：共享 AST 扩展和受控 PG 入口 | 独立最小原型取证后复审，不新增第二套执行内核 | 待新增该小节的同输入候选对照；按六个维度审查树形、参数、原 SQL 跨度与共享消费者交接，缺证保持未决。 |
| 完成条件：双方言示例、错误位置及隔离 | 本表各小节及[方言身份传递](#方言身份传递)的交错连接用例 | 单独细分原 SQL 诊断与双方言回归；扩展加载继续排除 | 待新增中文前缀、引号/限定名改写、EOF 和 Q/扩展 Parse 的位置断言；PG SQLSTATE/P 与 MySQL 自身诊断分别采证，不能把改写后的字节偏移当字符位置。 |

本轮复核源码的基线为 `00c684fb49`：`pg_name.rs::Resolver::scope` / `relation`、`rewrite` / `adapt` / `validate_relations` 仍是有界名称扫描、文本改写及成功解析 AST 的跨库守卫；`PgSession` 只保存局部路径及失败事务状态，`has_public` / `schema` 不能替代有效路径和目录模型；`Extended::process` 的普通 Parse 仍先适配和 markers，再向 `prepare_statement` 传 SQL，未知参数 OID 返回 0A000。未见上述入口提供完整 PG 语法表示、全链身份或原 SQL 持久位置映射，不从这些局部事实推断所有其他入口均不支持。

本轮实际阅读独立测试 `pg_name_test.rs::pg_introspection_names_token_scopes` / `pg_introspection_names_rejection_boundaries`，以及 `pg_query_test.rs::pg_introspection_namespace_session_state` / `pg_introspection_names_live_tables`：前两者断言 rewrite 的文本/拒绝，后两者有两连接局部路径、current_schema 及 Q/扩展查询名称适配的断言意图。它们没有替代本表逐能力的树形、绑定对象、精确错误位置和 PG/MySQL 全链路隔离验收；本轮未执行这些测试。`pkg/server/doc.go` 不存在，模块说明只作导航。

交给工作流 3 的只读输入是本章及第 1 章的矩阵/证据/用例格式：解析结构须保留操作类别、有序表达式、转换目标、数组/行嵌套、参数编号及原 SQL 跨度；名称绑定须提供分段/引号身份、CTE/别名作用域、连接数据库及有效名称环境；类型阶段再确定类型名与重载、共同元素类型、空数组定型、行字段类型、参数推断、转换/NULL 和结果元数据，不由语法接受替代类型判定。本章未证明这些交接接口已存在，也不命名新 API；工作流 3 应逐能力补齐类型目标和未知诊断，不接管 parser 完整实现。

跨工作流责任保持独立：工作流 5 拥有 search_path 设置/事务恢复、DML 副作用与递归执行；工作流 6 拥有 schema/对象/权限可见性及目录身份；工作流 7 拥有参数/结果编码、Describe 协作、SQLSTATE/P 传输与协议恢复。本章只约束交接，函数/运算符求值归工作流 4，客户端共存证据汇入工作流 8。本轮不执行这些工作流，也不创建运行器、数据库环境、代码或原型。

累计文档验收条件是：本章恰有十三个四级块且五个责任索引可达；上述来源四步和完成条件无遗漏；每个语法/名称规则均能定位模块、目标、范围外、验收与单独细分入口；正反例分方言写明前提、目标、错误责任及缺证状态；两方案维持最小原型取证门禁，工作流 5/6/7 未被吞并。本轮完成仅表示这些文档条件满足。全链路身份/缓存隔离、语法树保真、名称绑定、类型/OID、执行与副作用、精确 SQLSTATE/P、真实客户端/双方言差分及性能均未验证；未知错误码、是否提供 P 及其数值先补官方来源或参考端取证，禁止猜造。

本块没有新增外部行为规则；累计目标沿用各小节的具体官方来源。2026-10-23 复核 PostgreSQL **18** §4.2 [Value Expressions](https://www.postgresql.org/docs/18/sql-expressions.html) 的语法/类型交接分类、§5.10 [Schemas](https://www.postgresql.org/docs/18/ddl-schemas.html) 的名称责任和 §54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的原查询字符位置要求。其他规则与 MySQL 8.4 对照保留各块的官方依据及缺证说明；18.6 仅为来源目标，没有该补丁版本实测证据。

### 范围外

[章节验收与后续交接](#章节验收与后续交接)不增加能力设计、代码、原型或完整 parser 实施清单，不执行工作流 3；工作流 5/6/7 的状态、目录和协议责任独立交接。

[解析承载方案决策](#解析承载方案决策)本轮不写原型、API、parser/AST 或执行代码，不增加依赖、平台、数据库环境或性能基准；会话、类型、目录、协议及执行实现继续由相应工作流独立细分。

[方言身份传递](#方言身份传递)本轮不实现方言类型、解析原型、共享 AST、缓存接线或运行测试平台；不扩建 resolver、planner、表达式子系统。六类语法、引号/大小写、限定名、CTE 及 schema/search_path 名称规则由后续文档块定义；会话设置执行归工作流 5，目录实现归工作流 6，错误传输与恢复协作工作流 7。现有文本适配只作为有界入口证据，不承担通用 SQL 翻译方案，也不据其推断完整 PG 支持。

[类型转换语法边界](#类型转换语法边界)不实现 PG parser、转换执行、类型推断或元数据；不扩建类型目录、数组/行子系统。转换语义归工作流 3，协议 OID、错误传输与恢复协作工作流 7。

[大小写匹配语法边界](#大小写匹配语法边界)不实现 parser/AST 接线、匹配算法、locale/collation、NULL、重载或错误响应；求值语义交工作流 3、4，协议诊断交工作流 7，不扩展其他匹配家族。

[返回子句语法边界](#返回子句语法边界)不实现解析器、共享 AST 接线、投影求值、事务或 wire 编码；真实写入/返回行交工作流 5，类型和结果描述协作工作流 3、7。ON CONFLICT 本身及 OLD/NEW、MERGE 等组合不由本块扩写实现。

[冲突子句语法边界](#冲突子句语法边界)不实现 upsert、唯一性/索引推断、锁、并发、更新求值和事务；执行与副作用交工作流 5，名称/类型协作本章后续规则及工作流 3，错误响应交工作流 7。

[数组构造语法边界](#数组构造语法边界)不实现数组类型、下标、运算符、存储或 wire codec；ARRAY(子查询) 是另一个合法构造形式，留待单独定义支持集合，不能当作本块已覆盖。类型、转换及 OID 归工作流 3，参数/结果编码归工作流 7。

[行构造语法边界](#行构造语法边界)不实现 parser/AST、复合类型、行比较、NULL 求值或协议编码，不补建关系分组解析；类型与转换交工作流 3，比较/NULL 交工作流 3、4，元数据和错误响应交工作流 7。

[引号与大小写规则](#引号与大小写规则)不实现词法器、名称绑定、存储大小写映射或错误位置接线，不扩建目录、CTE 或 search_path；后端名称区分能力作为独立缺口交接，不以文本改写代替。

[限定名称解析规则](#限定名称解析规则)不实现 parser/AST、名称绑定、权限、跨库访问或完整 schema 模型；CTE/search_path 交后续块，会话状态和目录实现分别归工作流 5、6，协议诊断归工作流 7。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)不实现解析/绑定、递归执行、物化或优化，不扩建目录及 search_path；只固定名称可见性与入口门禁责任，执行协作工作流 5、目录协作工作流 6、诊断协作工作流 7。

[模式搜索路径解析契约](#模式搜索路径解析契约)不实现路径状态、RESET、事务恢复、目录对象或权限可见性；工作流 5、6 分别主责。只约束 resolver 消费顺序、隐式 catalog、空路径和限定名的交接，不承诺任意 schema 已可用。

### 验收

[章节验收与后续交接](#章节验收与后续交接)按本块累计条件检查十三个文档块、来源映射和双方言诊断责任；源码/测试阅读及文档完成均不能升级为功能通过。

[解析承载方案决策](#解析承载方案决策)的文档验收为：六个比较维度均有源码依据及缺证限制，六类语法信息齐全；已选择先做最小原型，输入、候选数据交接、成功/淘汰标准和后续任务边界明确。PG/MySQL 正反例、SQLSTATE/原 SQL 定位责任与未验证状态分开记录；不以候选节点存在或静态测试阅读声称方案已可用。

[方言身份传递](#方言身份传递)的文档通过条件：六段责任链均区分实际入口、状态拥有者/生命周期、目标与缺口；简单/扩展查询身份一致；PG/MySQL 同时连接及两 PG 连接交错场景可单独拆分，负例、错误责任与证据状态明确；局部测试阅读不升级为全链路支持。当前仅文档契约交付；parser 内部接线、共享名称绑定、规划/缓存、表达式上下文、错误位置及真实双方言隔离行为全部未验证，实施后须按第 1 章采集有效证据。

[类型转换语法边界](#类型转换语法边界)的文档验收为：上述输入形态、优先级/嵌套、参数位置、缺类型反例及词法保护均有双方言目标、诊断责任和证据状态；AST 候选与保真缺口明确。现有测试阅读不替代新增树形/位置断言及实际双方言对照，所有行为仍未验证。

[大小写匹配语法边界](#大小写匹配语法边界)的文档验收为：操作符类别、左右操作数、否定、ESCAPE 显式性与保真、优先级及不完整输入均有双方言目标和诊断责任；当前源码、测试意图与运行证据区分清楚。全部运行行为仍未验证，不能以共享语法或同值结果代替 PG 树形与双方言隔离证据。

[返回子句语法边界](#返回子句语法边界)的文档验收为：三种 DML、表达式/星号/别名、空投影与错位反例、零行和词法保护均有双方言目标与诊断责任；解析树、返回列元数据、真实副作用分开验收。现有非空 Returning 的 0A000 门禁不得被当作功能成功，全部运行行为未验证。

[冲突子句语法边界](#冲突子句语法边界)的文档验收为：目标的三种形态、两种动作、两类谓词、excluded 与目标别名、缺动作/缺目标反例均有保真和诊断责任；MySQL 对照、参数/词法保护及执行交接明确。所有运行行为未验证，不能将普通 INSERT 测试或文本改写当作冲突行为通过。

[数组构造语法边界](#数组构造语法边界)的文档验收为：基本构造、两种嵌套写法、空数组定型、混合类型、非矩形与未闭合括号分别有树形、语义交接和诊断责任；PG/MySQL 正反例、参数/词法保护及证据限制齐全。文档完成不表示数组可解析、可绑定或可编码，所有运行行为未验证。

[行构造语法边界](#行构造语法边界)的文档验收为：ROW(1,2)、(1,2)、(1)、ROW(1, 四个核心输入分别区分语法合法、类型可判定与可执行；单字段 ROW 对照、顺序/嵌套、参数、关系括号及词法保护均有双方言责任和证据限制。当前全部运行行为未验证，不以解析或名称扫描成功替代语义兼容。

[引号与大小写规则](#引号与大小写规则)的文档验收为：foo、Foo、"Foo"、"a""b"、未闭合/空引号及字符串/注释均有双方言正反例、前提和诊断责任；词法键与后端存储、原始与转换位置区分明确。现有适配和测试不能证明完整名称解析，运行能力全部未验证。

[限定名称解析规则](#限定名称解析规则)的文档验收为：t、public.t、db.public.t、other.public.t 和 a.id 均区分当前改写/拒绝与 PG/MySQL 目标；别名隐藏、缺列、残缺/多段名有诊断责任及缺证说明；隔离检查不被文本降级绕过。所有运行行为仍未验证，测试阅读不构成通过。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)的文档验收为：基本 WITH、嵌套同名、public.t、定义顺序/前向、自引用及递归结构反例均有双方言目标；作用域生命周期、当前 RECURSIVE 拒绝和未验证项明确。当前测试仅为静态证据，所有解析、绑定、结果及诊断行为均未实测。

[模式搜索路径解析契约](#模式搜索路径解析契约)的文档验收为：public 在/不在路径、显式 public.t、catalog 同名及顺序、空路径、两 PG 会话和 MySQL 旁路均有目标及诊断责任；原始/有效路径、当前布尔门禁与完整目标区分清楚。全部运行行为仍未验证，状态/目录阅读不升级为绑定或兼容通过。

### 后续细分

[章节验收与后续交接](#章节验收与后续交接)将本章与第 1 章证据契约交工作流 3；实现先核实连接身份和最小原型，再按单一语法或名称规则重排依赖，逐项补运行证据。

[解析承载方案决策](#解析承载方案决策)后续先单独拆两候选最小原型取证和决策复审，再按语法保真、共享准备/规划接线、原 SQL 诊断、参数身份及双方言隔离分别细分；每项限定输入与消费者边界。共同类型/执行/目录缺口另行交接，禁止借原型一次补齐全部 PG parser。

[方言身份传递](#方言身份传递)后续先做连接身份拥有权及关闭/重连边界的局部取证，再分别拆简单查询与扩展查询的身份承接、共享解析/名称绑定/规划/表达式上下文的必要局部接线及隔离回归；实施前确认实际落点，不将本责任链自动扩张为完整子系统重建。各任务明确单一交接点、所需状态、现有可复用实现和验收断言；DIALECT-IDENTITY 三类场景是待新增验收入口，不是既有测试或已登记通过的能力。

[类型转换语法边界](#类型转换语法边界)后续先拆最小 `::` 解析承载取证，比较现有 Cast 与受控入口能否保留上述字段；再按优先级/嵌套、参数位置与词法保护、原 SQL 错误定位分别细分。每项复用现有 parser 与独立测试布局，只补必要局部接线；将转换语义及参数类型推断交工作流 3，不在此建立完整类型系统。

[大小写匹配语法边界](#大小写匹配语法边界)后续先核对 Rust parser 到现有载体的最小接线及 MySQL 扩展基线，比较共享 AST 与受控 PG 入口的保真成本；再分别拆谓词/NOT/比较组合、ESCAPE 信息与词法保护、原 SQL 诊断及隔离验收。沿现有独立测试布局补专项树形/位置断言，不递归修建执行子系统；匹配算法和转义求值另交工作流 3、4。

[返回子句语法边界](#返回子句语法边界)后续先按三种 DML 核对 Rust 解析动作到共享载体的最小保真，再拆表达式/星号/别名与原 SQL 位置、planner/Describe 列交接、双方言拒绝隔离。功能开放须等待工作流 5 的真实副作用/回滚和工作流 7 的结果描述证据，不递归补建执行或协议子系统。

[冲突子句语法边界](#冲突子句语法边界)后续先比较共享 AST 与受控 PG 入口保真，再分别拆目标/动作与双谓词、excluded 绑定、原 SQL 跨度与参数映射、双方言隔离的独立验收。先补解析树和拒绝证据；真实唯一性、并发及更新条件效果由工作流 5 单独细分，不递归扩建执行子系统。

[数组构造语法边界](#数组构造语法边界)后续先比较共享 AST 与受控入口保留构造类别、嵌套及跨度的最小方案，再拆基本/嵌套/空列表树形、原 SQL 定位、参数身份与双方言隔离验收；共同类型、形状语义及数组 OID 交工作流 3，Bind/Describe/结果编码交工作流 7 单独细分，不递归补建数组子系统。

[行构造语法边界](#行构造语法边界)后续先比较共享 Row 载体与受控 PG 入口保留显式性、嵌套和原 SQL 跨度的最小接线，再分别拆行/分组/子查询树形、名称扫描交互、错误位置及双方言隔离断言。沿独立测试布局取证，不递归扩建表达式或关系子系统；行类型与 NULL/比较按工作流 3、4 单独细分。

[引号与大小写规则](#引号与大小写规则)后续先取证共享名称载体的 quoted/拼写/跨度保真，再分别拆折叠与精确绑定、双引号转义及词法保护、位置映射和双方言隔离；后端同名异大小写的可区分性单独评估，禁止递归补建存储或目录子系统。

[限定名称解析规则](#限定名称解析规则)后续先取证关系分段与列限定的上下文保真及当前数据库身份，再分别拆关系限定/跨库拒绝、列与别名作用域、原 SQL 位置和双方言隔离断言；沿现有独立测试补 adapt/guard 的必要覆盖，不扩建目录或权限子系统。

[公共表表达式遮蔽规则](#公共表表达式遮蔽规则)后续先核对顺序/嵌套/名称身份与跨度的保真，再分别拆普通 CTE 绑定与显式表绕过、前向及重复名诊断、查询块退出/跨语句与双方言隔离断言；递归入口依赖图与门禁另拆，不借名称任务补齐递归执行或物化子系统。

[模式搜索路径解析契约](#模式搜索路径解析契约)后续先核对状态到 resolver 的有效路径交接，再拆 public/显式限定、catalog 同名顺序与空路径、两会话及双方言隔离的独立验收；关系绑定时点与缓存失效先做参考端取证。任意 schema、权限、临时项及用户项分别交工作流 5、6 细分，不递归补建目录或会话子系统。

## 3. 共享类型与转换契约

### 模块

[规范类型与协议映射](#规范类型与协议映射)复用共享内核；`pkg/types/field_type.rs::FieldType` 是 parser 类型别名，`AggFieldType` 保留 MySQL 混合符号整数提升规则，不以 PG 目标改写 Go 既有语义。`pkg/server/conn.rs::NativeType` 的 code/flags/length/decimal、`QueryResult` 与 `PreparedMetadata` 承接原始类型；`runtime.rs` 的 Prepare 分支及 `result_metadata` 从结果字段提取这些信息，`protocol_result_column` 另产 MySQL 列描述。PG 出口为 `pg_result.rs::pg_type` / `encode_formats` / `value_text` / `value_binary`；入口为 `pg_extended.rs::parameter` / `binary_parameter`。这些是现有符号，不是新建接口。第 2 章交来的转换目标、参数编号、数组结构及名称身份须先定型；本块不替 parser 或 resolver 决定语法与绑定。

本块静态取证基线为 `69ef39f91a781a4147568622943d719c95e3f7cd`，日期 2026-10-23。已阅读上述符号及独立测试 `pkg/server/pg_types_test.rs` 的 `common_type_oids_and_values`、`native_metadata_bridge_preserves_boolean_and_mysql_columns`、`unsigned_binary_and_unsupported_types`、`missing_original_metadata_is_explicitly_unsupported`、`catalog_char_and_acl_wire_types`、`pg_binary_native_results`，以及 `pkg/types/field_type_test.rs::TestFieldType` 的宽度、精度和 flags 断言。前者覆盖意图包括同值异类型、unsigned 最大值、bytea、JSON/零日期/duration 拒绝及缺元数据；目录 char/ACL 断言不能证明通用数组支持。目标包没有 `doc.go`。全部测试未运行，源码阅读不是当前功能通过证据。

[布尔类型契约](#布尔类型契约)沿用上述桥接，补充取证 `pkg/types/datum.rs` 的 Kind 标签和 `Datum::ToBool`（MySQL 风格真值转换，不能直接作为 PG 隐式转换），以及 `pg_extended.rs::parameter` / `binary_parameter` / `Extended::process`、`pg_result.rs::pg_type` / `value_text` / `value_binary`。静态基线为 `9b3d77f784`（2026-10-23），目标包无 `doc.go`；独立测试 `pg_types_test.rs::common_type_oids_and_values` 断言 TRUE/FALSE 的 OID 16 与 t/f，`native_metadata_bridge_preserves_boolean_and_mysql_columns` 断言同值整数与布尔的 flags 区别，`pg_binary_native_results` 含 false→00；`pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 只含 true 成功和 unknown 拒绝的布尔参数断言；`pkg/types/datum_test.rs::test_to_bool` 是共享真值转换意图，不证明 PG 转换规则。以上测试均未运行。

[整数与无符号边界](#整数与无符号边界)取证基线为 `01ce26d145`。实际读取 `pkg/server/pg_result.rs::pg_type/value_text/value_binary`、`pkg/server/pg_extended.rs::parameter/binary_parameter/Extended::process`、`pkg/types/datum.rs::KindInt64/KindUint64/Datum` 与 `pkg/types/convert.rs::IntegerUnsignedUpperBound/IntegerSignedUpperBound/IntegerSignedLowerBound/ConvertIntToInt/ConvertUintToInt/ConvertIntToUint`。两个包均无 doc.go。独立测试已读 `pkg/server/pg_types_test.rs::unsigned_binary_and_unsupported_types/pg_binary_native_results/common_type_oids_and_values`、`pkg/server/pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 和 `pkg/types/convert_test.rs::test_convert`；它们是断言意图，均未运行。Datum 区分 i64/u64 承载，但不能单靠值盒确定 int2/int4/int8 声明身份。

[精确数值契约](#精确数值契约)取证基线为 `7fe5fefab8`（本轮只改文档）：[mydecimal.rs](../pkg/types/mydecimal.rs) 的 MyDecimal、fit_unscaled、FromString、Round、ToBin/FromBin，与独立 [mydecimal_test.rs](../pkg/types/mydecimal_test.rs) 的 TestRoundWithHalfEven、TestRoundWithTruncate、TestToBinFromBin；HalfEven 名称实际调用 ModeHalfUp，不能按名字推定银行家舍入。MyDecimal 以 9 个 base-10^9 word 分别容纳整数和小数，容量按两部分各自向上取整占 word，不能简单承诺任意 81 位组合；BigInt 是运算中间量，不表示存储域无限。MAX_DECIMAL_SCALE=30 用于特定编码/运算限制，不是所有方法统一的 PG scale 上限。

cast 另见 [builtin_cast.rs](../pkg/expression/builtin_cast.rs) 的 to_decimal、parse_decimal、produce_decimal：使用 rust_decimal::Decimal，解析失败可经浮点回退，负 scale 被 max(0)，超域产生 warning 并裁剪；常量 MAX_DECIMAL_WIDTH=65 不能证明实际能精确承载 65 位。参数见 [pg_extended.rs](../pkg/server/pg_extended.rs) 的 parameter/binary_parameter；结果见 [pg_result.rs](../pkg/server/pg_result.rs) 的 pg_type、value_text、value_binary、encode_formats。独立 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 text_parameters_are_typed_and_reject_loss 仅含 12.340 接受及 12bad 拒绝意图；[pg_types_test.rs](../pkg/server/pg_types_test.rs) 有 DECIMAL(8,2)→OID 1700/text 12.34 断言及 U64 文本入口。均未运行，不证明 PG 大精度、typmod、特殊值或 binary 支持；上述三个包均无 doc.go。

[浮点数值契约](#浮点数值契约)静态取证基线为 `cae627a7dc`（2026-10-23，批次 4 精确数值契约已交付）。`pkg/types/datum.rs::Datum` 的 GetFloat32/SetFloat32、GetFloat64/SetFloat64 分别保留 KindFloat32/KindFloat64；SetFloat32FromF64 可将未窄化 f64 写入 float32 标签，标签本身不能证明值已符合 float4 域。`pkg/types/convert.rs::format_float` 使用 ryu 最短格式并展开指数，不是 PG 输出格式的证据。PG 边界仍为 `pg_extended.rs::parameter/binary_parameter` 和 `pg_result.rs::pg_type/value_text/value_binary/encode_formats`：700/701 参数分别解析 f32/f64，再转共享参数小端编码；binary 输入从网络序转小端。结果 Value::Float 共用 f64，text 按 f64 输出，binary 先经文本再解析到目标宽度，尚无完整窄化溢出和 float4 文本定宽证据。

已读独立测试 `pkg/server/pg_types_test.rs::pg_binary_native_results`，浮点部分含 float4 1.5、float8 正无穷及两者 SQL NULL 长度 -1 的 binary 输出断言；`common_type_oids_and_values` 没有浮点场景。另读 `pkg/server/pg_extended_test.rs::text_parameters_are_typed_and_reject_loss` 与 `pkg/types/convert_test.rs::test_convert_to_string`，后者有 1.6_f32/-0.6_f64 的共享输出意图，不能替代 PG 可逆文本验收。两个目标包无 doc.go；所有测试均未运行，未见上述断言覆盖完整浮点边界，不把静态路径或 OID 视为功能通过。

[文本类型契约](#文本类型契约)静态基线为 `e3c4a89ce7`（批次 5 已交付）。已读 [field_type.rs](../pkg/types/field_type.rs) 的 InferParamTypeFromDatum/InferParamTypeFromUnderlyingValue：字符串推断补 MySQL charset/collation，可变长度设 UnspecifiedLength；[datum.rs](../pkg/types/datum.rs) 的 SetString/GetValue 保留 KindString 和 collation，SetBytes 另设 KindBytes。这些信息不能单独区分 PG text/varchar/bpchar 身份或声明长度。目标包 pkg/types、pkg/server 均无 doc.go。

PG 入口 [pg_extended.rs](../pkg/server/pg_extended.rs)::parameter 先校验 UTF-8/NUL，再将 25/1043/1042 同转 tp=253；binary_parameter 对三者复用 parameter。NULL 提前返回 tp=6，typed NULL 连续性缺证。出口 [pg_result.rs](../pkg/server/pg_result.rs)::pg_type 将普通非 binary 字符串映射为 25，charset=63 则为 bytea；value_text 拒绝 NUL，value_binary 的字符串分支发送文本字节，encode_formats 固定 typmod=-1。不能从此宣称已保留 varchar/char 长度、补空格或跨格式身份。

已读独立 [pg_types_test.rs](../pkg/server/pg_types_test.rs)::common_type_oids_and_values 的 hello→25/NULL 断言、catalog_char_and_acl_wire_types 的受控目录 1042 断言，以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs)::parse_bind_execute_sync 的 VARCHAR(30) 表入口。目录特例不证明普通 char 支持，上述片段不覆盖本块完整边界；所有测试未运行。

### 目标行为

#### 规范类型与协议映射

本块建立方向契约，不把 MySQL 类型名、PG OID 和规范引擎类型视为同一身份。这里“规范类型”指执行与准备阶段保留的类型身份、修饰符和值域约束，并不声称现有 `NativeType` 已完整表达所有 PG 类型。值相同仍可属于 bool、int2、int4、int8 或 numeric；类型描述必须先于行内容确定。双向映射允许提升或拒绝，不承诺可逆。

方向与共同约束如下；下表 NULL、错误和元数据规则适用于后续每个类型族。

| 方向 | 类型身份、值域与精度 | NULL 与错误 | wire 元数据、现状与责任 |
| --- | --- | --- | --- |
| PG 参数 → 规范值 | 先按 Parse 声明 OID、SQL 目标和上下文推断确定参数身份，再按 Bind 格式解码；OID 0 表示尚未指定，不能当 text 或零值。保留参数序号与重复引用的同一身份，检查目标值域/精度。 | 参数长度 -1 是 SQL NULL，不解码值但仍校验类型；空载荷不是 NULL。类型无法确定须拒绝，非法文本/二进制与溢出分开。 | ParameterDescription 返回确定的 OID，Bind 格式 0/1 只决定编码，不决定类型；没有参数 typmod 字段，SQL 目标约束仍须保留。现有 parameter 将若干 OID 转成 BinaryParam，NULL 仅留下 tp=6；binary_parameter 支持集合更小。类型保真由本章主责，消息与恢复交工作流 7。 |
| MySQL 参数 → 规范值 | 按自身参数类型及 unsigned 等标志解码；这些输入不是 PG OID。保留原 MySQL 域，不从 PG 反向猜测 unsigned 或 bool。 | SQL NULL 保留空值及独立类型信息；错误遵循 MySQL 原契约，不能套 PG SQLSTATE。 | conn/conn_stmt 的既有 MySQL 参数边界继续拥有 wire 解释；本块只约束共享桥接不得丢失信息，未验证全链参数保真。 |
| 解析/推导 → 规范类型 | 消费第 2 章保留的类型名、修饰符、数组结构及上下文；共同类型和 cast 决策先按方言确定，再复用 FieldType 能保真的部分。 | 有类型 NULL 沿用声明；无类型字面量/参数依 PG 上下文解析，不能一律映射 code=6 的 text 回退。缺身份拒绝，不读首行补全。 | Prepare 与执行输出应携带同一类型契约；现有 PreparedMetadata 只有结果 NativeType 与 parameter_count，未证明参数身份和所有 PG 修饰符保真。类型推导属本章，运算重载协作工作流 4。 |
| 规范类型/值 → PG 结果 | 先生成每列 OID、typlen、typmod，再编码每个值；同列不因数值大小、首行/后续行、NULL 或格式而改变类型。 | SQL NULL 的 DataRow 长度 -1、无载荷，仍有列类型；未知类型即使无行或全 NULL 也拒绝。值域不符不得截断、转 text 或改 OID 掩盖错误。 | pg_type 已按元数据选 OID；encode_formats 校验列/native 数量，当前 typmod 恒 -1，来源表/属性号为 0。本章给类型约束，工作流 7 负责 Describe/Execute 与错误传输及恢复。 |
| 规范类型/值 → MySQL 结果 | 保持 MySQL code、flags、charset、length、decimals 的原契约；PG 类型无保真对应时须有显式转换或拒绝。 | NULL 不改变列定义；不因 PG 映射扩大或缩减 MySQL 原域。 | ColumnInfo 与现有 MySQL 编码继续拥有 wire 描述；PG OID/typlen 不写入 MySQL code/length。PG 专属类型的 MySQL 暴露策略未取证，不能默认为字符串。 |

下表是**目标方向映射**，括号内为当前 PG 出口的局部事实；MySQL 一列为共享已有类型的承载约束，不宣称 PG 专属类型已有 MySQL 等价物。U 表示 UnsignedFlag，B 表示 IsBooleanFlag；字符集二进制身份须单独保留，不能用值是否可打印决定。每行 PG 三元组是 OID/typlen/typmod；typlen 是 PG 类型内部长度标识，既非实际 wire 载荷长度，也非 MySQL 显示宽度。无修饰符用 -1；有修饰符必须来自定型结果。

| 规范身份/保真要求 | MySQL wire 方向（输入/输出） | PG 方向（输入身份 → 结果三元组） | 损失、拒绝与当前缺口 |
| --- | --- | --- | --- |
| boolean，独立于整数 0/1 | 保留内核 B；既有 MySQL 列可与整数相同，不强造 wire bool 身份 | bool 16 → 16/1/-1 | 仅凭 TINYINT 或值 1 不能判 bool；现有 pg_type 优先 B，参数 bool 到 tp=1 后的身份连续性未证明。 |
| 有符号整数，精确位宽 | TINY=1、SHORT=2、LONG=3、INT24=9、LONGLONG=8，U 清除；不要把显示宽度当位数 | int2 21、int4 23、int8 20 → 分别 21/2/-1、23/4/-1、20/8/-1 | PG 域为有符号 16/32/64 位。现有 tiny→int2、short→int2、long/int24→int4、longlong→int8；PG 声明 int2 输入不能因值小变 tiny 身份。越界拒绝。 |
| 无符号整数，完整原域 | 同上 code + U，值不能先窄化到 i64 | 无 PG unsigned 输入类型；共享结果 U8/U16/U24/U32/U64 → int2/int4/int4/int8/numeric | 对应结果 21/2/-1、23/4/-1、23/4/-1、20/8/-1、1700/-1/-1；最大 u64 必须精确保留 18446744073709551615。当前有 tiny/short/long/longlong 提升分支；不从结果 numeric 反推出 unsigned，反向须有声明及范围校验。 |
| exact numeric，精度/有符号 scale | DECIMAL=0 或 NEWDECIMAL=246，length/decimal 只在语义相容时复用 | numeric 1700 → 1700/-1/定型 typmod；无约束为 -1 | 不经 f64；不能把 MySQL decimal 上限当 PG 上限。当前文本参数用 MyDecimal 校验，binary 参数未列入支持分支；结果 typmod 丢失。负 scale、特殊值与超出共享表示的域须独立取证，不能靠映射宣称覆盖。 |
| float4 / float8，各自精度 | FLOAT=4 / DOUBLE=5，保留位宽 | 700 → 700/4/-1；701 → 701/8/-1 | 不能以当前值可表示为由降精度；NaN、无穷、负零及溢出按族契约取证。现有参数/结果有分支，不代表特殊值行为已验收。 |
| text / varchar(n) / bpchar(n)，保留字符长度与身份 | VARCHAR=15、VAR_STRING=253、STRING=254 及文本 blob 家族，沿自身 charset/flags；PG char 不能等同内部目录 char | 25 → 25/-1/-1；1043、1042 → 各自 OID/-1/(n+4)，无约束 -1 | PG 长度为字符约束，不取 MySQL wire 字节宽度直接代入。当前普通文本出口折为 text；入口 25/1043/1042 同转 tp=253，声明身份/补空格语义未证明。非法编码和不允许的 NUL 拒绝。 |
| bytea，原始字节 | 二进制 charset 与 blob/string 家族；不能与同载荷 text 合并 | 17 → 17/-1/-1 | 可含零字节；文本转义与 binary 载荷分开验收。现有参数文本只收 hex，binary 可收原字节；结果按 charset=63 选 bytea，此信息不在 NativeType 内，桥接仍依赖 ColumnInfo。 |
| date，日历日期 | DATE=10 / NEWDATE=14，不能用零日期冒充 PG 日期 | 1082 → 1082/4/-1 | 共享范围之外及 MySQL 零日期须明确拒绝；现有结果有映射，参数 temporal 限制引擎日期域，不证明 PG 完整日期域或 infinity。 |
| time，日内时间，秒小数精度 | TIME=11 的 duration 域须与日内时间区分，非无条件双向 | 1083 → 1083/8/p；未声明 p 为 -1 | p 为 0–6。负 duration、25:00:00 不能无损映射日内时间，拒绝或走显式转换；24:00 边界独立取证。当前 code 11 选 time，不证明域已相同。 |
| timestamp，无时区墙钟时间 | DATETIME=12 / TIMESTAMP=7 不能仅凭名称判为 PG 瞬间 | 1114 → 1114/8/p；未声明 -1 | 当前 7/12 都映射 1114；精度、零日期及范围须校验，不能添造时区。MySQL TIMESTAMP 的会话处理不能证明 PG timestamp 等价。 |
| timestamptz，瞬间身份 | 现有 MySQL code 无独立保真对应；须明确转换与会话前提，禁止自动借 7 | 1184 → 1184/8/p；未声明 -1 | 不要求保留原输入时区拼写，但必须保留瞬间，显示依会话时区；现有 NativeType 四字段及 pg_type 没有此身份分支。缺信息拒绝，不冒充 1114。 |
| interval，月/日/时间分量 | 无直接 MySQL duration 等价；须显式转换或拒绝 | 1186 → 1186/16/字段掩码及 p 的类型修饰符；无约束 -1 | 月不折固定秒数；不能仅靠 TIME code 保真。当前通用出口未见分支；字段掩码/typmod 精确规则交 interval 块取官方源码证据，开放受限类型前必须补齐。 |
| UUID，128 位身份 | 无专属既有 wire code；binary/text 仅可作显式转换的结果 | 2950 → 2950/16/-1 | 不从字符串形状或 16 字节反推 UUID；缺规范身份或非法输入拒绝。当前通用 pg_type 未见 UUID 分支。 |
| JSON / JSONB，彼此独立身份 | JSON=245 不能证明同时保留两者语义；文本转换必须显式 | 114 → 114/-1/-1；3802 → 3802/-1/-1 | JSON 原文保真与 JSONB 规范化不可互换，JSON null 不是 SQL NULL；现有 code 245 出口明确拒绝，NULL 也不能绕过。参数/结果 codec 与范围交各族文档，不能用 text OID 假成功。 |
| 基础数组，元素类型、维度、下界及元素 NULL | 无 MySQL 通用数组 wire 等价，不能把字符串花括号视为规范数组 | 由元素身份确定独立数组 OID（int4[] 示例 1007），typlen=-1；typmod 承接元素修饰，不代表维度 | 数组 NULL、空数组、含 NULL 元素分开；必须保留形状/下界，不从首元素猜类型。当前 CatalogColumnType 有目录专用数组/vector 与编码，不证明任意表达式/参数数组支持。其他族数组 OID 在各族开放前核对官方目录。 |

numeric typmod 目标按官方 `make_numeric_typmod` 的精度高 16 位、scale 低 11 位编码并加 4，scale 的符号扩展按 `numeric_typmod_scale`，不能直接套 MySQL decimal 字段。字符 typmod 的 n+4 来自 `anychar_typmodin`。时间族与 interval 的精确修饰符、特殊域及各族 binary 布局在相应文档块取证；本块不发明格式或将其标为现有实现。未知引擎 code、缺必要规范字段和仅有目录专用身份，目标为显式不支持路径；不静默降级 text，不通过读取首行“恢复”身份。

方案比较：只扩展 pg_type/OID 表对整数提升等既有域可复用，但不能恢复已折叠的 bool 参数、varchar/bpchar、timestamptz、JSON/JSONB、数组身份与 typmod；已有 NativeType boolean 测试也表明相同 MySQL 列和值不足以判型。因此选择**复用 NativeType/FieldType 桥接并按必要保真缺口补充规范类型信息**，再由各 wire 适配。这里不命名新 API 或重建完整类型系统；后续先做独立静态/最小往返取证，确定字段、参数声明生命周期和共享消费者，缺证项不开放。方案不改变 MySQL 原有聚合、cast 和错误语义。

官方依据（2026-10-23 查阅）：PostgreSQL **18** §54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 的 Parse、Bind、ParameterDescription、RowDescription、DataRow 支持方向、格式与 NULL 约束；§52.64 [pg_type](https://www.postgresql.org/docs/18/catalog-pg-type.html) 支持 typlen/类型身份含义。OID/typlen 数值核对官方 **REL_18_0** [pg_type.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_type.dat)；typmod 核对同 tag 的 [numeric.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numeric.c) 与 [varchar.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/varchar.c)。§8.1 [Numeric Types](https://www.postgresql.org/docs/18/datatype-numeric.html)、§8.5 [Date/Time Types](https://www.postgresql.org/docs/18/datatype-datetime.html)、§8.14 [JSON Types](https://www.postgresql.org/docs/18/datatype-json.html) 分别支持位宽/精度、时间域区别、JSON 身份区别；[第 8 章](https://www.postgresql.org/docs/18/datatype.html)作为其他族取证索引，非完整 codec 依据。18.6 仍是来源目标；未运行该补丁参考端，REL_18_0 源码不能代替其运行证据。

#### 布尔类型契约

规范 bool 的非 NULL 值域只有 true/false，SQL NULL 表示未知且保留 bool 身份；不存在精度、scale 或长度修饰符（不适用）。布尔列、TRUE/FALSE 字面量与已定型参数都必须保留此身份，不能由整数 0/1、TINYINT 名称或 MySQL Boolean flag 单独推定 PostgreSQL 列语义。现有 Datum 无独立 bool Kind，出口用 IsBooleanFlag 选 OID 16；这只是承载手段，尚未证明声明列和参数的完整身份生命周期。

| 方向 | 目标身份、值与 NULL | 输入输出、错误及 wire | 现状与差距 |
| --- | --- | --- | --- |
| 字面量/列/参数推导 → 规范 bool | TRUE/FALSE 直接定型 bool；NULL::boolean、布尔目标列中的 NULL 保留类型；未知字符串仅在 bool 上下文按 bool 输入解析。Parse OID 16 或 SQL 布尔目标确定参数，OID 0 按上下文推断，无法推断为 42P18。 | ParameterDescription 为 16；结果 OID/typlen/typmod 为 16/1/-1。零行、首行 NULL、后续 NULL 均不改变描述。裸 NULL 不能无条件定型 bool。 | Extended::process 当前对缺参数推导拒绝；parameter 非 NULL bool 转 tp=1，NULL 转 tp=6，不能据此认定仍携带 bool。声明列持久化、Prepare/Execute 保真未验证；第 2 章提供语法/目标身份，本章定型。 |
| PG text 参数/显式文本输入 → bool | 合法表示包括 true/false、yes/no、on/off、1/0，忽略大小写与两端空白；允许唯一前缀如 t、tr、n、of，o 不唯一应拒绝。SQL NULL 用长度 -1，空串不是 NULL。 | 非法文本如空串、o、2、truth、unknown 为 22P02；非法编码目标 22021。以 OID 16 解释，不能先走数值真值转换。 | parameter 当前仅接受大小写不敏感的 t/true/1、f/false/0，未 trim；其他合法拼写缺失。现有测试不覆盖完整词集或有类型 NULL。 |
| PG binary 参数 → bool | 非 NULL 载荷恰好一字节，00 为 false，01–ff 均为 true；这是已声明 OID 16 的接收规则，不授权整数隐式转 bool。NULL 不消费载荷并保留类型。 | boolrecv 接收后规范化；空载荷目标 08P01，多字节剩余数据目标 22P03；长度 -1 为 NULL，其他负长度和不完整 Bind 归工作流 7。单字节没有非法“值”；把 02/ff 当非法值会偏离 PG。 | binary_parameter 当前只收 00/01，其他值/长度均 22P03；目录专用 catalog_binary_parameter 也有限制，不能推为通用支持。上述差距只记录，未修代码。 |
| 规范 bool → PG text/binary 结果 | 非 NULL text 为 t/f，binary 为 00/01；NULL DataRow 长度 -1、无载荷。先校验身份和值域，非法引擎值如带 bool 身份的整数 2 应拒绝，不能掩盖成 true。 | Describe 与执行结果同为 16/1/-1，格式码依 Bind，格式不改 OID。缺类型/非法内部值走明确错误路径（内部不变量目标 XX000），不静默降级 text；SQLSTATE 传输由工作流 7 承接。 | pg_type 使用 B，value_text 只收 1/true/0/false，value_binary 复用文本校验后发单字节；出口 io::Error 不能证明 XX000 已正确传输。完整 bool 列、NULL 与 Describe 一致性未验证。 |
| MySQL 输入/输出 ↔ 共享承载 | 保留原 code/flags、数值域与真值规则；MySQL Boolean flag、逻辑 bool 与整数三者分别记录。PG 布尔类型信息不能把 MySQL TINYINT 全域缩成 bool。 | 沿自身 wire 契约，无 PG OID/SQLSTATE（不适用）；共享 NULL 不改列定义。 | Datum::ToBool 可处理非零数等，保留其 Go/MySQL 契约；不能把它直接接为 PG 隐式转换。现有同列同值测试只证明桥接断言意图，双方言运行回归未验证。 |

转换边界先按方言与上下文决策再复用共享实现：PG 不允许整数直接充当 WHERE 条件（例如 WHERE 1 目标 42804），整数赋给 boolean 列也不能自动走真值转换（类型不匹配目标 42804）。显式 int4→bool 则合法，0→false、非零（含 -1、2）→true；bool→int4 为 false→0、true→1，结果 OID 23。不得把该显式规则泛化为任意整数类型、隐式重载或赋值转换。显式 text→bool 按输入规则，bool→text 的 SQL cast 输出 true/false（OID 25），区别于 bool wire text 的 t/f；有类型 NULL 的这些转换返回目标类型 NULL。其他来源类型的 cast 由后续转换块逐项取证，不套 MySQL 宽松转换；运算符重载及三值逻辑运算归工作流 4。

官方依据（2026-10-23）：PostgreSQL **18** §8.6 [Boolean Type](https://www.postgresql.org/docs/18/datatype-boolean.html) 支持词集、唯一前缀、空白/大小写、字面量与 NULL 推导及 t/f 输出。精确 codec 核对官方 **REL_18_0** [bool.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/bool.c) 的 boolin/boolrecv/boolsend/booltext；长度错误分别核对 [pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 的 pq_getmsgbyte 与 [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/tcop/postgres.c) 的 exec_bind_message 剩余载荷检查。显式转换资格核对同 tag [pg_cast.dat](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/include/catalog/pg_cast.dat) 的 int4/bool castcontext=e，值规则核对 [int.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c) 的 int4_bool/bool_int4；无隐式/赋值转换的拒绝目标是据此推导，尚未运行参考端。wire NULL 与描述字段沿用前块 §54.7 依据；18.6 补丁参考端仍未验证。

#### 整数与无符号边界

本块固定精确整数域；整数没有 scale，PG int2/int4/int8 无长度或精度 typmod（-1），十进制显示位数不是声明精度。共享承载必须同时保留声明宽度和 signed/unsigned，不能由当前值是否为正推断。PG 三种常规 SQL 整数均为有符号；目录 oid 的 unsigned 32 位编码不构成通用 unsigned 整数类型。以下为 MySQL 原整数域及 PG 出口目标（不含 Boolean flag 特例）：

| MySQL 身份/code | signed 值域 | unsigned 值域 | 按声明域选择 PG OID/typlen/typmod |
| --- | --- | --- | --- |
| TINYINT/1 | -128…127 | 0…255 | 两者 int2：21/2/-1 |
| SMALLINT/2 | -32768…32767 | 0…65535 | signed int2：21/2/-1；unsigned int4：23/4/-1 |
| MEDIUMINT/9 | -8388608…8388607 | 0…16777215 | 两者 int4：23/4/-1 |
| INT/3 | -2147483648…2147483647 | 0…4294967295 | signed int4：23/4/-1；unsigned int8：20/8/-1 |
| BIGINT/8 | -9223372036854775808…9223372036854775807 | 0…18446744073709551615 | signed int8：20/8/-1；unsigned numeric：1700/-1/-1 |

此表扩宽与 pg_type 当前分支一致，u64 目标选无约束 numeric，不擅自改为 numeric(20,0)。值域保真不等于来源类型可逆：PG int4 不能反推原 MySQL U16，PG numeric 不能反推 U64。反向必须有目标 FieldType/unsigned 身份；缺声明拒绝或保留原规范身份，不从 OID 猜源类型。MySQL 自身输出保留原 code/UnsignedFlag，不因 PG 扩宽改写 MySQL 列定义。

| 方向 | 身份、精确值与 NULL | 错误、wire 及现状差距 |
| --- | --- | --- |
| PG 声明/上下文 → 规范整数及参数描述 | smallint/int2、integer/int/int4、bigint/int8 分别严格为上述 16/32/64 位 signed 全域。参数显式 OID 21/23/20 或上下文定型；OID 0 无法推断目标 42P18。NULL 保留声明宽度而非归 text。 | ParameterDescription 保持输入 OID；结果分别 21/2/-1、23/4/-1、20/8/-1。当前 process 对缺推导报 0A000；parameter(NULL) 为 tp=6，类型生命周期未验证。第 2 章交接类型语法，本章定型。 |
| PG text 参数/显式文本转换 → 规范整数 | 按已定型 signed 域精确解析，覆盖正负号、首尾空白及 PG 整数输入语法；NULL 长度 -1，空串不是 NULL。整数字符串不得经 f64 中转或接受截断数字前缀。 | 合法整数越界为 22003，非法如空串、1x、1.5 为 22P02，非法字符编码 22021。现有 parameter 用 Rust parse i16/i32/i64，将范围错误也映射 22P02，未 trim，不能证明完整 PG 输入语法。PG 18 的进制前缀及下划线规则以 numutils.c 为准，后续补输入用例，不能只凭 Rust parse 推定等价。 |
| PG binary 参数 → 规范整数 | OID 21/23/20 分别恰好 2/4/8 字节网络序、有符号二补码；每个合法长度的位模式都在该整数域内。长度 -1 为 typed NULL，无载荷。 | 接收器读不足目标 08P01；多余载荷目标 22P03，外层 Bind 帧损坏归工作流 7。当前 binary_parameter 将错误长度统一报 22P03，再转共享 little-endian 参数；不存在合法长度的“数值越界位模式”。 |
| 规范整数/MySQL unsigned → PG text/binary 结果 | 先按声明域定型，再按上表整体扩宽；U64 即使值为 0 也保持 numeric。text 输出精确十进制整数；binary int2/int4/int8 按 2/4/8 字节网络序。NULL 为 DataRow 长度 -1。 | value_text 当前直接格式化整数，未按 OID 再检查整数范围；binary 用 parse 检查范围。u64::MAX 的 text/OID 已有断言意图，numeric binary 当前无分支，不能把十进制 ASCII 填进 numeric binary。开放该格式前依赖 numeric 块补齐 codec；未实现路径明确拒绝目标 0A000，禁止回退 float8 或伪装 int8。 |
| PG/共享整数 → 显式窄化、目标 unsigned | 整数之间只在目标域内精确转换；负值转共享 unsigned 目标拒绝，超过 unsigned 上界同样拒绝；这是桥接契约，PG 没有可写作 ::unsigned 的常规整数类型。numeric→U64 仅在有限、无小数且域内的明确桥接路径接收；一般 numeric→PG integer 舍入规则留 numeric/cast 块，不把此桥接规则当 PG 通用 cast。typed NULL 转为目标类型 NULL。 | PG 路径域外目标 22003，不 wrap、饱和或返回截断成功。MySQL ConvertIntToUint 的 AllowNegativeToUnsigned、携带裁剪值的错误和 Context 策略保持原 Go/MySQL 行为，不直接当 PG 转换。共享错误到 SQLSTATE 尚未验证。 |

算术和转换结果的身份必须在 Prepare/Describe 时由已选择的 PG 重载/目标类型确定，执行时不能因某一行溢出而临时提升 OID；例如 int4+int4 的 int4 溢出应 22003，而预先显式转 int8 后计算可得到 int8，不能直接套共享 MySQL 的 BIGINT 推导。`2147483647::int4 + 1::int4`、bigint 最大值加一及各 signed 最小值取负均应有溢出验收；NULL 结果保留同一结果类型。unsigned 来源的预定扩宽是上表的边界策略，不授权改变 PG 算术规则。运算符/聚合完整重载归工作流 4，本块只规定结果类型交接与溢出不改型；显式、赋值、隐式 cast 的资格按后续转换块确定，wire 扩宽不能自动成为 SQL 隐式 cast。

官方依据（2026-10-23）：PostgreSQL **18** §8.1.1 [Integer Types](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-INT) 定义三种 signed 域及越界错误；§54.7 [Message Formats](https://www.postgresql.org/docs/18/protocol-message-formats.html) 定义 NULL、参数 OID 与列描述。精确输入和 codec 核对官方 **REL_18_0** [int.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int.c) 的 int2in/int4in、recv/send、int4pl、int2um/int4um，以及 [int8.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/int8.c) 的 int8in/recv/send、int8pl/int8um 和整型窄化；[numutils.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numutils.c) 的 pg_strtoint16_safe/32_safe/64_safe 区分语法和范围错误；[pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 与 [postgres.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/tcop/postgres.c) 的读取/剩余载荷检查支持二进制长度错误目标。错误名称见 PostgreSQL 18 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)。OID 沿前块 pg_type.dat 依据；MySQL 域来自本仓库上下界函数及 test_convert，不以 MySQL 类型名推导 PG 等价。18.6 仍仅为来源目标，未运行该补丁参考端。

#### 精确数值契约

numeric 与 decimal 是 PG 同一精确数值类型（OID 1700、typlen -1），区别于 float4/float8。目标须保存有限精确值、显示 scale、声明 precision/有符号 scale 及特殊值身份；只扩充 OID 或透传字符串不足以证明支持。现有 MyDecimal 与 cast 的 Decimal 容量/错误策略不同，须先验证可保真子域，再决定规范表示补充；不得将 PG 域缩成 MySQL DECIMAL 或用 f64 中转。尚不能承载的合法 PG 域应明确报项目不支持 0A000，记录兼容缺口；PG 自身域外才是 22003，不能混淆。

| 方向 | 值、类型、NULL 与精度目标 | 错误、wire 元数据及当前差距 |
| --- | --- | --- |
| PG 声明/推导 → 规范身份 | numeric(p,s) 的 p=1…1000，s=-1000…1000，可 s>p；numeric(p) 为 s=0；裸 numeric 不强制 scale，有限域上限为小数点前 131072 位、后 16383 位。NULL 保留目标 numeric 身份。 | 无约束 typmod=-1；有约束为 `((p<<16) | (s & 0x7ff))+4`，解码 s 须 11 位符号扩展。非法声明目标 22023。ParameterDescription 仅含 OID，不传 p/s；上下文及结果描述保存修饰符。现有结果一律 -1，不能伪称 constrained numeric。声明语法交工作流 2。 |
| text 参数/文本显式转换 → numeric | OID 1700 下完整解析十进制与指数输入，允许规范所容许的符号/空白；空串、12bad 非法，不能接受数字前缀或回退零。无约束 12.340 保留显示 scale；有约束先舍入再检查整数域。NULL 长度 -1，无载荷；空载荷不是 NULL。 | 非法语法 22P02，非法字符编码 22021，数值范围/舍入后超域 22003。parameter 当前调用 FromString 后保留原字节，所有解析错误映为 22P02；不是完整 PG 输入/范围分类证据。 |
| binary 参数 → numeric | 网络序 16 位字段依次为 ndigits（unsigned 计数）、weight（signed）、sign、dscale，后随 ndigits 个 base-10000 digit（0…9999）；有限值为符号乘各 digit×10000^(weight-i)。长度应为 8+2×ndigits；NULL 在外层长度 -1 表示。 | sign 为 0000/4000/C000/D000/F000（正/负/NaN/+∞/-∞），dscale 为 0…16383；它是显示 scale，不是带符号的声明 scale。接收有限数先截去 dscale 隐藏位，再应用 typmod 舍入/范围。非法 sign/digit/scale 22P03；读取不足 08P01，额外载荷 22P03（沿前块消息边界）。当前非 NULL 1700 无 binary 分支，报 0A000；NULL 能走公共分支不证明身份连续。 |
| 规范 numeric/MySQL DECIMAL/U64 → PG text/binary 结果 | Decimal 原身份保留，U64 沿前块输出无约束 numeric；text 精确输出十进制及应保留的小数零，binary 转换 base-10^9 到 base-10000，不能发送 MyDecimal::ToBin 格式或 ASCII。两格式解码后的值与显示 scale 一致，不要求载荷相同。NULL 为 DataRow 长度 -1。 | OID 1700/typlen -1，typmod 随声明或表达式定型，text/binary 不改变类型；零行、全 NULL 同样 Describe。value_text 当前透传值文本，未校验 numeric 域；value_binary 无 1700；encode_formats 固定 typmod -1。未开放 codec 明确拒绝目标 0A000，不降为 float8/int8，io::Error 到 SQLSTATE 尚未验证。 |
| numeric → 目标精度/整数/共享 DECIMAL | precision/scale coercion 的半值远离零；负 scale 在小数点左舍入，s>p 限制小数点后前导零。numeric→PG signed integer 先按整数舍入再检查对应域；整数→numeric 精确保值。typed NULL 转为目标类型 NULL。 | 正负超域均 22003，不裁剪、wrap 或仅警告成功；如 99.95/-99.95→numeric(3,1) 均因进位超域失败。截断只用于显式截断语义或 binary 接收 dscale 规则，不能代替 cast 舍入。共享 unsigned 的有限无小数桥接仍沿前块；MySQL warning/裁剪原行为独立保留。显式/赋值/隐式资格交本章转换块，运算重载交工作流 4。 |

特殊值单列：目标规范身份须区分 NaN、Infinity、-Infinity 与 SQL NULL；NaN 可进入 constrained numeric，无穷只可进入无约束 numeric，进入有限精度声明目标 22003。输出 text 使用 NaN/Infinity/-Infinity，binary 使用上述专属 sign。MyDecimal 的符号与有限 word 布局没有特殊值标记；Value::Float 的特殊值输出分支或 Value::Text 透传不能证明 numeric 特殊值已可表示、转换或求值。支持前明确 0A000，禁止把它当普通字符串成功；特殊值比较/算术由工作流 4 承接。

官方依据（2026-10-23）：PostgreSQL **18** §8.1.2 [Arbitrary Precision Numbers](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-NUMERIC-DECIMAL) 支持精度、scale、舍入和特殊值目标。精确布局、typmod、错误及转换以官方 **REL_18_0** [numeric.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/numeric.c) 的 make_numeric_typmod/numeric_typmod_scale、numerictypmodin、numeric_in/out、numeric_recv/send、apply_typmod/apply_typmod_special、numeric_int4 为据；NULL/描述与外层载荷沿前块 §54.7、pqformat.c/postgres.c 来源。以上是规范和源码阅读证据，非参考端实测；来源指定的 18.6 补丁仍未验证。

#### 浮点数值契约

real/float4 与 double precision/float8 是两个规范身份，目标分别为 IEEE 754 binary32/binary64（有效二进制精度 24/53 位），PG OID/typlen/typmod 分别 700/4/-1、701/8/-1。有限最大绝对值为 (2−2^-23)×2^127、(2−2^-52)×2^1023；最小正规正数为 2^-126、2^-1022，最小次正规正数为 2^-149、2^-1074。这些是二进制域边界，不以文档概述的十进制量级代替；近似类型不能承诺十进制精确计算。[PostgreSQL 18 §8.1.3](https://www.postgresql.org/docs/18/datatype-numeric.html#DATATYPE-FLOAT) 为位宽、精度、特殊值和溢出目标依据。

推导消费工作流 2 的类型节点：real/float4 固定为 float4，double precision/float8 为 float8；float(p) 的 p 为二进制精度，1…24 选择 real、25…53 选择 double precision，无 p 选择 double precision。p=0 或 p=54 由声明解析拒绝 22023（REL_18_0 [gram.y::opt_float](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/parser/gram.y)），不把 MySQL FLOAT(M,D) 当 PG 声明。裸 `1.5`、`1e0` 按 PG 数值常量规则初始为 numeric，不能因带小数点或指数直接推导 float8；显式转换和上下文才能改变类型（[18 §4.1.2.6](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-SYNTAX-CONSTANTS-NUMERIC)）。OID 0 参数按上下文定型，无法确定时目标 42P18；ParameterDescription 确定 OID，格式码不决定宽度。

| 方向 | 值域、精度、NULL 与错误目标 | wire 身份、现状及差距 |
| --- | --- | --- |
| PG text 参数 → 规范浮点 | 按目标宽度直接解析十进制/指数，允许首尾空白及大小写不敏感 NaN、±Infinity/±Inf；拒绝空串、尾随垃圾为 22P02，编码错误 22021。有限文本溢出为无穷或非零下溢为零时 22003；可表示次正规数成功。float4 不先解析 float8 再窄化，以免二次舍入。 | OID 700/701 决定 f32/f64；长度 -1 为 typed SQL NULL，无载荷，空串不是 NULL。当前 Rust parse 的词法、超域分类与 PG 尚不等价，解析失败统一 22P02；NULL 参数仅 tp=6，身份连续性未验证。 |
| PG binary 参数 → 规范浮点 | 载荷恰为 4/8 字节的 IEEE 位型，含次正规、±0、NaN、±∞；接收本身无十进制舍入，不能将无穷当有限超域拒绝。过短目标 08P01、额外字节 22P03，消息截断仍归工作流 7。 | 网络大端；外层 -1 为 NULL，0 长度非法。当前 binary_parameter 已有网络序转换，但短/长均映为 22P03；保留位宽、零符号与特殊值的全链证据未验证。 |
| 规范浮点 → PG text 结果 | 先以规范身份校验并取得目标宽度的值；默认 extra_float_digits=1 时按该宽度输出可回读同值的最短十进制，不打印 f32 扩宽后的 f64 噪声。特殊值文本 NaN/Infinity/-Infinity，零符号须可观察；SQL NULL 长度 -1。 | 两格式均固定 700/4/-1 或 701/8/-1；当前 pg_type 有 4→700、5→701，value_text 共用 f64 to_string，不能证明 float4 或 PG 精确拼写。会话非默认 extra_float_digits 的状态/设置归工作流 5，不能将其有损文本仍标为无损往返。 |
| 规范浮点 → PG binary 结果 | 按身份输出 4/8 字节网络序，有限值和 ±0 保持目标宽度位型；±∞保持符号，NaN 按 NaN 类别验收，不承诺 NaN payload/signaling 位跨文本或运算保存。NULL 无载荷。 | 当前 value_binary 经文本重解析，不能据 1.5 测试推断所有位型可逆；内部未窄化 float4 不得在出口静默变无穷/零，已开放域错误 22003，缺保真表示/codec 时明确 0A000，工作流 7 承接 SQLSTATE 传输。 |
| float4 ↔ float8 / 数值 cast | float4→float8 有限值精确扩宽；float8→float4 按目标宽度舍入，有限溢出、非零变零拒绝 22003，次正规非零成功；原有无穷/NaN 合法传递。typed NULL 保留目标身份。浮点→整数先按参考平台 rint 舍入再检查域，NaN/无穷拒绝 22003；numeric 半值远离零规则不能套用。 | 目标 OID 由 cast 预先确定，不能按值动态提升。显式/赋值/隐式资格交本章转换块核定，算术/重载/round 归工作流 4；不把本表当完整隐式转换矩阵。MySQL 参数和结果维持自身 FLOAT/DOUBLE code、flags 与告警契约，不套 PG 错误；共享 f64 容器不是双方言等价证据。 |

特殊值须区分 SQL NULL：NaN 是非 NULL 的浮点值，PG 比较语义中 NaN 等于 NaN 且大于其他值，±0 数值相等但 codec 验收分别检查符号；无穷、NaN、零的比较与算术由工作流 4 拥有。本块不要求文本往返保存 NaN payload；binary 直通是否保存其位型须另采证，不能将全部 NaN 位串判为非法。舍入验收记录参考端平台与模式，常见最近偶数规则须有具体半值断言，不能用统一 epsilon 淹没 float4/float8 差异。

官方源码依据为 **REL_18_0** [float.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/utils/adt/float.c) 的 float4in_internal/float8in_internal、float4out/float8out、float4recv/send、float8recv/send、ftod/dtof、dtoi4：分别核对直接目标宽度解析、输入/范围错误、输出及窄化和整数转换；二次舍入反例取其注释 `7.038531e-26`。网络位型与不足读取见同 tag [pqformat.c](https://raw.githubusercontent.com/postgres/postgres/REL_18_0/src/backend/libpq/pqformat.c) 的 pq_sendfloat4/8、pq_getmsgfloat4/8、pq_copymsgbytes；额外载荷错误沿前块消息边界来源。输出配置以 PostgreSQL **18** [§19.11.2 extra_float_digits](https://www.postgresql.org/docs/18/runtime-config-client.html#GUC-EXTRA-FLOAT-DIGITS) 为据。查阅日期 2026-10-23；以上均为规范阅读，18.6 补丁和实际参考端行为仍未验证。

#### 文本类型契约

本块定义通用 text、varchar、SQL char（内部 bpchar），与目录单字节 `"char"` 区分。长度按字符计数，wire 长度按字节计数；UTF-8 下不按字素簇或视觉宽度截断，不自动 Unicode 规范化。数值精度/scale 不适用，约束是字符数 n。无长度 varchar 保持无约束身份，SQL char 省略长度按 1；无长度 bpchar 是独立的无约束、尾空格不显著形式，不能误套 char(1)。声明长度 n 的 PG 18 合法域为 1..10485760；不合法声明目标 22023，由工作流 2 保留类型语法并交本章校验。

| 方向 | 值、NULL 与错误目标 | 身份、元数据及现状差距 |
| --- | --- | --- |
| PG text 参数 → 规范文本 | 按已确定 OID 完整解码；空载荷为空串，长度 -1 为 SQL NULL，无载荷；两者不得合并。UTF-8 基线拒绝 NUL、非法序列为 22021，不替换字符或截断至 NUL。 | 25/1043/1042 各自保留，OID 0 仍交定型，不猜 text。ParameterDescription 只有 OID，无 typmod 字段；SQL 目标的长度约束不能因此丢失。当前输入校验存在，身份/长度接线未证明。 |
| PG binary 参数 → 规范文本 | 载荷是协商编码的文本字节，不带内部 varlena 头、不以 NUL 终止；空载荷合法，NULL 仍为 -1。与 text 参数同样验证编码/NUL，binary 不绕过长度约束。 | 格式 1 不改变类型，现有路径复用 parameter；并非任意 bytes/bytea。UTF-8 之外的协商和转码归工作流 5、7，未开放编码明确 0A000，不能假称已转码。 |
| 规范文本 → PG text/binary 结果 | 两格式都输出已定型值的文本字节；char(n) 的补空格保留，varchar/text 的尾空格保留；NULL 长度 -1，非 NULL 空串为 0。非法内部编码/NUL 在发布结果前拒绝，目标 22021；缺身份/codec 的能力门禁为 0A000。 | text 为 25/-1/-1；varchar(n)、char(n) 为 1043、1042，typlen 均 -1，typmod=n+4；无约束 typmod=-1。Prepare/Describe/Execute、零行及全 NULL 均一致，不按首行猜。当前普通 text fallback、固定 -1 和 io::Error 尚不能证明身份与 SQLSTATE 传输。 |
| 赋值/长度 coercion 与显式 cast | 向 varchar(n)/char(n) 赋值超长且超出部分含任何不是 U+0020 的字符时 22001；仅多出的 U+0020 可裁去。显式 cast 可截到 n 字符且不切断多字节编码。char(n) 短值补 U+0020，varchar 不补；char 转 text/varchar 去除尾部补空格，text/varchar 不通用 trim。typed NULL 保留目标身份。 | 目标 OID/typmod 由声明或表达式定型决定。隐式/赋值/显式资格交本章转换块，不由 codec 擅自截断；尚无 PG 长度 coercion 完整证据。通用字符值不承诺数值转换、排序或匹配算法。 |
| MySQL 参数/结果 ↔ 共享值 | 保留其自身 charset、collation、声明长度、严格/非严格 sql_mode 与 CHAR 尾空格规则；字符长度和协议字节宽度分开。NUL、非法序列、截断的错误/告警按 MySQL 参考端单独验收，不强套 PG 22021/22001。 | 保留 VARCHAR/VAR_STRING/STRING code、flags、charset 与长度元数据，不将 PG OID 或 n+4 写回 MySQL。共享 KindString、名称相似或 charset=63 均不是双方言语义等价证据；不在本块重写既有 MySQL 路径。 |

规范依据（查阅于 2026-10-23）：[PostgreSQL 18 §8.3 Character Types](https://www.postgresql.org/docs/18/datatype-character.html) 支持字符长度、补空格、赋值与 cast 差别、尾空格及 NUL 禁令；[REL_18_0 varchar.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/varchar.c) 的 anychar_typmodin、bpchar_input、bpcharrecv/bpcharsend、varchar/varcharrecv/varcharsend 支持 typmod 与文本载荷边界；[REL_18_0 varlena.c](https://github.com/postgres/postgres/blob/REL_18_0/src/backend/utils/adt/varlena.c) 的 textrecv/textsend 支持无内部头的文本字节编码。[PG 18 §54.7 Bind/DataRow/RowDescription/ParameterDescription](https://www.postgresql.org/docs/18/protocol-message-formats.html) 支持 -1 NULL、字节长度及元数据字段；[PG 18 Appendix A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 支持 SQLSTATE 分类。固定源码标签是 18.0 取证，不等于已在目标 18.6 运行或证明补丁版全部一致。

### 范围外

[规范类型与协议映射](#规范类型与协议映射)只定义映射与交接，不实现生产代码、类型系统、完整 cast、codec、数据库环境或差分平台。逐族输入语法、舍入/时区、数组形状及精确二进制布局交本章后续文档块，不因列出 OID 提前承诺支持。parser/名称绑定归工作流 2；运算重载归 4；时区等会话状态归 5；目录类型身份归 6；消息状态机、格式选择、错误响应和恢复归 7；真实客户端与双方言共存证据归 8。扩展、自定义类型及目录专用类型向通用引擎推广均不在本块范围。

[布尔类型契约](#布尔类型契约)不实现存储或类型系统，不改 Go/MySQL 真值行为；不扩展为 bool 数组、完整 cast 矩阵、运算符/聚合或协议状态机。声明解析、运算、消息恢复分别交工作流 2、4、7；本块只固定类型与 codec 的可观察交接。

[整数与无符号边界](#整数与无符号边界)不实现算术、cast 或 numeric codec，不扩写聚合、serial/identity、oid、数组及完整类型推断。工作流 2 提供声明/转换语法；4 拥有重载求值；7 拥有帧校验、错误响应和恢复；8 采集客户端与双方言证据。MySQL 原行为不因 PG 目标更改。

[精确数值契约](#精确数值契约)不实现大数表示、cast 或 codec，不改 MySQL/Go 裁剪及告警行为；不扩大到算术/聚合、索引排序、numeric 数组或协议状态机。工作流 2 接声明语法，4 接数值运算与特殊值比较，7 接消息/错误响应及恢复，8 接客户端差分。

[浮点数值契约](#浮点数值契约)不实现内部表示、cast、codec 或运算，不改 Go/MySQL 的浮点行为，不扩展浮点数组、索引、聚合或完整推导系统。类型语法归工作流 2，比较/算术归 4，输出会话配置归 5，消息错误与恢复归 7，真实客户端与双方言差分归 8。

[文本类型契约](#文本类型契约)不实现生产表示、cast、codec 或 MySQL 行为修改；比较、排序、collation 算法、LIKE/正则交工作流 4，编码会话状态交 5，目录专用类型交 6，协议分帧/错误恢复交 7，双方言客户端差分交 8。此处仅交接语义，不扩建子系统。

### 验收

[规范类型与协议映射](#规范类型与协议映射)的以下能力 ID 为**待新增**验收入口，非既有测试或功能通过记录。每项按第 1 章记录方言、定型前提、输入、类型、NULL、wire 元数据、错误与当前证据；本轮全部运行行为为未验证。成功项 SQLSTATE 不适用；错误目标参照 PostgreSQL 18 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)，不以现有 io::Error 自动推导其正确传输。

| 入口与场景 | 可观察目标与反例 | 当前证据状态/责任 |
| --- | --- | --- |
| TYPE-MAP-IDENTITY：同值不同类型 | PG 定型 `1::int2`、`1::int4`、`1::int8`、`1::numeric` 及 TRUE 保持各自 OID/typlen/typmod；text '1' 不变整数。两种格式的身份一致。 | 既有 boolean bridge 测试只断言 MySQL 同列同值及 B 差异；完整场景待新增、未验证。本章定型，工作流 7 描述。 |
| TYPE-MAP-EMPTY：空结果 | `SELECT 1::int4 AS v WHERE false` 返回零行但列为 23/4/-1；Prepare/statement Describe 与 portal Describe/Execute 的类型相同，格式字段按实际模式分别验收，不能要求 text/binary 格式码相等。 | 待新增、未验证；缺元数据即使零行也拒绝，不依赖首行。 |
| TYPE-MAP-NULL：全 NULL | `SELECT NULL::int4` 返回 23/4/-1、DataRow 长度 -1；typed NULL 参数在 Bind 前后仍保留 int4 身份；空 text/bytea 与 SQL NULL 区分；JSON null 独立。 | 既有 NULL JSON 拒绝断言不能证明此场景；待新增、未验证。不确定裸 NULL 类型先按 PG 上下文取证，不能沿当前 code 6 回退视为通过。 |
| TYPE-MAP-UNSIGNED：提升 | 共享引擎/MySQL U16/U32/U64 边界，分别得到 int4/int8/numeric；u64 最大值不丢位，PG numeric 反向窄化须检查目标 unsigned 域。MySQL 描述仍保留 U。 | unsigned_binary_and_unsupported_types 有 u64 文本断言；Prepare、一致性、binary numeric 与反向边界待新增、未验证。PG 窄化溢出目标 22003，禁止截断或浮点提升。 |
| TYPE-MAP-UNKNOWN：未知或缺类型 | 直接构造未知 code、native 数量不匹配及缺必要身份；分别覆盖有行、零行、全 NULL，均不得发成功描述或猜 text；未知 PG 类型入口拒绝，已知类型但未实现 codec 明确不支持。 | 既有 missing_original_metadata 测试覆盖缺类型的有行边界，JSON 非支持含 NULL 有断言；其余待新增、未验证。项目不支持路径目标 0A000，非宣称参考 PG 也拒绝该类型。 |
| TYPE-MAP-DIRECTIONS：参数/推导/结果 | 每个已开放族跑 text/binary 参数→规范值→结果；OID 声明与上下文共同决定身份，OID 0 无法推断则拒绝，目标 42P18；反例为同载荷不同 OID、非法 text、错误 binary 长度及范围越界。 | 参数身份及 typmod 往返待新增、未验证；错误目标依原因分别 22P02、22P03、22003，字符编码 22021；具体日期等族细化错误另取证，不一律套 22P02。 |
| TYPE-MAP-NONISOMORPHIC：不可逆域 | 零日期、负/超过一天的 duration、缺时区瞬间、JSON 身份折叠、空数组无元素身份、未知数组下界不得静默包装；有明确转换才能按转换目标返回。 | 既有测试有零日期和 25:00:00 拒绝意图；其余待新增、未验证；不支持范围目标 0A000，非法已支持域的输入按各族数据错误，MySQL 原行为单独对照。 |

文档完成条件是五个公共小节均能关联本块，上述方向/类型族及五项必需边界（同值异类型、空结果、全 NULL、unsigned、未知类型）无缺项，当前事实、目标和缺证分开。文档结构通过不等于 Describe/Execute、codec、cast、SQLSTATE、双方言隔离或性能通过；本轮没有当前客户端、差分记录或有效运行用例数。

[布尔类型契约](#布尔类型契约)验收入口以下均为**待新增**，不是已执行用例；当前运行证据全部为“未验证”，成功项 SQLSTATE 不适用。每项同时采集 AsterSQL 与精确 PG 参考版本、值/NULL、类型、格式和错误；错误恢复交工作流 7，不能以 io::Error 或测试名称代替 wire 错误断言。

| 能力 ID / 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| BOOL-IDENTITY | SELECT TRUE,FALSE,NULL::boolean；声明 bool 列含 true/false/NULL，并并列 int4 值 1/0/NULL；bool 16/1/-1，整数 23/4/-1，不能同值改型。 | common_type_oids_and_values 与 bridge 测试只覆盖部分字面量；声明列/typed NULL 待新增，未验证。 |
| BOOL-TEXT | OID 16 参数逐测完整合法词、前缀、大小写和空白；空串、o、2、truth、unknown 拒绝 22P02，非法编码 22021；输出固定 t/f。 | text_parameters_are_typed_and_reject_loss 仅 true/unknown；其余待新增，未验证。 |
| BOOL-BINARY | OID 16 载荷 00/01/02/ff 依次 false/true/true/true，结果规范化 00/01；长度 0 为 08P01，长度 2 为 22P03。非法二进制“值”反例是拒绝 02/ff 的错误实现；带 bool 身份的内部值 2 则应拒绝。 | pg_binary_native_results 只含 false 输出；输入边界及内部非法值拒绝待新增，未验证。 |
| BOOL-NULL-DESCRIBE | OID 16 的 text/binary NULL 参数、首行/全 NULL 及零行 bool 结果；ParameterDescription 为 16，statement/portal Describe 与 Execute 列身份始终 16/1/-1；NULL 长度 -1，空载荷独立拒绝。 | 参数 tp=6 与准备/执行桥接尚缺身份连续性证据；待新增，未验证。 |
| BOOL-CAST | 0::int4::boolean=false，2/-1::int4 显式转为 true；TRUE::int4=1，FALSE::int4=0；TRUE::text='true'；NULL 转换保留目标身份；WHERE 1 与整数赋 bool 列拒绝 42804。 | Datum::ToBool/test_to_bool 是 MySQL 意图，不能证明 PG；完整显式/隐式及双方言隔离待新增，未验证。 |

[整数与无符号边界](#整数与无符号边界)以下入口均为**待新增**；所有运行行为未验证。成功项 SQLSTATE 不适用，失败须断言具体 SQLSTATE，不以 is_err 代替。

| 能力 ID / 场景 | 可观察目标与反例 | 现有断言意图/缺口 |
| --- | --- | --- |
| INT-RANGE | PG int2/int4/int8 各 min/max 成功，min-1/max+1 的 text 参数及显式窄化拒绝 22003；空串/1x/1.5 拒绝 22P02；按官方输入语法补空白、符号、进制和下划线正反例。 | text_parameters_are_typed_and_reject_loss 仅 int4 最小值和上越界 is_err；其余及错误码未验证。 |
| INT-UNSIGNED | 对表内每个 signed/unsigned 域测 min/max 与外侧一值；合法 U8/U16/U24/U32/U64 依次映射 int2/int4/int4/int8/numeric。U64 的 9223372036854775808 和 18446744073709551615 精确保留，18446744073709551616→U64 及 -1→各 unsigned 拒绝。 | test_convert 有各宽度上下界及部分裁剪错误值断言；unsigned_binary_and_unsupported_types 有最大 U64 text/OID 断言。桥接拒绝、全宽 wire 和双方言隔离未验证。 |
| INT-CODEC | 每个 PG signed 宽度对 min/-1/0/1/max 做 text/binary 参数→结果，binary 长度/网络序准确；少/多一字节分别 08P01/22P03。U16/U32 扩宽两格式精确，U64 numeric 两格式精确或未开放 binary 时显式拒绝。 | pg_binary_native_results 有 i64::MIN、-2 int2、i32::MAX 输出断言；完整往返、长度错误及 numeric binary 未验证。 |
| INT-NULL-METADATA | 每个 signed 类型、各 unsigned 提升后类型含 typed NULL 参数、首行 NULL、全 NULL、空结果、混合 min/max 行；ParameterDescription 输入身份及 statement/portal Describe/Execute 结果 OID/typlen/typmod 稳定，格式选择不改变类型。 | NULL 参数现仅 is_null 断言，不证明类型连续性；U64 的零值与最大值均须保持 1700/-1/-1，全部待新增、未验证。 |
| INT-CAST-ARITH | 所有 signed 宽度间域内 cast 保值，窄化越界拒绝；负值转 unsigned 不 wrap。int4 最大值加一/各 signed 最小值取负拒绝 22003，先显式转足够宽类型后的可表示结果成功；NULL 不改结果类型，溢出不动态改 OID。 | 共享转换测试只证明 MySQL 意图；PG 重载、显隐式资格、错误码与元数据全链未验证，算术执行交工作流 4。 |

[精确数值契约](#精确数值契约)以下入口均为**待新增**，所有运行行为未验证；每项分别采集 text/binary、值与 scale、OID/typlen/typmod、NULL 和 SQLSTATE，成功项 SQLSTATE 不适用。

| 能力 ID / 场景 | 可观察目标和反例 | 已读证据与验证缺口 |
| --- | --- | --- |
| NUM-PRECISION | numeric(3,1)：±99.9 成功；±99.94 舍入为 ±99.9；±99.95 拒绝 22003。p=1/1000、s=-1000/1000 声明边界成功，p=0/1001、s=±1001 拒绝 22023；numeric(3,5) 的 .00999 成功，.009995 进位超域拒绝。 | MyDecimal 测试有 word 边界，不证明 PG 声明域；全部 PG 断言未验证。 |
| NUM-ROUND | ±1.25→numeric(3,1) 为 ±1.3；±1.249 为 ±1.2；numeric(2,-3) 的 99499/-99499→99000/-99000，99500/-99500 拒绝 22003。±1.5→int4 为 ±2；整数上下界附近舍入后超域拒绝。 | TestRoundWithHalfEven 有正负半值/负 scale，TestRoundWithTruncate 有另一模式；均未运行，不证明 PG cast。 |
| NUM-LARGE | 无约束 numeric 接收 18446744073709551615、82 位全 9 整数及 1000 位有限整数，保持精确；按 PG 实现极限验证小数点前 131072/131073 位和后 16383/16384 位的成功/22003。共享表示不足须明确 0A000 并记为兼容缺口。 | U64 仅文本断言；大精度表示、两格式往返、极限全部未验证，禁止 float8 回退。 |
| NUM-TEXT-SPECIAL | 12.340、1.25e2 精确；空串、12bad、NaNx 拒绝 22P02；非法编码 22021。NaN 保持 numeric，±Infinity 在无约束成功、constrained 拒绝 22003；当前缺表示时明确 0A000。 | 参数已有 12.340/12bad 意图；其余未验证，字符串透传不能验收特殊值。 |
| NUM-BINARY | 12.340 的字段为 (2,0,0000,3;12,3400)，结果 scale=3；U64 最大值两格式无损。构造 (2,0,0000,2;12,3456) 接收为 12.34，不能误按普通 typmod 舍入为 12.35。负值、零、特殊 sign 分别往返；非法 digit=10000/sign/scale、短载荷/额外字节分别按方向表拒绝。 | 当前 numeric binary 无实现；MyDecimal TestToBinFromBin 是 MySQL 格式，不能替代此项，全部未验证。 |
| NUM-NULL-METADATA | NULL::numeric(8,2)、typed NULL 参数、首行 NULL/全 NULL、空结果及多行不同值，均保持 1700/-1 与预定 typmod；numeric(8,2) typmod=524294，无约束/U64 为 -1。statement/portal Describe 与 Execute 身份一致，参数 OID=1700；负 scale typmod 往返不误读成正 scale。 | 既有 DECIMAL(8,2) 仅 text/OID 意图；typmod、NULL、Prepare 全链及双方言隔离未验证。 |

[浮点数值契约](#浮点数值契约)以下能力入口均为**待新增**，全部运行行为为未验证；每项分别采集两宽度、两格式、类型身份、NULL 与 SQLSTATE，成功项 SQLSTATE 不适用。codec 有限值按目标宽度位型比较，零另比符号，NaN 比类别；文本同时核对 PG 默认拼写和回读，不用统一宽容误差替代。运算另定逐操作误差模型，不沿用 codec 放宽阈值。

| 能力 ID / 场景 | 可观察目标与反例 | 已读证据与缺口 |
| --- | --- | --- |
| FLOAT-IDENTITY | 1.5::real 与 1.5::double precision 同值异型，分别 700/4/-1、701/8/-1；裸 1.5/1e0 为 numeric；float(1)/(24) 为 float4、float(25)/(53) 为 float8，float(0)/(54) 拒绝 22023。 | pg_type 有映射但无这些推导/Describe 全链断言；未验证。 |
| FLOAT-ROUNDTRIP | ±1.5、0.1、16777217 text→float4 为 16777216，float8 保留 16777217；直接 float4 输入 7.038531e-26 对照官方结果，拒绝二次舍入。默认可逆文本→同宽解码位型一致，binary 长度/字节准确。 | pg_binary_native_results 仅 1.5_f32；完整 text/binary 往返、默认拼写未验证。 |
| FLOAT-RANGE | 各宽度 ±最大有限、±最小正规及其相邻位型，±最小次正规（位型 00000001 / 0000000000000001 加符号）均成功；text 1e39→float4、1e309→float8 拒绝 22003；1e-100→float4、1e-400→float8 下溢为零拒绝。binary 对应无穷位型成功，不能按 text 溢出拒绝。 | 有网络序转换代码，无完整边界/错误断言，未验证。 |
| FLOAT-SPECIAL-ZERO | 两宽度 text/binary 的 +0/-0 符号保留，float4 binary 00000000/80000000、float8 0000000000000000/8000000000000000；NaN/±Infinity/±Inf 文本输入，输出规范词，非 NULL。NaN 与自身相等、±0 相等的 SQL 比较由工作流 4 验收。 | 仅 float8 +∞ binary 输出意图；负无穷、NaN、带符号零和比较未验证；payload 保存无承诺。 |
| FLOAT-INVALID | 空串、1x、NaNx 拒绝 22P02，非法编码 22021；float4 的 0/3/5 字节、float8 的 0/7/9 字节分别覆盖不足/额外错误；NULL 不走长度错误路径。 | 现有 binary 长度一律 22P03，与目标短载荷分类有差距；精确 wire 错误未验证。 |
| FLOAT-CAST | float4→float8→float4 保留有限位型；float8 1e39 窄化溢出、1e-100 窄化变零均 22003；±2.5→int4 在最近偶数平台为 ±2，±3.5 为 ±4；NaN/∞转整数拒绝。 | 共享转换/格式化测试不能证明 PG cast 或双方言隔离，全部未验证；类型资格交转换块。 |
| FLOAT-NULL-METADATA | 两宽度 typed NULL 参数、首行/全 NULL、空结果及跨边界多行；ParameterDescription 700/701、statement/portal Describe 与 Execute 的 OID/typlen/typmod 连续，NULL 长度 -1，格式变化不改类型。 | pg_binary_native_results 有两宽度 NULL 长度 -1 的局部输出意图；参数 NULL tp=6 与共用 f64 值盒仍不证明身份连续；完整客户端证据未验证。 |

[文本类型契约](#文本类型契约)以下 TEXT 用例族为待新增，按第 1 章登记两方言独立 case；所有运行行为**未验证**。PG 基线为 UTF-8，text/binary 参数和结果均检查，成功 SQLSTATE 不适用。MySQL 基线固定 utf8mb4、具体 collation、sql_mode 和 CHAR 取回模式，记录其独立错误码/SQLSTATE/告警，不从 PG 推导。现有 hello、NULL 和目录 char 断言仅为静态入口。

| 场景 | PG 可观察目标 | MySQL 对照与证据状态 |
| --- | --- | --- |
| TEXT-EMPTY-NULL | text/varchar(2) 空串结果长度 0；char(2) 空串成为两个空格；三类 typed NULL 长度 -1，无载荷，元数据仍保留。 | 自身空串/NULL 分开，CHAR 取回的尾空格按已记录模式验收；完整三类型两格式缺证。 |
| TEXT-MULTIBYTE | 中🙂 在 varchar(2) 成功，UTF-8 字符数 2、字节数 7；e 加 U+0301 是 2 字符，不能归一化当 1；char(3) 的中补两个空格，载荷 5 字节。 | utf8mb4 下字符限制与协议字节宽度分别断言，不复用 PG typmod；未验证。 |
| TEXT-LENGTH-CAST | 对 varchar(2)/char(2) 赋值 ab 成功，abc 拒绝 22001，ab 加一个 ASCII 空格允许裁掉；ab 后加制表符拒绝。显式 cast abc 到两者得到 ab；char(3) 输入 a 输出 a 后跟两空格。n=0、10485761 声明拒绝 22023；n=1、10485760 声明接受，不代表极大载荷已测。 | 自身 VARCHAR/CHAR 长度边界分别在严格及非严格模式采集拒绝或截断/告警，禁止用 PG cast 规则替代；全部未验证。 |
| TEXT-SPACES | text/varchar 的 a 后跟空格在两格式输出保留；char(3) 转 text/varchar 后为 a；char 比较忽略尾空格的责任交工作流 4，输出字节不先 trim。 | 分开验收 CHAR 取回和 VARCHAR 保留，以及 collation 的 PAD 属性；不能仅凭结果相同推断比较等价，未验证。 |
| TEXT-INVALID | Bind 两格式的 61 00 62、FF、C3 28 拒绝 22021；不得输出替换字符。合法多字节不能被字节截断。结果方向用构造非法内部值的候选测试检查提前拒绝与错误传输。 | NUL 的合法性、非法 utf8mb4 的错误/告警独立采集，不套 PG 拒绝；当前 PG 输入校验和结果 NUL 校验有源码证据，运行未验证。 |
| TEXT-METADATA | 声明 text/varchar(2)/char(2) 依次 25/1043/1042，typlen=-1，typmod=-1/6/6；无长度 varchar 为 1043/-1/-1，SQL char 为 1042/-1/5。检查无约束 bpchar、typed NULL、零行、重复 Execute 和两种结果格式，OID 不随值改变。 | 检查自身 type code、charset、flags、长度，不含 PG typmod。普通结果目前折 text，目录 1042 测试不能代替；全链路未验证。 |

### 后续细分

[规范类型与协议映射](#规范类型与协议映射)仅给候选边界：先取证 FieldType/NativeType 能保留的身份、修饰符与参数生命周期，再决定最小内部表示补充；cast 候选依赖该身份并先分方言确定转换规则；参数 codec 候选依赖确定的参数类型及值域；结果 codec 候选依赖相同的准备/执行列类型契约。各族顺序服从本章后续文档，不生成可执行代码任务、不递归扩建共享内核。协议消息/恢复的实现由工作流 7 独立承接。

来源步骤覆盖：来源任务步骤 1 的“规范类型↔MySQL/PG 映射、推导、参数/结果、NULL”由两个方向表与共同约束覆盖；本编号任务要求的方案比较由保真缺口决策覆盖；验收五边界由 TYPE-MAP-IDENTITY/EMPTY/NULL/UNSIGNED/UNKNOWN 覆盖。来源步骤 2–4 的逐族完整语义、cast 与精确 codec 留在本章其余编号文档块，本块映射索引不替代那些交付。第 1 章证据规则与第 2 章类型/数组语法交接保持只读，未发现需要改写前序契约的冲突。

[布尔类型契约](#布尔类型契约)候选边界为：内部表示先验证 bool 身份能随字面量、声明列、有类型 NULL 和参数跨 Prepare/Execute 保留；cast 依赖该身份并区分显式、赋值与隐式上下文；参数 codec 依赖已定型 OID，补齐文本词集与二进制接收域；结果 codec 依赖同一列身份并保持 Describe/Execute 一致。这里只描述候选边界与依赖，不生成可执行实现任务。

布尔块来源覆盖：来源步骤 2 的 bool 族由方向表覆盖；步骤 3 的 bool 显式/隐式边界由转换段及 BOOL-CAST 覆盖；步骤 4 的 text/binary、NULL、非法输入、元数据一致性由 BOOL-TEXT/BINARY/NULL-DESCRIBE 覆盖。与前块的“不能自动将非零整数视为 bool”一致，显式 int4 cast 和 OID 16 单字节接收是有来源的限定场景；未发现需改动第 1/2 章或来源任务的冲突。

[整数与无符号边界](#整数与无符号边界)候选边界：内部表示先验证声明宽度、signed/unsigned 与 typed NULL 跨 Prepare/Execute 保留；cast 依赖身份及方言上下文，独立核对窄化与负值；参数 codec 依赖已定型 OID，补输入语法及错误分类；结果 codec 依赖稳定结果元数据，U64 binary 另依赖 numeric 契约。本块不生成可执行实现任务。来源步骤 2 的整数族由值域表及方向表覆盖，步骤 3 的 unsigned/溢出由桥接及算术交接覆盖，步骤 4 的格式/NULL/非法输入/元数据由 INT-RANGE/UNSIGNED/CODEC/NULL-METADATA/CAST-ARITH 覆盖。未发现与前序布尔限定场景或第 1/2 章冲突；功能、参考端与性能均未实测。

[精确数值契约](#精确数值契约)候选边界：先核定 MyDecimal 与 cast Decimal 可保真域及规范身份的 precision/scale/特殊值缺口，再决定内部表示；cast 依赖该表示和方言资格，保留 PG 舍入/错误及 MySQL 原行为分流；参数 codec 依赖已定型 OID 和上下文 typmod，结果 codec 依赖同一身份及稳定 Describe 元数据。这里只给边界与依赖，不生成可执行任务。来源步骤 2 的 numeric 族由身份与方向表覆盖，步骤 3 的精度溢出/cast 由 NUM-PRECISION/ROUND 覆盖，步骤 4 的格式、非法输入、NULL 与元数据由 NUM-TEXT-SPECIAL/BINARY/NULL-METADATA 覆盖；NUM-LARGE 承接前块 U64 与更大精度差距。未发现前块或第 1/2 章冲突；本轮无功能、性能及参考端实测。

[浮点数值契约](#浮点数值契约)候选边界：内部表示先核对 KindFloat32、共享 f64 值盒与 NativeType 的定宽不变量及 typed NULL 保真；cast 依赖该不变量和方言上下文，分别核对舍入/范围与特殊值；参数 codec 依赖确定 OID，直接按目标宽度解析并分类错误；结果 codec 依赖同一列身份和输出配置，分别输出目标宽度文本及网络位型。这里只给边界和依赖，不生成实现任务。来源步骤 2 的浮点族由身份/方向表覆盖，步骤 3 的舍入和溢出由 FLOAT-RANGE/CAST 覆盖，步骤 4 的格式、NULL、非法输入和元数据由其余验收入口覆盖。与 numeric 的精确性和半值规则分开，与第 1/2 章术语及前序块无冲突；没有功能或性能实测。

[文本类型契约](#文本类型契约)候选边界与依赖为：内部表示先核对身份、声明字符长度、charset/collation 与 typed NULL 的连续性；cast 在该身份基础上区分显式截断、赋值拒绝和 char 去尾空格；参数 codec 依赖 OID 定型与 SQL 目标约束，分别校验编码/NULL；结果 codec 依赖同一列身份和编码状态，保持 OID/typmod 与补空格字节一致。这里只描述边界，不生成实现任务。来源步骤 2 由类型/方向表覆盖，步骤 3 由长度 coercion/cast 规则覆盖，步骤 4 由 TEXT 用例族覆盖；与第 1/2 章及前五块无契约冲突。无功能、兼容性或性能实测。

## 4. 方言函数与运算符契约

## 5. SQL会话与事务语义

## 6. 系统目录与内省契约

## 7. 线协议认证与数据通道

## 8. 客户端与双方言共存验收
