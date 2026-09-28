// Copyright 2026 AsterSQL.

// TestMain's Rust runtime adapter: run common setup before the suite, retain
// test failures, and check for leaked native workers only after a successful
// run. The four Go IgnoreTopFunction entries name Go-only dependencies; none
// runs in this crate, so no Rust worker is exempted by name or by a baseline.
use std::time::Duration;

pub fn run(tests: Vec<libtest_mimic::Trial>) {
    testsetup::SetupForCommonTest();
}

#[cfg(target_os = "linux")]
fn extra_threads() -> Result<Vec<u64>, String> {
    // The runner executes on the process main thread. Reading the kernel task
    // directory also catches detached std::thread and foreign native workers.
    let main = u64::from(std::process::id());
    std::fs::read_dir("/proc/self/task")
        .map_err(|error| error.to_string())?
        .map(|entry| {
            let entry = entry.map_err(|error| error.to_string())?;
            entry
                .file_name()
                .to_string_lossy()
                .parse::<u64>()
                .map_err(|error| error.to_string())
        })
        .filter(|id| !matches!(id, Ok(id) if *id == main))
        .collect()
}

#[cfg(target_os = "macos")]
fn extra_threads() -> Result<Vec<u64>, String> {
    use libc::*;
    // libc exposes task_threads/thread_info, but not mach_port_deallocate.
    unsafe extern "C" {
        fn mach_port_deallocate(task: mach_port_t, name: mach_port_t) -> kern_return_t;
    }
    struct Snapshot {
        task: mach_port_t,
        ports: thread_act_array_t,
        count: mach_msg_type_number_t,
    }
    impl Drop for Snapshot {
        fn drop(&mut self) {
            // SAFETY: task_threads returned this array and a send right for
            // each element. Release both on every path, including query errors.
            unsafe {
                for port in std::slice::from_raw_parts(self.ports, self.count as usize) {
                    mach_port_deallocate(self.task, *port);
                }
                vm_deallocate(
                    self.task,
                    self.ports as vm_address_t,
                    self.count as vm_size_t * std::mem::size_of::<thread_t>() as vm_size_t,
                );
            }
        }
    }
    // SAFETY: every FFI buffer has the layout/length required by Mach. We only
    // inspect our own task; the snapshot owns all acquired kernel resources.
    unsafe {
        let task = mach_task_self();
        let mut ports = std::ptr::null_mut();
        let mut count = 0;
        let status = task_threads(task, &mut ports, &mut count);
        if status != KERN_SUCCESS {
            return Err(format!("task_threads: {status}"));
        }
        let snapshot = Snapshot { task, ports, count };
        let mut current = 0;
        if pthread_threadid_np(0, &mut current) != 0 {
            return Err("pthread_threadid_np failed".into());
        }
        let mut extra = Vec::new();
        for port in std::slice::from_raw_parts(snapshot.ports, snapshot.count as usize) {
            let mut info: thread_identifier_info = std::mem::zeroed();
            let mut size = THREAD_IDENTIFIER_INFO_COUNT;
            let status = thread_info(
                *port,
                THREAD_IDENTIFIER_INFO as thread_flavor_t,
                (&mut info as *mut thread_identifier_info).cast(),
                &mut size,
            );
            if status != KERN_SUCCESS {
                // A worker can finish between enumeration and inspection.
                // Retry the whole snapshot; never silently ignore query errors.
                return Err(format!("thread_info: {status}"));
            }
            if info.thread_id != current {
                extra.push(info.thread_id);
            }
        }
        Ok(extra)
    }
}

#[cfg(windows)]
fn extra_threads() -> Result<Vec<u64>, String> {
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::GetCurrentThreadId,
        },
    };
    struct Snapshot(HANDLE);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            // SAFETY: the snapshot owns the valid handle returned below.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    // SAFETY: the entry's size is initialized as required by ToolHelp; all
    // handles and buffers remain valid through enumeration and are released.
    unsafe {
        let raw = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if raw == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let snapshot = Snapshot(raw);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let current = GetCurrentThreadId();
        let process = std::process::id();
        let mut extra = Vec::new();
        let mut success = Thread32First(snapshot.0, &mut entry);
        while success != 0 {
            if entry.th32OwnerProcessID == process && entry.th32ThreadID != current {
                extra.push(u64::from(entry.th32ThreadID));
            }
            success = Thread32Next(snapshot.0, &mut entry);
        }
        let error = GetLastError();
        if error != ERROR_NO_MORE_FILES {
            return Err(format!("thread enumeration: {error}"));
        }
        Ok(extra)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
compile_error!("logutil TestMain needs native thread enumeration for this platform");
