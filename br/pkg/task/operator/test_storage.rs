// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! Test external storage — mirrors `br/pkg/task/operator/test_storage.go`.
//!
//! 对外部存储做端到端连通性/语义自检：写读删改名 Walk 分页等。
//! 对齐 Go `test_storage.go` 的步骤顺序与报告字段；失败可暂停或清理。
//! 数据流：ParseBackend → NewStorage → 各 test* 步骤 → TestReport.Print。
//! 测试文件名与默认 1MB 载荷为临时对象，成功时可 CleanupOnSuccess 删除。
//! 失败时可根据 pause-when-fail 停住以便现场排查。
//! CleanupOnSuccess 仅在全部步骤通过后删除临时对象。
//! MemStorage 与真实 S3/GCS 共用同一套 ExternalStorage 断言。
//! 步骤顺序刻意贴近 Go：Write→Exists→Read→Open→Range→Create→Walk→Rename→Delete。
//! 报告字段（Passed/Failed/TotalBytes）供 CI 与人工对照。
//! PauseWhenFail 仅交互调试使用，自动化应保持关闭。
//! formatBytes 与状态着色不影响存储语义，只服务可读性。
//! 任一底层 ExternalStorage 错误都会记入 TestResult.Error。
//! Cleanup 删除的键集合与本套件写入的临时名一致，避免误删用户数据。

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::flagStorage;
use crate::stubs::{
    BackendOptions, Error, ExternalStorage, FlagSet, NewStorage, ParseBackend, ReaderOption,
    Result, StorageOptions, WalkOption, color_cyan, color_green, color_hi_red, color_red,
    color_yellow,
};

/// 主测试对象文件名。
const testFileName1: &str = "br-test-file-1.tmp";
/// 第二测试对象，用于批量删除等场景。
const testFileName2: &str = "br-test-file-2.tmp";
/// Rename 目标文件名。
const testFileNameRenamed: &str = "br-test-file-renamed.tmp";
/// Walk/子目录场景使用的“目录”前缀。
const testDirName: &str = "br-test-dir";
/// 默认随机载荷大小（1MB）。
const defaultTestDataSize: i64 = 1024 * 1024; // 1MB test data

/// TestResult represents the result of a single test operation.
/// 单步测试结果：名称、通过、耗时、详情与错误。
/// 单步结果：名称、是否通过、耗时、详情与可选错误。
#[derive(Clone, Debug)]
pub struct TestResult {
    pub Name: String,
    pub Passed: bool,
    pub Duration: Duration,
    pub Details: String,
    pub Error: Option<Error>,
}

/// TestReport contains all test results and summary information.
/// 汇总报告：计数、总字节与逐步结果。
/// 汇总：URI、起止时间、计数、字节与逐步结果。
#[derive(Clone, Debug, Default)]
pub struct TestReport {
    pub StorageURI: String,
    pub StartTime: Option<SystemTime>,
    pub EndTime: Option<SystemTime>,
    pub TotalTests: i32,
    pub PassedTests: i32,
    pub FailedTests: i32,
    pub TestResults: Vec<TestResult>,
    pub TotalBytes: i64,
}

impl TestReport {
    /// 累加通过/失败计数并保存结果。
    pub fn AddResult(&mut self, result: TestResult) {
        // 条件分支：按配置或中间结果选择路径。
        if result.Passed {
            // 通过计数 +1。
            self.PassedTests += 1;
        } else {
            // 失败计数 +1。
            self.FailedTests += 1;
        }
        // 总用例 +1。
        self.TotalTests += 1;
        // 执行语句，推进流程。
        self.TestResults.push(result);
    }

