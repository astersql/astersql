// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Unix dynamic pprof listener ported from `br/pkg/utils/dyn_pprof_unix.go`.
//!
//! POSIX 平台在收到 SIGUSR1 类信号后按需启动 status/pprof HTTP 监听。
//! 与 Go 一致：监听绑定 `0.0.0.0:0`；失败只打日志并退出后台循环。
//! 非 POSIX 平台见 `dyn_pprof_other` 空实现。

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd", unix))]
mod imp {
    use std::sync::Arc;
    use std::thread;

    use astersql_br_pkg_logutil::{Field, log};
    use astersql_util::security::TLS;
    use signal_hook::consts::signal::SIGUSR1;
    use signal_hook::iterator::Signals;

    use crate::pprof::StartStatusListener;

    pub(crate) fn listen_for_start_signal<F>(mut signals: Signals, mut start: F)
    where
        F: FnMut() -> Result<(), String>,
    {
        for signal in signals.forever() {
            if signal != SIGUSR1 {
                continue;
            }
            log::L().Info(
                "signal received, starting pprof...",
                [Field::string("signal", "SIGUSR1")],
            );
            if let Err(err) = start() {
                log::Warn("failed to start pprof", [Field::string("error", err)]);
                return;
            }
        }
    }

    /// 注册 SIGUSR1，并在独立线程中按需启动 pprof/status 监听。
    pub fn StartDynamicPProfListener(tls: Option<Arc<TLS>>) {
        let Ok(signals) = Signals::new([SIGUSR1]) else {
            log::Warn("failed to register SIGUSR1 listener", []);
            return;
        };
        thread::spawn(move || {
            listen_for_start_signal(signals, || {
                StartStatusListener("0.0.0.0:0", tls.as_deref()).map_err(|err| err.to_string())
            });
        });
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd", unix))]
pub use imp::StartDynamicPProfListener;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd", unix))]
pub(crate) use imp::listen_for_start_signal;
