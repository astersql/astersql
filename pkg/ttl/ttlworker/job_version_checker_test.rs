// Copyright 2026 AsterSQL.

use crate::job_version_checker::{
    JobVersionCheckResult, JobVersionChecker, ServerInfo, VersionInfo, version_infos_consistent,
};

fn server(version: &str, hash: &str, assumed: bool) -> ServerInfo {
    ServerInfo {
        version: VersionInfo {
            version: version.into(),
            git_hash: hash.into(),
        },
        assumed,
    }
}

#[test]
fn server_version_gate_uses_complete_build_and_ignores_assumed_entries() {
    let current = VersionInfo {
        version: "v1".into(),
        git_hash: "a".into(),
    };
    assert!(
        version_infos_consistent(
            &current,
            &[
                ("self".into(), Some(server("v1", "a", false))),
                ("assumed".into(), Some(server("old", "old", true))),
            ]
        )
        .unwrap()
    );
    assert!(
        !version_infos_consistent(
            &current,
            &[("self".into(), Some(server("v1", "b", false))),]
        )
        .unwrap()
    );
    assert!(version_infos_consistent(&current, &[]).is_err());
}

#[test]
fn checker_blocks_known_mismatch_but_falls_back_on_unknown_state_and_caches() {
    let mut checker = JobVersionChecker::default();
    let local = server("v1", "a", false);
    assert_eq!(
        checker.check(
            1,
            Ok(Some(local.clone())),
            Ok(vec![("old".into(), Some(server("v0", "z", false)))])
        ),
        JobVersionCheckResult::BlockJob
    );
    assert_eq!(
        checker.check(30, Err("lookup".into()), Err("lookup".into())),
        JobVersionCheckResult::BlockJob
    );
    assert_eq!(
        checker.check(61, Err("lookup".into()), Err("lookup".into())),
        JobVersionCheckResult::FallbackToPrimaryKey
    );
    assert_eq!(
        checker.check(
            72,
            Ok(Some(local.clone())),
            Ok(vec![("self".into(), Some(local))])
        ),
        JobVersionCheckResult::AllowIndexScan
    );
}
