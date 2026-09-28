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

//! TiDB unit-test driver (Go `tools/check/ut.go`).
//! 这个模块复刻 Go 版 `ut` 工具的主流程，用来在仓库内发现、构建并执行测试二进制。
//! 它不是通用测试框架，而是为了兼容既有 `tools/check/ut.go` 使用方式而保留的专用驱动。
//! Rust 版本把原先分散的全局状态收口到 `UtState`，但仍尽量维持参数语义、输出文案和错误路径一致。
//! 阅读时可以把它拆成三段流水线：枚举包与用例、调度执行任务、汇总 JUnit 与覆盖率结果。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::longtests::{LONG_TEST_WORKER_COUNT, long_tests};
use crate::stubs::{
    self, CommandResult, CommandSpec, CoverProfile, CoverProfileBlock, compile_regex,
    parse_profiles_from_reader,
};

/// 这里保留 Go 工具的帮助文本布局，方便与现有脚本和历史输出逐行对照。
/// 返回 `true` 只表示帮助打印成功，并不代表已经执行过测试动作。
/// `run()` 在遇到未知子命令时也会走到这里，因此帮助本身被当成成功分支。
/// 文本内容刻意不做本地化，避免破坏依赖固定帮助片段的外部调用方。
/// Go `usage` help text; return value matches Go (true).
pub fn usage() -> bool {
    let msg = r#"// run all tests
ut

// show usage
ut -h

// list all packages
ut list

// list test cases of a single package
ut list $package

// list test cases that match a pattern
ut list $package 'r:$regex'

// run all tests
ut run

// run test all cases of a single package
ut run $package

// run test cases of a single package
ut run $package $test

// run test cases that match a pattern
ut run $package 'r:$regex'

// run test cases of multiple packages
ut run-multi $package1 $package2 ...

// build all test package
ut build

// build a test package
ut build xxx

// write the junitfile
ut run --junitfile xxx

// test with race flag
ut run --race

// test with test.short flag
ut run --short

// test with long flag
// when the '--long' flag is set, ut will only run the long tests and have different strategies for concurrency to make them stabler.
ut run --long"#;

    println!("{msg}");
    true
}

/// 统一生成 Go 模块前缀，供包过滤、JUnit 类名和批量构建参数共同复用。
/// 这里继续按路径段拼接，而不是硬编码整串字符串，减少平台分隔符差异。
/// 返回值代表逻辑模块路径，不是当前仓库在本机上的绝对磁盘路径。
/// 只要上游 Go 模块名不变，依赖此函数的其他逻辑就不需要单独调整。
/// Go `modulePath` — `filepath.Join("github.com", "pingcap", "tidb")`.
pub fn module_path() -> String {
    Path::new("github.com")
        .join("pingcap")
        .join("tidb")
        .to_string_lossy()
        .to_string()
}

/// 单个待执行测试任务只记录包名和测试名，对应 Go 版的 `task` 结构。
/// 运行参数不放进这里，是因为并发度、覆盖率等控制位都来自共享运行状态。
/// 该结构既会进入任务队列，也会参与 `--only` 与 `--except` 的文本过滤。
/// 字段保持简单能降低复制、打散和跨线程传递时的额外成本。
/// Go `task`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Task {
    pub pkg: String,
    pub test: String,
}

impl fmt::Display for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.pkg, self.test)
    }
}

/// 把 Go 版分散的可变全局变量集中收口，便于在 Rust 中安全地克隆到工作线程。
/// 这里保存整个执行会话需要的上下文，包括并发度、工作目录、覆盖率路径和过滤文件。
/// 线程侧拿到的是状态快照，从而避免直接共享可变全局带来的同步复杂度。
/// 这种设计既保留原命令行语义，又避免引入 `static mut` 一类不安全写法。
/// Mutable globals from Go (`p`, `buildParallel`, flags, …).
#[derive(Clone, Debug, Default)]
pub struct UtState {
    pub p: usize,
    pub build_parallel: usize,
    pub work_dir: PathBuf,
    pub junitfile: String,
    pub coverprofile: String,
    pub cover_file_temp_dir: PathBuf,
    pub race: bool,
    pub short: bool,
    pub long: bool,
    pub except: String,
    pub only: String,
}

/// 实现 `ut list` 子命令，负责列出包或列出某个包中可运行的测试函数。
/// 当目标包尚未生成测试二进制时，这里会先触发构建再继续枚举。
/// 两段式参数分别表示“列整个包”与“列包内匹配某模式的测试”，语义与 Go 保持一致。
/// 返回布尔值而非结果对象，是为了延续原工具依靠退出码表达成败的风格。
/// Go `cmdList`.
pub fn cmd_list(state: &mut UtState, args: &[String]) -> bool {
    let mut pkgs = match list_packages(state) {
        Ok(pkgs) => pkgs,
        Err(err) => {
            eprintln!("list package error {err}");
            return false;
        }
    };

    if args.is_empty() {
        for pkg in pkgs {
            println!("{pkg}");
        }
        return false;
    }

    if args.len() == 1 || args.len() == 2 {
        let pkg = args[0].clone();
        pkgs = filter_strings(pkgs, |s| s == &pkg);
        if pkgs.len() != 1 {
            println!("package not exist {pkg}");
            return false;
        }

        if let Err(err) = build_test_binary(state, &pkg) {
            eprintln!("build package error {pkg} {err}");
            return false;
        }
        let exist = match test_binary_exist(state, &pkg) {
            Ok(exist) => exist,
            Err(err) => {
                eprintln!("check test binary existence error {err}");
                return false;
            }
        };
        if !exist {
            println!("no test case in  {pkg}");
            return false;
        }

        let mut res = match list_test_cases(state, &pkg, Vec::new()) {
            Ok(res) => res,
            Err(err) => {
                eprintln!("list test cases for package error {err}");
                return false;
            }
        };

        if args.len() == 2 {
            res = match filter_test_cases(res, &args[1]) {
                Ok(res) => res,
                Err(err) => {
                    println!("filter test cases error {err}");
                    return false;
                }
            };
        }

        for x in res {
            println!("{}", x.test);
        }
    }
    true
}

/// 实现 `ut build`，只生成测试二进制，不进入测试发现与执行阶段。
/// 不带参数时会批量构建所有参与 `ut` 的包，带包名时只处理单个目录。
/// 这个子命令常用于尽早暴露编译问题，因此失败会立刻向上返回。
/// 之所以先统一走包枚举逻辑，是为了复用同一套模块前缀和跳过目录规则。
/// Go `cmdBuild`.
pub fn cmd_build(state: &mut UtState, args: &[String]) -> bool {
    let pkgs = match list_packages(state) {
        Ok(pkgs) => pkgs,
        Err(err) => {
            eprintln!("list package error {err}");
            return false;
        }
    };

    if args.is_empty() {
        if let Err(err) = build_test_binary_multi(state, &pkgs) {
            eprintln!("build package error {:?} {err}", pkgs);
            return false;
        }
        return true;
    }

    if !args.is_empty() {
        let pkg = &args[0];
        if let Err(err) = build_test_binary(state, pkg) {
            eprintln!("build package error {pkg} {err}");
            return false;
        }
    }
    true
}

/// `run-multi` 接收一组显式包名，适合外部调度器按分片把多个包一次性交给 `ut`。
/// 它先统一构建这些包的测试二进制，再并发收集每个包下真实存在的测试函数。
/// 日志中的 `count` 统计的是展开后的任务数，而不是输入参数里的包数量。
/// 这样能把构建开销和执行开销分开观测，方便定位瓶颈落在哪一段。
/// Go `cmdRunMulti`.
pub fn cmd_run_multi(state: &mut UtState, pkgs: &[String]) -> bool {
    if pkgs.is_empty() {
        return true;
    }

    let start = Instant::now();
    if let Err(err) = build_test_binary_multi(state, pkgs) {
        eprintln!("build package error {:?} {err}", pkgs);
        return false;
    }

    let tasks = match list_test_cases_for_pkgs(state, pkgs) {
        Ok(tasks) => tasks,
        Err(err) => {
            eprintln!("run existing test cases error {err}");
            return false;
        }
    };

    println!(
        "building task finish, maxproc={}, count={}, takes={:?}",
        state.build_parallel,
        tasks.len(),
        start.elapsed()
    );
    run_test_cases(state, tasks)
}

