// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/pkg/importer/opts` public contracts vs Go.
//!
//! 这组测试把 opts 包的公开契约分成四类：
//! 默认值与正常组合、边界拷贝语义、无错误路径下的覆盖，
//! 以及 option 闭包可重复应用时的资源/克隆行为。
//! 因为 opts 包本质上是“配置闭包集合”，
//! 所以这里最重要的不是复杂算法，
//! 而是应用顺序、拷贝时机和默认值是否稳定。

use crate::mydump::{
    MDLoaderSetupConfig, ReturnPartialResultOnError, WithMaxScanFiles, WithScanFileConcurrency,
    WithSkipRealSizeEstimation,
};
use crate::*;

#[test]
fn go_rust_public_contract_matches() {
    // 顶层测试只负责串联四类契约，方便失败时按分组定位。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

fn contract_normal() {
    // Default config matches Go NewDefaultGetPreInfoConfig.
    // 默认配置必须保持“全关闭”，否则调用方的基线行为会悄悄变化。
    let def = NewDefaultGetPreInfoConfig();
    assert!(!def.IgnoreDBNotExist);
    assert!(!def.ForceReloadCache);

    // Option setters mutate config like Go closures.
    // 这里验证闭包能直接覆盖结构体字段，而不需要额外 builder。
    let mut cfg = *NewDefaultGetPreInfoConfig();
    WithIgnoreDBNotExist(true)(&mut cfg);
    ForceReloadCache(true)(&mut cfg);
    assert!(cfg.IgnoreDBNotExist);
    assert!(cfg.ForceReloadCache);

    // ApplyGetPreInfoOptions composes base + opts (PreInfoGetter call-site pattern).
    // 组合后的结果是上层最常见的使用方式。
    let applied =
        ApplyGetPreInfoOptions(None, &[WithIgnoreDBNotExist(true), ForceReloadCache(false)]);
    assert!(applied.IgnoreDBNotExist);
    assert!(!applied.ForceReloadCache);

    // Precheck builder options clone into config (Go slices.Clone).
    // 这一步把 getter 选项与 mydump 选项一起装进 builder 配置。
    let pre_opts = vec![WithIgnoreDBNotExist(true)];
    let md_opts = vec![WithScanFileConcurrency(4), WithMaxScanFiles(10)];
    let mut builder = PrecheckItemBuilderConfig::default();
    WithPreInfoGetterOptions(&pre_opts)(&mut builder);
    WithMDLoaderSetupOptions(&md_opts)(&mut builder);
    assert_eq!(builder.PreInfoGetterOptions.len(), 1);
    assert_eq!(builder.MDLoaderSetupOptions.len(), 2);

    let mut md_cfg = MDLoaderSetupConfig::default();
    // 再把 builder 内的 mydump option 真正执行到配置对象上。
    for opt in &builder.MDLoaderSetupOptions {
        opt(&mut md_cfg);
    }
    assert_eq!(md_cfg.scan_file_concurrency, 4);
    assert_eq!(md_cfg.max_scan_files, 10);
    assert!(md_cfg.support_partial_result);
}

fn contract_boundary() {
    // Nil / None Clone → default (Go nil receiver).
    // `Clone(None)` 是 Go nil receiver 迁移后最容易被忽视的兼容点。
    let from_nil = GetPreInfoConfig::Clone(None);
    assert_eq!(*from_nil, *NewDefaultGetPreInfoConfig());

    // Non-nil Clone deep-copies independent values.
    // 修改副本后原对象不能被联动修改，说明这里是深拷贝语义。
    let src = GetPreInfoConfig {
        IgnoreDBNotExist: true,
        ForceReloadCache: true,
    };
    let mut cloned = GetPreInfoConfig::Clone(Some(&src));
    cloned.IgnoreDBNotExist = false;
    assert!(src.IgnoreDBNotExist);
    assert!(!cloned.IgnoreDBNotExist);

    // Empty option slices → empty vectors after builder option apply.
    // 空切片不能产生幽灵 option。
    let mut builder = PrecheckItemBuilderConfig::default();
    WithPreInfoGetterOptions(&[])(&mut builder);
    WithMDLoaderSetupOptions(&[])(&mut builder);
    assert!(builder.PreInfoGetterOptions.is_empty());
    assert!(builder.MDLoaderSetupOptions.is_empty());

    // slices.Clone independence: mutating caller slice after option construction
    // must not change already-captured builder config.
    // 这锁住“捕获时克隆，而不是延迟借用”的关键契约。
    let mut caller_pre = vec![WithIgnoreDBNotExist(true)];
    let opt = WithPreInfoGetterOptions(&caller_pre);
    caller_pre.clear();
    let mut builder2 = PrecheckItemBuilderConfig::default();
    opt(&mut builder2);
    assert_eq!(builder2.PreInfoGetterOptions.len(), 1);

    let mut caller_md = vec![WithSkipRealSizeEstimation(true)];
    let md_opt = WithMDLoaderSetupOptions(&caller_md);
    caller_md.push(WithMaxScanFiles(1));
    let mut builder3 = PrecheckItemBuilderConfig::default();
    md_opt(&mut builder3);
    assert_eq!(builder3.MDLoaderSetupOptions.len(), 1);
    let mut md_cfg = MDLoaderSetupConfig::default();
    builder3.MDLoaderSetupOptions[0](&mut md_cfg);
    assert!(md_cfg.skip_real_size_estimation);
    assert_eq!(md_cfg.max_scan_files, 0);

    // Go only overwrites ScanFileConcurrency for a strictly positive input.
    // Zero must preserve the existing value instead of coercing it to one.
    let mut md_cfg = MDLoaderSetupConfig {
        scan_file_concurrency: 7,
        ..Default::default()
    };
    WithScanFileConcurrency(0)(&mut md_cfg);
    assert_eq!(md_cfg.scan_file_concurrency, 7);

    // Go accepts signed ints and treats every non-positive value as a no-op.
    WithScanFileConcurrency(-1)(&mut md_cfg);
    assert_eq!(md_cfg.scan_file_concurrency, 7);

    let mut max_cfg = MDLoaderSetupConfig {
        max_scan_files: 9,
        support_partial_result: false,
        ..Default::default()
    };
    WithMaxScanFiles(-1)(&mut max_cfg);
    assert_eq!(max_cfg.max_scan_files, 9);
    assert!(!max_cfg.support_partial_result);

    ReturnPartialResultOnError(true)(&mut max_cfg);
    assert!(max_cfg.support_partial_result);
    ReturnPartialResultOnError(false)(&mut max_cfg);
    assert!(!max_cfg.support_partial_result);
}

fn contract_error() {
    // Zero-value / default builder config is usable (Go zero struct).
    // 虽然这里没有真实报错路径，但要确认零值对象本身就是可用状态。
    let builder = PrecheckItemBuilderConfig::default();
    assert!(builder.PreInfoGetterOptions.is_empty());
    assert!(builder.MDLoaderSetupOptions.is_empty());

    // Applying no options leaves defaults (no panic / no error path in Go).
    // 空 option 列表应是纯 no-op。
    let cfg = ApplyGetPreInfoOptions(Some(&GetPreInfoConfig::default()), &[]);
    assert_eq!(cfg, GetPreInfoConfig::default());

    // Later option overrides earlier one (last-write-wins, same as Go apply loop).
    // 这体现了按顺序应用闭包的最后写入生效规则。
    let cfg = ApplyGetPreInfoOptions(
        None,
        &[WithIgnoreDBNotExist(true), WithIgnoreDBNotExist(false)],
    );
    assert!(!cfg.IgnoreDBNotExist);
}

fn contract_resource_cleanup() {
    // Builder option can be applied multiple times; each apply refreshes from
    // the captured clone (Go option func re-assigns slices.Clone result).
    // 因而第一次应用后对 builder 的修改，不应污染第二次应用。
    let opts = vec![ForceReloadCache(true)];
    let builder_opt = WithPreInfoGetterOptions(&opts);
    let mut a = PrecheckItemBuilderConfig::default();
    let mut b = PrecheckItemBuilderConfig::default();
    builder_opt(&mut a);
    a.PreInfoGetterOptions.clear();
    builder_opt(&mut b);
    assert!(a.PreInfoGetterOptions.is_empty());
    assert_eq!(b.PreInfoGetterOptions.len(), 1);

    let mut cfg = *NewDefaultGetPreInfoConfig();
    // 末尾再确认闭包本身仍能正确驱动配置变更。
    b.PreInfoGetterOptions[0](&mut cfg);
    assert!(cfg.ForceReloadCache);

    // Dropping configs/options is side-effect free (no external resources).
    drop(a);
    drop(b);
    drop(builder_opt);
    drop(opts);
}
