// Copyright 2024 PingCAP, Inc.
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

// 泛型集合：基于 `Key` 字符串键的增删查、并/交/差与组合枚举。
//
// 对应 Go `pkg/util/set` 中 `Set[T Key]`。元素经 `Key()` 转为稳定字符串后存入
// map；`ToList`/`String` 按 key 排序以保证输出稳定。`CombSet` 用回溯枚举固定大小组合。

use std::collections::HashMap;

// Key is the interface for the key of a set item.
// Key 对应 Go 的 `type Key interface { Key() string }`。
// Go 通过该方法把泛型元素转换成稳定字符串 key；Rust 保留同名方法方便调用点对照。
/// 集合元素的稳定字符串键；同 key 在 Add 时后写覆盖先写。
pub trait Key {
    /// 返回用作 map 键的字符串。
    fn Key(&self) -> String;
}

// Set is the interface for a set.
// Set 对应 Go 的泛型接口 `Set[T Key]`，描述集合增删查、列表化、克隆和字符串化能力。
// Go 的变参和返回 interface 在这里分别用切片与 trait object 表达。
/// 泛型集合接口：增删查、有序列表、克隆与 `{k1, k2}` 形式字符串化。
pub trait Set<T: Key + Clone> {
    /// 批量添加；同 key 新值覆盖旧值。
    fn Add(&mut self, items: &[T]);
    /// 按 `item.Key()` 判断是否已是成员。
    fn Contains(&self, item: &T) -> bool;
    /// 按 key 删除；对未初始化内部 map 为无操作。
    fn Remove(&mut self, item: &T);
    /// 返回按 Key 字典序排序的成员列表。
    fn ToList(&self) -> Vec<T>;
    /// 当前成员数；未初始化时为 0。
    fn Size(&self) -> usize;
    /// 深拷贝为新的 trait object 集合。
    fn Clone(&self) -> Box<dyn Set<T>>;
    /// 形如 `{k1, k2, ...}` 的稳定字符串表示。
    fn String(&self) -> String;
}

// setImpl 对应 Go 的 `type setImpl[T Key] struct { s map[string]T }`。
// Go map 可以是 nil，Add 首次写入时才 make；Option<HashMap<...>> 保留这个延迟初始化语义。
/// 默认实现：`Option<HashMap>` 模拟 Go 的懒 make。
struct setImpl<T: Key + Clone> {
    s: Option<HashMap<String, T>>,
}

// NewSet creates a new set.
// NewSet 对应 Go 构造函数 `NewSet[T Key]() Set[T]`。
// Go 返回的是接口值，底层为 `new(setImpl[T])`；Rust 用 Box<dyn Set<T>> 表达动态分发。
/// 创建空集合（内部 map 尚未分配）。
pub fn NewSet<T: Key + Clone + 'static>() -> Box<dyn Set<T>> {
    Box::new(setImpl::<T> { s: None })
}

