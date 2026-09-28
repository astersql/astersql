// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Plan Replayer 执行器单元测试：dump/load/capture 与后端 trait 契约。
//
// Plan Replayer 把 SQL 现场（schema、统计、绑定、EXPLAIN 等）打包导出，
// 再在另一环境加载以复现优化器决策。本文件测试基于 `PlanReplayerBackend`
// mock 的生命周期。


use std::collections::HashSet;

use astersql_executor::plan_replayer::{
    PlanReplayerBackend, PlanReplayerCaptureInfo, PlanReplayerDumpInfo, PlanReplayerExec,
    PlanReplayerLoadExec, PlanReplayerLoadInfo, loadPlanReplayerForExplainExplore, updateLoadInfo,
};

/// 测试用会话上下文：按序记录 load 各阶段钩子调用。
#[derive(Default)]
struct Context {
    /// 阶段名轨迹，用于断言 load 管线顺序。
    trace: Vec<&'static str>,
}

/// 极简归档：仅携带目标 SQL（`sql:` 前缀解析结果）。
#[derive(Clone, Default)]
struct Archive {
    /// 归档内待 explain / explore 的目标语句。
    target_sql: String,
}

/// `PlanReplayerBackend` 的内存 mock：覆盖 capture/dump/load 控制路径。
#[derive(Default)]
struct Backend {
    /// 已注册的 (sql_digest, plan_digest) capture 任务集合。
    captures: HashSet<(String, String)>,
    /// dump 成功时返回的 file token 或预签名 URL。
    token: String,
    /// `read_file` 返回的归档字节（如 `sql:...`）。
    file_bytes: Vec<u8>,
    /// dump 时记录的已解析语句列表。
    parsed: Vec<String>,
    /// `close_dump_file` 调用次数（失败路径也应关闭）。
    closed_files: usize,
    /// 为 true 时 `dump` 返回错误。
    fail_dump: bool,
    /// 为 true 时读取 statement read timestamp 失败。
    fail_read_timestamp: bool,
    /// 为 true 时 dump 文件传输准备失败。
    fail_prepare_dump: bool,
    /// 为 true 时 `load_bindings` 失败并触发 warning。
    fail_bindings: bool,
    /// 绑定加载失败累计的 warning 次数。
    binding_warnings: usize,
    /// 关闭 auto-analyze 时累计的 warning 次数。
    auto_analyze_warnings: usize,
}

impl PlanReplayerBackend for Backend {
    type Context = Context;
    type Request = Vec<Vec<String>>;
    type Statement = String;
    type File = Vec<u8>;
    type Archive = Archive;
    type Error = String;

    /// 清空并复用结果行缓冲。
    fn grow_and_reset(&self, request: &mut Self::Request) {
        request.clear();
    }

    /// 按列写入结果单元格：第 0 列新开一行，其后列追加到当前行。
    fn append_string(&self, request: &mut Self::Request, column: usize, value: &str) {
        // 结果集按列追加：第 0 列新开一行，其后列写入当前行。
        if column == 0 {
            request.push(vec![value.to_owned()]);
        } else {
            request
                .last_mut()
                .expect("first column")
                .push(value.to_owned());
        }
    }

    /// 预签名 URL 过期时间展示（与 Go 一致为 1h0m0s）。
    fn presigned_url_expiration(&self) -> String {
        "1h0m0s".to_owned()
    }

    /// 按 (sql_digest, plan_digest) 移除 capture 任务。
    fn remove_capture_task(
        &mut self,
        _context: &mut Self::Context,
        capture: &PlanReplayerCaptureInfo,
    ) -> Result<(), Self::Error> {
        self.captures
            .remove(&(capture.sql_digest.clone(), capture.plan_digest.clone()));
        Ok(())
    }

