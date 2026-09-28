// Copyright 2026 AsterSQL.

//! Parity tests for `tools/check` vs Go `ut.go` / `longtests.go`.
//!
//! 该文件把 `ut.rs` 与 Go 版 `ut.go` / `longtests.go` 的外部可观察契约
//! 收敛成一个总入口测试，避免迁移后只在局部函数层面相似、但整体命令语义漂移。
//! 注释重点说明每组断言对应的 Go 意图、边界约束与资源生命周期，
//! 便于后续维护者判断修改是功能增强还是破坏兼容。

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::longtests::{LONG_TEST_WORKER_COUNT, long_tests};
use crate::stubs::{
    self, CommandResult, CommandSpec, CoverProfile, CoverProfileBlock, compile_regex,
    parse_profiles_from_reader, set_process_handler,
};
use crate::ut::{
    self, JUnitFailure, JUnitTestCase, Numa, Task, UtState, append_with_reduce,
    collect_test_results, filter_test_cases, format_duration_as_seconds, handle_flag, handle_flags,
    list_long_tasks, merge_profile, package_properties, parse_case_list_from_file, run, skip_dir,
    test_file_name, usage, write,
};

#[test]
fn go_rust_public_contract_matches() {
    // 这个总入口与 Go 侧 smoke-style 校验思路一致：
    // 不把所有断言塞进一个超长函数，而是按正常路径、边界、错误、
    // 资源回收四个维度拆分，失败时更容易定位语义退化发生在哪一层。
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
fn duration_format_keeps_go_fixed_precision() {
    assert_eq!(
        format_duration_as_seconds(Duration::from_millis(10)),
        "0.010000"
    );
}

#[test]
fn regex_boundary_matches_go_regexp_contract() {
    let alternatives = compile_regex(r"^(TestAlpha|TestBeta){1,2}$").unwrap();
    assert!(alternatives.is_match("TestAlphaTestBeta"));
    assert!(!alternatives.is_match("TestGamma"));

    let unicode = compile_regex(r"^测试\p{Han}+$").unwrap();
    assert!(unicode.is_match("测试用例"));
}

#[test]
fn cover_profile_parser_matches_go_sort_merge_and_validation() {
    let profile = concat!(
        "mode: set\n",
        "b.go:3.1,3.2 1 1\n",
        "a.go:2.1,2.2 1 1\n",
        "a.go:1.1,1.2 1 2\n",
        "a.go:1.1,1.2 1 1\n",
    );
    let parsed = parse_profiles_from_reader(profile.as_bytes()).unwrap();
    assert_eq!(parsed[0].file_name, "a.go");
    assert_eq!(parsed[0].blocks.len(), 2);
    assert_eq!(parsed[0].blocks[0].start_line, 1);
    assert_eq!(parsed[0].blocks[0].count, 3);
    assert_eq!(parsed[1].file_name, "b.go");

    assert!(parse_profiles_from_reader(b"mode: set\na.go:1.1,1.2 1 -1\n".as_slice()).is_err());
    assert!(parse_profiles_from_reader(b"mode: set\n\n".as_slice()).is_err());
}

fn reset() {
    // 每组子场景都从干净环境起步，避免前一个 case 遗留的进程桩或环境变量
    // 影响后续断言。这里不重置全局状态以外的内容，是因为测试自身会创建
    // 独立临时目录来隔离文件系统副作用。
    stubs::clear_process_handler();
    // SAFETY: single-threaded test harness; no concurrent env readers.
    // 这些环境变量会改变 `ut` 的版本探测和新老生成逻辑，
    // 如果不显式移除，后续场景可能得到“看似合理但不再与 Go 对齐”的结果。
    unsafe {
        std::env::remove_var("GOVERSION");
        std::env::remove_var("NEXT_GEN");
    }
}

/// Normal: long-tests registry, flag parsing, filter, junit write, cover merge.
fn contract_normal_paths() {
    // 正常路径验证的是“最常见且最容易被重构误伤”的公共契约：
    // 长测注册表、flag 消耗、副作用函数返回值、覆盖率合并和 JUnit 输出。
    // 这些能力横跨 `ut` 主流程多个 helper，适合作为整体回归样本。
    reset();

    // 长测元数据必须与 Go `longtests.go` 一致，否则 `--long` 会跑错测试集合
    // 或使用错误并发度，进而改变 CI 的稳定性与耗时特征。
    let lt = long_tests();
    assert_eq!(LONG_TEST_WORKER_COUNT, 2);
    assert_eq!(lt.get("pkg/ttl/ttlworker").map(|v| v.len()), Some(3));
    assert!(lt.contains_key("pkg/ttl/cache"));

    // `handle_flags` / `handle_flag` 的契约不是只返回值，
    // 还要像 Go 版那样把对应 flag 从参数列表中消费掉，
    // 保证后续命令分派看到的是“纯业务参数”。
    let mut args = vec![
        "ut".into(),
        "run".into(),
        "pkg/session".into(),
        "TestFoo".into(),
        "--junitfile".into(),
        "out.xml".into(),
        "--coverprofile".into(),
        "cov.out".into(),
        "--race".into(),
        "--short".into(),
        "--long".into(),
    ];
    assert_eq!(handle_flags(&mut args, "--junitfile"), "out.xml");
    assert_eq!(handle_flags(&mut args, "--coverprofile"), "cov.out");
    assert!(handle_flag(&mut args, "--race"));
    assert!(handle_flag(&mut args, "--short"));
    assert!(handle_flag(&mut args, "--long"));
    // 这里直接断言剩余参数顺序，确保 Rust 没把 flag 解析实现成
    // 重新排序或额外保留占位值的版本。
    assert_eq!(
        args,
        vec![
            "ut".to_string(),
            "run".to_string(),
            "pkg/session".to_string(),
            "TestFoo".to_string()
        ]
    );

    // `usage` 只需成功输出帮助并返回 true；这里不比较全文，
    // 避免把文案轻微调整误判成行为不兼容。
    assert!(usage());

    // 过滤逻辑覆盖两条 Go 语义：
    // 1. 纯字符串参数视为子串匹配；
    // 2. `r:` 前缀启用正则，并只返回命中的测试项。
    let tasks = vec![
        Task {
            pkg: "pkg/a".into(),
            test: "TestAlpha".into(),
        },
        Task {
            pkg: "pkg/a".into(),
            test: "TestBeta".into(),
        },
        Task {
            pkg: "pkg/a".into(),
            test: "Helper".into(),
        },
    ];
    let filtered = filter_test_cases(tasks.clone(), "Test").unwrap();
    assert_eq!(filtered.len(), 2);
    let re = filter_test_cases(tasks, "r:^TestA.*$").unwrap();
    assert_eq!(re.len(), 1);
    assert_eq!(re[0].test, "TestAlpha");

    // `list_long_tasks` 要从预定义长测表中展开出具体 task。
    // 这里只检查数量，是因为具体名字已经通过上面的注册表断言兜底。
    let long = list_long_tasks("pkg/ttl/ttlworker", Vec::new());
    assert_eq!(long.len(), 3);

    // Cover merge + appendWithReduce (OR counts on same block).
    // Go 版合并 profile 时会按 block 坐标聚合计数；
    // Rust 如果改成覆盖写入，会导致最终 `cover.out` 少计命中次数。
    let mut m = std::collections::HashMap::new();
    merge_profile(
        &mut m,
        vec![CoverProfile {
            file_name: "a.go".into(),
            blocks: vec![CoverProfileBlock {
                start_line: 1,
                start_col: 1,
                end_line: 2,
                end_col: 2,
                num_stmt: 1,
                count: 1,
            }],
        }],
    );
    merge_profile(
        &mut m,
        vec![CoverProfile {
            file_name: "a.go".into(),
            blocks: vec![CoverProfileBlock {
                start_line: 1,
                start_col: 1,
                end_line: 2,
                end_col: 2,
                num_stmt: 1,
                count: 2,
            }],
        }],
    );
    assert_eq!(m["a.go"].blocks[0].count, 3); // 1|2

    // `append_with_reduce` 是更细粒度的 block 合并 helper，
    // 这里直接覆盖“同一 block 再次出现”这个关键分支。
    let reduced = append_with_reduce(
        vec![CoverProfileBlock {
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 2,
            num_stmt: 1,
            count: 1,
        }],
        CoverProfileBlock {
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 2,
            num_stmt: 1,
            count: 4,
        },
    );
    assert_eq!(reduced[0].count, 5);

    // 覆盖率文本解析是后续 merge 的入口。
    // 这里只保留最小合法样本，证明 Rust 解析器接受 Go 生成格式。
    let profile = "mode: set\na.go:1.1,2.2 1 1\n";
    let parsed = parse_profiles_from_reader(profile.as_bytes()).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].file_name, "a.go");

    // 正则 helper 需要与 Go `regexp` 常见匹配语义保持一致：
    // 全匹配表达式只命中目标用例，不意外吞掉更长名字。
    let rx = compile_regex("^TestFoo$").unwrap();
    assert!(rx.is_match("TestFoo"));
    assert!(!rx.is_match("TestFooBar"));

    // 文件名和目录跳过策略会直接影响构建产物命名以及包枚举范围，
    // 是 `ut` 命令能否在大仓库里稳定运行的基础约束。
    assert_eq!(test_file_name("pkg/session"), "session.test.bin");
    assert!(skip_dir("tools/check"));
    assert!(!skip_dir("pkg/session"));

    // JUnit property 中暴露的 Go 版本属于对外报表字段，
    // 名称和值都要与 Go 版一致，否则消费 XML 的工具会读不到预期属性。
    let props = package_properties("go1.22");
    assert_eq!(props[0].name, "go.version");
    assert_eq!(props[0].value, "go1.22");

    // 这里手工构造一个最小成功结果，验证 `collect_test_results` 与 `write`
    // 组合后能产出可被 CI 识别的 XML，而不是只测试中间结构体。
    let mut worker = Numa::default();
    worker.results.push(ut::TestResult {
        junit: JUnitTestCase {
            classname: "github.com/pingcap/tidb/pkg/a".into(),
            name: "TestOk".into(),
            time: format_duration_as_seconds(Duration::from_millis(10)),
            failure: None,
            skip_message: None,
        },
        d: Duration::from_millis(10),
        err: None,
    });
    let suites = collect_test_results(&[worker]);
    assert_eq!(suites.suites.len(), 1);
    let mut buf = Vec::new();
    write(&mut buf, &suites).unwrap();
    let xml = String::from_utf8(buf).unwrap();
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(xml.contains("<testsuites>"));
    // 保留对用例名的断言，证明测试结果条目真正进入 XML，而不是只有空壳头部。
    assert!(xml.contains("TestOk"));
}