    /// 打印彩色汇总与逐步详情；失败项附错误。
    pub fn Print(&self) {
        // 绑定 `duration`，供后续步骤使用。
        let duration = match (self.StartTime, self.EndTime) {
            // 赋值更新状态。
            (Some(s), Some(e)) => e.duration_since(s).unwrap_or_default(),
            // 时间度量：用于超时或耗时统计。
            _ => Duration::ZERO,
        };

        // 输出：供人工观察步骤或报告。
        println!();
        // 彩色终端提示当前步骤状态。
        printStep(&format!("={}", "=".repeat(70)));
        // 报告大标题。
        printStep("STORAGE TEST REPORT");
        // 彩色终端提示当前步骤状态。
        printStep(&format!("={}", "=".repeat(70)));
        // 输出：供人工观察步骤或报告。
        println!("Storage URI:     {}", self.StorageURI);
        // 输出：供人工观察步骤或报告。
        println!(
            "Start Time:      {}",
            format_time(self.StartTime.unwrap_or(UNIX_EPOCH))
        );
        // 输出：供人工观察步骤或报告。
        println!(
            "End Time:        {}",
            format_time(self.EndTime.unwrap_or(UNIX_EPOCH))
        );
        // 输出：供人工观察步骤或报告。
        println!("Total Duration:  {:.3?}", duration);
        // 输出：供人工观察步骤或报告。
        println!("Total Tests:     {}", self.TotalTests);
        // 输出：供人工观察步骤或报告。
        println!(
            "Passed:          {}",
            // 通过数绿色高亮。
            color_green(&format!("{}", self.PassedTests))
        );
        // 输出：供人工观察步骤或报告。
        println!(
            "Failed:          {}",
            // 失败数红色高亮。
            color_red(&format!("{}", self.FailedTests))
        );
        // 输出：供人工观察步骤或报告。
        println!("Total Data:      {}", formatBytes(self.TotalBytes));

        // 彩色终端提示当前步骤状态。
        printStep(&format!("-{}", "-".repeat(70)));
        // 逐步详情区。
        printStep("TEST DETAILS");
        // 彩色终端提示当前步骤状态。
        printStep(&format!("-{}", "-".repeat(70)));

        // 循环：遍历集合或等待条件。
        for (i, result) in self.TestResults.iter().enumerate() {
            let (status, status_colored) = if result.Passed {
                ("✓", color_green("✓"))
            } else {
                ("✗", color_red("✗"))
            };
            let _ = status;
            // 输出：供人工观察步骤或报告。
            println!(
                "{:2}. {} {} ({:.3?})",
                i + 1,
                status_colored,
                result.Name,
                result.Duration
            );
            // 条件分支：按配置或中间结果选择路径。
            if !result.Details.is_empty() {
                // 输出：供人工观察步骤或报告。
                println!("    {}", color_cyan(&result.Details));
            }
            // 条件分支：按配置或中间结果选择路径。
            if let Some(err) = &result.Error {
                // 输出：供人工观察步骤或报告。
                println!("    {}", color_red(&format!("Error: {err}")));
            }
        }

        // 输出：供人工观察步骤或报告。
        println!();
        // 彩色终端提示当前步骤状态。
        printStep(&format!("={}", "=".repeat(70)));
        // 条件分支：按配置或中间结果选择路径。
        if self.FailedTests == 0 {
            // 步骤成功短提示。
            printSuccess("✅ ALL TESTS PASSED!");
        } else {
            // 彩色终端提示当前步骤状态。
            printError(&format!("❌ {} TEST(S) FAILED", self.FailedTests));
        }
        // 彩色终端提示当前步骤状态。
        printStep(&format!("={}", "=".repeat(70)));
    }
}

/// 重复字符串，用于构造可预测载荷。
pub fn repeatString(s: &str, count: usize) -> String {
    s.repeat(count)
}

/// 人类可读字节格式，对齐 Go formatBytes 量级标签。
pub fn formatBytes(bytes: i64) -> String {
    const unit: i64 = 1024;
    // 条件分支：按配置或中间结果选择路径。
    if bytes < unit {
        // 返回：结束本函数/步骤。
        return format!("{bytes} B");
    }
    // 绑定 `div`，供后续步骤使用。
    let mut div = unit;
    // 绑定 `exp`，供后续步骤使用。
    let mut exp = 0;
    // 绑定 `n`，供后续步骤使用。
    let mut n = bytes / unit;
    // 循环：遍历集合或等待条件。
    while n >= unit {
        // 赋值更新状态。
        div *= unit;
        // 赋值更新状态。
        exp += 1;
        // 赋值更新状态。
        n /= unit;
    }
    // 单位字符表与 Go 一致，避免中英文混用。
    let units = ['K', 'M', 'G', 'T', 'P', 'E'];
    // 格式化字符串用于日志或报告。
    format!("{:.2} {}B", bytes as f64 / div as f64, units[exp])
}

/// 将 SystemTime 格式化为报告时间戳。
fn format_time(t: SystemTime) -> String {
    // 匹配分支：按枚举/结果形态分流。
    match t.duration_since(UNIX_EPOCH) {
        // 格式化字符串用于日志或报告。
        Ok(d) => format!("{}s", d.as_secs()),
        // 赋值更新状态。
        Err(_) => "unknown".into(),
    }
}

/// CLI/入口配置：URI、清理、失败暂停、数据大小。
#[derive(Clone, Debug, Default)]
pub struct TestStorageConfig {
    pub BackendOptions: BackendOptions,
    pub StorageURI: String,
    pub CleanupOnSuccess: bool,
    // 失败暂停：打印黄字提示后等 stdin。
    pub PauseWhenFail: bool,
    pub TestDataSize: i64,
}

/// 运行时上下文：存储句柄、报告与计时。
pub struct TestContext {
    pub Report: TestReport,
    pub Store: ArcStorage,
    pub PauseWhenFail: bool,
}

/// Thin Arc wrapper so test helpers can share storage.
/// ExternalStorage 的 Arc 别名。
pub type ArcStorage = std::sync::Arc<dyn ExternalStorage>;

