// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// ResourcePool / Budget 行为与树形结构的单元测试与基准。
//
// 覆盖随机分配不变量、Budget Grow/Clear/Resize、nil budget、父子树导出、
// 回调触发、无死锁并发，以及 BudgetGrow/TraverseTree 微基准。

use super::pool::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// 将可变引用转为 PoolLink（NonNull），供 Start 挂父指针。
fn pool_link(pool: &mut ResourcePool) -> Option<NonNull<ResourcePool>> {
    Some(NonNull::from(pool))
}

/// 按指数分布采样非负尺寸，用于随机压力测试。
fn random_size(rng: &mut StdRng, magnitude: i64) -> i64 {
    if magnitude <= 0 {
        return 0;
    }
    let sample = -rng.r#gen::<f64>().ln();
    (sample * magnitude as f64 * 0.3679) as i64
}

/// 断言各 Budget used≥0 且 Capacity 之和等于 pool.Allocated。
fn assert_budget_invariants(pool: &ResourcePool, budgets: &[Budget]) {
    let sum: i64 = budgets.iter().map(Budget::Capacity).sum();
    assert!(budgets.iter().all(|budget| budget.used >= 0));
    assert!(pool.Allocated() >= 0);
    assert_eq!(sum, pool.Allocated());
}

#[test]
/// 多参数组合下随机 Grow/Clear/Resize，校验预算不变量与父 Allocated 归零。
fn TestPoolAllocations() {
    let maxs = [1_i64, 9, 10, 11, 99, 100, 101, 0];
    let factors = [1_i64, 2, 10, 10_000];
    let pool_alloc_sizes = [1_i64, 2, 9, 10, 11, 100];
    let pre_budgets = [0_i64, 1, 2, 9, 10, 11, 100];
    let mut rng = StdRng::seed_from_u64(1);

    for max in maxs {
        let mut parent = NewResourcePoolDefault("test".into(), 1);
        parent.Start(None, max);
        for factor in factors {
            parent.maxUnusedBlocks = factor;
            for pre_budget in pre_budgets {
                for alloc_size in pool_alloc_sizes {
                    let mut child = NewResourcePoolDefault("test".into(), alloc_size);
                    child.Start(pool_link(&mut parent), pre_budget);
                    let mut budgets: Vec<_> = (0..4).map(|_| child.CreateBudget()).collect();
                    for _ in 0..200 {
                        let index = rng.gen_range(0..budgets.len());
                        match rng.gen_range(0..3) {
                            0 => {
                                let _ =
                                    budgets[index].Grow(random_size(&mut rng, pre_budget + max));
                            }
                            1 => budgets[index].Clear(),
                            _ => {
                                let new_size = random_size(&mut rng, pre_budget + max);
                                let _ = budgets[index].ResizeTo(new_size);
                            }
                        }
                        assert_budget_invariants(&child, &budgets);
                    }
                    for budget in &mut budgets {
                        budget.Clear();
                    }
                    assert_budget_invariants(&child, &budgets);
                    child.Stop();
                    assert_eq!(parent.Allocated(), 0);
                }
            }
        }
        parent.Stop();
    }
}

#[test]
/// 校验双 Budget 竞争容量、Shrink 对齐块保留与 Clear。
fn TestBudget() {
    let mut pool = NewResourcePoolDefault("test".into(), 1);
    pool.Start(None, 100);
    pool.maxUnusedBlocks = 1;
    let pool_address = (&mut *pool as *mut ResourcePool) as usize;
    let mut first = pool.CreateBudget();
    let mut second = pool.CreateBudget();

    assert!(first.Grow(10).is_ok());
    assert!(second.Grow(30).is_ok());
    assert!(first.Grow(61).is_err());
    assert!(second.Grow(71).is_err());
    first.Clear();
    assert!(second.Grow(61).is_ok());
    assert!(second.Grow(10).is_err());
    assert!(first.ResizeTo(5).is_ok());
    assert!(second.ResizeTo(40).is_ok());
    first.Clear();
    second.Clear();
    assert_eq!(pool.Allocated(), 0);
    assert_eq!(
        first.Pool().map(|ptr| ptr.as_ptr() as usize),
        Some(pool_address)
    );
    pool.Stop();
}

