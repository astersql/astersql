// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn normalized_error_exposes_the_go_error_contract() {
    let cases = [
        (
            &ErrNoLeader,
            "KV:ErrNoLeader",
            "region has no leader, region '%d'",
        ),
        (
            &ErrKVEpochNotMatch,
            "Ingest:EpochNotMatch",
            "epoch not match",
        ),
        (&ErrKVNotLeader, "Ingest:NotLeader", "not leader"),
        (&ErrKVServerIsBusy, "Ingest:ServerIsBusy", "server is busy"),
        (
            &ErrKVRegionNotFound,
            "Ingest:RegionNotFound",
            "region not found",
        ),
        (
            &ErrKVReadIndexNotReady,
            "Ingest:ReadIndexNotReady",
            "read index not ready",
        ),
        (&ErrKVDiskFull, "Ingest:StoreDiskFull", "store disk full"),
        (
            &ErrKVIngestFailed,
            "Ingest:ErrKVIngestFailed",
            "ingest tikv failed",
        ),
        (
            &ErrKVRaftProposalDropped,
            "Ingest:ErrKVRaftProposalDropped",
            "raft proposal dropped",
        ),
    ];
    for (error, code, message) in cases {
        assert_eq!(error.Code(), 0);
        assert_eq!(error.ID(), code);
        assert_eq!(error.RFCCode(), code);
        assert_eq!(error.MessageTemplate(), message);
        assert_eq!(error.GetMsg(), message);
        assert_eq!(error.Error(), format!("[{code}]{message}"));
    }

    let generated = ErrNoLeader.GenWithStackByArgs(42);
    assert_eq!(
        generated.MessageTemplate(),
        "region has no leader, region '%d'"
    );
    assert_eq!(generated.GetSelfMsg(), "region has no leader, region '42'");
    assert_eq!(
        generated.Error(),
        "[KV:ErrNoLeader]region has no leader, region '42'"
    );
}

#[test]
fn normalized_error_generation_and_identity_match_go() {
    let generated = ErrKVDiskFull.GenWithStack("store 1 disk full");
    assert_eq!(generated.MessageTemplate(), "store 1 disk full");
    assert_eq!(generated.Error(), "[Ingest:StoreDiskFull]store 1 disk full");
    assert!(IsKVDiskFullError(&generated));

    let same_class = NormalizedError::new("different detail", "Ingest:StoreDiskFull");
    assert!(ErrKVDiskFull.Is(&same_class));
    assert_eq!(ErrKVDiskFull, same_class);
    assert!(!ErrKVDiskFull.Is(&ErrKVServerIsBusy));
}