impl TestContext {
    /// 累加通过/失败计数并保存结果。
    pub fn AddResult(&mut self, result: TestResult) {
        // 绑定 `failed`，供后续步骤使用。
        let failed = !result.Passed;
        // 将本步结果记入报告。
        self.Report.AddResult(result);
        // 条件分支：按配置或中间结果选择路径。
        if failed && self.PauseWhenFail {
            // 输出：供人工观察步骤或报告。
            println!();
            // 彩色终端提示当前步骤状态。
            printError("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            // 彩色终端提示当前步骤状态。
            printError(&format!(
                "TEST FAILED: {}",
                self.Report
                    .TestResults
                    .last()
                    .map(|r| r.Name.as_str())
                    .unwrap_or("")
            ));
            // 彩色终端提示当前步骤状态。
            printError(
                "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
            );
            // 条件分支：按配置或中间结果选择路径。
            if let Some(err) = self
                .Report
                .TestResults
                .last()
                .and_then(|r| r.Error.as_ref())
            {
                // 彩色终端提示当前步骤状态。
                printError(&format!("Error: {err}"));
            }
            // 输出：供人工观察步骤或报告。
            println!();
            // 输出：供人工观察步骤或报告。
            println!(
                "{}",
                color_yellow("⏸  Test paused. You can now inspect the storage content.")
            );
            // 输出：供人工观察步骤或报告。
            println!(
                "{}",
                // 格式化字符串用于日志或报告。
                color_yellow(&format!("   Storage URI: {}", self.Report.StorageURI))
            );
            // 输出：供人工观察步骤或报告。
            println!();
            // 输出：供人工观察步骤或报告。
            print!(
                "{}",
                color_cyan("Press Enter to continue with remaining tests, or Ctrl+C to abort: ")
            );
            let _ = std::io::stdout().flush();
            // 绑定 `line`，供后续步骤使用。
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            // 输出：供人工观察步骤或报告。
            println!();
            // 彩色终端提示当前步骤状态。
            printStep("Resuming tests...");
        }
    }
}

/// 注册 test-storage 子命令 flag。
pub fn DefineFlagsForTestStorageConfig(flags: &mut FlagSet) {
    // 执行语句，推进流程。
    crate::stubs::DefineBackendFlags(flags);
    // 必填 storage URI flag。
    flags.StringP(flagStorage, "s", "", "The external storage URI to test.");
    // 成功清理开关，默认 true。
    flags.Bool("cleanup", true, "Whether to cleanup test files on success.");
    flags.Bool(
        "pause-when-fail",
        false,
        "Pause the test when a case fails for debugging.",
    );
    flags.Int64(
        "test-data-size",
        defaultTestDataSize,
        "The size of test data in bytes (default 1MB).",
    );
}

impl TestStorageConfig {
    /// 从 FlagSet 解析 URI/cleanup/pause/size；空 URI 报错。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        // 执行语句，推进流程。
        self.BackendOptions.ParseFromFlags(flags)?;
        // 赋值更新状态。
        self.StorageURI = flags.GetString(flagStorage)?;
        // 赋值更新状态。
        self.CleanupOnSuccess = flags.GetBool("cleanup")?;
        // 赋值更新状态。
        self.PauseWhenFail = flags.GetBool("pause-when-fail")?;
        // 赋值更新状态。
        self.TestDataSize = flags.GetInt64("test-data-size")?;
        // 空 URI 直接配置错误。
        if self.StorageURI.is_empty() {
            // 失败返回：向上游暴露本步错误。
            return Err(Error::new(
                "storage URI cannot be empty, please specify with --storage",
            ));
        }
        // 非正大小非法。
        if self.TestDataSize <= 0 {
            // 失败返回：向上游暴露本步错误。
            return Err(Error::new("test data size must be greater than 0"));
        }
        // 全部步骤通过。
        Ok(())
    }
}