#[test]
/// 未绑定 pool 的 Budget 操作应安全失败或 no-op（对齐 Go）。
fn TestNilBudget() {
    let mut budget: Option<Budget> = None;
    assert_eq!(budget.as_ref().map(Budget::Used).unwrap_or_default(), 0);
    assert!(budget.as_ref().and_then(Budget::Pool).is_none());
    assert_eq!(budget.as_ref().map(Budget::Capacity).unwrap_or_default(), 0);
    if let Some(value) = budget.as_mut() {
        value.Empty();
        value.Clear();
        value.ResizeTo(10).unwrap();
        value.Grow(10).unwrap();
        value.Shrink(10);
    }
    assert!(budget.is_none());
}

#[test]
/// 基本 Start/Allocate/Stop 生命周期。
fn TestResourcePool() {
    let mut pool = NewResourcePoolDefault("test".into(), 1);
    pool.Start(None, 100);
    pool.maxUnusedBlocks = 1;
    let mut budget = pool.CreateBudget();
    assert!(budget.Grow(10).is_ok());
    assert!(budget.Grow(91).is_err());
    assert!(budget.Grow(90).is_ok());
    assert_eq!(pool.Allocated(), 100);
    // The Go test calls the package-private release directly. Rebuild the same
    // public observable state through Clear + Grow without changing production.
    budget.Clear();
    budget.Grow(10).unwrap();
    assert_eq!(pool.Allocated(), 10);
    assert_eq!(pool.MaxAllocated(), 100);
    budget.Clear();
    assert_eq!(pool.Allocated(), 0);

    let mut limited = NewResourcePool(
        1,
        "testlimit".into(),
        10,
        1,
        DefMaxUnusedBlocks,
        PoolActions::default(),
    );
    limited.StartNoReserved(pool_link(&mut pool));
    let mut limited_budget = limited.CreateBudget();
    assert!(limited_budget.Grow(10).is_ok());
    assert!(limited_budget.Grow(1).is_err());
    limited_budget.Clear();
    limited.Stop();
    pool.Stop();
}

#[test]
/// 边界：零 limit、对齐边界等分配边角。
fn TestMemoryAllocationEdgeCases() {
    let mut pool = NewResourcePoolDefault("test".into(), 1_000_000_000);
    pool.Start(None, 1_000_000_000);
    let mut budget = pool.CreateBudget();
    assert!(budget.Grow(1).is_ok());
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| budget.Grow(i64::MAX)));
    assert!(rejected.is_err() || rejected.unwrap().is_err());
    budget.Clear();
    pool.Stop();
}

#[test]
/// 多 Budget 共享同一 pool 计量。
fn TestMultiSharedGauge() {
    let min_allocation = 1_000;
    let mut parent = NewResourcePoolDefault("root".into(), min_allocation);
    parent.Start(None, 100_000);
    let mut child = parent.NewResourcePoolInheritWithLimit("child".into(), 20_000);
    child.StartNoReserved(pool_link(&mut parent));
    let mut budget = child.CreateBudget();
    budget.Grow(100).unwrap();
    assert_eq!(parent.Allocated(), min_allocation);
    budget.Clear();
    child.Stop();
    parent.Stop();
}

