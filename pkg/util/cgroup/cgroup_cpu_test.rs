// Copyright 2022 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// cgroup CPU 集成向测试：在真实容器环境中调用 `GetCgroupCPU`。
//
// 非容器环境跳过；老内核（≤4.7）上缺少 cpu controller 的错误可忽略。

use super::{GetCgroupCPU, InContainer};
use regex::Regex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

/// 解析 `uname` 发布串，判断内核是否新于给定 major.minor。
fn check_kernel_version_newer_than(major: i32, minor: i32) -> bool {
    let mut uts = std::mem::MaybeUninit::<libc::utsname>::zeroed();
    // SAFETY: uname initializes the supplied utsname on success.
    let rc = unsafe { libc::uname(uts.as_mut_ptr()) };
    assert_eq!(0, rc, "uname failed: {}", std::io::Error::last_os_error());
    // SAFETY: rc == 0 means uname initialized uts, whose release field is NUL terminated.
    let uts = unsafe { uts.assume_init() };
    let release = unsafe { std::ffi::CStr::from_ptr(uts.release.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    eprintln!("kernel release string: {release}");

    let version_re = Regex::new(r"[0-9]+\.[0-9]+\.[0-9]+").unwrap();
    let version = version_re
        .find(&release)
        .unwrap_or_else(|| panic!("release str is {release}"))
        .as_str();
    let parts: Vec<i32> = version
        .split('.')
        .map(|part| {
            part.parse()
                .unwrap_or_else(|_| panic!("invalid kernel version {version}"))
        })
        .collect();
    assert_eq!(3, parts.len(), "kernel verion str is {version}");
    eprintln!(
        "parsed kernel version parts: major {}, minor {}, patch {}",
        parts[0], parts[1], parts[2]
    );
    parts[0] > major || (parts[0] == major && parts[1] > minor)
}

#[test]
/// 容器内并发空转线程背景下读取 CPU period；校验非零且大于 1。
fn test_get_cgroup_cpu() {
    if !InContainer() {
        eprintln!("Not in container, skip this test case.");
        return;
    }

    // 启动若干空转线程以制造一点 CPU 活动，再采样 cgroup。
    let exit = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..10)
        .map(|_| {
            let exit = Arc::clone(&exit);
            thread::spawn(move || {
                while !exit.load(Ordering::Acquire) {
                    thread::yield_now();
                }
            })
        })
        .collect();

    let result = GetCgroupCPU();
    exit.store(true, Ordering::Release);
    for worker in workers {
        worker.join().expect("worker cleanup");
    }

    match result {
        Ok(cpu) => {
            assert_ne!(0, cpu.Period);
            assert!(1_i64 < cpu.Period);
        }
        Err(err) if err.to_string().contains("no cpu controller detected") => {
            if check_kernel_version_newer_than(4, 7) {
                panic!("linux version > v4.7 and err still happens: {err}");
            }
            eprintln!("the error '{err}' is ignored because the kernel is too old");
        }
        Err(err) => panic!("GetCgroupCPU failed: {err:#}"),
    }
}