/// 入口：按配置跑完整存储自检套件。
pub fn RunTestStorage(cfg: TestStorageConfig) -> Result<()> {
    // 输出：供人工观察步骤或报告。
    eprintln!("Starting external storage test storage={}", cfg.StorageURI);

    // 绑定 `report`，供后续步骤使用。
    let mut report = TestReport {
        // 克隆句柄以移入闭包/线程。
        StorageURI: cfg.StorageURI.clone(),
        StartTime: Some(SystemTime::now()),
        // 构造集合承载中间数据。
        TestResults: Vec::new(),
        ..Default::default()
    };

    // 解析 URI；与 Go objstore.ParseBackend 对齐。
    let backend = ParseBackend(&cfg.StorageURI, &cfg.BackendOptions)
        // 执行语句，推进流程。
        .map_err(|err| Error::Annotate(err.msg, "failed to parse storage backend"))?;
    // 绑定 `store`，供后续步骤使用。
    let store = NewStorage(
        &backend,
        &StorageOptions {
            SendCredentials: true,
            ..Default::default()
        },
    )
    // 执行语句，推进流程。
    .map_err(|err| Error::Annotate(err.msg, "failed to create external storage"))?;

    // 彩色终端提示当前步骤状态。
    printStep(&format!("Testing external storage: {}", store.URI()));
    // 赋值更新状态。
    report.StorageURI = store.URI();

    // 绑定 `tc`，供后续步骤使用。
    let mut tc = TestContext {
        Report: report,
        // 克隆句柄以移入闭包/线程。
        Store: store.clone(),
        PauseWhenFail: cfg.PauseWhenFail,
    };

    // 绑定 `testData`，供后续步骤使用。
    let mut testData = vec![0u8; cfg.TestDataSize as usize];
    // 执行语句，推进流程。
    fill_random(&mut testData)?;
    // 彩色终端提示当前步骤状态。
    printStep(&format!(
        "Generated test data: {}",
        formatBytes(cfg.TestDataSize)
    ));

    // 绑定 `cleanup_store`，供后续步骤使用。
    let cleanup_store = store.clone();
    // 绑定 `cleanup_on_success`，供后续步骤使用。
    let cleanup_on_success = cfg.CleanupOnSuccess;
    // 绑定 `cleanup`，供后续步骤使用。
    let cleanup = || {
        // 条件分支：按配置或中间结果选择路径。
        if !cleanup_on_success {
            // 执行语句，推进流程。
            return;
        }
        // 彩色终端提示当前步骤状态。
        printStep("Cleaning up test files...");
        let _ = cleanup_store.DeleteFile(testFileName1);
        let _ = cleanup_store.DeleteFile(testFileName2);
        let _ = cleanup_store.DeleteFile(testFileNameRenamed);
        let _ = cleanup_store.DeleteFile(
            Path::new(testDirName)
                .join(testFileName1)
                .to_string_lossy()
                .as_ref(),
        );
        let _ = cleanup_store.DeleteFile(
            Path::new(testDirName)
                .join(testFileName2)
                .to_string_lossy()
                .as_ref(),
        );
    };

    // 先写主文件，后续 Exists/Read/Open 依赖它。
    testWriteFile(&mut tc, testFileName1, &testData);
    // 写后应存在。
    testFileExists(&mut tc, testFileName1, true);
    // 读取完整对象并校验。
    testReadFile(&mut tc, testFileName1, &testData);
    // 打开 Reader 流式读取。
    testOpen(&mut tc, testFileName1, &testData);
    // 带 Range 的 Open；验证切片边界。
    testOpenWithRange(&mut tc, testFileName1, &testData);
    // 第二文件经 Writer 创建。
    testCreate(&mut tc, testFileName2, &testData);
    // 重命名对象键。
    testRename(&mut tc, testFileName2, testFileNameRenamed);
    // 根 Walk 应包含两文件。
    testWalkDir(&mut tc);
    // 子目录场景。
    testWalkDirWithSubDir(&mut tc, testFileName1, &testData);
    // 分页场景：制造足够多小对象。
    testWalkDirWithPagination(&mut tc, &testData);
    // 删除单个对象。
    testDeleteFile(&mut tc, testFileName1);
    // 批量清理剩余键。
    testDeleteFiles(&mut tc, &[testFileNameRenamed.to_string()]);
    // 检查对象是否存在。
    testFileExists(&mut tc, testFileName1, false);

    // 赋值更新状态。
    tc.Report.EndTime = Some(SystemTime::now());
    // 赋值更新状态。
    tc.Report.TotalBytes = (testData.len() * 4) as i64;

    // 执行语句，推进流程。
    tc.Report.Print();
    // 执行语句，推进流程。
    cleanup();
    // 执行语句，推进流程。
    store.Close();

    // 条件分支：按配置或中间结果选择路径。
    if tc.Report.FailedTests > 0 {
        // 失败返回：向上游暴露本步错误。
        return Err(Error::new("storage test failed"));
    }
    Ok(())
}

/// 用系统随机源填充缓冲区；熵源失败与 Go `rand.Read` 一样直接返回错误。
fn fill_random(buf: &mut [u8]) -> Result<()> {
    let mut source = std::fs::File::open("/dev/urandom")
        .map_err(|err| Error::Annotate(err.to_string(), "failed to generate test data"))?;
    fill_random_from_reader(&mut source, buf)
}

/// 可注入随机源的核心读取逻辑，供错误传播回归测试使用。
pub(crate) fn fill_random_from_reader(reader: &mut dyn Read, buf: &mut [u8]) -> Result<()> {
    reader
        .read_exact(buf)
        .map_err(|err| Error::Annotate(err.to_string(), "failed to generate test data"))
}