/// `ut` 的主执行入口，负责解释“全量、单包、单测”三种常见运行形态。
/// 它还会把 `--long`、`--only`、`--except` 等过滤条件折叠进最终任务集合。
/// 这里先构建任务再统一进入执行阶段，保证不同入口共享同一套调度逻辑。
/// 输出里的统计文本尽量保持与 Go 版一致，方便沿用现有日志采集规则。
/// Go `cmdRun`.
pub fn cmd_run(state: &mut UtState, args: &[String]) -> bool {
    let mut pkgs = match list_packages(state) {
        Ok(pkgs) => pkgs,
        Err(err) => {
            println!("list packages error {err}");
            return false;
        }
    };
    let mut tasks: Vec<Task> = Vec::with_capacity(5000);
    let start = Instant::now();

    if state.long {
        pkgs = long_tests().keys().map(|s| (*s).to_string()).collect();
    }

    if args.is_empty() {
        if let Err(err) = build_test_binary_multi(state, &pkgs) {
            eprintln!("build package error {:?} {err}", pkgs);
            return false;
        }

        if state.long {
            for pkg in &pkgs {
                tasks = list_long_tasks(pkg, tasks);
            }
        } else {
            tasks = match list_test_cases_for_pkgs(state, &pkgs) {
                Ok(tasks) => tasks,
                Err(err) => {
                    eprintln!("run existing test cases error {err}");
                    return false;
                }
            };
        }
    }

    if args.len() == 1 {
        let pkg = &args[0];
        if let Err(err) = build_test_binary(state, pkg) {
            eprintln!("build package error {pkg} {err}");
            return false;
        }
        let exist = match test_binary_exist(state, pkg) {
            Ok(exist) => exist,
            Err(err) => {
                eprintln!("check test binary existence error {err}");
                return false;
            }
        };

        if !exist {
            println!("no test case in  {pkg}");
            return false;
        }

        if state.long {
            tasks = list_long_tasks(pkg, tasks);
        } else {
            tasks = match list_test_cases(state, pkg, tasks) {
                Ok(tasks) => tasks,
                Err(err) => {
                    eprintln!("list test cases error {err}");
                    return false;
                }
            };
        }
    }

    if args.len() == 2 {
        let pkg = &args[0];
        if let Err(err) = build_test_binary(state, pkg) {
            eprintln!("build package error {pkg} {err}");
            return false;
        }
        let exist = match test_binary_exist(state, pkg) {
            Ok(exist) => exist,
            Err(err) => {
                eprintln!("check test binary existence error {err}");
                return false;
            }
        };
        if !exist {
            println!("no test case in  {pkg}");
            return false;
        }

        tasks = match list_test_cases(state, pkg, tasks) {
            Ok(tasks) => tasks,
            Err(err) => {
                eprintln!("list test cases error {err}");
                return false;
            }
        };
        tasks = match filter_test_cases(tasks, &args[1]) {
            Ok(tasks) => tasks,
            Err(err) => {
                eprintln!("filter test cases error {err}");
                return false;
            }
        };
    }

    if !state.except.is_empty() {
        let list = match parse_case_list_from_file(&state.except) {
            Ok(list) => list,
            Err(err) => {
                eprintln!("parse --except file error {err}");
                return false;
            }
        };
        tasks.retain(|task| !list.contains(&task.to_string()));
    }

    if !state.only.is_empty() {
        let list = match parse_case_list_from_file(&state.only) {
            Ok(list) => list,
            Err(err) => {
                eprintln!("parse --only file error {err}");
                return false;
            }
        };
        tasks.retain(|task| list.contains(&task.to_string()));
    }

    println!(
        "building task finish, parallelism={}, count={}, takes={:?}",
        state.build_parallel,
        tasks.len(),
        start.elapsed()
    );
    run_test_cases(state, tasks)
}

/// 实际消费任务队列的调度层，负责确定 worker 数量、打散顺序和结果归集方式。
/// `long` 模式会收紧并发并提高单测可用 CPU，目的是减少长稳测之间的资源干扰。
/// 这里用有界通道模拟 Go 的缓冲 channel，既保留背压语义，也避免一次性塞满内存。
/// 所有 worker 结束后才会写 JUnit 和覆盖率，确保输出文件看到的是完整结果。
/// Go `runTestCases` — buffered channel + worker goroutines.
pub fn run_test_cases(state: &mut UtState, mut tasks: Vec<Task>) -> bool {
    let test_worker_count = if state.long {
        LONG_TEST_WORKER_COUNT
    } else if state.p == 0 {
        1
    } else {
        state.p
    };

    shuffle(&mut tasks);

    let (tx, rx) = mpsc::sync_channel::<Task>(100);
    let rx = Arc::new(Mutex::new(rx));
    let state_arc = Arc::new(state.clone());
    let mut handles = Vec::with_capacity(test_worker_count);
    let works: Arc<Mutex<Vec<Numa>>> = Arc::new(Mutex::new(Vec::with_capacity(test_worker_count)));

    for _ in 0..test_worker_count {
        let rx = Arc::clone(&rx);
        let state_arc = Arc::clone(&state_arc);
        let works = Arc::clone(&works);
        handles.push(thread::spawn(move || {
            let mut worker = Numa::default();
            loop {
                let task = {
                    let guard = rx.lock().unwrap();
                    guard.recv()
                };
                match task {
                    Ok(task) => worker.run_one(&state_arc, task),
                    Err(_) => break,
                }
            }
            works.lock().unwrap().push(worker);
        }));
    }

    let start = Instant::now();
    for task in tasks {
        let _ = tx.send(task);
    }
    drop(tx);
    for h in handles {
        let _ = h.join();
    }
    println!("run all tasks takes {:?}", start.elapsed());

    let works = works.lock().unwrap().clone();

    if !state.junitfile.is_empty() {
        let out = collect_test_results(&works);
        match File::create(&state.junitfile) {
            Ok(mut f) => {
                if let Err(err) = write(&mut f, &out) {
                    println!("write junit file error: {err}");
                    return false;
                }
            }
            Err(err) => {
                println!("create junit file fail: {err}");
                return false;
            }
        }
    }

    if !state.coverprofile.is_empty() {
        collect_cover_profile_file(state);
    }

    for work in &works {
        if work.fail {
            return false;
        }
    }
    true
}

/// 并发枚举多个包的测试函数，对应 Go 版借助 `errgroup` 展开的那段逻辑。
/// 每个包各在线程里调用 `list_test_cases`，主线程再把成功结果汇总成总任务列表。
/// 一旦某个包出错，这里会记住第一份错误并在收集结束后整体返回，而不是静默吞掉。
/// 这样既保住了并发吞吐，也保住了问题定位时所需的首个错误上下文。
/// Go `listTestCasesForPkgs` with errgroup-style concurrency.
pub fn list_test_cases_for_pkgs(state: &UtState, pkgs: &[String]) -> Result<Vec<Task>, String> {
    let (tx, rx) = mpsc::channel::<Result<Vec<Task>, String>>();
    let mut spawned = 0usize;
    for pkg in pkgs {
        let exist = test_binary_exist(state, pkg)?;
        if !exist {
            println!("no test case in  {pkg}");
            continue;
        }
        let pkg_copy = pkg.clone();
        let state = state.clone();
        let tx = tx.clone();
        spawned += 1;
        thread::spawn(move || {
            let res = list_test_cases(&state, &pkg_copy, Vec::new()).map_err(|err| {
                eprintln!("list test cases error {pkg_copy} {err}");
                with_trace(err)
            });
            let _ = tx.send(res);
        });
    }
    drop(tx);

    let mut tasks = Vec::new();
    let mut err: Option<String> = None;
    for _ in 0..spawned {
        match rx.recv() {
            Ok(Ok(t)) => tasks.extend(t),
            Ok(Err(e)) => {
                if err.is_none() {
                    err = Some(e);
                }
            }
            Err(_) => break,
        }
    }
    if let Some(e) = err {
        return Err(e);
    }
    Ok(tasks)
}

