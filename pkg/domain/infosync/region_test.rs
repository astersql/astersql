// Copyright 2026 AsterSQL.

use crate::*;
use std::collections::HashMap;
use std::sync::Arc;

fn init(client: Option<Arc<dyn PdHttpClient>>) {
    GlobalInfoSyncerInit(
        "region-test".into(),
        Arc::new(|| 1),
        None,
        None,
        client,
        Codec::default(),
        false,
        None,
    )
    .unwrap();
}

struct RegionClient;

impl RegionClient {
    fn require_released_client_lock(&self) -> Result<()> {
        let syncer = getGlobalInfoSyncer()?;
        syncer
            .pdHTTPCli
            .try_write()
            .map(|_| ())
            .map_err(|_| Error::External("pd http client lock held during callback".into()))
    }
}

impl PdHttpClient for RegionClient {
    fn get_regions_replicated_state(&self, range: &KeyRange) -> Result<String> {
        self.require_released_client_lock()?;
        assert_eq!(range.start_key, b"a");
        assert_eq!(range.end_key, b"z");
        Ok("REPLICATED".into())
    }

    fn get_region_distribution(
        &self,
        range: &KeyRange,
        engine: &str,
    ) -> Result<RegionDistributions> {
        self.require_released_client_lock()?;
        assert_eq!(range.start_key, b"a");
        assert_eq!(range.end_key, b"z");
        assert_eq!(engine, "tiflash");
        Ok(RegionDistributions {
            RegionCount: 2,
            StorePeerCount: HashMap::from([(7, 2)]),
        })
    }

    fn get_scheduler_config(&self, name: &str) -> Result<ConfigValue> {
        self.require_released_client_lock()?;
        assert_eq!(name, "balance-region");
        Ok(ConfigValue::Bool(true))
    }

    fn create_scheduler(&self, name: &str, input: &HashMap<String, ConfigValue>) -> Result<()> {
        self.require_released_client_lock()?;
        assert_eq!(name, "balance-region");
        assert_eq!(input.get("enabled"), Some(&ConfigValue::Bool(true)));
        Ok(())
    }

    fn cancel_scheduler_job(&self, name: &str, job_id: u64) -> Result<()> {
        self.require_released_client_lock()?;
        assert_eq!(name, "balance-region");
        assert_eq!(job_id, 42);
        Ok(())
    }
}

#[test]
fn region_calls_match_go_without_holding_the_client_slot_lock() {
    let _guard = crate::info_test::serial();
    init(Some(Arc::new(RegionClient)));

    assert_eq!(
        GetReplicationState(b"a".to_vec(), b"z".to_vec()).unwrap(),
        PlacementScheduleState::PlacementScheduleStateScheduled
    );
    assert_eq!(
        GetRegionDistributionByKeyRange(b"a".to_vec(), b"z".to_vec(), "tiflash").unwrap(),
        RegionDistributions {
            RegionCount: 2,
            StorePeerCount: HashMap::from([(7, 2)]),
        }
    );
    assert_eq!(
        GetSchedulerConfig("balance-region").unwrap(),
        ConfigValue::Bool(true)
    );
    let input = HashMap::from([("enabled".into(), ConfigValue::Bool(true))]);
    CreateSchedulerConfigWithInput("balance-region", &input).unwrap();
    CancelSchedulerJob("balance-region", 42).unwrap();
}

#[test]
fn replication_state_and_missing_client_match_go_fallbacks() {
    let _guard = crate::info_test::serial();
    init(None);

    assert_eq!(PlacementScheduleState::default().String(), "PENDING");
    assert_eq!(
        GetReplicationState(Vec::new(), Vec::new()).unwrap(),
        PlacementScheduleState::PlacementScheduleStatePending
    );
    assert!(matches!(
        GetRegionDistributionByKeyRange(Vec::new(), Vec::new(), "tikv"),
        Err(Error::PdHttpClientMissing)
    ));
    assert!(matches!(
        GetSchedulerConfig("balance-region"),
        Err(Error::PdHttpClientMissing)
    ));
    assert!(matches!(
        CreateSchedulerConfigWithInput("balance-region", &HashMap::new()),
        Err(Error::PdHttpClientMissing)
    ));
    assert!(matches!(
        CancelSchedulerJob("balance-region", 1),
        Err(Error::PdHttpClientMissing)
    ));
}