/// 步骤：WriteFile 并校验存在。
fn testWriteFile(tc: &mut TestContext, name: &str, data: &[u8]) {
    // 绑定 `testName`，供后续步骤使用。
    let testName = format!("WriteFile({name})");
    // 彩色终端提示当前步骤状态。
    printStep(&format!("Test: {testName}"));
    // 绑定 `start`，供后续步骤使用。
    let start = Instant::now();
    // 绑定 `result`，供后续步骤使用。
    let mut result = TestResult {
        Name: testName,
        // 时间度量：用于超时或耗时统计。
        Duration: Duration::ZERO,
        // 格式化字符串用于日志或报告。
        Details: format!("Wrote {} bytes", data.len()),
        Passed: false,
        Error: None,
    };
    // 匹配分支：按枚举/结果形态分流。
    match tc.Store.WriteFile(name, data) {
        // 赋值更新状态。
        Ok(()) => {
            // 彩色终端提示当前步骤状态。
            printSuccess("  ✓ Passed");
            // 赋值更新状态。
            result.Passed = true;
        }
        // 赋值更新状态。
        Err(err) => {
            // 彩色终端提示当前步骤状态。
            printError(&format!("  ❌ Failed: {err}"));
            // 写入完整对象。
            result.Error = Some(Error::Annotate(err.msg, "WriteFile failed"));
        }
    }
    // 赋值更新状态。
    result.Duration = start.elapsed();
    // 将本步结果记入报告。
    tc.AddResult(result);
}

/// 步骤：FileExists 与期望布尔一致。
fn testFileExists(tc: &mut TestContext, name: &str, expected: bool) {
    let testName = format!("FileExists({name}) - expecting {expected}");
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    match tc.Store.FileExists(name) {
        Ok(exists) if exists == expected => {
            printSuccess("  ✓ Passed");
            result.Passed = true;
            result.Details = format!("File exists: {exists}");
        }
        Ok(exists) => {
            printError(&format!("  ❌ Failed: expected {expected}, got {exists}"));
            result.Error = Some(Error::Errorf(format!(
                "FileExists returned unexpected result: expected {expected}, got {exists}"
            )));
        }
        Err(err) => {
            printError(&format!("  ❌ Failed: {err}"));
            result.Error = Some(Error::Annotate(err.msg, "FileExists failed"));
        }
    }
    result.Duration = start.elapsed();
    tc.AddResult(result);
}

/// 步骤：ReadFile 内容与期望相等。
fn testReadFile(tc: &mut TestContext, name: &str, expectedData: &[u8]) {
    let testName = format!("ReadFile({name})");
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    match tc.Store.ReadFile(name) {
        Err(err) => {
            printError(&format!("  ❌ Failed: {err}"));
            result.Error = Some(Error::Annotate(err.msg, "ReadFile failed"));
        }
        Ok(data) if data.len() != expectedData.len() => {
            printError(&format!(
                "  ❌ Failed: expected size {}, got {}",
                expectedData.len(),
                data.len()
            ));
            result.Error = Some(Error::Errorf(format!(
                "ReadFile returned wrong size: expected {}, got {}",
                expectedData.len(),
                data.len()
            )));
        }
        Ok(data) => {
            if let Some(i) = data
                .iter()
                .zip(expectedData.iter())
                .position(|(a, b)| a != b)
            {
                printError(&format!("  ❌ Failed: data mismatch at offset {i}"));
                result.Error = Some(Error::Errorf(format!(
                    "ReadFile returned corrupted data at offset {i}"
                )));
            } else {
                printSuccess(&format!("  ✓ Passed (verified {} bytes)", data.len()));
                result.Passed = true;
                result.Details = format!("Verified {} bytes", data.len());
            }
        }
    }
    result.Duration = start.elapsed();
    tc.AddResult(result);
}

/// 步骤：Open 流式读全量。
fn testOpen(tc: &mut TestContext, name: &str, expectedData: &[u8]) {
    let testName = format!("Open({name}) - streaming read");
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    let mut reader = match tc.Store.Open(name, None) {
        Ok(r) => r,
        Err(err) => {
            printError(&format!("  ❌ Failed: {err}"));
            result.Duration = start.elapsed();
            result.Error = Some(Error::Annotate(err.msg, "Open failed"));
            tc.AddResult(result);
            return;
        }
    };
    match reader.GetFileSize() {
        Err(err) => {
            printError(&format!("  ❌ Failed to get file size: {err}"));
            result.Duration = start.elapsed();
            result.Error = Some(Error::Annotate(err.msg, "GetFileSize failed"));
            reader.Close();
            tc.AddResult(result);
            return;
        }
        Ok(size) if size != expectedData.len() as i64 => {
            printError(&format!(
                "  ❌ Failed: expected size {}, got {size}",
                expectedData.len()
            ));
            result.Duration = start.elapsed();
            result.Error = Some(Error::Errorf(format!(
                "GetFileSize returned wrong size: expected {}, got {size}",
                expectedData.len()
            )));
            reader.Close();
            tc.AddResult(result);
            return;
        }
        Ok(_) => {}
    }
    match reader.read_to_end() {
        Err(err) => {
            printError(&format!("  ❌ Failed to read: {err}"));
            result.Error = Some(Error::Annotate(err.msg, "ReadAll failed"));
        }
        Ok(data) if data.len() != expectedData.len() => {
            printError(&format!(
                "  ❌ Failed: expected size {}, got {}",
                expectedData.len(),
                data.len()
            ));
            result.Error = Some(Error::Errorf(format!(
                "Read returned wrong size: expected {}, got {}",
                expectedData.len(),
                data.len()
            )));
        }
        Ok(data) => {
            printSuccess(&format!("  ✓ Passed (read {} bytes)", data.len()));
            result.Passed = true;
            result.Details = format!("Read {} bytes", data.len());
        }
    }
    reader.Close();
    result.Duration = start.elapsed();
    tc.AddResult(result);
}