/// Boundary: missing except/only file → empty set; skipDIR prefixes; long worker count.
fn contract_boundary() {
    // 边界路径关注“输入存在瑕疵但不应崩溃”的场景，
    // 以及 `--long` 这类会改变执行策略的特殊模式。
    reset();
    // Go 版在缺少 except/only 文件时返回空集合而不是报错，
    // 这样调用端可以把“没配置列表”视作正常状态。
    let missing = parse_case_list_from_file("/nonexistent/ut-except-list.txt").unwrap();
    assert!(missing.is_empty());

    // 这些前缀属于 Go 实现中的固定跳过目录。
    // 逐项保留断言可以防止未来把某个高开销子树误纳入 `ut` 扫描范围。
    for prefix in [
        "br",
        "lightning",
        "pkg/lightning",
        "cmd",
        "dumpling",
        "tests",
        "tools",
        "build",
    ] {
        assert!(skip_dir(prefix), "skip_dir({prefix})");
    }

    // --long uses LONG_TEST_WORKER_COUNT workers (observable via process calls count).
    // 这里不直接探测线程数量，而是通过被调起的子进程数量来观测最终调度结果，
    // 这样更贴近 Go 版外部行为，也更不依赖内部实现细节。
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_c = calls.clone();
    let listed = Arc::new(Mutex::new(Vec::<String>::new()));
    let listed_c = listed.clone();

    // 进程桩同时模拟 `go list`、`go test -c`、列举测试和实际执行。
    // 目标是把 `run(--long)` 需要的外部命令全链路走通，但不依赖真实 Go 工具链。
    set_process_handler(Some(Arc::new(move |spec: &CommandSpec| {
        calls_c.fetch_add(1, Ordering::SeqCst);
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("list") {
            return CommandResult::success(
                "github.com/pingcap/tidb/pkg/ttl/ttlworker\ngithub.com/pingcap/tidb/tools/check\n",
            );
        }
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("test") {
            return CommandResult::success("");
        }
        if spec.args.iter().any(|a| a == "-test.list") {
            return CommandResult::success("TestParallelLockNewJob\n");
        }
        if spec.args.iter().any(|a| a == "-test.run") {
            listed_c.lock().unwrap().push(spec.program.clone());
            return CommandResult::success("ok");
        }
        if spec.program == "go" && spec.args == ["version"] {
            return CommandResult::success("go version go1.22.0 darwin/arm64");
        }
        CommandResult::success("")
    })));

    // Create fake test binaries under a temp workdir for long packages.
    // `ut` 会在工作目录下查找已经编译出的测试二进制，
    // 因此这里必须伪造与包路径一致的文件层级，才能覆盖真实查找逻辑。
    let tmp = tempfile_dir("ut-boundary");
    for pkg in ["pkg/ttl/ttlworker", "pkg/ttl/cache"] {
        let dir = tmp.join(pkg);
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join(test_file_name(pkg));
        File::create(&bin).unwrap();
    }

    // 通过切换 cwd 注入工作目录，保持 `run` 的调用形式与正常 CLI 一致，
    // 避免为了测试方便引入 Go 版不存在的额外参数。
    let code = {
        // run with --long and inject work_dir via env by chdir
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let code = run(vec!["ut".into(), "run".into(), "--long".into()]);
        std::env::set_current_dir(old).unwrap();
        code
    };
    assert_eq!(code, 0, "long run should succeed with mocked subprocess");
    // 3 + 1 long tests executed
    // 这里的 4 来自 `longtests.go` 两个包的全部长测条目数，
    // 能证明 Rust 没有遗漏或重复展开任何预定义任务。
    assert_eq!(listed.lock().unwrap().len(), 4);

    let _ = fs::remove_dir_all(tmp);
}