/// 读取 `--only` 或 `--except` 文件，把每一行还原成一个可直接查找的任务键。
/// 文件不存在时返回空集合而不是报错，这是对齐 Go 版“过滤文件可选”的宽松语义。
/// 这里不额外裁剪空白或做格式校验，目的是让文本比对规则与旧脚本保持一致。
/// 使用 `HashSet` 是因为后续只关心成员是否存在，不需要保留原始顺序。
/// Go `parseCaseListFromFile`.
pub fn parse_case_list_from_file(file_name: &str) -> Result<HashSet<String>, String> {
    let mut ret = HashSet::new();
    let path = Path::new(file_name);
    // filepath.Clean equivalent is mostly identity for simple paths.
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(ret),
        Err(err) => return Err(with_trace(err.to_string())),
    };
    for line in content.lines() {
        ret.insert(line.to_string());
    }
    Ok(ret)
}

/// 从参数数组中摘走一个“带值 flag”，并把对应值单独返回给调用方保存。
/// 该函数会重建参数列表，因此后续子命令解析可以像没见过该 flag 一样继续处理。
/// 逻辑顺序严格模拟 Go 对 `os.Args` 的处理方式，避免迁移后边界条件发生变化。
/// 若 flag 出现但缺少值，结果会退化为空串，这与原实现的容错方式保持一致。
/// Go `handleFlags` — strip `--flag value` from argv.
pub fn handle_flags(args: &mut Vec<String>, flag: &str) -> String {
    let mut res = String::new();
    let mut tmp = Vec::with_capacity(args.len());
    let mut i = 0;

    while i < args.len() {
        if args[i] == flag {
            i += 1;
            break;
        }
        tmp.push(args[i].clone());
        i += 1;
    }

    if i < args.len() {
        res = args[i].clone();
        i += 1;
    }

    while i < args.len() {
        tmp.push(args[i].clone());
        i += 1;
    }

    *args = tmp;
    res
}

/// 处理不带值的布尔开关，并把匹配到的参数从原始参数数组中剔除出去。
/// 返回值只表达“是否出现过”，上层状态机可以直接赋给 `race`、`short`、`long`。
/// 它会保留其他参数的相对顺序，避免破坏位置相关的子命令语义。
/// 这种先摘 flag 再分发子命令的方式，最大程度贴近原始 CLI 的解析时序。
/// Go `handleFlag` — strip boolean flag.
pub fn handle_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let mut found = false;
    let mut tmp = Vec::with_capacity(args.len());
    for arg in args.iter() {
        if arg == flag {
            found = true;
            continue;
        }
        tmp.push(arg.clone());
    }
    *args = tmp;
    found
}

/// 真正的进程入口只负责把 `run()` 的退出码转成系统级退出行为。
/// 这样测试或其他封装可以直接调用 `run()`，而不必真的终止当前进程。
/// 与 Go 版一致，非零返回会立刻 `exit`，从而保持命令行工具的直观语义。
/// 将解析与执行下沉到 `run()` 也让后续验证更容易复用同一套主流程。
/// Process entry matching Go `main` (exits on failure).
pub fn main() {
    let code = run(std::env::args().collect());
    if code != 0 {
        std::process::exit(code);
    }
}

/// 可测试的主流程：解析参数、初始化状态、分发子命令并清理临时目录。
/// 与 Go 版不同的是它显式返回退出码，从而避免库式调用场景直接结束进程。
/// 覆盖率临时目录也在这里统一创建和回收，确保执行结束后不会残留中间文件。
/// 整个函数相当于 CLI 外壳，真正的业务动作都被拆进若干 `cmd_*` 子函数。
/// Runnable entry returning an exit code (for parity tests).
pub fn run(mut args: Vec<String>) -> i32 {
    let mut state = UtState::default();
    state.junitfile = handle_flags(&mut args, "--junitfile");
    state.coverprofile = handle_flags(&mut args, "--coverprofile");
    state.except = handle_flags(&mut args, "--except");
    state.only = handle_flags(&mut args, "--only");
    state.race = handle_flag(&mut args, "--race");
    state.short = handle_flag(&mut args, "--short");
    state.long = handle_flag(&mut args, "--long");

    let mut cover_temp: Option<PathBuf> = None;
    if !state.coverprofile.is_empty() {
        match tempfile_cov_dir() {
            Ok(dir) => {
                state.cover_file_temp_dir = dir.clone();
                cover_temp = Some(dir);
            }
            Err(_) => {
                println!(
                    "create temp dir fail {}",
                    state.cover_file_temp_dir.display()
                );
                return 1;
            }
        }
    }

    state.p = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    state.build_parallel = state.p * 2;
    match std::env::current_dir() {
        Ok(wd) => state.work_dir = wd,
        Err(err) => {
            println!("os.Getwd() error {err}");
            return 1;
        }
    }

    let mut is_succeed = false;
    if args.len() == 1 {
        is_succeed = cmd_run(&mut state, &[]);
    }

    if args.len() >= 2 {
        match args[1].as_str() {
            "list" => is_succeed = cmd_list(&mut state, &args[2..]),
            "build" => is_succeed = cmd_build(&mut state, &args[2..]),
            "run" => is_succeed = cmd_run(&mut state, &args[2..]),
            "run-multi" => is_succeed = cmd_run_multi(&mut state, &args[2..]),
            _ => is_succeed = usage(),
        }
    }

    if let Some(dir) = cover_temp {
        // Go uses os.Remove; prefer RemoveAll so non-empty temp dirs are cleaned.
        let _ = fs::remove_dir_all(dir);
    }

    if is_succeed { 0 } else { 1 }
}

/// 为覆盖率分片文件创建一次性目录，目录名使用时间戳降低并发冲突概率。
/// 这里不直接依赖 Go 的 `MkdirTemp`，而是用标准库组合出等价行为。
/// 调用方会把目录写入状态对象，供每个测试函数生成独立的 cover 片段。
/// 如果目录创建失败，主流程会立即中止，因为后续覆盖率路径都建立在它之上。
fn tempfile_cov_dir() -> io::Result<PathBuf> {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    path.push(format!("cov{nanos}"));
    fs::create_dir(&path)?;
    Ok(path)
}

/// 把每个测试函数单独写出的覆盖率片段合并成最终的 `coverprofile` 文件。
/// 结果文件会先写入 `mode: set` 头，再按解析后的 block 顺序重新拼回标准格式。
/// 此处遇到 IO 或解析错误会直接退出进程，因为半成品覆盖率文件没有继续使用价值。
/// 整体策略与 Go 版一致：先聚合到内存，再一次性顺序刷到目标文件。
/// Go `collectCoverProfileFile`.
pub fn collect_cover_profile_file(state: &UtState) {
    let files = match fs::read_dir(&state.cover_file_temp_dir) {
        Ok(files) => files,
        Err(err) => {
            println!("collect cover file error: {err}");
            std::process::exit(255);
        }
    };

    let mut w = match File::create(&state.coverprofile) {
        Ok(w) => w,
        Err(err) => {
            println!("create cover file error: {err}");
            std::process::exit(255);
        }
    };
    if let Err(err) = w.write_all(b"mode: set\n") {
        println!("create cover file error: {err}");
        std::process::exit(255);
    }

    let mut result: HashMap<String, CoverProfile> = HashMap::new();
    for file in files.flatten() {
        if file.path().is_dir() {
            continue;
        }
        collect_one_cover_profile_file(state, &mut result, &file.file_name().to_string_lossy());
    }

    let mut w1 = BufWriter::new(w);
    for prof in result.values() {
        for block in &prof.blocks {
            if let Err(err) = writeln!(
                w1,
                "{}:{}.{},{}.{} {} {}",
                prof.file_name,
                block.start_line,
                block.start_col,
                block.end_line,
                block.end_col,
                block.num_stmt,
                block.count
            ) {
                println!("flush data to cover profile file error: {err}");
                std::process::exit(255);
            }
        }
        if let Err(err) = w1.flush() {
            println!("flush data to cover profile file error: {err}");
            std::process::exit(255);
        }
    }
}

