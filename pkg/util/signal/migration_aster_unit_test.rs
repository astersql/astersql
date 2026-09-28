// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// signal 包迁移回归测试（Unix）。
//
// 覆盖：仅 SIGUSR1 产生诊断栈转储、关机分发保留信号号、非法信号发送返回错误且不终止进程。

#[cfg(unix)]
mod unix {
    use astersql_util_signal::{
        dispatch_shutdown_signal, handle_usr1_signal, send_to_current_process,
    };
    use std::sync::{Arc, Mutex};

    /// 非 USR1 不产生 dump；USR1 产生的文本含约定标记。
    #[test]
    fn diagnostic_dump_is_only_created_for_usr1() {
        assert!(handle_usr1_signal(libc::SIGTERM).is_none());
        let dump = handle_usr1_signal(libc::SIGUSR1).expect("SIGUSR1 must create a dump");
        assert!(dump.contains("Got signal"));
        assert!(dump.contains("stack backtrace"));
        assert!(dump.contains("Finished dumping"));
    }

    /// 关机分发应把收到的信号原样交给回调。
    #[test]
    fn shutdown_dispatch_preserves_the_received_signal() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        dispatch_shutdown_signal(libc::SIGQUIT, move |sig| {
            captured.lock().unwrap().push(sig);
        });
        assert_eq!(*received.lock().unwrap(), vec![libc::SIGQUIT]);
    }

    /// 非法信号号应返回 Err，且不因发送失败而终止当前测试进程。
    #[test]
    fn invalid_signal_send_reports_error_without_terminating_process() {
        assert!(send_to_current_process(-1).is_err());
    }
}
