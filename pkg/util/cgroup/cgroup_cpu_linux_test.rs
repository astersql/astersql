// Copyright 2026 AsterSQL.

#[path = "cgroup.rs"]
mod cgroup;
pub use cgroup::*;
#[path = "cgroup_cpu.rs"]
mod cgroup_cpu;
use cgroup_cpu::*;
#[path = "cgroup_cpu_linux.rs"]
mod cgroup_cpu_linux;
pub use cgroup_cpu_linux::*;

#[test]
fn cgroup_probe_matches_go_for_non_utf8_proc_contents() {
    assert!(cgroup_cpu_linux::in_container_content(
        std::path::Path::new(procPathCGroup),
        &[0xff, b'd', b'o', b'c', b'k', b'e', b'r'],
    ));
}
