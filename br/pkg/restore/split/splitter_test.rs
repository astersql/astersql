// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use astersql_errors::New;

use crate::client::SplitClient;
use crate::region::RegionInfo;
use crate::splitter::NewPipelineRegionsSplitter;
use crate::stubs::{
    CodecPDClient, Context, GetRegionOption, GetStoreOption, Result, metapb, pdhttp, pdpb,
};
use crate::sum_sorted::{Span, Value, Valued};

#[derive(Clone, Default)]
struct RecordingSplitClient {
    split_keys: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl SplitClient for RecordingSplitClient {
    fn GetStore(&self, _: &Context, _: u64, _: &[GetStoreOption]) -> Result<metapb::Store> {
        Err(New("not used"))
    }

    fn GetRegion(&self, _: &Context, _: &[u8]) -> Result<RegionInfo> {
        Err(New("not used"))
    }

    fn GetRegionByID(&self, _: &Context, _: u64) -> Result<RegionInfo> {
        Err(New("not used"))
    }

    fn SplitKeysAndScatter(&self, _: &Context, _: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        Err(New("not used"))
    }

    fn SplitKeys(&self, _: &Context, _: &[Vec<u8>]) -> Result<Vec<RegionInfo>> {
        Err(New("not used"))
    }

    fn SplitWaitAndScatter(
        &self,
        _: &Context,
        _: &RegionInfo,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>> {
        *self.split_keys.lock().unwrap() = keys.to_vec();
        Ok(Vec::new())
    }

    fn GetOperator(&self, _: &Context, _: u64) -> Result<pdpb::GetOperatorResponse> {
        Err(New("not used"))
    }

    fn ScanRegions(
        &self,
        _: &Context,
        _: &[u8],
        _: &[u8],
        _: i32,
        _: &[GetRegionOption],
    ) -> Result<Vec<RegionInfo>> {
        Err(New("not used"))
    }

    fn GetPlacementRule(&self, _: &Context, _: &str, _: &str) -> Result<pdhttp::Rule> {
        Err(New("not used"))
    }

    fn SetPlacementRule(&self, _: &Context, _: &pdhttp::Rule) -> Result<()> {
        Err(New("not used"))
    }

    fn DeletePlacementRule(&self, _: &Context, _: &str, _: &str) -> Result<()> {
        Err(New("not used"))
    }

    fn SetStoresLabel(&self, _: &Context, _: &[u64], _: &str, _: &str) -> Result<()> {
        Err(New("not used"))
    }

    fn GetCodecPDClient(&self) -> Option<CodecPDClient> {
        None
    }
}

#[test]
fn malformed_encoded_split_key_matches_go_ignored_decode_error() {
    let client = RecordingSplitClient::default();
    let recorded = client.split_keys.clone();
    let splitter = NewPipelineRegionsSplitter(Box::new(client), 0, i64::MAX);
    let region = RegionInfo {
        Region: Some(metapb::Region {
            StartKey: b"region-start".to_vec(),
            EndKey: b"region-end".to_vec(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let valued = Valued {
        Key: Span {
            StartKey: b"bad".to_vec(),
            EndKey: b"worse".to_vec(),
        },
        Value: Value { Size: 1, Number: 1 },
    };

    splitter
        .splitRegionByPoints(&Context::Background(), 1, 0, &region, &[valued])
        .unwrap();

    // Go ignores DecodeBytes' error and forwards its nil rawKey.
    assert_eq!(*recorded.lock().unwrap(), vec![Vec::<u8>::new()]);
}