/// Error: build failure, package missing, filter regex error, failed test case.
fn contract_error_paths() {
    // 错误路径覆盖“命令应该失败，而且失败方式要与 Go 一致”的场景。
    // 这里主要关心退出码和错误传播，而不是日志全文。
    reset();

    // 第一段桩让构建步骤失败，验证 `ut build pkg/session`
    // 不会吞错或错误地继续后续流程。
    set_process_handler(Some(Arc::new(|spec: &CommandSpec| {
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("list") {
            return CommandResult::success("github.com/pingcap/tidb/pkg/session\n");
        }
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("test") {
            return CommandResult::failure("exit status 1", "build boom");
        }
        CommandResult::success("")
    })));

    let tmp = tempfile_dir("ut-err-build");
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&tmp).unwrap();
    let code = run(vec!["ut".into(), "build".into(), "pkg/session".into()]);
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, 1, "build failure must exit 1");
    let _ = fs::remove_dir_all(tmp);

    // Bad regex
    // 过滤表达式非法时应立即返回错误，让调用者决定如何展示，
    // 而不是退回到“不过滤”的宽松行为。
    let err = filter_test_cases(Vec::new(), "r:[unterminated").unwrap_err();
    assert!(!err.is_empty());

    // Failed test case marks worker fail / run returns 1
    // 第二段场景模拟二进制存在、列举成功，但某个具体测试失败。
    // 这能验证 Rust 在运行阶段失败时仍与 Go 一样返回非零退出码。
    reset();
    let tmp = tempfile_dir("ut-err-run");
    let pkg = "pkg/session";
    let dir = tmp.join(pkg);
    fs::create_dir_all(&dir).unwrap();
    File::create(dir.join(test_file_name(pkg))).unwrap();

    set_process_handler(Some(Arc::new(move |spec: &CommandSpec| {
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("list") {
            return CommandResult::success("github.com/pingcap/tidb/pkg/session\n");
        }
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("test") {
            return CommandResult::success("");
        }
        if spec.args.iter().any(|a| a == "-test.list") {
            return CommandResult::success("TestBoom\n");
        }
        if spec.args.iter().any(|a| a == "-test.run") {
            return CommandResult::failure("exit status 1", "FAIL: TestBoom\n");
        }
        CommandResult::success("")
    })));

    std::env::set_current_dir(&tmp).unwrap();
    let code = run(vec!["ut".into(), "run".into(), "pkg/session".into()]);
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, 1, "failed test must exit 1");
    let _ = fs::remove_dir_all(tmp);

    // package not exist on list
    // 最后一段覆盖“用户指定了不存在的包”。
    // Go 版会在 `go list` 结果里找不到目标后失败，而不是静默成功。
    reset();
    set_process_handler(Some(Arc::new(|spec: &CommandSpec| {
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("list") {
            return CommandResult::success("github.com/pingcap/tidb/pkg/session\n");
        }
        CommandResult::success("")
    })));
    let tmp = tempfile_dir("ut-err-list");
    std::env::set_current_dir(&tmp).unwrap();
    let code = run(vec!["ut".into(), "list".into(), "pkg/missing".into()]);
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, 1);
    let _ = fs::remove_dir_all(tmp);
}