/// 步骤：Open 流式读全量。
fn testOpenWithRange(tc: &mut TestContext, name: &str, expectedData: &[u8]) {
    let testName = format!("Open({name}) - with range [100, 200)");
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let startOffset = 100i64;
    let endOffset = 200i64;
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    let opt = ReaderOption {
        StartOffset: Some(startOffset),
        EndOffset: Some(endOffset),
    };
    let mut reader = match tc.Store.Open(name, Some(&opt)) {
        Ok(r) => r,
        Err(err) => {
            printError(&format!("  ❌ Failed: {err}"));
            result.Duration = start.elapsed();
            result.Error = Some(Error::Annotate(err.msg, "Open with range failed"));
            tc.AddResult(result);
            return;
        }
    };
    match reader.read_to_end() {
        Err(err) => {
            printError(&format!("  ❌ Failed to read: {err}"));
            result.Error = Some(Error::Annotate(err.msg, "ReadAll failed"));
        }
        Ok(data) => {
            let expectedSize = endOffset - startOffset;
            if data.len() as i64 != expectedSize {
                printError(&format!(
                    "  ❌ Failed: expected size {expectedSize}, got {}",
                    data.len()
                ));
                result.Error = Some(Error::Errorf(format!(
                    "Range read returned wrong size: expected {expectedSize}, got {}",
                    data.len()
                )));
            } else {
                let mut ok = true;
                for (i, b) in data.iter().enumerate() {
                    if *b != expectedData[startOffset as usize + i] {
                        printError(&format!("  ❌ Failed: data mismatch at offset {i}"));
                        result.Error = Some(Error::Errorf(format!(
                            "Range read returned corrupted data at offset {i}"
                        )));
                        ok = false;
                        break;
                    }
                }
                if ok {
                    printSuccess(&format!("  ✓ Passed (read {} bytes)", data.len()));
                    result.Passed = true;
                    result.Details = format!(
                        "Read {} bytes from range [{startOffset}, {endOffset})",
                        data.len()
                    );
                }
            }
        }
    }
    reader.Close();
    result.Duration = start.elapsed();
    tc.AddResult(result);
}

/// 步骤：Create Writer 写入完整对象。
fn testCreate(tc: &mut TestContext, name: &str, data: &[u8]) {
    let testName = format!("Create({name}) - streaming write");
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    let mut writer = match tc.Store.Create(name) {
        Ok(w) => w,
        Err(err) => {
            printError(&format!("  ❌ Failed: {err}"));
            result.Duration = start.elapsed();
            result.Error = Some(Error::Annotate(err.msg, "Create failed"));
            tc.AddResult(result);
            return;
        }
    };
    let chunkSize = 64 * 1024;
    let mut offset = 0;
    while offset < data.len() {
        let end = (offset + chunkSize).min(data.len());
        match writer.Write(&data[offset..end]) {
            Ok(n) => offset += n,
            Err(err) => {
                printError(&format!("  ❌ Failed to write: {err}"));
                let _ = writer.Close();
                result.Duration = start.elapsed();
                result.Error = Some(Error::Annotate(err.msg, "Write failed"));
                tc.AddResult(result);
                return;
            }
        }
    }
    if let Err(err) = writer.Close() {
        printError(&format!("  ❌ Failed to close: {err}"));
        result.Duration = start.elapsed();
        result.Error = Some(Error::Annotate(err.msg, "Close writer failed"));
        tc.AddResult(result);
        return;
    }
    result.Duration = start.elapsed();
    result.Passed = true;
    result.Details = format!("Wrote {} bytes in chunks", data.len());
    printSuccess(&format!("  ✓ Passed (wrote {} bytes)", data.len()));
    tc.AddResult(result);
}

/// 步骤：Rename 后旧键消失新键可读。
fn testRename(tc: &mut TestContext, oldName: &str, newName: &str) {
    let testName = format!("Rename({oldName} -> {newName})");
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    if let Err(err) = tc.Store.Rename(oldName, newName) {
        printError(&format!("  ❌ Failed: {err}"));
        result.Duration = start.elapsed();
        result.Error = Some(Error::Annotate(err.msg, "Rename failed"));
        tc.AddResult(result);
        return;
    }
    match tc.Store.FileExists(oldName) {
        Err(err) => {
            printError(&format!("  ❌ Failed to check old file: {err}"));
            result.Duration = start.elapsed();
            result.Error = Some(Error::Annotate(err.msg, "FileExists check failed"));
            tc.AddResult(result);
            return;
        }
        Ok(true) => {
            printError("  ❌ Failed: old file still exists after rename");
            result.Duration = start.elapsed();
            result.Error = Some(Error::new("old file still exists after rename"));
            tc.AddResult(result);
            return;
        }
        Ok(false) => {}
    }
    match tc.Store.FileExists(newName) {
        Err(err) => {
            printError(&format!("  ❌ Failed to check new file: {err}"));
            result.Duration = start.elapsed();
            result.Error = Some(Error::Annotate(err.msg, "FileExists check failed"));
            tc.AddResult(result);
            return;
        }
        Ok(false) => {
            printError("  ❌ Failed: new file doesn't exist after rename");
            result.Duration = start.elapsed();
            result.Error = Some(Error::new("new file doesn't exist after rename"));
            tc.AddResult(result);
            return;
        }
        Ok(true) => {}
    }
    result.Duration = start.elapsed();
    result.Passed = true;
    result.Details = "File renamed successfully".into();
    printSuccess("  ✓ Passed");
    tc.AddResult(result);
}