/// 读取一个临时覆盖率文件并把其中的 profile 结构并入总表。
/// 每个文件通常对应单个测试函数的执行结果，因此这里是总合并流程的最小输入单元。
/// 解析失败会被视为致命错误，因为这意味着整体覆盖率已经不再可信。
/// 把单文件处理单独拆出来，也让主循环只需要关心目录遍历和跳过子目录。
/// Go `collectOneCoverProfileFile`.
pub fn collect_one_cover_profile_file(
    state: &UtState,
    result: &mut HashMap<String, CoverProfile>,
    file_name: &str,
) {
    let path = state.cover_file_temp_dir.join(file_name);
    let f = match File::open(&path) {
        Ok(f) => f,
        Err(err) => {
            println!("open temp cover file error: {err}");
            std::process::exit(255);
        }
    };
    let profs = match parse_profiles_from_reader(f) {
        Ok(profs) => profs,
        Err(err) => {
            println!("parse cover profile file error: {err}");
            std::process::exit(255);
        }
    };
    merge_profile(result, profs);
}

/// 合并同一源码文件的覆盖率块，目标是把多个运行片段折叠成一份稳定 profile。
/// 这里先按起始位置排序，再走双指针归并，从而复用 Go 版已经验证过的思路。
/// 相同位置的 block 会继续交给 `append_with_reduce` 做按位合并和一致性检查。
/// 这样处理后，无论测试函数执行顺序如何，最终输出都能收敛到同样的结果形状。
/// Go `mergeProfile`.
pub fn merge_profile(m: &mut HashMap<String, CoverProfile>, profs: Vec<CoverProfile>) {
    for mut prof in profs {
        prof.blocks.sort_by(|a, b| compare_profile_block(a, b));
        if !m.contains_key(&prof.file_name) {
            m.insert(prof.file_name.clone(), prof);
            continue;
        }

        let old = m.get_mut(&prof.file_name).expect("profile exists");
        let mut tmp: Vec<CoverProfileBlock> = Vec::new();
        let mut i = 0;
        let mut j = 0;
        while i < old.blocks.len() && j < prof.blocks.len() {
            let v1 = old.blocks[i].clone();
            let v2 = prof.blocks[j].clone();

            match compare_profile_block(&v1, &v2) {
                std::cmp::Ordering::Less => {
                    tmp = append_with_reduce(tmp, v1);
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    tmp = append_with_reduce(tmp, v2);
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    tmp = append_with_reduce(tmp, v1);
                    tmp = append_with_reduce(tmp, v2);
                    i += 1;
                    j += 1;
                }
            }
        }
        while i < old.blocks.len() {
            tmp = append_with_reduce(tmp, old.blocks[i].clone());
            i += 1;
        }
        while j < prof.blocks.len() {
            tmp = append_with_reduce(tmp, prof.blocks[j].clone());
            j += 1;
        }

        old.blocks = tmp;
    }
}

/// 这是覆盖率块归并时的去重器，行为上类似 `append`，但会折叠连续重复位置。
/// 当起止位置完全相同且 `num_stmt` 一致时，命中计数会按位或到最后一个块上。
/// 如果同位置块的语句数不一致，会立刻 panic，因为那说明 profile 数据自相矛盾。
/// 只检查最后一个元素之所以成立，是因为调用方已经保证输入整体按位置有序。
/// Go `appendWithReduce`.
pub fn append_with_reduce(
    mut input: Vec<CoverProfileBlock>,
    b: CoverProfileBlock,
) -> Vec<CoverProfileBlock> {
    if let Some(last) = input.last_mut() {
        if b.start_line == last.start_line
            && b.start_col == last.start_col
            && b.end_line == last.end_line
            && b.end_col == last.end_col
        {
            if b.num_stmt != last.num_stmt {
                panic!(
                    "inconsistent NumStmt: changed from {} to {}",
                    last.num_stmt, b.num_stmt
                );
            }
            last.count |= b.count;
            return input;
        }
    }
    input.push(b);
    input
}

/// 定义覆盖率块的排序键，先比起始行，再比起始列。
/// Go 版比较器只关心这两个字段，Rust 版本保持同样的最小判定条件。
/// 之所以不继续比较结束位置，是因为当前归并逻辑默认同起点块已足够区分顺序。
/// 排序与双指针归并都会依赖这个比较器，它是覆盖率合并稳定性的基础。
/// Go `compareProfileBlock`.
pub fn compare_profile_block(x: &CoverProfileBlock, y: &CoverProfileBlock) -> std::cmp::Ordering {
    if x.start_line != y.start_line {
        return x.start_line.cmp(&y.start_line);
    }
    x.start_col.cmp(&y.start_col)
}

/// 把某个包中新发现的测试函数转成 `Task`，并追加到已有任务向量末尾。
/// 它本身不做过滤和排序，只负责把字符串形式的测试名封装成统一任务对象。
/// 错误会附带堆栈后上抛，方便判断失败发生在用例枚举而不是后续执行阶段。
/// 保持“传入旧列表、返回新列表”的形状，是为了贴近 Go 版 append 式写法。
/// Go `listTestCases`.
pub fn list_test_cases(
    state: &UtState,
    pkg: &str,
    mut tasks: Vec<Task>,
) -> Result<Vec<Task>, String> {
    let new_cases = list_new_test_cases(state, pkg).map_err(|err| {
        eprintln!("list test case error {pkg} {err}");
        with_trace(err)
    })?;
    for case in new_cases {
        tasks.push(Task {
            pkg: pkg.to_string(),
            test: case,
        });
    }
    Ok(tasks)
}

/// 根据命令行第二参数过滤测试任务，支持子串匹配和 `r:` 前缀正则两种模式。
/// 这里直接在测试名字段上工作，不会重新访问包或二进制，因此代价很低。
/// 正则编译失败会立刻返回错误，避免把拼写问题误判成“没有匹配项”。
/// 子串模式保持宽松匹配，是沿用历史 `ut run pkg TestFoo` 的常见用法。
/// Go `filterTestCases`.
pub fn filter_test_cases(tasks: Vec<Task>, arg1: &str) -> Result<Vec<Task>, String> {
    if let Some(pattern) = arg1.strip_prefix("r:") {
        let r = compile_regex(pattern)?;
        let tmp = tasks
            .into_iter()
            .filter(|task| r.is_match(&task.test))
            .collect();
        return Ok(tmp);
    }
    let tmp = tasks
        .into_iter()
        .filter(|task| task.test.contains(arg1))
        .collect();
    Ok(tmp)
}

/// `--long` 模式不再从测试二进制枚举，而是从预定义长测表中展开任务。
/// 这样可以精确控制哪些耗时长或稳定性敏感的用例进入长测执行通道。
/// 若包名不在长测映射里，函数会静默返回原列表，这与 Go 的查表行为一致。
/// 它只负责展开，不负责资源策略，真正的 CPU 分配仍在执行阶段决定。
/// Go `listLongTasks`.
pub fn list_long_tasks(pkg: &str, mut tasks: Vec<Task>) -> Vec<Task> {
    if let Some(tests) = long_tests().get(pkg) {
        for test in tests {
            tasks.push(Task {
                pkg: pkg.to_string(),
                test: (*test).to_string(),
            });
        }
    }
    tasks
}

/// 通过 `go list ./...` 获取仓库内包列表，再裁掉模块前缀和不参与 `ut` 的目录。
/// 过滤规则必须与 Go 版一致，否则全量跑测范围会变化并影响历史预期。
/// 这里保留 `state` 参数是为了接口统一，即使当前实现并不直接消费它。
/// 返回结果是相对模块路径，后续构建和执行都会把它拼回工作目录再使用。
/// Go `listPackages`.
pub fn list_packages(state: &UtState) -> Result<Vec<String>, String> {
    let list_path = format!(".{}...", std::path::MAIN_SEPARATOR);
    let cmd = TestCommand::new("go", vec!["list".to_string(), list_path]);
    let ss = cmd_to_lines(cmd).map_err(with_trace)?;

    let module = module_path();
    let mut ret = Vec::new();
    for s in ss {
        if !s.starts_with(&module) {
            continue;
        }
        if s.len() <= module.len() {
            continue;
        }
        let pkg = s[module.len() + 1..].to_string();
        if skip_dir(&pkg) {
            continue;
        }
        ret.push(pkg);
    }
    let _ = state;
    Ok(ret)
}