impl<T: Key + Clone + 'static> Set<T> for setImpl<T> {
    // Add 对应 Go 方法 `func (s *setImpl[T]) Add(items ...T)`。
    // Go 变参在函数体里表现为切片；这里用 `&[T]` 保留“一次添加多个元素”的语义。
    fn Add(&mut self, items: &[T]) {
        // Go 在 nil map 上写入前必须 make；Option::get_or_insert_with 对应这段懒初始化分支。
        let map = self.s.get_or_insert_with(HashMap::new);
        for item in items {
            // Go 使用 `item.Key()` 作为 map key，同 key 的新值会覆盖旧值；HashMap::insert 保留该行为。
            map.insert(item.Key(), item.clone());
        }
    }

    // Contains 对应 Go 方法 `func (s *setImpl[T]) Contains(item T) bool`。
    // 原实现先检查底层 map 是否为 nil，nil 时直接返回 false。
    fn Contains(&self, item: &T) -> bool {
        let Some(map) = &self.s else {
            // 对应 Go 的 `if s.s == nil { return false }`。
            return false;
        };
        map.contains_key(&item.Key())
    }

    // ToList 对应 Go 方法 `func (s *setImpl[T]) ToList() []T`。
    // Rust 引用不能是 nil；未初始化的内部 map 对应一个空 Vec。
    fn ToList(&self) -> Vec<T> {
        let Some(map) = &self.s else {
            // Go 的 nil map 不产生元素；Rust 返回空 Vec。
            return Vec::new();
        };
        let mut list: Vec<T> = map.values().cloned().collect();
        // to make the result stable
        // 保留 Go 的 sort.Slice 稳定输出意图：按元素 Key 的字典序排序，避免 map 迭代顺序影响结果。
        list.sort_by(|left, right| left.Key().cmp(&right.Key()));
        list
    }

    // Remove 对应 Go 方法 `func (s *setImpl[T]) Remove(item T)`。
    // Go 的 delete 对 nil map 是安全无操作；Rust 先判断 Option 是否已经初始化。
    fn Remove(&mut self, item: &T) {
        if let Some(map) = &mut self.s {
            map.remove(&item.Key());
        }
    }

    // Size 对应 Go 方法 `func (s *setImpl[T]) Size() int`。
    // Rust 引用不能是 nil；未初始化的内部 map 返回 0。
    fn Size(&self) -> usize {
        self.s.as_ref().map_or(0, HashMap::len)
    }

    // Clone 对应 Go 方法 `func (s *setImpl[T]) Clone() Set[T]`。
    // Go 通过 ToList 后 Add 到新集合，既复制成员又保持接口返回形状；这里保留同样的两步控制流。
    fn Clone(&self) -> Box<dyn Set<T>> {
        let mut clone = NewSet::<T>();
        let list = self.ToList();
        clone.Add(&list);
        clone
    }

    // String 对应 Go 方法 `func (s *setImpl[T]) String() string`。
    // 原实现收集所有 Key 后排序，再用 fmt.Sprintf("{%v}", strings.Join(...)) 输出。
    fn String(&self) -> String {
        let mut items: Vec<String> = match &self.s {
            Some(map) => map.values().map(|item| item.Key()).collect(),
            None => Vec::new(),
        };
        // sort.Strings(items) 让字符串化结果不受 Go map 随机迭代顺序影响。
        items.sort();
        format!("{{{}}}", items.join(", "))
    }
}

// ListToSet converts a list to a set.
// ListToSet 对应 Go 函数 `ListToSet[T Key](items ...T) Set[T]`。
// Go 变参迁移为切片；函数按原顺序创建空集合并逐个 Add。
/// 将切片转为集合；对应 Go 变参 `ListToSet`。
pub fn ListToSet<T: Key + Clone + 'static>(items: &[T]) -> Box<dyn Set<T>> {
    let mut s = NewSet::<T>();
    for item in items {
        // 原 Go 每次传单个 item 给 Add；Rust 用单元素切片表示这次变参调用。
        s.Add(&[item.clone()]);
    }
    s
}

// UnionSet returns the union set of the given sets.
// UnionSet 对应 Go 函数 `UnionSet[T Key](ss ...Set[T]) Set[T]`，返回多个集合的并集。
// Go 的接口切片迁移为 trait object 引用切片。
/// 多个集合的并集；空输入返回新空集合，单输入返回克隆。
pub fn UnionSet<T: Key + Clone + 'static>(ss: &[&dyn Set<T>]) -> Box<dyn Set<T>> {
    if ss.is_empty() {
        // 对应 Go 的 `len(ss) == 0` 分支：没有输入集合时返回一个新空集合。
        return NewSet::<T>();
    }
    if ss.len() == 1 {
        // 单集合并集等于自身副本；调用 Clone 保留 Go 的“返回克隆而非原集合”的语义。
        return ss[0].Clone();
    }
    let mut s = NewSet::<T>();
    for set in ss {
        // Go 展开 `set.ToList()...` 后批量 Add；这里先取 Vec，再以切片传入。
        let list = set.ToList();
        s.Add(&list);
    }
    s
}

