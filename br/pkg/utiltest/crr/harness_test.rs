// Copyright 2026 AsterSQL.

use crate::{
    AddRoundRobinRegions, BuildRegionLayout, Context, NewLocalTestHarnessWithTestContext,
    NewTestContextWithSeed, Storage,
};

#[test]
fn local_harnesses_with_the_same_seed_use_independent_temp_dirs() {
    let ctx = Context::background();
    let tc = NewTestContextWithSeed(7);
    let layout = BuildRegionLayout(vec![AddRoundRobinRegions(1, vec![1])]).unwrap();

    let first = NewLocalTestHarnessWithTestContext(&ctx, &tc, layout.clone()).unwrap();
    let second = NewLocalTestHarnessWithTestContext(&ctx, &tc, layout).unwrap();
    assert_ne!(
        first.Downstream.URI(),
        second.Downstream.URI(),
        "each Go TempDir call creates an independent harness root"
    );
    first.Close();

    second
        .Upstream
        .WriteFile(&ctx, "still-live", b"payload")
        .expect("closing one harness must not remove another harness's storage");
}