/// 单个工作线程对应一个 `Numa` 实例，内部累积自己的失败标记和测试结果。
/// 名称沿袭 Go 版本，虽然这里没有直接建模 NUMA 拓扑，但保留了历史语义。
/// 结果按 worker 分桶收集，可以避免多个线程同时向同一向量写入的同步开销。
/// 汇总阶段再把各 worker 的结果拼成 JUnit 输出，既简单又贴近原实现。
/// Go `numa`.
#[derive(Clone, Debug, Default)]
pub struct Numa {
    pub fail: bool,
    pub results: Vec<TestResult>,
}

impl Numa {
    /// worker 级别的单任务入口，负责执行一个测试并把失败信息立即打到终端。
    /// 失败时同时打印包名、测试名和捕获到的输出，便于与 Go 版日志快速对照。
    /// 这里不会中断整个 worker 循环，因为 `ut` 需要尽可能跑完剩余测试再统一汇总。
    /// `fail` 字段只记录“是否出现过失败”，更详细的信息仍保存在 `results` 里。
    pub fn run_one(&mut self, state: &UtState, task: Task) {
        let res = self.run_test_case(state, &task.pkg, &task.test);
        if res.junit.failure.is_some() {
            println!("[FAIL]  {} {}", task.pkg, task.test);
            if let Some(err) = &res.err {
                let contents = res
                    .junit
                    .failure
                    .as_ref()
                    .map(|f| f.contents.as_str())
                    .unwrap_or("");
                eprint!("err={err}\n{contents}");
            }
            self.fail = true;
        }
        self.results.push(res);
    }

    /// 真正调用测试二进制执行单个函数，并把输出包装成 JUnit 兼容结果。
    /// 这里最多重试三次，但只针对段错误、断点陷阱和 `panic during panic` 这类已知噪声。
    /// 重试前会清空缓冲内容，避免前一次异常输出污染最终失败报告。
    /// 无论成功失败都会记录耗时，因为后续按包聚合结果时需要统一统计时间。
    pub fn run_test_case(&self, state: &UtState, pkg: &str, func_name: &str) -> TestResult {
        let mut res = TestResult {
            junit: JUnitTestCase {
                classname: Path::new(&module_path())
                    .join(pkg)
                    .to_string_lossy()
                    .to_string(),
                name: func_name.to_string(),
                ..JUnitTestCase::default()
            },
            ..TestResult::default()
        };

        let mut buf = String::new();
        let mut err: Option<String> = None;
        let mut start = Instant::now();
        for _ in 0..3 {
            let mut cmd = self.test_command(state, pkg, func_name);
            cmd.dir = state.work_dir.join(pkg);
            cmd.capture_output = true;

            if state.short {
                cmd.args.push("--test.short".to_string());
            }
            if state.long {
                cmd.args.push("-long".to_string());
            }

            start = Instant::now();
            let result = cmd.run_result();
            buf = result.stdout.clone();
            if result.ok {
                err = None;
                break;
            }
            err = result.err.clone();
            if let Some(ref e) = err {
                if e == "signal: segmentation fault (core dumped)"
                    || e == "signal: trace/breakpoint trap (core dumped)"
                    || buf.contains("panic during panic")
                {
                    buf.clear();
                    continue;
                }
            }
            break;
        }
        if let Some(e) = err {
            res.junit.failure = Some(JUnitFailure {
                message: "Failed".to_string(),
                contents: buf,
                ..JUnitFailure::default()
            });
            res.err = Some(e);
        }

        res.d = start.elapsed();
        res.junit.time = format_duration_as_seconds(res.d);
        res
    }

    /// 组装单测执行命令，核心是定位测试二进制并附加覆盖率、CPU 和超时参数。
    /// `-test.run ^name$` 保证一次进程只跑一个目标函数，便于失败重试和结果归因。
    /// 长测模式会给单个用例更多 CPU，并把默认超时从 2 分钟放宽到 30 分钟。
    /// 这里返回的是命令描述对象，而不是直接执行结果，方便上层继续补充目录和输出策略。
    pub fn test_command(&self, state: &UtState, pkg: &str, func_name: &str) -> TestCommand {
        let mut args: Vec<String> = Vec::with_capacity(10);
        let exe = format!(".{}{}", std::path::MAIN_SEPARATOR, test_file_name(pkg));

        if !state.coverprofile.is_empty() {
            let file_name = format!(
                "{}.{}",
                pkg.replace(std::path::MAIN_SEPARATOR, "_"),
                func_name
            );
            let tmp_file = state.cover_file_temp_dir.join(file_name);
            args.push("-test.coverprofile".to_string());
            args.push(tmp_file.to_string_lossy().to_string());
        }
        let mut test_cpu = 1;
        if state.long && state.p > LONG_TEST_WORKER_COUNT {
            test_cpu = state.p / LONG_TEST_WORKER_COUNT;
        }
        args.push("-test.cpu".to_string());
        args.push(test_cpu.to_string());
        if !state.race && !state.long {
            args.extend(["-test.timeout".to_string(), "2m".to_string()]);
        } else {
            args.extend(["-test.timeout".to_string(), "30m".to_string()]);
        }

        args.extend(["-test.run".to_string(), format!("^{func_name}$")]);

        TestCommand::new(exe, args)
    }
}

/// 保存一次测试执行的最终产物：JUnit 案例、耗时以及可能的底层错误文本。
/// `junit` 面向最终报告，`err` 面向终端诊断，两者同时保留可以减少信息丢失。
/// 结构体本身很薄，主要承担把执行层结果桥接到汇总层的职责。
/// 后续按包聚合报告时，真正被统计的是其中的 `classname`、`failure` 与耗时。
/// Go `testResult`.
#[derive(Clone, Debug, Default)]
pub struct TestResult {
    pub junit: JUnitTestCase,
    pub d: Duration,
    pub err: Option<String>,
}

/// 把各 worker 分散保存的测试结果重新按包归类，生成最终的 JUnit suites 结构。
/// 由于任务执行前已经被打散，结果天然无序，所以这里必须显式按 `classname` 聚合。
/// 同时累计每个包下所有测试的耗时，保证 suite 级时间与 Go 版统计口径一致。
/// 生成后的结构随后会交给 `write()` 序列化成 XML，供 CI 或本地工具直接消费。
/// Go `collectTestResults`.
pub fn collect_test_results(workers: &[Numa]) -> JUnitTestSuites {
    let version = go_version();
    let mut pkgs: HashMap<String, Vec<JUnitTestCase>> = HashMap::new();
    let mut durations: HashMap<String, Duration> = HashMap::new();

    for n in workers {
        for res in &n.results {
            pkgs.entry(res.junit.classname.clone())
                .or_default()
                .push(res.junit.clone());
            *durations.entry(res.junit.classname.clone()).or_default() += res.d;
        }
    }

    let mut suites = JUnitTestSuites::default();
    for (pkg, cases) in pkgs {
        let suite = JUnitTestSuite {
            tests: cases.len(),
            failures: failure_cases(&cases),
            time: format_duration_as_seconds(*durations.get(&pkg).unwrap_or(&Duration::ZERO)),
            name: pkg,
            properties: package_properties(&version),
            test_cases: cases,
        };
        suites.suites.push(suite);
    }
    suites
}

/// 统计一个测试包里失败案例的数量，供 `testsuite failures` 属性直接使用。
/// 这里仅通过 `failure` 字段是否存在来判断，不关心失败内容的具体文本。
/// 逻辑保持极简，是为了让 suite 汇总过程更容易与 Go 版逐项对比。
/// 统计结果只影响报告元数据，不会回写执行层的成败判断。
/// Go `failureCases`.
pub fn failure_cases(input: &[JUnitTestCase]) -> usize {
    input.iter().filter(|case| case.failure.is_some()).count()
}