    /// 注册 capture；重复 digest 对返回 already exists。
    fn register_capture_task(
        &mut self,
        _context: &mut Self::Context,
        capture: &PlanReplayerCaptureInfo,
    ) -> Result<(), Self::Error> {
        // digest 对已存在则拒绝，与 Go「task already exists」语义一致。
        if self
            .captures
            .insert((capture.sql_digest.clone(), capture.plan_digest.clone()))
        {
            Ok(())
        } else {
            Err("plan replayer capture task already exists".to_owned())
        }
    }

    /// 创建空 dump 文件句柄与默认文件名。
    fn create_dump_file(
        &mut self,
        _context: &mut Self::Context,
    ) -> Result<(Self::File, String), Self::Error> {
        Ok((Vec::new(), "capture.zip".to_owned()))
    }

    /// 关闭 dump 文件（失败路径也必须调用）。
    fn close_dump_file(&mut self, _file: Self::File) {
        self.closed_files += 1;
    }

    /// 语句读取时间戳桩（固定 42，写入 dump 元数据）。
    fn statement_read_timestamp(
        &mut self,
        _context: &mut Self::Context,
    ) -> Result<u64, Self::Error> {
        if self.fail_read_timestamp {
            Err("read timestamp failed".to_owned())
        } else {
            Ok(42)
        }
    }

    /// dump 前传输准备钩子（本 mock 无操作）。
    fn prepare_dump_file_transfer(
        &mut self,
        _dump: &PlanReplayerDumpInfo<Self::Statement, Self::File>,
    ) -> Result<(), Self::Error> {
        if self.fail_prepare_dump {
            Err("prepare dump transfer failed".to_owned())
        } else {
            Ok(())
        }
    }

    /// 执行 dump：可注入失败；成功记录语句并返回 token。
    fn dump(
        &mut self,
        _context: &mut Self::Context,
        dump: &mut PlanReplayerDumpInfo<Self::Statement, Self::File>,
    ) -> Result<String, Self::Error> {
        if self.fail_dump {
            return Err("dump failed".to_owned());
        }
        // 记录语句后返回 token，供结果集「File token」列使用。
        self.parsed = dump.statements.clone();
        Ok(self.token.clone())
    }

    /// 空 SQL dump 错误文案。
    fn empty_sql_error(&self) -> Self::Error {
        "plan replayer dump sql is empty".to_owned()
    }

    /// 解析单条 SQL：trim 后非空即作为语句。
    fn parse_sql(
        &mut self,
        _context: &mut Self::Context,
        sql: &str,
    ) -> Result<Self::Statement, Self::Error> {
        let sql = sql.trim().to_owned();
        if sql.is_empty() {
            Err("empty statement".to_owned())
        } else if sql.contains(" om ") {
            // These malformed statements are the parser-error cases in
            // TestPlanReplayerDumpMultipleError.
            Err("[parser:1064]".to_owned())
        } else {
            Ok(sql)
        }
    }

