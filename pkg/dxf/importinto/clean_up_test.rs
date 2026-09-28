// Copyright 2026 AsterSQL.

use std::collections::HashMap;

use astersql_lightning_verification::DataKVGroupID;

use crate::{Checksum, PostProcessStepMeta, meterDataFromPostProcess};

#[test]
fn meter_data_matches_go_uint64_aggregation() {
    let meta = PostProcessStepMeta {
        Checksum: HashMap::from([
            (
                DataKVGroupID,
                Checksum {
                    KVs: 7,
                    Size: 11,
                    ..Default::default()
                },
            ),
            (
                1,
                Checksum {
                    Size: u64::MAX,
                    ..Default::default()
                },
            ),
            (
                2,
                Checksum {
                    Size: 1,
                    ..Default::default()
                },
            ),
        ]),
        ..Default::default()
    };

    assert_eq!(meterDataFromPostProcess(&meta), (7, 11, 0));
}