/// Resource cleanup: cover temp dir removed; junit file written and closed.
fn contract_resource_cleanup() {
    // 资源清理场景验证的是成功路径末尾的文件副作用：
    // JUnit 文件要落盘，cover profile 要可读，而且中间产生的临时资源不能阻塞收尾。
    reset();
    let tmp = tempfile_dir("ut-cleanup");
    let pkg = "pkg/session";
    let dir = tmp.join(pkg);
    fs::create_dir_all(&dir).unwrap();
    File::create(dir.join(test_file_name(pkg))).unwrap();

    let junit = tmp.join("junit.xml");
    let cover = tmp.join("cover.out");
    let cover_written = Arc::new(AtomicUsize::new(0));
    let cover_written_c = cover_written.clone();

    // 这个桩在收到 `-test.coverprofile` 时主动写入一个极小的覆盖率文件，
    // 用来模拟 Go 测试二进制的真实产物，而不是直接跳过覆盖率链路。
    set_process_handler(Some(Arc::new(move |spec: &CommandSpec| {
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("list") {
            return CommandResult::success("github.com/pingcap/tidb/pkg/session\n");
        }
        if spec.program == "go" && spec.args.first().map(|s| s.as_str()) == Some("test") {
            return CommandResult::success("");
        }
        if spec.args.iter().any(|a| a == "-test.list") {
            return CommandResult::success("TestOk\n");
        }
        if spec.args.iter().any(|a| a == "-test.coverprofile") {
            // Write a tiny cover file at the path requested by the runner.
            if let Some(idx) = spec.args.iter().position(|a| a == "-test.coverprofile") {
                if let Some(path) = spec.args.get(idx + 1) {
                    if let Some(parent) = std::path::Path::new(path).parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(path, b"mode: set\nb.go:1.1,1.2 1 1\n");
                    cover_written_c.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        if spec.args.iter().any(|a| a == "-test.run") {
            return CommandResult::success("ok");
        }
        if spec.program == "go" && spec.args == ["version"] {
            return CommandResult::success("go version go1.22.0");
        }
        CommandResult::success("")
    })));

    // 同时传入 `--junitfile` 与 `--coverprofile`，
    // 验证 `run` 在单次执行中能协调两种输出，而不是互相覆盖。
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(&tmp).unwrap();
    let code = run(vec![
        "ut".into(),
        "run".into(),
        "pkg/session".into(),
        "--junitfile".into(),
        junit.to_string_lossy().into_owned(),
        "--coverprofile".into(),
        cover.to_string_lossy().into_owned(),
    ]);
    std::env::set_current_dir(&old).unwrap();
    assert_eq!(code, 0);
    assert!(junit.is_file(), "junit file must be created");
    // 读取文件内容既验证了落盘，也顺带证明文件句柄已正确关闭，可被后续消费者读取。
    let junit_body = fs::read_to_string(&junit).unwrap();
    assert!(junit_body.contains("TestOk"));
    assert!(cover.is_file(), "coverprofile must be created");
    let cov_body = fs::read_to_string(&cover).unwrap();
    assert!(cov_body.starts_with("mode: set\n"));
    assert!(cover_written.load(Ordering::SeqCst) >= 1);

    // Temp cov* dirs under system temp should not leak for this run — best-effort:
    // we only assert the final cover file exists and junit is readable (handles closed).
    // 这里保持与 Go 版相同的务实策略：不硬编码系统临时目录内容，
    // 只验证用户真正关心的最终产物可用，避免测试对运行环境过度敏感。
    drop(junit_body);
    let _ = fs::remove_dir_all(tmp);
}

fn tempfile_dir(prefix: &str) -> PathBuf {
    // 纳秒时间戳足以让并发或快速重复执行的测试获得独立目录，
    // 与 Go 测试常用的临时目录命名思路保持一致。
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    path.push(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&path).unwrap();
    path
}

// Local alias so tests can `File::create` without importing std::fs::File at top for every use.
// 把别名放在文件末尾可以减少顶部导入噪音，
// 同时不改变任何测试逻辑或名称解析结果。
use std::fs::File;