/// 定义 `ut` 全量扫描时需要跳过的目录前缀，避免把不属于目标范围的包纳入执行面。
/// 这些规则直接继承自 Go 工具的历史选择，因此不要随意扩缩，否则会改变默认跑测范围。
/// 使用前缀匹配而不是精确相等，是为了同时覆盖对应目录下的所有子包。
/// 例如排除 `tools` 后，`tools/check` 这类内部工具包也会自然被一并跳过。
/// Go `skipDIR`.
pub fn skip_dir(pkg: &str) -> bool {
    let skip = [
        "br".to_string(),
        "lightning".to_string(),
        Path::new("pkg")
            .join("lightning")
            .to_string_lossy()
            .to_string(),
        "cmd".to_string(),
        "dumpling".to_string(),
        "tests".to_string(),
        "tools".to_string(),
        "build".to_string(),
    ];
    skip.iter().any(|ignore| pkg.starts_with(ignore.as_str()))
}

/// 统一构造 `go test` 命令，并注入仓库默认依赖的 `intest` 构建标签。
/// 当 `NEXT_GEN=1` 时还会额外叠加 `nextgen`，保持与现有环境变量约定一致。
/// 把标签拼装集中到这里，可以避免各处构建和执行命令出现细微参数漂移。
/// 返回 `TestCommand` 而非直接运行，方便后续统一走 stub 和结果收集逻辑。
/// Go `goTestCmd`.
pub fn go_test_cmd(args: &[String]) -> TestCommand {
    let mut cmd = TestCommand::new("go", vec!["test".to_string()]);
    let mut tags = "--tags=intest".to_string();
    if std::env::var("NEXT_GEN").ok().as_deref() == Some("1") {
        tags.push_str(",nextgen");
    }
    cmd.args.push(tags);
    cmd.args.extend_from_slice(args);
    cmd
}

/// 为单个包执行 `go test -c`，生成后续枚举和单测执行都会依赖的测试二进制。
/// 覆盖率、`race`、`short` 等开关都在这里折算成编译参数，确保产物与运行模式一致。
/// 单包构建失败会原样向上返回，让调用方根据子命令语义决定是否继续。
/// 选择单独函数而不是内联命令拼装，主要是为了与批量构建路径共享参数约束。
/// Go `buildTestBinary`.
pub fn build_test_binary(state: &UtState, pkg: &str) -> Result<(), String> {
    let mut args = vec![
        "-c".to_string(),
        "-vet".to_string(),
        "off".to_string(),
        "-o".to_string(),
        test_file_name(pkg),
    ];
    if !state.coverprofile.is_empty() {
        args.push("-cover".to_string());
    }
    if state.race {
        args.push("-race".to_string());
    }
    if state.short {
        args.push("--test.short".to_string());
    }
    let mut cmd = go_test_cmd(&args);
    cmd.dir = state.work_dir.join(pkg);
    match cmd.run_result() {
        r if r.ok => Ok(()),
        r => Err(with_trace(r.err.unwrap_or_else(|| "build failed".into()))),
    }
}

/// 先对 `cmd/tidb-server` 触发一次特殊 `go test`，为后续批量构建预热编译缓存。
/// `-toolexec` 会把真实链接替换成脚本，以便只做编译阶段而不付出完整链接成本。
/// 这是 Go 版批量构建提速的关键前置步骤，Rust 版本保留同样的优化策略。
/// 失败时直接中止批量构建，因为缓存预热失败通常意味着后续整体也会失败。
/// Go `generateBuildCache`.
pub fn generate_build_cache(state: &UtState) -> Result<(), String> {
    let mut cmd = go_test_cmd(&["-exec=true".to_string(), "-vet=off".to_string()]);
    let go_compile_without_link = format!(
        "-toolexec={}",
        state
            .work_dir
            .join("tools")
            .join("check")
            .join("go-compile-without-link.sh")
            .to_string_lossy()
    );
    cmd.args.push(go_compile_without_link);
    cmd.dir = state.work_dir.join("cmd").join("tidb-server");
    match cmd.run_result() {
        r if r.ok => Ok(()),
        r => Err(with_trace(
            r.err
                .unwrap_or_else(|| "generate build cache failed".into()),
        )),
    }
}

/// 批量构建路径会先生成缓存，再借助 `xprog` 一次性并行产出多个测试二进制。
/// 相比逐包循环调用 `go test -c`，这种 staged build 在大仓库上通常更快。
/// 这里传给 `go test` 的是完整模块路径列表，而不是仓库相对路径。
/// 构建并发度使用 `build_parallel`，并且刻意与执行测试时的 worker 并发度分离控制。
/// Go `buildTestBinaryMulti`.
pub fn build_test_binary_multi(state: &UtState, pkgs: &[String]) -> Result<(), String> {
    generate_build_cache(state).map_err(with_trace)?;

    let xprog_path = state.work_dir.join("tools").join("bin").join("xprog");
    let packages: Vec<String> = pkgs
        .iter()
        .map(|pkg| {
            Path::new(&module_path())
                .join(pkg)
                .to_string_lossy()
                .to_string()
        })
        .collect();

    let mut args = vec![
        "-p".to_string(),
        state.build_parallel.to_string(),
        "--exec".to_string(),
        xprog_path.to_string_lossy().to_string(),
        "-vet".to_string(),
        "off".to_string(),
        "-count".to_string(),
        "0".to_string(),
    ];
    if !state.coverprofile.is_empty() {
        args.push("-cover".to_string());
    }
    if state.race {
        args.push("-race".to_string());
    }
    if state.short {
        args.push("--test.short".to_string());
    }
    args.extend(packages);
    let mut cmd = go_test_cmd(&args);
    cmd.dir = state.work_dir.clone();
    match cmd.run_result() {
        r if r.ok => Ok(()),
        r => Err(with_trace(
            r.err.unwrap_or_else(|| "build multi failed".into()),
        )),
    }
}

/// 检查某个包的测试二进制是否已经落盘，用于区分“无测试”与“尚未构建”。
/// Rust 版本把大多数 IO 失败都视作不存在，以贴近 Go 对 `PathError` 的宽松判定。
/// 这样即便文件被并发删除或权限异常，也不会把纯缺失场景误升级成致命错误。
/// 调用方通常会在判空后打印 `no test case`，而不是继续尝试列举测试函数。
/// Go `testBinaryExist`.
pub fn test_binary_exist(state: &UtState, pkg: &str) -> Result<bool, String> {
    let path = test_file_full_path(state, pkg);
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        // Go treats *os.PathError as missing; other IO errors map similarly.
        Err(_) => Ok(false),
    }
}

/// 约定测试二进制名称只取包路径最后一段，再统一追加 `.test.bin` 后缀。
/// 这与 Go 工具的命名方式一致，便于外部脚本直接预测产物文件名。
/// 使用最后一段而不是完整路径，可以避免路径分隔符进入文件名导致平台差异。
/// 这也意味着不同父目录下的同名包必须依赖各自目录隔离，不能输出到同一目录。
/// Go `testFileName`.
pub fn test_file_name(pkg: &str) -> String {
    let file = Path::new(pkg)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    format!("{file}.test.bin")
}

/// 基于工作目录、包路径和约定文件名拼出测试二进制的完整落盘位置。
/// 统一从这里取路径，可以避免存在性检查、枚举和执行各自拼出不同结果。
/// `work_dir` 来自进程启动目录，因此该路径天然绑定当前仓库根，而不是模块名。
/// 只要目录结构与 Go 仓库布局一致，其他调用方就不需要关心平台路径细节。
/// Go `testFileFullPath`.
pub fn test_file_full_path(state: &UtState, pkg: &str) -> PathBuf {
    state.work_dir.join(pkg).join(test_file_name(pkg))
}