// AndSet returns the intersection set of the given sets.
// AndSet 对应 Go 函数 `AndSet[T Key](ss ...Set[T]) Set[T]`，返回多个集合的交集。
/// 多个集合的交集；元素须出现在全部输入中。
pub fn AndSet<T: Key + Clone + 'static>(ss: &[&dyn Set<T>]) -> Box<dyn Set<T>> {
    if ss.is_empty() {
        // 没有输入集合时，Go 代码返回 NewSet，而不是 nil。
        return NewSet::<T>();
    }
    if ss.len() == 1 {
        // 单集合交集也返回克隆，避免调用者修改结果时影响原集合。
        return ss[0].Clone();
    }
    let mut s = NewSet::<T>();
    for item in ss[0].ToList() {
        let mut contained = true;
        for set in &ss[1..] {
            if !set.Contains(&item) {
                // 保留 Go 的早停逻辑：只要某个集合不包含该元素，就无需继续检查后续集合。
                contained = false;
                break;
            }
        }
        if contained {
            // 只有元素出现在所有输入集合中时，才加入交集结果。
            s.Add(&[item]);
        }
    }
    s
}

// DiffSet returns a set of items that are in s1 but not in s2.
// DiffSet({1, 2, 3, 4}, {2, 3}) = {1, 4}
// DiffSet 对应 Go 函数 `DiffSet[T Key](s1, s2 Set[T]) Set[T]`，返回 s1 相对 s2 的差集。
/// 差集：属于 `s1` 但不属于 `s2` 的元素。
pub fn DiffSet<T: Key + Clone + 'static>(s1: &dyn Set<T>, s2: &dyn Set<T>) -> Box<dyn Set<T>> {
    let mut s = NewSet::<T>();
    for item in s1.ToList() {
        if !s2.Contains(&item) {
            // Go 中 `s.Add(item)` 只在 s2 不含该 key 时执行；这里保持相同过滤条件。
            s.Add(&[item]);
        }
    }
    s
}

// CombSet returns all combinations of `numberOfItems` items in the given set.
// For example ({a, b, c}, 2) returns {ab, ac, bc}.
// CombSet 对应 Go 函数 `CombSet[T Key](s Set[T], numberOfItems int) []Set[T]`。
// 它先把集合转成稳定顺序列表，再用递归回溯枚举固定大小的所有组合。
/// 枚举集合中恰好 `numberOfItems` 个元素的所有组合（顺序稳定）。
pub fn CombSet<T: Key + Clone + 'static>(
    s: &dyn Set<T>,
    numberOfItems: isize,
) -> Vec<Box<dyn Set<T>>> {
    let item_list = s.ToList();
    let mut curr_set = NewSet::<T>();
    combSetIterate(&item_list, curr_set.as_mut(), 0, numberOfItems)
}

// combSetIterate 对应 Go 私有递归函数 `combSetIterate`。
// Go 通过同一个 currSet 做 Add/Remove 回溯；Rust 用可变 trait object 引用表达原地修改。
/// 回溯辅助：在 `itemList[depth..]` 上选/不选，收集大小为 `numberOfItems` 的组合。
fn combSetIterate<T: Key + Clone + 'static>(
    itemList: &[T],
    currSet: &mut dyn Set<T>,
    depth: usize,
    numberOfItems: isize,
) -> Vec<Box<dyn Set<T>>> {
    if currSet.Size() as isize == numberOfItems {
        // 达到目标大小时必须 Clone 当前集合；否则后续回溯 Remove 会改变已收集结果。
        return vec![currSet.Clone()];
    }
    if depth == itemList.len() || currSet.Size() as isize > numberOfItems {
        // Go 返回 nil slice 表示没有组合；Rust 用空 Vec 表达同一“无结果”语义。
        return Vec::new();
    }
    let mut res = Vec::new();
    // 第一条递归分支：选择当前 depth 的元素。
    let item = itemList[depth].clone();
    currSet.Add(&[item.clone()]);
    res.extend(combSetIterate(itemList, currSet, depth + 1, numberOfItems));
    // 回溯资源收尾：移除刚才加入的元素，恢复 currSet，保证第二条分支看到的是“不选当前元素”的状态。
    currSet.Remove(&item);
    res.extend(combSetIterate(itemList, currSet, depth + 1, numberOfItems));
    res
}
