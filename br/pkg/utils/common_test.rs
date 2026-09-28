// Copyright 2026 AsterSQL.

use super::common::{CheckNextGenCompatibility, GenGlobalIDs};
use super::stubs::{KvContext, Storage};
use astersql_config_kerneltype::IsClassic;

struct TestStorage;

impl Storage for TestStorage {}

#[test]
fn gen_global_ids_reuses_storage_state_like_go() {
    let storage = TestStorage;

    let first = GenGlobalIDs(KvContext::todo(), 2, &storage).expect("first allocation");
    let second = GenGlobalIDs(KvContext::todo(), 2, &storage).expect("second allocation");

    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 2);
    assert_eq!(second[0], first[1] + 1);
}

#[test]
fn compatibility_matches_go_kernel_specific_paths() {
    if IsClassic() {
        assert!(!CheckNextGenCompatibility("", true));
        assert!(!CheckNextGenCompatibility("keyspace", false));
    } else {
        assert!(std::panic::catch_unwind(|| CheckNextGenCompatibility("", false)).is_err());
        assert!(CheckNextGenCompatibility("keyspace", true));
    }
}