/// 执行测试二进制的 `-test.list Test`，从标准输出中解析出真实存在的测试函数名。
/// 这里过滤掉 `TestT` 和 `TestBenchDaily`，是对齐 Go 工具中的历史特例规则。
/// 即使命令失败，只要已经拿到输出，也仍会尝试从中提取测试名而不是立刻报错。
/// 这样可以兼容某些测试二进制行为古怪但仍能列出用例的边缘情况。
/// Go `listNewTestCases`.
pub fn list_new_test_cases(state: &UtState, pkg: &str) -> Result<Vec<String>, String> {
    let exe = format!(".{}{}", std::path::MAIN_SEPARATOR, test_file_name(pkg));
    let mut cmd = TestCommand::new(exe, vec!["-test.list".to_string(), "Test".to_string()]);
    cmd.dir = state.work_dir.join(pkg);
    cmd.capture_output = true;
    let result = cmd.run_result();
    let res: Vec<String> = result.stdout.split('\n').map(|s| s.to_string()).collect();
    if !result.ok && res.iter().all(|s| s.is_empty()) {
        println!("err == {}", result.err.unwrap_or_default());
    }
    Ok(filter_strings(res, |s| {
        s.starts_with("Test") && s != "TestT" && s != "TestBenchDaily"
    }))
}

/// 把命令标准输出按换行拆成字符串数组，供包列表和版本信息这类文本接口复用。
/// 这里统一打开捕获输出，避免各调用点重复设置命令对象的收集策略。
/// 命令失败时会附带堆栈上抛，便于快速看出是哪一个外部工具执行出了问题。
/// 返回值保留空行行为，与 Go 版按 `Split` 切分的结果保持一致。
/// Go `cmdToLines`.
pub fn cmd_to_lines(cmd: TestCommand) -> Result<Vec<String>, String> {
    let mut cmd = cmd;
    cmd.capture_output = true;
    let result = cmd.run_result();
    if !result.ok {
        return Err(with_trace(
            result.err.unwrap_or_else(|| "command failed".into()),
        ));
    }
    Ok(result.stdout.split('\n').map(|s| s.to_string()).collect())
}

/// 一个轻量过滤助手，把 Go 版通用 `filter` 模式映射成 Rust 迭代器写法。
/// 保留独立函数而不是在各处内联，可以让迁移后的控制流程更容易与原文件对照。
/// 这里消费原向量并返回新向量，避免调用者在共享切片和所有权之间反复转换。
/// 谓词只接收借用字符串，足够表达前缀、相等和包含等大多数过滤需求。
/// Go `filter`.
pub fn filter_strings<F>(input: Vec<String>, mut f: F) -> Vec<String>
where
    F: FnMut(&String) -> bool,
{
    input.into_iter().filter(|s| f(s)).collect()
}

/// 在执行前打散任务顺序，降低同类慢测或易失败测例连续堆叠的概率。
/// Rust 这里用一个简单的线性同余序列近似 Go `math/rand.Intn` 的交换式洗牌。
/// 目标不是密码学随机，而是足够便宜且能让任务分布不再固定。
/// 若输入为空会立即返回，避免后续取模时出现除零问题。
/// Go `shuffle` using `math/rand`-style Intn swaps.
pub fn shuffle(tasks: &mut [Task]) {
    let len = tasks.len();
    if len == 0 {
        return;
    }
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    if seed == 0 {
        seed = 1;
    }
    for i in 0..len {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let pos = (seed as usize) % len;
        tasks.swap(i, pos);
    }
}

/// 用于把原始错误文本和采集到的栈信息拼接在一起，模拟 Go `withTrace` 的效果。
/// 这里存的是字符串而非具体错误类型，便于跨线程和跨接口一路透传。
/// 结构体本身不参与控制流，只承担“让错误更可诊断”的展示职责。
/// `Display` 会把两段内容按换行拼接，最终输出风格尽量贴近 Go 版。
/// Go `errWithStack`.
#[derive(Clone, Debug)]
pub struct ErrWithStack {
    pub err: String,
    pub buf: String,
}

impl fmt::Display for ErrWithStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.err, self.buf)
    }
}

/// 为错误补上一份当前线程回溯文本，尽量模拟 Go `runtime.Stack` 带来的定位体验。
/// 如果传入错误本身为空串，则直接返回空串，避免制造无意义的回溯噪声。
/// 这里返回普通字符串，是因为全文件的大多数错误通道都已经选择了 `String`。
/// 统一从这里包装错误后，终端输出就能同时看到简要原因和一段调用轨迹。
/// Go `withTrace`.
pub fn with_trace<E: ToString>(err: E) -> String {
    let err = err.to_string();
    if err.is_empty() {
        return err;
    }
    // Capture a Rust backtrace string analogous to runtime.Stack.
    let buf = std::backtrace::Backtrace::force_capture().to_string();
    ErrWithStack { err, buf }.to_string()
}

/// 把 `Duration` 格式化成秒数字符串，供 JUnit `time` 属性直接消费。
/// 选择秒小数而不是更人类化的格式，是为了匹配 XML 消费方的常见预期。
/// 这里复用了标准库的浮点秒表示，避免手写拆分逻辑带来额外误差。
/// 该格式与 Go 版 `Seconds()` 输出口径一致，便于对比迁移前后的结果。
/// Go `formatDurationAsSeconds`.
pub fn format_duration_as_seconds(d: Duration) -> String {
    format!("{:.6}", d.as_secs_f64())
}

/// 为每个 JUnit suite 生成属性列表，目前只写入 Go 版本这一项元信息。
/// 把它抽成函数后，后续若要补充更多属性，不必改动汇总主流程。
/// 属性值来自执行环境而非单测结果本身，因此适合在 suite 级别统一共享。
/// 返回向量而不是单值，也与 JUnit XML 的一对多结构自然对应。
/// Go `packageProperties`.
pub fn package_properties(go_version: &str) -> Vec<JUnitProperty> {
    vec![JUnitProperty {
        name: "go.version".to_string(),
        value: go_version.to_string(),
    }]
}

/// 获取当前使用的 Go 版本字符串，优先读取 `GOVERSION` 环境变量。
/// 这样可以在 CI 或 stub 场景里跳过真实 `go version` 调用，让结果更可控。
/// 如果外部命令失败，则降级为 `unknown`，避免报告生成因附加元数据失败而中断。
/// 返回值会写进 JUnit 属性，帮助排查不同 Go 版本造成的行为差异。
/// Go `goVersion`.
pub fn go_version() -> String {
    if let Ok(version) = std::env::var("GOVERSION") {
        return version;
    }
    let mut cmd = TestCommand::new("go", vec!["version".to_string()]);
    cmd.capture_output = true;
    match cmd.run_result() {
        r if r.ok => r
            .stdout
            .trim()
            .trim_start_matches("go version ")
            .to_string(),
        _ => "unknown".to_string(),
    }
}

/// 把 JUnit 结果写成 XML 文本，先输出声明头，再输出手工拼装的主体内容。
/// Rust 版本没有直接依赖通用 XML 序列化，而是沿着当前轻量数据模型手写输出。
/// 这样能更精确控制属性转义与节点顺序，也避免为这个工具额外引入重依赖。
/// 任何写入失败都会回传给调用方，由上层决定打印信息还是终止流程。
/// Go `write` — xml.Header + MarshalIndent.
pub fn write(out: &mut dyn Write, suites: &JUnitTestSuites) -> io::Result<()> {
    out.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>"#)?;
    out.write_all(b"\n")?;
    out.write_all(suites.to_xml().as_bytes())?;
    Ok(())
}

/// 对外部命令的一层轻量封装，把程序名、参数、目录和输出策略打包在一起。
/// 该结构是 Rust 版的重要替身点，因为真实执行会经过 `stubs::run_command`。
/// 这样测试或迁移验证时可以拦截系统调用，而不必真的触发 `go` 或测试二进制。
/// 字段设计尽量贴近 Go `exec.Cmd` 的常用子集，方便按原始逻辑逐步搬运。
/// Go `exec.Cmd` wrapper routed through stubs.
#[derive(Clone, Debug, Default)]
pub struct TestCommand {
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
    pub capture_output: bool,
}

