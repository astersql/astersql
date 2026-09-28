// Copyright 2026 AsterSQL.

use std::collections::HashMap;

use crate::recover::{RecoverRegion, SortRecoverRegions};

/// Go indexes `peers[0]`: a region entry without any peer violates the recovery
/// metadata invariant and must not be silently omitted from the recovery plan.
#[test]
#[should_panic]
fn sort_recover_regions_rejects_region_without_peers() {
    let mut regions = HashMap::<u64, Vec<RecoverRegion>>::from([(42, Vec::new())]);

    let _ = SortRecoverRegions(&mut regions);
}