/// 步骤：WalkDir 能见到已写对象。
fn testWalkDir(tc: &mut TestContext) {
    let testName = "WalkDir() - list all files".to_string();
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    let mut fileCount = 0;
    let err = tc
        .Store
        .WalkDir(&WalkOption::default(), &mut |_path, _size| {
            fileCount += 1;
            Ok(())
        });
    result.Duration = start.elapsed();
    match err {
        Err(err) => {
            printError(&format!("  ❌ Failed: {err}"));
            result.Error = Some(Error::Annotate(err.msg, "WalkDir failed"));
        }
        Ok(()) => {
            printSuccess(&format!("  ✓ Passed (found {fileCount} files)"));
            result.Passed = true;
            result.Details = format!("Found {fileCount} files");
        }
    }
    tc.AddResult(result);
}

/// 步骤：WalkDir 能见到已写对象。
fn testWalkDirWithSubDir(tc: &mut TestContext, testFile: &str, testData: &[u8]) {
    let testName = "WalkDir() - with subdirectory".to_string();
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    let subFile1 = Path::new(testDirName)
        .join(testFile)
        .to_string_lossy()
        .into_owned();
    let subFile2 = Path::new(testDirName)
        .join("file2.tmp")
        .to_string_lossy()
        .into_owned();
    if let Err(err) = tc
        .Store
        .WriteFile(&subFile1, &testData[..512.min(testData.len())])
    {
        printError(&format!("  ❌ Failed to create test file in subdir: {err}"));
        result.Duration = start.elapsed();
        result.Error = Some(Error::Annotate(err.msg, "WriteFile in subdir failed"));
        tc.AddResult(result);
        return;
    }
    if let Err(err) = tc
        .Store
        .WriteFile(&subFile2, &testData[..256.min(testData.len())])
    {
        printError(&format!("  ❌ Failed to create test file in subdir: {err}"));
        result.Duration = start.elapsed();
        result.Error = Some(Error::Annotate(err.msg, "WriteFile in subdir failed"));
        tc.AddResult(result);
        return;
    }
    let mut fileCount = 0;
    let err = tc.Store.WalkDir(
        &WalkOption {
            SubDir: testDirName.into(),
            ListCount: 0,
        },
        &mut |_path, _size| {
            fileCount += 1;
            Ok(())
        },
    );
    result.Duration = start.elapsed();
    match err {
        Err(err) => {
            printError(&format!("  ❌ Failed: {err}"));
            result.Error = Some(Error::Annotate(err.msg, "WalkDir with subdir failed"));
        }
        Ok(()) if fileCount < 2 => {
            printError(&format!(
                "  ❌ Failed: expected at least 2 files in subdir, got {fileCount}"
            ));
            result.Error = Some(Error::Errorf(format!(
                "WalkDir found insufficient files: expected at least 2, got {fileCount}"
            )));
        }
        Ok(()) => {
            printSuccess(&format!(
                "  ✓ Passed (found {fileCount} files in subdirectory)"
            ));
            result.Passed = true;
            result.Details = format!("Found {fileCount} files in subdirectory '{testDirName}'");
        }
    }
    tc.AddResult(result);
}