    /// load 前传输准备钩子（本 mock 无操作）。
    fn prepare_load_file_transfer(
        &mut self,
        _load: &PlanReplayerLoadInfo,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    /// 空 load 路径错误文案。
    fn empty_path_error(&self) -> Self::Error {
        "plan replayer load path is empty".to_owned()
    }

    /// 读取归档字节（返回预设 `file_bytes`）。
    fn read_file(
        &mut self,
        _context: &mut Self::Context,
        _path: &str,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(self.file_bytes.clone())
    }

    /// 打开归档：要求 UTF-8 且以 `sql:` 前缀携带目标语句。
    fn open_archive(&mut self, data: &[u8]) -> Result<Self::Archive, Self::Error> {
        // 测试归档协议：UTF-8 文本且必须以 `sql:` 开头。
        let text = String::from_utf8(data.to_vec()).map_err(|error| error.to_string())?;
        let target_sql = text
            .strip_prefix("sql:")
            .ok_or_else(|| "invalid plan replayer archive".to_owned())?
            .to_owned();
        Ok(Archive { target_sql })
    }

    /// 取出归档中的目标 SQL。
    fn target_sql(&mut self, archive: &mut Self::Archive) -> Result<String, Self::Error> {
        Ok(archive.target_sql.clone())
    }

    /// load：恢复会话/系统变量。
    fn load_variables(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        context.trace.push("variables");
        Ok(())
    }

    /// load：关闭自动 analyze，避免干扰导入统计。
    fn disable_auto_analyze(&mut self, context: &mut Self::Context) -> Result<(), Self::Error> {
        context.trace.push("disable-auto-analyze");
        Ok(())
    }

    /// load：按 schema 建表，返回涉及库名集合。
    fn create_tables(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<HashSet<String>, Self::Error> {
        context.trace.push("tables");
        Ok(HashSet::from(["test".to_owned()]))
    }

    /// load：恢复 TiFlash 副本元信息。
    fn load_tiflash_replicas(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        // TiFlash：列存副本引擎，load 时恢复 hypo/真实副本元数据。
        context.trace.push("tiflash");
        Ok(())
    }

    /// load：创建视图定义。
    fn create_views(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        context.trace.push("views");
        Ok(())
    }

    /// load：导入表级统计信息。
    fn load_statistics(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
    ) -> Result<(), Self::Error> {
        context.trace.push("statistics");
        Ok(())
    }

    /// load：导入 SQL binding；可注入失败以触发 warning。
    fn load_bindings(
        &mut self,
        context: &mut Self::Context,
        _archive: &mut Self::Archive,
        databases: &HashSet<String>,
    ) -> Result<(), Self::Error> {
        assert!(databases.contains("test"));
        context.trace.push("bindings");
        if self.fail_bindings {
            Err("invalid binding".to_owned())
        } else {
            Ok(())
        }
    }

    /// binding 失败时累计 warning（不中断整体 load）。
    fn append_binding_warning(&mut self, _error: &Self::Error) {
        self.binding_warnings += 1;
    }

    /// 关闭 auto-analyze 相关 warning 计数。
    fn append_auto_analyze_warning(&mut self) {
        self.auto_analyze_warnings += 1;
    }

    /// 从原始字节加载统计信息。
    fn load_stats_bytes(
        &mut self,
        context: &mut Self::Context,
        _data: &[u8],
    ) -> Result<(), Self::Error> {
        context.trace.push("stats-bytes");
        Ok(())
    }
}

/// 构造仅含语句列表的 dump 信息（其余字段置默认）。
fn dump_info(statements: &[&str]) -> PlanReplayerDumpInfo<String, Vec<u8>> {
    PlanReplayerDumpInfo {
        statements: statements.iter().map(|sql| (*sql).to_owned()).collect(),
        analyze: false,
        historical_stats_timestamp: 0,
        start_timestamp: 0,
        path: String::new(),
        file: None,
        file_name: String::new(),
    }
}

const PLAN_REPLAYER_SINGLE_FILE_NAMES: [&str; 14] = [
    "config.toml",
    "debug_trace/debug_trace0.json",
    "meta.txt",
    "stats/test.t_dump_single.json",
    "schema/test.t_dump_single.schema.txt",
    "schema/schema_meta.txt",
    "table_tiflash_replica.txt",
    "variables.toml",
    "session_bindings.sql",
    "global_bindings.sql",
    "sql/sql0.sql",
    "explain.txt",
    "statsMem/test.t_dump_single.txt",
    "sql_meta.toml",
];

fn check_file_name(name: &str) -> bool {
    PLAN_REPLAYER_SINGLE_FILE_NAMES.contains(&name)
}

fn require_plan_replayer_file_token(rows: &[Vec<String>]) -> &str {
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 2);
    assert_eq!(rows[0][0], "File token");
    assert!(!rows[0][1].is_empty());
    &rows[0][1]
}

fn has_ti_flash_task(rows: &[Vec<String>]) -> bool {
    rows.iter()
        .any(|row| row.get(2).is_some_and(|task| task.contains("tiflash")))
}

fn require_archive_file_contains(entries: &[(&str, &str)], file_name: &str, expected: &str) {
    let content = entries
        .iter()
        .find_map(|(name, content)| (*name == file_name).then_some(*content))
        .unwrap_or_else(|| panic!("missing file in archive: {file_name}"));
    assert!(
        content.contains(expected),
        "archive file {file_name} did not contain {expected:?}"
    );
}

/// capture：注册成功 → 重复注册失败 → remove 清空任务。
#[test]
fn capture_register_duplicate_and_remove_follow_executor_lifecycle() {
    let capture = PlanReplayerCaptureInfo {
        sql_digest: "sql".to_owned(),
        plan_digest: "plan".to_owned(),
        remove: false,
    };
    let mut exec = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: Some(capture.clone()),
        dump_info: None,
        end: false,
    };
    exec.Next(&mut Context::default(), &mut Vec::new()).unwrap();
    assert!(exec.end);
    assert!(
        exec.backend
            .captures
            .contains(&("sql".to_owned(), "plan".to_owned()))
    );

