// Copyright 2026 AsterSQL.
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
// `cgmon` 迁移期单元测试：验证 cgroup 监控器探针、配额换算与启停语义。
//
// 通过注入假探针覆盖：探针失败时回退主机默认值、CPU 配额向上取整、
// 内存取主机与 cgroup 较小值，以及 `start`/`stop` 幂等与立即刷新。

#![allow(non_snake_case)]

#[cfg(test)]
mod tests {
    use crate::CgroupMonitor;
    use anyhow::{Result, anyhow};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    /// 构造带自定义探针的 `CgroupMonitor`，刷新周期设为很长以便测试主动调用刷新。
    fn monitor<CPUCount, CPUQuota, TotalMemory, MemoryLimit>(
        cpu_count: CPUCount,
        cpu_quota: CPUQuota,
        total_memory: TotalMemory,
        memory_limit: MemoryLimit,
    ) -> CgroupMonitor
    where
        CPUCount: Fn() -> Result<usize> + Send + Sync + 'static,
        CPUQuota: Fn() -> Result<(i64, i64)> + Send + Sync + 'static,
        TotalMemory: Fn() -> Result<u64> + Send + Sync + 'static,
        MemoryLimit: Fn() -> Result<u64> + Send + Sync + 'static,
    {
        CgroupMonitor::with_probes(
            Duration::from_secs(3600),
            cpu_count,
            cpu_quota,
            total_memory,
            memory_limit,
        )
    }

    #[test]
    /// 探针报错时仍应暴露主机默认 CPU/内存，并返回对应错误。
    fn cgroup_probe_errors_return_errors_after_publishing_host_defaults() {
        let monitor = monitor(
            || Ok(8),
            || Err(anyhow!("mock cpu error")),
            || Ok(16 * 1024),
            || Err(anyhow!("mock memory error")),
        );

        assert_eq!(
            monitor.refresh_cgroup_cpu().unwrap_err().to_string(),
            "mock cpu error"
        );
        assert_eq!(monitor.last_cpu(), 8);
        assert_eq!(monitor.max_procs_metric(), 8.0);

        assert_eq!(
            monitor.refresh_cgroup_memory().unwrap_err().to_string(),
            "mock memory error"
        );
        assert_eq!(monitor.last_memory_limit(), 16 * 1024);
        assert_eq!(monitor.memory_limit_metric(), (16 * 1024) as f64);
    }

    #[test]
    /// CPU 配额：正且低于主机容量时向上取整；无限制或超过主机则用主机核数。
    fn cpu_quota_uses_ceiling_only_when_positive_and_below_host_capacity() {
        let constrained = monitor(|| Ok(8), || Ok((100_000, 250_001)), || Ok(1), || Ok(1));
        constrained.refresh_cgroup_cpu().unwrap();
        assert_eq!(constrained.last_cpu(), 3);

        let unlimited = monitor(|| Ok(8), || Ok((0, -1)), || Ok(1), || Ok(1));
        unlimited.refresh_cgroup_cpu().unwrap();
        assert_eq!(unlimited.last_cpu(), 8);

        let above_host = monitor(|| Ok(8), || Ok((100, 900)), || Ok(1), || Ok(1));
        above_host.refresh_cgroup_cpu().unwrap();
        assert_eq!(above_host.last_cpu(), 8);
    }

    #[test]
    /// 内存刷新取 cgroup 与主机较小者；主机虚拟内存读取失败则保留错误且限额为 0。
    fn memory_refresh_selects_the_smaller_limit_and_preserves_system_read_errors() {
        let constrained = monitor(|| Ok(1), || Ok((1, 1)), || Ok(32_000), || Ok(12_000));
        constrained.refresh_cgroup_memory().unwrap();
        assert_eq!(constrained.last_memory_limit(), 12_000);

        let above_host = monitor(|| Ok(1), || Ok((1, 1)), || Ok(32_000), || Ok(64_000));
        above_host.refresh_cgroup_memory().unwrap();
        assert_eq!(above_host.last_memory_limit(), 32_000);

        let failed = monitor(
            || Ok(1),
            || Ok((1, 1)),
            || Err(anyhow!("virtual memory unavailable")),
            || Ok(12_000),
        );
        assert_eq!(
            failed.refresh_cgroup_memory().unwrap_err().to_string(),
            "virtual memory unavailable"
        );
        assert_eq!(failed.last_memory_limit(), 0);
    }

    #[test]
    /// `start` 仅成功一次并立即触发刷新；`stop` 幂等。
    fn monitor_starts_once_refreshes_immediately_and_stops_idempotently() {
        let cpu_calls = Arc::new(AtomicUsize::new(0));
        let memory_calls = Arc::new(AtomicUsize::new(0));
        let cpu_calls_for_probe = Arc::clone(&cpu_calls);
        let memory_calls_for_probe = Arc::clone(&memory_calls);
        let monitor = monitor(
            || Ok(4),
            move || {
                cpu_calls_for_probe.fetch_add(1, Ordering::SeqCst);
                Ok((100, 200))
            },
            || Ok(8_000),
            move || {
                memory_calls_for_probe.fetch_add(1, Ordering::SeqCst);
                Ok(4_000)
            },
        );

        assert!(monitor.start());
        assert!(!monitor.start());
        let deadline = Instant::now() + Duration::from_secs(2);
        while (cpu_calls.load(Ordering::SeqCst) == 0 || memory_calls.load(Ordering::SeqCst) == 0)
            && Instant::now() < deadline
        {
            thread::yield_now();
        }
        assert_eq!(cpu_calls.load(Ordering::SeqCst), 1);
        assert_eq!(memory_calls.load(Ordering::SeqCst), 1);
        assert!(monitor.stop());
        assert!(!monitor.stop());
    }
}