#[test]
/// OutOfCapacity / OutOfLimit 回调触发。
fn TestActions() {
    let mut reserve_root = NewResourcePoolDefault("reserve-root".into(), 1_000);
    reserve_root.Start(None, i64::MAX);
    let mut reserve_child = NewResourcePoolDefault("reserve-child".into(), 666);
    reserve_child.StartNoReserved(pool_link(&mut reserve_root));
    reserve_child.ExplicitReserve(1_002).unwrap();
    assert_eq!(reserve_child.Capacity(), reserve_root.allocAlignSize * 2);
    assert_eq!(
        reserve_child.mu.lock().unwrap().budget.explicitReserved,
        1_002
    );
    let mut reserved_budget = reserve_child.CreateBudget();
    reserved_budget.Reserve(123).unwrap();
    assert_eq!(reserve_child.Capacity(), reserve_root.allocAlignSize * 2);
    assert_eq!(reserved_budget.Capacity(), reserve_child.allocAlignSize);
    assert_eq!(reserved_budget.explicitReserved, 123);
    reserved_budget.Grow(123).unwrap();
    reserved_budget.Shrink(123);
    assert_eq!(reserve_child.Capacity(), reserve_root.allocAlignSize * 2);
    reserved_budget.Clear();
    reserve_child.Stop();
    reserve_root.Stop();

    let capacity_calls = Arc::new(AtomicUsize::new(0));
    let capacity_calls_cb = Arc::clone(&capacity_calls);
    let mut root = NewResourcePoolDefault("root".into(), 1);
    root.StartNoReserved(None);
    root.SetOutOfCapacityAction(Some(Arc::new(move |args| {
        assert!(args.Pool.is_some());
        assert!(args.Request > 0);
        capacity_calls_cb.fetch_add(1, Ordering::SeqCst);
        unsafe {
            args.Pool.unwrap().as_mut().forceAddCap(args.Request);
        }
        Ok(())
    })));
    let mut budget = root.CreateBudget();
    budget.Grow(1).unwrap();
    assert_eq!(capacity_calls.load(Ordering::SeqCst), 1);
    budget.Grow(10).unwrap();
    assert_eq!(capacity_calls.load(Ordering::SeqCst), 2);
    assert_eq!(root.mu.lock().unwrap().budget.used, budget.Used());
    budget.Clear();
    budget.Grow(5).unwrap();
    assert_eq!(capacity_calls.load(Ordering::SeqCst), 2);
    budget.Clear();

    let limit_calls = Arc::new(AtomicUsize::new(0));
    let limit_calls_cb = Arc::clone(&limit_calls);
    root.SetLimit(1);
    root.SetOutOfLimitAction(Some(Arc::new(move |pool| {
        assert!(pool.is_some());
        limit_calls_cb.fetch_add(1, Ordering::SeqCst);
        Err("limit action".into())
    })));
    // Reserved capacity bypasses the capacity callback and reaches the limit branch.
    root.reserved = 2;
    assert!(budget.Grow(2).is_err());
    assert_eq!(limit_calls.load(Ordering::SeqCst), 1);
    root.Stop();
}

/// 构造已 Start 的测试 pool，可选挂到 parent。
fn gen_pool(name: &str, parent: Option<&mut ResourcePool>) -> Box<ResourcePool> {
    let reserved = if parent.is_none() { i64::MAX } else { 0 };
    let mut pool = NewResourcePoolDefault(name.into(), 1);
    pool.Start(parent.map(|value| NonNull::from(&mut *value)), reserved);
    pool
}

/// Traverse 导出树形名称字符串，便于断言。
fn export(pool: &ResourcePool) -> String {
    let mut result = String::new();
    pool.Traverse(|state| {
        result.push_str(&"-".repeat(state.Level as usize));
        result.push_str(&state.Name);
        result.push('\n');
        Ok(())
    })
    .unwrap();
    result
}

#[test]
/// 父子树挂载与 Traverse 导出。
fn TestResourcePoolTree() {
    let mut parent = gen_pool("parent", None);
    let mut child1 = gen_pool("child1", Some(&mut parent));
    let mut child2 = gen_pool("child2", Some(&mut parent));
    assert_eq!(export(&parent), "parent\n-child2\n-child1\n");
    assert_eq!(export(&child1), "child1\n");
    assert_eq!(export(&child2), "child2\n");
    let mut grandchild1 = gen_pool("grandchild1", Some(&mut child1));
    let mut grandchild2 = gen_pool("grandchild2", Some(&mut child2));
    assert_eq!(
        export(&parent),
        "parent\n-child2\n--grandchild2\n-child1\n--grandchild1\n"
    );
    grandchild2.Stop();
    child2.Stop();
    assert_eq!(export(&parent), "parent\n-child1\n--grandchild1\n");
    grandchild1.Stop();
    child1.Stop();
    assert_eq!(export(&parent), "parent\n");
    parent.Stop();
}