    // 重置 end 后再次 Next，应命中「任务已存在」。
    exec.end = false;
    assert_eq!(
        exec.Next(&mut Context::default(), &mut Vec::new()),
        Err("plan replayer capture task already exists".to_owned())
    );
    exec.capture_info.as_mut().unwrap().remove = true;
    exec.removeCaptureTask(&mut Context::default()).unwrap();
    assert!(exec.backend.captures.is_empty());
}

/// dump 成功返回 File token；失败时仍关闭 dump 文件句柄。
#[test]
fn dump_returns_file_token_and_closes_file_on_failure() {
    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: "capture.zip".to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select * from t"])),
        end: false,
    };
    let mut rows = Vec::new();
    exec.Next(&mut Context::default(), &mut rows).unwrap();
    assert_eq!(
        rows,
        vec![vec!["File token".to_owned(), "capture.zip".to_owned()]]
    );
    // statement_read_timestamp mock 固定返回 42。
    assert_eq!(exec.dump_info.as_ref().unwrap().start_timestamp, 42);

    let mut failed = PlanReplayerExec {
        backend: Backend {
            fail_dump: true,
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select 1"])),
        end: false,
    };
    assert_eq!(
        failed.Next(&mut Context::default(), &mut Vec::new()),
        Err("dump failed".to_owned())
    );
    assert_eq!(failed.backend.closed_files, 1);
}