impl TestCommand {
    /// 统一初始化命令对象，默认不设置工作目录，也不主动捕获输出。
    /// 调用方随后可以按需要补上 `dir` 和 `capture_output`，覆盖不同场景的需求。
    /// 泛型参数允许传入字面量或现成字符串，减少构造命令时的样板转换。
    /// 这个工厂函数让各处的命令创建形式保持一致，便于后续统一扩展字段。
    pub fn new<S: Into<String>>(program: S, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            dir: PathBuf::new(),
            capture_output: false,
        }
    }

    /// 把当前命令对象转换成 stub 层需要的 `CommandSpec`，并返回统一结果结构。
    /// 真正的进程创建被集中在 `stubs::run_command`，这里自身不包含平台相关细节。
    /// 这种间接层让同一份高层逻辑既能跑真实命令，也能在测试中注入假结果。
    /// 返回的 `CommandResult` 同时承载成功位、标准输出和错误文本三类信息。
    pub fn run_result(&mut self) -> CommandResult {
        let spec = CommandSpec {
            program: self.program.clone(),
            args: self.args.clone(),
            dir: self.dir.clone(),
            capture_output: self.capture_output,
        };
        stubs::run_command(&spec)
    }
}

/// JUnit 根节点对应的数据结构，内部保存多个按包分组的测试套件。
/// 这里没有直接建模 XML 命名空间等复杂特性，因为当前产物只需满足常见 CI 消费。
/// 顶层结构保持极简，方便在收集结果时直接 `push` 新 suite。
/// 真正的文本序列化逻辑放在 `to_xml()`，避免数据与展示代码完全耦死。
/// Go `JUnitTestSuites`.
#[derive(Clone, Debug, Default)]
pub struct JUnitTestSuites {
    pub suites: Vec<JUnitTestSuite>,
}

impl JUnitTestSuites {
    /// 把顶层 suite 集合串成 `<testsuites>` 根节点。
    /// 这里采用手工字符串拼接，是因为数据模型很小且转义需求可控。
    /// 节点顺序由 `suites` 向量本身决定，不额外做排序或重排。
    /// 这样可以最大限度保留汇总阶段已经形成的结果顺序。
    pub fn to_xml(&self) -> String {
        let mut s = String::from("<testsuites>");
        for suite in &self.suites {
            s.push_str(&suite.to_xml());
        }
        s.push_str("</testsuites>");
        s
    }
}

/// 表示单个包级测试套件，包含统计数字、属性列表以及具体测试用例。
/// 字段布局贴合最终 XML 的属性和子节点，减少写出时的额外映射成本。
/// `tests` 与 `failures` 会在构造时一次性算好，序列化阶段不再重复遍历。
/// 这种形状与 Go 版“先组装对象再编码”的思路保持一致。
/// Go `JUnitTestSuite`.
#[derive(Clone, Debug, Default)]
pub struct JUnitTestSuite {
    pub tests: usize,
    pub failures: usize,
    pub time: String,
    pub name: String,
    pub properties: Vec<JUnitProperty>,
    pub test_cases: Vec<JUnitTestCase>,
}

impl JUnitTestSuite {
    /// 把一个包级 suite 序列化成 `<testsuite>` 节点，并顺序写出属性与用例。
    /// 仅当 `properties` 非空时才生成 `<properties>`，避免产生无意义的空节点。
    /// 属性和值都会先经过 XML 转义，防止包名或版本文本破坏文档结构。
    /// 这里不缩进也不换行，保持与当前手写输出策略一致。
    fn to_xml(&self) -> String {
        let mut s = format!(
            "<testsuite tests=\"{}\" failures=\"{}\" time=\"{}\" name=\"{}\">",
            self.tests,
            self.failures,
            xml_escape_attr(&self.time),
            xml_escape_attr(&self.name)
        );
        if !self.properties.is_empty() {
            s.push_str("<properties>");
            for p in &self.properties {
                s.push_str(&format!(
                    "<property name=\"{}\" value=\"{}\"></property>",
                    xml_escape_attr(&p.name),
                    xml_escape_attr(&p.value)
                ));
            }
            s.push_str("</properties>");
        }
        for case in &self.test_cases {
            s.push_str(&case.to_xml());
        }
        s.push_str("</testsuite>");
        s
    }
}

/// 表示单个测试函数在 JUnit 中的记录，既能表达成功，也能表达跳过或失败。
/// `classname` 对应包路径，`name` 对应具体测试名，是报告聚合的核心维度。
/// 失败内容单独挂在可选字段上，这样成功案例不会携带多余的空字符串负担。
/// 结构保持与 Go 版字段名一致，有助于迁移过程中逐项核对输出。
/// Go `JUnitTestCase`.
#[derive(Clone, Debug, Default)]
pub struct JUnitTestCase {
    pub classname: String,
    pub name: String,
    pub time: String,
    pub skip_message: Option<JUnitSkipMessage>,
    pub failure: Option<JUnitFailure>,
}

impl JUnitTestCase {
    /// 把单个测试结果序列化成 `<testcase>` 节点，并按需附加跳过或失败子节点。
    /// 失败内容走文本节点转义，避免日志中的尖括号和与号把 XML 结构写坏。
    /// 跳过与失败都建模成可选字段，因此这里只在有值时才拼接对应节点。
    /// 手工拼装虽然朴素，但足以覆盖当前报告所需的全部结构。
    fn to_xml(&self) -> String {
        let mut s = format!(
            "<testcase classname=\"{}\" name=\"{}\" time=\"{}\">",
            xml_escape_attr(&self.classname),
            xml_escape_attr(&self.name),
            xml_escape_attr(&self.time)
        );
        if let Some(skip) = &self.skip_message {
            s.push_str(&format!(
                "<skipped message=\"{}\"></skipped>",
                xml_escape_attr(&skip.message)
            ));
        }
        if let Some(fail) = &self.failure {
            s.push_str(&format!(
                "<failure message=\"{}\" type=\"{}\">{}</failure>",
                xml_escape_attr(&fail.message),
                xml_escape_attr(&fail.failure_type),
                xml_escape_text(&fail.contents)
            ));
        }
        s.push_str("</testcase>");
        s
    }
}

/// 表示 JUnit `<skipped>` 节点上的信息，目前只需要保留一条描述消息。
/// 单独拆成结构而不是直接用字符串，是为了贴合 XML 节点和属性的层级语义。
/// 即便当前文件很少生产跳过结果，也保留该模型以与 Go 结构保持完整对应。
/// 未来若要补充更多跳过原因字段，可以在这里继续扩展而不破坏外层接口。
/// Go `JUnitSkipMessage`.
#[derive(Clone, Debug, Default)]
pub struct JUnitSkipMessage {
    pub message: String,
}

/// 表示 JUnit 属性的键值对，通常用于记录执行环境而非单测本身的结果。
/// 当前主要存放 Go 版本，但结构故意保持通用，便于后续追加更多属性。
/// 属性被挂在 suite 级别，因此同一包下所有测试共享同一份环境描述。
/// 这种建模让 XML 输出过程只需逐项展开，不必再写额外的特殊分支。
/// Go `JUnitProperty`.
#[derive(Clone, Debug, Default)]
pub struct JUnitProperty {
    pub name: String,
    pub value: String,
}

/// 表示失败用例在 JUnit 中的载荷，既包含简短消息，也包含完整失败输出。
/// `failure_type` 目前通常留空，因为原始 Go 工具同样没有细分失败类型。
/// 把标准输出和标准错误合并进 `contents`，有助于 CI 页面直接展示上下文。
/// 调用方只在确实失败时才填充该结构，从而让成功案例保持最小表示。
/// Go `JUnitFailure`.
#[derive(Clone, Debug, Default)]
pub struct JUnitFailure {
    pub message: String,
    pub failure_type: String,
    pub contents: String,
}

/// 对 XML 属性值做最小必要转义，避免引号或尖括号破坏生成结果。
/// 这里只处理当前输出路径会遇到的核心字符，不追求完整 XML 规范实现。
/// 属性和文本节点分开处理，是因为两者对双引号的约束并不相同。
/// 简单的字符串替换已足够满足本工具产物规模，也避免引入额外 XML 依赖。
fn xml_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 对 XML 文本节点做转义，用于失败输出等可能包含特殊字符的自由文本。
/// 与属性版本相比，这里不需要处理双引号，因为文本节点不受属性边界约束。
/// 失败日志往往包含 `<`、`>` 或 `&`，如果不转义会直接把报告写坏。
/// 单独保留这个函数也能让 `to_xml()` 保持可读，而不是夹杂大量替换细节。
fn xml_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