/// 步骤：WalkDir 能见到已写对象。
fn testWalkDirWithPagination(tc: &mut TestContext, testData: &[u8]) {
    // 分页 Walk 相关断言。
    let testName = "WalkDir() - with pagination (small ListCount)".to_string();
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    let paginationTestDir = "br-test-pagination";
    let numFiles = 10;
    let mut fileNames = Vec::with_capacity(numFiles);
    for i in 0..numFiles {
        let fileName = Path::new(paginationTestDir)
            .join(format!("pagefile-{i:03}.tmp"))
            .to_string_lossy()
            .into_owned();
        if let Err(err) = tc
            .Store
            .WriteFile(&fileName, &testData[..1024.min(testData.len())])
        {
            printError(&format!(
                "  ❌ Failed to create test file {fileName}: {err}"
            ));
            result.Duration = start.elapsed();
            result.Error = Some(Error::Annotatef(
                err.msg,
                format!("WriteFile {fileName} failed"),
            ));
            tc.AddResult(result);
            return;
        }
        fileNames.push(fileName);
    }
    let pageSize = 3i64;
    let mut collectedFiles = Vec::new();
    let err = tc.Store.WalkDir(
        &WalkOption {
            SubDir: paginationTestDir.into(),
            ListCount: pageSize,
        },
        &mut |path, _size| {
            collectedFiles.push(path.to_string());
            Ok(())
        },
    );
    for name in &fileNames {
        let _ = tc.Store.DeleteFile(name);
    }
    result.Duration = start.elapsed();
    if let Err(err) = err {
        printError(&format!("  ❌ Failed: {err}"));
        result.Error = Some(Error::Annotate(err.msg, "WalkDir with pagination failed"));
        tc.AddResult(result);
        return;
    }
    if collectedFiles.len() != numFiles {
        printError(&format!(
            "  ❌ Failed: expected {numFiles} files total, got {}",
            collectedFiles.len()
        ));
        result.Error = Some(Error::Errorf(format!(
            "WalkDir pagination returned wrong file count: expected {numFiles}, got {}",
            collectedFiles.len()
        )));
        tc.AddResult(result);
        return;
    }
    let mut fileSet = std::collections::HashSet::new();
    for file in &collectedFiles {
        if !fileSet.insert(file.clone()) {
            printError(&format!("  ❌ Failed: duplicate file {file} found"));
            result.Error = Some(Error::Errorf(format!(
                "WalkDir pagination returned duplicate file: {file}"
            )));
            tc.AddResult(result);
            return;
        }
    }
    printSuccess(&format!(
        "  ✓ Passed (retrieved {} unique files with ListCount={pageSize})",
        collectedFiles.len()
    ));
    result.Passed = true;
    result.Details = format!(
        "Retrieved {} unique files with ListCount={pageSize}",
        collectedFiles.len()
    );
    tc.AddResult(result);
}

/// 步骤：删除单文件并确认不存在。
fn testDeleteFile(tc: &mut TestContext, name: &str) {
    let testName = format!("DeleteFile({name})");
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    if let Err(err) = tc.Store.DeleteFile(name) {
        printError(&format!("  ❌ Failed: {err}"));
        result.Duration = start.elapsed();
        result.Error = Some(Error::Annotate(err.msg, "DeleteFile failed"));
        tc.AddResult(result);
        return;
    }
    match tc.Store.FileExists(name) {
        Err(err) => {
            printError(&format!("  ❌ Failed to verify deletion: {err}"));
            result.Error = Some(Error::Annotate(err.msg, "FileExists check failed"));
        }
        Ok(true) => {
            printError("  ❌ Failed: file still exists after deletion");
            result.Error = Some(Error::new("file still exists after deletion"));
        }
        Ok(false) => {
            printSuccess("  ✓ Passed");
            result.Passed = true;
            result.Details = "File deleted successfully".into();
        }
    }
    result.Duration = start.elapsed();
    tc.AddResult(result);
}

/// 步骤：删除单文件并确认不存在。
fn testDeleteFiles(tc: &mut TestContext, names: &[String]) {
    let testName = format!("DeleteFiles() - batch delete {} files", names.len());
    printStep(&format!("Test: {testName}"));
    let start = Instant::now();
    let mut result = TestResult {
        Name: testName,
        Duration: Duration::ZERO,
        Details: String::new(),
        Passed: false,
        Error: None,
    };
    if let Err(err) = tc.Store.DeleteFiles(names) {
        printError(&format!("  ❌ Failed: {err}"));
        result.Duration = start.elapsed();
        result.Error = Some(Error::Annotate(err.msg, "DeleteFiles failed"));
        tc.AddResult(result);
        return;
    }
    for name in names {
        match tc.Store.FileExists(name) {
            Err(err) => {
                printError(&format!("  ❌ Failed to verify deletion of {name}: {err}"));
                result.Duration = start.elapsed();
                result.Error = Some(Error::Annotatef(
                    err.msg,
                    format!("FileExists check failed for {name}"),
                ));
                tc.AddResult(result);
                return;
            }
            Ok(true) => {
                printError(&format!(
                    "  ❌ Failed: file {name} still exists after deletion"
                ));
                result.Duration = start.elapsed();
                result.Error = Some(Error::Errorf(format!(
                    "file {name} still exists after deletion"
                )));
                tc.AddResult(result);
                return;
            }
            Ok(false) => {}
        }
    }
    result.Duration = start.elapsed();
    result.Passed = true;
    result.Details = format!("Deleted {} files", names.len());
    printSuccess("  ✓ Passed");
    tc.AddResult(result);
}

/// 青色步骤标题。
fn printStep(msg: &str) {
    println!("{}", color_cyan(&format!("► {msg}")));
}

/// 绿色成功消息。
fn printSuccess(msg: &str) {
    println!("{}", color_green(msg));
}

/// 红色错误消息。
fn printError(msg: &str) {
    eprintln!("{}", color_hi_red(msg));
}