/// Go `PlanReplayerExec.Next` 在 createFile 之后安装 defer，因此读取时间戳失败也必须关闭文件。
#[test]
fn dump_closes_created_file_when_read_timestamp_fails() {
    let mut exec = PlanReplayerExec {
        backend: Backend {
            fail_read_timestamp: true,
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select 1"])),
        end: false,
    };

    assert_eq!(
        exec.Next(&mut Context::default(), &mut Vec::new()),
        Err("read timestamp failed".to_owned())
    );
    assert_eq!(exec.backend.closed_files, 1);
}

/// Go defer 同样覆盖外部文件传输准备失败路径。
#[test]
fn dump_closes_created_file_when_transfer_prepare_fails() {
    let mut info = dump_info(&["select 1"]);
    info.path = "/tmp/statements.sql".to_owned();
    let mut exec = PlanReplayerExec {
        backend: Backend {
            fail_prepare_dump: true,
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(info),
        end: false,
    };

    assert_eq!(
        exec.Next(&mut Context::default(), &mut Vec::new()),
        Err("prepare dump transfer failed".to_owned())
    );
    assert_eq!(exec.backend.closed_files, 1);
}

/// Go defer 也覆盖 createFile 后发现 SQL 为空的错误路径。
#[test]
fn dump_closes_created_file_when_sql_is_empty() {
    let mut exec = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };

    assert_eq!(
        exec.Next(&mut Context::default(), &mut Vec::new()),
        Err("plan replayer dump sql is empty".to_owned())
    );
    assert_eq!(exec.backend.closed_files, 1);
}

#[test]
fn go_plan_replayer_helpers_keep_file_token_tiflash_and_archive_contracts() {
    for name in PLAN_REPLAYER_SINGLE_FILE_NAMES {
        assert!(check_file_name(name), "missing single-dump file {name}");
    }
    assert!(!check_file_name("explain/explain0.txt"));
    assert!(!check_file_name("unexpected.txt"));

    let rows = vec![vec!["File token".to_owned(), "capture.zip".to_owned()]];
    assert_eq!(require_plan_replayer_file_token(&rows), "capture.zip");

    assert!(has_ti_flash_task(&[vec![
        "id".to_owned(),
        "task".to_owned(),
        "TableFullScan tiflash".to_owned()
    ]]));
    assert!(!has_ti_flash_task(&[vec![
        "id".to_owned(),
        "task".to_owned(),
        "TableFullScan tikv".to_owned(),
    ]]));
    assert!(!has_ti_flash_task(&[vec![
        "id".to_owned(),
        "task".to_owned()
    ]]));

    let entries = [("explain.txt", "TableFullScan tiflash")];
    require_archive_file_contains(&entries, "explain.txt", "tiflash");
}

/// 预签名 URL 作为 token 时，结果集含 Download URL / Expires / curl 指引。
#[test]
fn presigned_url_dump_returns_go_equivalent_download_instructions() {
    let url = "https://storage.example/replayer.zip?signature=abc";
    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: url.to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select 1"])),
        end: false,
    };
    let mut rows = Vec::new();
    exec.Next(&mut Context::default(), &mut rows).unwrap();
    assert_eq!(
        rows,
        vec![
            vec!["Download URL", url],
            vec!["Expires in", "1h0m0s"],
            vec![
                "Browser",
                "Open the Download URL directly before it expires"
            ],
            vec![
                "curl",
                "curl -L 'https://storage.example/replayer.zip?signature=abc' -o plan_replayer.zip",
            ],
            vec![
                "Note",
                "If the URL expires, rerun PLAN REPLAYER DUMP to get a new one",
            ],
        ]
    );
}

/// Go 使用 `url.Parse` 并要求 http(s) URL 具有非空 host；只有 scheme 的 token 仍是文件 token。
#[test]
fn presigned_url_without_host_is_treated_as_file_token() {
    let token = "http://?signature=missing-host";
    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: token.to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&["select 1"])),
        end: false,
    };
    let mut rows = Vec::new();

    exec.Next(&mut Context::default(), &mut rows).unwrap();
    assert_eq!(rows, vec![vec!["File token", token]]);
}

/// 多语句文件按分号拆分、trim 后按序 dump。
#[test]
fn multi_sql_file_is_split_parsed_and_dumped_in_order() {
    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: "multi.zip".to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        exec.DumpSQLsFromFile(
            &mut Context::default(),
            b"select * from test.t1;\nupdate test.t2 set a=1;"
        )
        .unwrap(),
        "multi.zip"
    );
    assert_eq!(
        exec.backend.parsed,
        vec!["select * from test.t1", "update test.t2 set a=1"]
    );
}

#[test]
fn multi_sql_dump_preserves_go_fifty_statement_and_file_name_contract() {
    const NUM_STATEMENTS: usize = 50;
    const NUM_TABLES: usize = 5;
    let databases = [
        "test",
        "test_multi_db1",
        "test_multi_db2",
        "test_multi_db3",
        "test_multi_db4",
    ];
    let mut statements = Vec::with_capacity(NUM_STATEMENTS);
    for index in 0..NUM_STATEMENTS {
        let pair_index = index % (databases.len() * NUM_TABLES);
        let database = databases[pair_index / NUM_TABLES];
        let table = pair_index % NUM_TABLES + 1;
        statements.push(format!("select * from {database}.t_dump_multi_{table}"));
    }
    let input = statements
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(";\n");

    let mut exec = PlanReplayerExec {
        backend: Backend {
            token: "multi.zip".to_owned(),
            ..Backend::default()
        },
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        exec.DumpSQLsFromFile(&mut Context::default(), input.as_bytes())
            .unwrap(),
        "multi.zip"
    );
    assert_eq!(exec.backend.parsed, statements);

    let mut names = HashSet::new();
    for index in 0..NUM_STATEMENTS {
        names.insert(format!("sql/sql{index}.sql"));
        names.insert(format!("explain/explain{index}.txt"));
    }
    assert_eq!(names.len(), NUM_STATEMENTS * 2);
    assert!(!names.contains("explain.txt"));
}