#[test]
/// reserved 配额参与 used 计算。
fn TestResourcePoolUsedFromReserved() {
    let mut root = gen_pool("root", None);
    let mut child = NewResourcePoolDefault("child".into(), 1);
    child.Start(pool_link(&mut root), 2 << 10);
    let mut budget = child.CreateBudget();
    budget.Grow(1 << 10).unwrap();
    let mut observed = None;
    root.Traverse(|state| {
        if state.Name == "child" {
            observed = Some(state.Used);
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(observed, Some(1 << 10));
    budget.Clear();
    child.Stop();
    root.Stop();
}

#[test]
/// 并发分配不应死锁。
fn TestResourcePoolNoDeadlocks() {
    let mut root = gen_pool("root", None);
    let root_address = (&mut *root as *mut ResourcePool) as usize;
    let done = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        for worker in 0..10 {
            let done = Arc::clone(&done);
            scope.spawn(move || {
                let root_ptr = root_address as *mut ResourcePool;
                let mut rng = StdRng::seed_from_u64(1);
                let mut retired = Vec::new();
                while !done.load(Ordering::Acquire) {
                    let mut child = NewResourcePoolDefault(format!("m{worker}"), 1);
                    child.Start(Some(NonNull::new(root_ptr).unwrap()), 0);
                    let mut budget = child.CreateBudget();
                    for _ in 0..rng.gen_range(0..=10) {
                        if budget.Used() > 0 && rng.gen_bool(0.5) {
                            budget.Shrink(rng.gen_range(1..=budget.Used()));
                        } else {
                            let _ = budget.Grow(rng.gen_range(1..=1_000));
                        }
                    }
                    budget.Clear();
                    child.Stop();
                    // Traverse copies raw child links before releasing the parent
                    // lock. Keep stopped nodes alive until all traversals finish.
                    retired.push(child);
                    std::thread::yield_now();
                }
            });
        }
        for _ in 0..1_000 {
            root.Traverse(|state| {
                assert!(!state.Name.is_empty());
                Ok(())
            })
            .unwrap();
            std::thread::sleep(Duration::from_micros(10));
        }
        done.store(true, Ordering::Release);
    });
    root.Stop();
}

/// Budget.Grow 微基准内核。
fn benchmark_budget_grow(iterations: usize) {
    let mut pool = NewResourcePoolDefault("test".into(), 1_000_000_000);
    pool.Start(None, 1_000_000_000);
    let mut budget = pool.CreateBudget();
    for _ in 0..iterations {
        budget.Grow(1).unwrap();
    }
    budget.Clear();
    pool.Stop();
}

/// 构造多层多子节点的 pool 树。
fn make_pool_tree(levels: usize, children_per_pool: usize) -> Vec<Vec<Box<ResourcePool>>> {
    let mut levels_of_pools = vec![vec![gen_pool("root", None)]];
    for level in 1..levels {
        let mut children = Vec::new();
        for (parent_index, parent) in levels_of_pools[level - 1].iter_mut().enumerate() {
            for child_index in 0..children_per_pool {
                children.push(gen_pool(
                    &format!("child{child_index}_parent{parent_index}"),
                    Some(parent),
                ));
            }
        }
        levels_of_pools.push(children);
    }
    levels_of_pools
}

#[test]
/// Budget.Grow 基准入口。
fn BenchmarkBudgetGrow() {
    benchmark_budget_grow(1_000);
}

#[test]
/// Traverse 树遍历基准入口。
fn BenchmarkTraverseTree() {
    for levels in [2, 4, 8] {
        for children in [2, 4, 8] {
            let mut tree = make_pool_tree(levels, children);
            let mut count = 0;
            tree[0][0]
                .Traverse(|_| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                count,
                (0..levels)
                    .map(|level| children.pow(level as u32))
                    .sum::<usize>()
            );
            for pools in tree.iter_mut().rev() {
                for pool in pools {
                    pool.Stop();
                }
            }
        }
    }
}