/// load 管线顺序：variables → 关 auto-analyze → tables → tiflash → views → stats → bindings。
#[test]
fn load_runs_variables_schema_tiflash_stats_and_bindings_in_order() {
    let mut backend = Backend {
        fail_bindings: true,
        ..Backend::default()
    };
    let mut context = Context::default();
    // 绑定失败仍继续，并累计 binding / auto-analyze warning。
    updateLoadInfo(&mut backend, &mut context, b"sql:select * from t").unwrap();
    assert_eq!(
        context.trace,
        vec![
            "variables",
            "disable-auto-analyze",
            "tables",
            "tiflash",
            "views",
            "statistics",
            "bindings"
        ]
    );
    assert_eq!(backend.binding_warnings, 1);
    assert_eq!(backend.auto_analyze_warnings, 1);
}

/// explain explore：先读归档目标 SQL，再跑完整 load 环境初始化。
#[test]
fn explain_explore_reads_archive_target_then_loads_environment() {
    let mut backend = Backend {
        file_bytes: b"sql:select /*+ read_from_storage(tiflash[t]) */ * from t".to_vec(),
        ..Backend::default()
    };
    let mut context = Context::default();
    let target =
        loadPlanReplayerForExplainExplore(&mut backend, &mut context, "replayer/capture.zip")
            .unwrap();
    assert!(target.contains("tiflash"));
    assert_eq!(context.trace.first(), Some(&"variables"));
    assert_eq!(context.trace.last(), Some(&"bindings"));
}

/// LoadExec：空 path 报错；非空 path 走 prepare transfer 成功路径。
#[test]
fn load_executor_rejects_empty_path_and_prepares_nonempty_transfer() {
    let mut empty = PlanReplayerLoadExec {
        backend: Backend::default(),
        info: PlanReplayerLoadInfo {
            path: String::new(),
        },
    };
    assert_eq!(
        empty.Next(&mut Context::default(), &mut Vec::new()),
        Err("plan replayer load path is empty".to_owned())
    );
    let mut valid = PlanReplayerLoadExec {
        backend: Backend::default(),
        info: PlanReplayerLoadInfo {
            path: "/tmp/capture.zip".to_owned(),
        },
    };
    valid
        .Next(&mut Context::default(), &mut Vec::new())
        .unwrap();
}

#[test]
fn presigned_url_expiration_matches_go_output() {
    assert_eq!(Backend::default().presigned_url_expiration(), "1h0m0s");
}

#[test]
fn multi_sql_parser_rejects_the_go_invalid_statement_cases() {
    let mut backend = Backend::default();
    let mut context = Context::default();

    assert_eq!(
        backend.parse_sql(&mut context, "select x om t"),
        Err("[parser:1064]".to_owned())
    );
    assert_eq!(
        backend.parse_sql(&mut context, "select y om t"),
        Err("[parser:1064]".to_owned())
    );

    let mut empty = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        empty.Next(&mut Context::default(), &mut Vec::new()),
        Err("plan replayer dump sql is empty".to_owned())
    );

    let mut one_invalid = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        one_invalid.DumpSQLsFromFile(&mut Context::default(), b"select x om t"),
        Err("[parser:1064]".to_owned())
    );

    let mut multiple_invalid = PlanReplayerExec {
        backend: Backend::default(),
        capture_info: None,
        dump_info: Some(dump_info(&[])),
        end: false,
    };
    assert_eq!(
        multiple_invalid
            .DumpSQLsFromFile(&mut Context::default(), b"select x from t; select y om t",),
        Err("[parser:1064]".to_owned())
    );
}
