// Copyright 2022 PingCAP, Inc.
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

// 基于 trie 的 schema/table 规则选择器。
//
// 对应 Go `pkg/util/table-rule-selector`。用字符/`*`/`?`/`[range]` 节点建 trie，
// schema 层通过 nextLevel 挂 table 层；Match 结果带缓存，Insert/Remove 会失效缓存。
// 规则值为 `Arc<dyn Any>`，对齐 Go `any`。

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// Go 的 `any` 规则值可以承载任意对象；Rust 用线程安全的 Arc<dyn Any> 表达共享动态值。
pub type Rule = Arc<dyn Any + Send + Sync>;
type SelectorResult<T> = Result<T, String>;
type NodeRef = Arc<RwLock<node>>;
type ItemRef = Arc<RwLock<itemKind>>;

//  1. asterisk character (*, also called "star") matches zero or more characters,
//     for example, doc* matches doc and document but not dodo;
//     asterisk character must be the last character of wildcard word.
//  2. the question mark ? matches exactly one character
// asterisk/question/range 系列常量保持 Go 字节匹配语义；原实现按 byte 处理 pattern。
const asterisk: u8 = b'*';
const question: u8 = b'?';
const rangeOpen: u8 = b'[';
const rangeClose: u8 = b']';
const rangeNot: u8 = b'!';
const rangeBetween: u8 = b'-';

// maxCacheNum 对应 Go 中 cache 的最大条目数；超过后删除任意一个缓存项。
const maxCacheNum: usize = 1024;

// Selector stores rules of schema/table for easy retrieval
// Selector 对应 Go 接口，描述 schema/table 规则的插入、匹配、删除和枚举能力。
// Rust 保留 Go 方法名和参数顺序；rule 用 Option 表达 Go nil 检查。
pub trait Selector: Send + Sync {
    // Insert will insert one rule into trie
    // if table is empty, insert rule into schema level
    // otherwise insert rule into table level
    fn Insert(
        &self,
        schema: &str,
        table: &str,
        rule: Option<Rule>,
        insertType: i32,
    ) -> SelectorResult<()>;
    // Match will return all matched rules
    fn Match(&self, schema: &str, table: &str) -> RuleSet;
    // Remove will remove one rule
    fn Remove(&self, schema: &str, table: &str) -> SelectorResult<()>;
    // AllRules will returns all rules
    fn AllRules(
        &self,
    ) -> (
        HashMap<String, RuleSet>,
        HashMap<String, HashMap<String, RuleSet>>,
    );
}

// RuleSet is a set of rules that selected
// RuleSet 对应 Go 的 []any；Arc 允许克隆规则集合来表达 Go slice clone 行为。
#[derive(Clone, Default)]
pub struct RuleSet(pub Vec<Rule>);

impl RuleSet {
    // clone 对应 Go 的 RuleSet.clone：nil 仍返回 nil，非 nil 则复制 slice 头和元素引用。
    fn clone_rule_set(&self) -> RuleSet {
        RuleSet(self.0.clone())
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn extend(&mut self, other: RuleSet) {
        self.0.extend(other.0);
    }
}

// matchedResult 保存一次匹配中命中的 table-level 节点和 schema/table 规则。
// nodes 对应 Go 的 []*node，rules 对应 RuleSet。
#[derive(Default)]
struct matchedResult {
    nodes: Vec<NodeRef>,
    rules: RuleSet,
}

impl matchedResult {
    // empty 对应 Go 的空匹配结果判断。
    fn empty(&self) -> bool {
        self.nodes.is_empty() && self.rules.is_empty()
    }
}

// trieSelector 对应 Go 结构体中嵌入 sync.RWMutex、cache 和 root。
// guard 复刻 Go RWMutex 的操作级锁边界，节点使用 Arc<RwLock<_>> 保存线程安全指针图。
pub struct trieSelector {
    pub(crate) cache: RwLock<HashMap<String, RuleSet>>,
    root: NodeRef,
    guard: RwLock<()>,
}

impl trieSelector {
    // new_empty 供测试构造可访问 cache 的具体 trieSelector，对应 Go 中 `s.(*trieSelector)`。
    pub(crate) fn new_empty() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            root: newNode(),
            guard: RwLock::new(()),
        }
    }
}

// node 对应 Go trie 节点：普通字符、星号、问号和 range item 分开存放。
#[derive(Default)]
struct node {
    characters: HashMap<u8, ItemRef>,
    asterisk: Option<ItemRef>,
    question: Option<ItemRef>,
    rItems: Vec<ItemRef>,
}

// itemKind 对应 Go `item` 接口：baseItem 与 rangeItem 都暴露 child/rule/nextLevel 操作。
// 使用 enum 明确表达两种 item 的所有权。
enum itemKind {
    base(baseItem),
    range(rangeItem),
}

impl itemKind {
    // child 对应 Go item.child，返回 trie 的下一层字符节点。
    fn child(&self) -> Option<NodeRef> {
        match self {
            itemKind::base(i) => i.child(),
            itemKind::range(i) => i.baseItem.child(),
        }
    }

    // setChild 对应 Go item.setChild；插入新路径时为 item 补齐 child 节点。
    fn setChild(&mut self, c: NodeRef) {
        match self {
            itemKind::base(i) => i.setChild(c),
            itemKind::range(i) => i.baseItem.setChild(c),
        }
    }

    // getRule 对应 Go item.getRule；None 表达 Go nil rule slice。
    fn getRule(&self) -> Option<RuleSet> {
        match self {
            itemKind::base(i) => i.getRule(),
            itemKind::range(i) => i.baseItem.getRule(),
        }
    }

    // setRule 对应 Go item.setRule(...any)，Replace 分支会把规则集合替换为单个新 rule。
    fn setRule(&mut self, rules: RuleSet) {
        match self {
            itemKind::base(i) => i.setRule(rules),
            itemKind::range(i) => i.baseItem.setRule(rules),
        }
    }

    // resetRule 对应 Go item.resetRule，用于 Remove 的懒删除。
    fn resetRule(&mut self) {
        match self {
            itemKind::base(i) => i.resetRule(),
            itemKind::range(i) => i.baseItem.resetRule(),
        }
    }

    // appendRule 对应 Go item.appendRule；Append 和首次 Insert 都追加到 rule slice。
    fn appendRule(&mut self, rule: Rule) {
        match self {
            itemKind::base(i) => i.appendRule(rule),
            itemKind::range(i) => i.baseItem.appendRule(rule),
        }
    }

    // getNextLevel 对应 schema level 指向 table level 的 nextLevel 指针。
    fn getNextLevel(&self) -> Option<NodeRef> {
        match self {
            itemKind::base(i) => i.getNextLevel(),
            itemKind::range(i) => i.baseItem.getNextLevel(),
        }
    }

    // setNextLevel 在 schema pattern 首次挂 table rule 时创建 table-level trie。
    fn setNextLevel(&mut self, c: NodeRef) {
        match self {
            itemKind::base(i) => i.setNextLevel(c),
            itemKind::range(i) => i.baseItem.setNextLevel(c),
        }
    }

    fn as_range(&self) -> Option<&rangeItem> {
        match self {
            itemKind::range(i) => Some(i),
            itemKind::base(_) => None,
        }
    }
}

// baseItem 保存 item 的公共字段：child、rule 和 schema->table 的 nextLevel。
#[derive(Default)]
struct baseItem {
    ch: Option<NodeRef>,
    rule: Option<RuleSet>,
    // schema level ->(to) table level
    nextLevel: Option<NodeRef>,
}

impl baseItem {
    fn child(&self) -> Option<NodeRef> {
        self.ch.clone()
    }

    fn setChild(&mut self, c: NodeRef) {
        self.ch = Some(c);
    }

    fn getRule(&self) -> Option<RuleSet> {
        self.rule.clone()
    }

    fn setRule(&mut self, rules: RuleSet) {
        self.rule = Some(rules);
    }

    fn resetRule(&mut self) {
        self.rule = None;
    }

    fn appendRule(&mut self, rule: Rule) {
        match &mut self.rule {
            Some(rules) => rules.0.push(rule),
            None => self.rule = Some(RuleSet(vec![rule])),
        }
    }

    fn getNextLevel(&self) -> Option<NodeRef> {
        self.nextLevel.clone()
    }

    fn setNextLevel(&mut self, c: NodeRef) {
        self.nextLevel = Some(c);
    }
}

// newNode 对应 Go newNode，创建 characters map 非 nil 的 trie 节点。
fn newNode() -> NodeRef {
    Arc::new(RwLock::new(node {
        characters: HashMap::new(),
        ..Default::default()
    }))
}

// ran 对应 Go 的单个 range 片段，start/end 为字节边界。
#[derive(Clone)]
struct ran {
    start: u8,
    end: u8,
    hasBetween: bool,
}

// rangeItem 对应 Go 的 [a-z] / [!a] 这类范围匹配 item。
struct rangeItem {
    baseItem: baseItem,
    hasNot: bool,
    ranges: Vec<ran>,
}

impl rangeItem {
    // equal 对应 Go rangeItem.equal：两个范围互相包含才视为等价。
    fn equal(&self, i2: &rangeItem) -> bool {
        self.match_range(i2) && i2.match_range(self)
    }

    // match_range 对应 Go rangeItem.match。
    fn match_range(&self, i2: &rangeItem) -> bool {
        if self.hasNot != i2.hasNot {
            return false;
        }
        for r in &self.ranges {
            let mut matched = false;
            for r2 in &i2.ranges {
                if r2.start <= r.start && r.end <= r2.end {
                    matched = true;
                    break;
                }
            }
            if !matched {
                return false;
            }
        }
        true
    }

    // matchChar 对应 Go rangeItem.matchChar，hasNot 会反转匹配结果。
    fn matchChar(&self, c: u8) -> bool {
        for r in &self.ranges {
            if r.start <= c && c <= r.end {
                return !self.hasNot;
            }
        }
        self.hasNot
    }

    // str 对应 Go rangeItem.str，用于 AllRules/travel 还原 range pattern 文本。
    fn str(&self) -> String {
        let mut ret = String::from("[");
        if self.hasNot {
            ret.push(rangeNot as char);
        }
        for r in &self.ranges {
            if r.hasBetween {
                ret.push(r.start as char);
                ret.push(rangeBetween as char);
                ret.push(r.end as char);
            } else {
                ret.push(r.start as char);
            }
        }
        ret.push(']');
        ret
    }
}

// NewTrieSelector returns a trie Selector
// NewTrieSelector 创建 trieSelector，并初始化空 cache 和 root 节点。
pub fn NewTrieSelector() -> Box<dyn Selector> {
    Box::new(trieSelector::new_empty())
}

// Insert means insert a new rule
pub const Insert: i32 = 0;
// Replace means update an old rule
pub const Replace: i32 = 1;
// Append means delete an old rule
pub const Append: i32 = 2;

impl Selector for trieSelector {
    // Insert implements Selector's interface.
    // Insert 先校验 schema/rule，再按 table 是否为空分发到 schema 或 table 层。
    fn Insert(
        &self,
        schema: &str,
        table: &str,
        rule: Option<Rule>,
        insertType: i32,
    ) -> SelectorResult<()> {
        if schema.is_empty() || rule.is_none() {
            return Err(format!(
                "schema pattern {} or rule {:?} can't be empty",
                schema, "<rule>"
            ));
        }

        let _guard = self.guard.write().expect("selector write lock");
        let err = if table.is_empty() {
            self.insertSchema(schema, rule.expect("checked above"), insertType)
        } else {
            self.insertTable(schema, table, rule.expect("checked above"), insertType)
        };

        // 对应 errors.Trace(err)，错误上下文已在分层插入函数中保留。
        err
    }

    // Match implements Selector's interface.
    // Match 先查 schema/table cache，未命中时匹配 schema 层，再进入命中的 table-level trie。
    fn Match(&self, schema: &str, table: &str) -> RuleSet {
        let _guard = self.guard.write().expect("selector write lock");
        // try to find schema/table in cache
        let cacheKey = quoteSchemaTable(schema, table);
        if let Some(rules) = self.cache.read().expect("cache read lock").get(&cacheKey) {
            return rules.clone_rule_set();
        }

        let mut matchedSchemaResult = matchedResult {
            nodes: Vec::with_capacity(4),
            rules: RuleSet(Vec::with_capacity(4)),
        };
        let mut rules = RuleSet::default();

        // find matched rules
        // Go 代码在 cache miss 后升级为写锁并持有到缓存写入完成，避免并发修改 trie。
        self.matchNode(self.root.clone(), schema, &mut matchedSchemaResult);

        // not found matched rules in schema level
        if matchedSchemaResult.empty() {
            self.addToCache(cacheKey, RuleSet::default());
            return RuleSet::default();
        }

        rules.extend(matchedSchemaResult.rules.clone_rule_set());

        for si in matchedSchemaResult.nodes {
            let mut matchedTableResult = matchedResult {
                nodes: Vec::new(),
                rules: RuleSet(Vec::with_capacity(4)),
            };
            // find matched rules in table level
            self.matchNode(si, table, &mut matchedTableResult);
            rules.extend(matchedTableResult.rules);
        }

        // not found matched rule in table level, return mathed rule in schema level
        self.addToCache(cacheKey, rules.clone_rule_set());
        rules.clone_rule_set()
    }

    // Remove implements Selector interface.
    // TODO: remove useless nodes and lazy deletion
    // Remove 保留 Go 的懒删除策略：只清空叶子 rule，不回收无用节点。
    fn Remove(&self, schema: &str, table: &str) -> SelectorResult<()> {
        let _guard = self.guard.write().expect("selector write lock");
        if schema.is_empty() {
            return Err(format!("schema/table {}/{} is not valid", schema, table));
        }

        let schemaItems = self.track(self.root.clone(), schema).map_err(|err| {
            format!(
                "track schema/table {}/{} in schema level: {}",
                schema, table, err
            )
        })?;

        let schemaLeafItem = schemaItems[schemaItems.len() - 1].clone();
        if !table.is_empty() {
            let nextLevel = schemaLeafItem
                .read()
                .expect("item read lock")
                .getNextLevel()
                .ok_or_else(|| {
                    format!(
                        "table level while we track chema/table {}/{} not found",
                        schema, table
                    )
                })?;

            let tableItems = self.track(nextLevel, table).map_err(|err| {
                format!(
                    "track schema/table {}/{} in table level: {}",
                    schema, table, err
                )
            })?;

            if tableItems[tableItems.len() - 1]
                .read()
                .expect("item read lock")
                .getRule()
                .is_none()
            {
                return Err(format!(
                    "schema/table {}/{} in table level not found",
                    schema, table
                ));
            }

            // remove table level nodes
            tableItems[tableItems.len() - 1]
                .write()
                .expect("item write lock")
                .resetRule();
            self.clearCache();
            return Ok(());
        }

        if schemaLeafItem
            .read()
            .expect("item read lock")
            .getRule()
            .is_none()
        {
            return Err(format!(
                "schema/table {}/{} in schema level not found",
                schema, table
            ));
        }

        schemaLeafItem.write().expect("item write lock").resetRule();
        self.clearCache();
        Ok(())
    }

    // AllRules implements Selector's AllRules
    // AllRules 先遍历 schema 层规则和 schema->table 节点，再逐个遍历 table 层规则。
    fn AllRules(
        &self,
    ) -> (
        HashMap<String, RuleSet>,
        HashMap<String, HashMap<String, RuleSet>>,
    ) {
        let _guard = self.guard.read().expect("selector read lock");
        let mut tableRules: HashMap<String, HashMap<String, RuleSet>> = HashMap::new();
        let mut schemaNodes: HashMap<String, NodeRef> = HashMap::new();
        let mut schemaRules: HashMap<String, RuleSet> = HashMap::new();
        let mut word: Vec<u8> = Vec::new();

        // guard 的读锁防止遍历时 trie 被修改。
        self.travel(
            self.root.clone(),
            &mut word,
            Some(&mut schemaRules),
            Some(&mut schemaNodes),
        );

        for (schema, n) in schemaNodes {
            let mut rules = tableRules.remove(&schema).unwrap_or_default();

            word.clear();
            self.travel(n, &mut word, Some(&mut rules), None);
            if !rules.is_empty() {
                tableRules.insert(schema, rules);
            }
        }
        (schemaRules, tableRules)
    }
}

impl trieSelector {
    // insertSchema 对应 schema 级规则插入，错误时补充 "insert into schema selector" 上下文。
    fn insertSchema(&self, schema: &str, rule: Rule, insertType: i32) -> SelectorResult<()> {
        self.insert(self.root.clone(), schema, Some(rule), insertType)
            .map(|_| ())
            .map_err(|err| format!("insert into schema selector: {}", err))
    }

    // insertTable 先确保 schema pattern 存在，再通过 schema item 的 nextLevel 插入 table pattern。
    fn insertTable(
        &self,
        schema: &str,
        table: &str,
        rule: Rule,
        insertType: i32,
    ) -> SelectorResult<()> {
        let schemaEntity = self
            .insert(self.root.clone(), schema, None, Insert)
            .map_err(|err| format!("insert into schema selector: {}", err))?;

        if schemaEntity
            .read()
            .expect("item read lock")
            .getNextLevel()
            .is_none()
        {
            schemaEntity
                .write()
                .expect("item write lock")
                .setNextLevel(newNode());
        }

        let nextLevel = schemaEntity
            .read()
            .expect("item read lock")
            .getNextLevel()
            .expect("created next level");
        self.insert(nextLevel, table, Some(rule), insertType)
            .map(|_| ())
            .map_err(|err| format!("insert into table selector: {}", err))
    }

    // getRangeItem 解析以 '[' 开头的 pattern 片段，返回 rangeItem 和闭括号相对位置。
    // nextI = -1 表示没有闭括号，调用方应把 '[' 当普通字符处理。
    fn getRangeItem(&self, pattern: &str) -> (Option<rangeItem>, isize) {
        let bytes = pattern.as_bytes();
        let mut nextI: isize = -1;
        for i in 0..bytes.len() {
            if bytes[i] == rangeClose {
                nextI = i as isize;
                break;
            }
        }
        if nextI == -1 {
            return (None, nextI);
        }

        let mut item = rangeItem {
            baseItem: baseItem::default(),
            hasNot: false,
            ranges: Vec::new(),
        };
        let mut startI = 1usize;
        if startI < bytes.len() && bytes[startI] == rangeNot {
            startI += 1;
            item.hasNot = true;
        }

        let mut i = startI;
        while i < nextI as usize {
            if i + 2 < nextI as usize && bytes[i + 1] == rangeBetween {
                item.ranges.push(ran {
                    start: bytes[i],
                    end: bytes[i + 2],
                    hasBetween: true,
                });
                i += 3;
            } else {
                item.ranges.push(ran {
                    start: bytes[i],
                    end: bytes[i],
                    hasBetween: false,
                });
                i += 1;
            }
        }

        // Change the `[!]` to `[\!-\!]`.
        // Go 特判 `[!]`：它不是取反空集合，而是按字面量 `!` 匹配。
        if item.ranges.is_empty() && item.hasNot {
            item.hasNot = false;
            item.ranges.push(ran {
                start: rangeNot,
                end: rangeNot,
                hasBetween: false,
            });
        }
        (Some(item), nextI)
    }

    // if rule is nil, just extract nodes
    // insert 是插入和 schema 节点提取共用的核心流程；rule=None 对应 Go nil，只建路径不挂规则。
    fn insert(
        &self,
        root: NodeRef,
        pattern: &str,
        rule: Option<Rule>,
        insertType: i32,
    ) -> SelectorResult<ItemRef> {
        let bytes = pattern.as_bytes();
        let mut n = root;
        let mut hadAsterisk = false;
        let mut entity: Option<ItemRef> = None;

        let mut i = 0usize;
        while i < bytes.len() {
            if hadAsterisk {
                return Err(format!("pattern {} is not valid", pattern));
            }

            let mut parsedRange: Option<rangeItem> = None;
            let mut nextI: isize = -1;

            // 这段 switch 保留 Go 对四类 pattern 字节的分派：*、?、[range]、普通字符。
            match bytes[i] {
                asterisk => {
                    entity = n.read().expect("node read lock").asterisk.clone();
                    hadAsterisk = true;
                }
                question => {
                    entity = n.read().expect("node read lock").question.clone();
                }
                rangeOpen => {
                    let (rItem, foundNextI) = self.getRangeItem(&pattern[i..]);
                    parsedRange = rItem;
                    nextI = foundNextI;
                    if nextI == -1 {
                        entity = n
                            .read()
                            .expect("node read lock")
                            .characters
                            .get(&bytes[i])
                            .cloned();
                    } else {
                        entity = None;
                        // range item 需要按范围等价性复用已有 item，而不是按原始字符串复用。
                        for nrItem in &n.read().expect("node read lock").rItems {
                            let isEqual = nrItem
                                .read()
                                .expect("item read lock")
                                .as_range()
                                .map(|existing| {
                                    parsedRange
                                        .as_ref()
                                        .map(|r| r.equal(existing))
                                        .unwrap_or(false)
                                })
                                .unwrap_or(false);
                            if isEqual {
                                entity = Some(nrItem.clone());
                                break;
                            }
                        }
                    }
                }
                _ => {
                    entity = n
                        .read()
                        .expect("node read lock")
                        .characters
                        .get(&bytes[i])
                        .cloned();
                }
            }

            if entity.is_none() {
                // Go 先创建 baseItem，再在 range 分支把它嵌入 rangeItem；Rust 用 enum 表达两种 item。
                let newEntity = match bytes[i] {
                    rangeOpen if nextI != -1 => {
                        let mut rItem = parsedRange.expect("range item parsed when nextI != -1");
                        rItem.baseItem = baseItem::default();
                        Arc::new(RwLock::new(itemKind::range(rItem)))
                    }
                    _ => Arc::new(RwLock::new(itemKind::base(baseItem::default()))),
                };

                match bytes[i] {
                    asterisk => {
                        n.write().expect("node write lock").asterisk = Some(newEntity.clone());
                    }
                    question => {
                        n.write().expect("node write lock").question = Some(newEntity.clone());
                    }
                    rangeOpen => {
                        if nextI == -1 {
                            n.write()
                                .expect("node write lock")
                                .characters
                                .insert(bytes[i], newEntity.clone());
                        } else {
                            n.write()
                                .expect("node write lock")
                                .rItems
                                .push(newEntity.clone());
                        }
                    }
                    _ => {
                        n.write()
                            .expect("node write lock")
                            .characters
                            .insert(bytes[i], newEntity.clone());
                    }
                }
                entity = Some(newEntity);
            }

            let current = entity.clone().expect("entity must exist after creation");
            if current.read().expect("item read lock").child().is_none() {
                current
                    .write()
                    .expect("item write lock")
                    .setChild(newNode());
            }
            n = current
                .read()
                .expect("item read lock")
                .child()
                .expect("child just created");

            if nextI != -1 {
                i += nextI as usize;
            }
            i += 1;
        }

        let entity = entity.ok_or_else(|| format!("pattern {} is empty", pattern))?;
        if let Some(rule) = rule {
            if insertType == Insert && entity.read().expect("item read lock").getRule().is_some() {
                return Err(format!("pattern {} already exists", pattern));
            }
            if insertType == Replace {
                entity
                    .write()
                    .expect("item write lock")
                    .setRule(RuleSet(vec![rule]));
            } else {
                entity.write().expect("item write lock").appendRule(rule);
            }
            // 任一规则更新都会让 schema/table 匹配缓存失效。
            self.clearCache();
        }

        Ok(entity)
    }

    // track 按 pattern 精确走 trie，用于 Remove 找到要懒删除的叶子 item。
    fn track(&self, n: NodeRef, pattern: &str) -> SelectorResult<Vec<ItemRef>> {
        let bytes = pattern.as_bytes();
        let mut items: Vec<ItemRef> = Vec::with_capacity(bytes.len());
        let mut n = n;

        let mut i = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                asterisk => {
                    let item = n
                        .read()
                        .expect("node read lock")
                        .asterisk
                        .clone()
                        .ok_or_else(|| format!("pattern {} not found", pattern))?;

                    if i != bytes.len() - 1 {
                        return Err(format!("pattern {} is not valid", pattern));
                    }

                    items.push(item);
                }
                question => {
                    let item = n
                        .read()
                        .expect("node read lock")
                        .question
                        .clone()
                        .ok_or_else(|| format!("pattern {} not found", pattern))?;
                    items.push(item.clone());
                    n = item
                        .read()
                        .expect("item read lock")
                        .child()
                        .expect("question item should have child");
                }
                rangeOpen => {
                    let (rItem, nextI) = self.getRangeItem(&pattern[i..]);
                    if nextI == -1 {
                        let item = n
                            .read()
                            .expect("node read lock")
                            .characters
                            .get(&bytes[i])
                            .cloned()
                            .ok_or_else(|| format!("pattern {} not found", pattern))?;
                        items.push(item.clone());
                        n = item
                            .read()
                            .expect("item read lock")
                            .child()
                            .expect("literal '[' item should have child");
                    } else {
                        let mut matchIdx: isize = -1;
                        for (idx, existing) in
                            n.read().expect("node read lock").rItems.iter().enumerate()
                        {
                            let isEqual = existing
                                .read()
                                .expect("item read lock")
                                .as_range()
                                .map(|range| {
                                    rItem.as_ref().map(|r| range.equal(r)).unwrap_or(false)
                                })
                                .unwrap_or(false);
                            if isEqual {
                                matchIdx = idx as isize;
                                break;
                            }
                        }
                        if matchIdx == -1 {
                            return Err(format!("pattern {} not found", pattern));
                        }
                        let item =
                            n.read().expect("node read lock").rItems[matchIdx as usize].clone();
                        items.push(item.clone());
                        n = item
                            .read()
                            .expect("item read lock")
                            .child()
                            .expect("range item should have child");
                        i += nextI as usize;
                    }
                }
                _ => {
                    let item = n
                        .read()
                        .expect("node read lock")
                        .characters
                        .get(&bytes[i])
                        .cloned()
                        .ok_or_else(|| format!("pattern {} not found", pattern))?;
                    items.push(item.clone());
                    n = item
                        .read()
                        .expect("item read lock")
                        .child()
                        .expect("character item should have child");
                }
            }

            i += 1;
        }

        Ok(items)
    }

    // travel 深度遍历 trie，把命中的规则写入 rules，把 schema 层 nextLevel 写入 nodes。
    fn travel(
        &self,
        n: NodeRef,
        word: &mut Vec<u8>,
        mut rules: Option<&mut HashMap<String, RuleSet>>,
        mut nodes: Option<&mut HashMap<String, NodeRef>>,
    ) {
        // Rust 的 NodeRef 非空，因此不需要 Go 的 nil node 分支。
        if let Some(entity) = n.read().expect("node read lock").asterisk.clone() {
            let mut pattern = word.clone();
            pattern.push(asterisk);
            insertMatchedItemIntoMap(
                String::from_utf8_lossy(&pattern).to_string(),
                entity,
                rules.as_deref_mut(),
                nodes.as_deref_mut(),
            );
        }

        if let Some(entity) = n.read().expect("node read lock").question.clone() {
            let mut pattern = word.clone();
            pattern.push(question);
            insertMatchedItemIntoMap(
                String::from_utf8_lossy(&pattern).to_string(),
                entity.clone(),
                rules.as_deref_mut(),
                nodes.as_deref_mut(),
            );
            self.travel(
                entity
                    .read()
                    .expect("item read lock")
                    .child()
                    .expect("question child"),
                &mut pattern,
                rules.as_deref_mut(),
                nodes.as_deref_mut(),
            );
        }

        for rItem in &n.read().expect("node read lock").rItems {
            let rangeText = rItem
                .read()
                .expect("item read lock")
                .as_range()
                .map(|range| range.str())
                .unwrap_or_default();
            let mut pattern = word.clone();
            pattern.extend(rangeText.as_bytes());
            insertMatchedItemIntoMap(
                String::from_utf8_lossy(&pattern).to_string(),
                rItem.clone(),
                rules.as_deref_mut(),
                nodes.as_deref_mut(),
            );
            self.travel(
                rItem
                    .read()
                    .expect("item read lock")
                    .child()
                    .expect("range child"),
                &mut pattern,
                rules.as_deref_mut(),
                nodes.as_deref_mut(),
            );
        }

        for (charByte, baseItem) in &n.read().expect("node read lock").characters {
            let mut pattern = word.clone();
            pattern.push(*charByte);
            insertMatchedItemIntoMap(
                String::from_utf8_lossy(&pattern).to_string(),
                baseItem.clone(),
                rules.as_deref_mut(),
                nodes.as_deref_mut(),
            );
            self.travel(
                baseItem
                    .read()
                    .expect("item read lock")
                    .child()
                    .expect("character child"),
                &mut pattern,
                rules.as_deref_mut(),
                nodes.as_deref_mut(),
            );
        }
    }

    // matchNode 按输入字符串递归匹配 trie，收集当前层命中的规则和 nextLevel。
    // 原 Go `for i := range s` 再用 s[i] 取 byte；偏移按 rune 推进，但匹配值仍是首字节。
    fn matchNode(&self, n: NodeRef, s: &str, mr: &mut matchedResult) {
        self.matchNodeBytes(n, s.as_bytes(), mr);
    }

    // 使用字节切片允许像 Go string 一样从 UTF-8 rune 内部递归，不触发 Rust str 边界 panic。
    fn matchNodeBytes(&self, n: NodeRef, bytes: &[u8], mr: &mut matchedResult) {
        let mut n = n;
        let mut entity: Option<ItemRef> = None;

        let mut i = 0;
        while i < bytes.len() {
            if let Some(item) = n.read().expect("node read lock").asterisk.clone() {
                appendMatchedItem(item, mr);
            }

            if let Some(item) = n.read().expect("node read lock").question.clone() {
                if i == bytes.len() - 1 {
                    appendMatchedItem(item.clone(), mr);
                }

                self.matchNodeBytes(
                    item.read()
                        .expect("item read lock")
                        .child()
                        .expect("question child"),
                    &bytes[i + 1..],
                    mr,
                );
            }

            for rItem in &n.read().expect("node read lock").rItems {
                let matched = rItem
                    .read()
                    .expect("item read lock")
                    .as_range()
                    .map(|range| range.matchChar(bytes[i]))
                    .unwrap_or(false);
                if matched {
                    if i == bytes.len() - 1 {
                        appendMatchedItem(rItem.clone(), mr);
                    }

                    self.matchNodeBytes(
                        rItem
                            .read()
                            .expect("item read lock")
                            .child()
                            .expect("range child"),
                        &bytes[i + 1..],
                        mr,
                    );
                }
            }

            entity = n
                .read()
                .expect("node read lock")
                .characters
                .get(&bytes[i])
                .cloned();
            if entity.is_none() {
                return;
            }
            n = entity
                .as_ref()
                .expect("entity checked")
                .read()
                .expect("item read lock")
                .child()
                .expect("character child");

            i += goUtf8RuneWidth(&bytes[i..]);
        }

        if let Some(entity) = entity {
            appendMatchedItem(entity, mr);
        }

        if let Some(item) = n.read().expect("node read lock").asterisk.clone() {
            appendMatchedItem(item, mr);
        };
    }

    // addToCache 写入 Match 缓存；超过 maxCacheNum 时删除 map 中遍历到的第一个 key。
    fn addToCache(&self, key: String, rules: RuleSet) {
        let mut cache = self.cache.write().expect("cache write lock");
        cache.insert(key, rules);
        if cache.len() > maxCacheNum {
            if let Some(literal) = cache.keys().next().cloned() {
                cache.remove(&literal);
            }
        }
    }

    // clearCache 在 Insert/Remove 后清空缓存，避免返回旧规则。
    fn clearCache(&self) {
        *self.cache.write().expect("cache write lock") = HashMap::new();
    }
}

// appendMatchedItem 对应 Go helper：命中 item 时追加其规则，并收集 schema->table nextLevel。
fn appendMatchedItem(entity: ItemRef, mr: &mut matchedResult) {
    if let Some(rules) = entity.read().expect("item read lock").getRule() {
        mr.rules.extend(rules);
    }

    if let Some(nextLevel) = entity.read().expect("item read lock").getNextLevel() {
        mr.nodes.push(nextLevel);
    }
}

// insertMatchedItemIntoMap 用于 AllRules/travel，把当前 pattern 对应的规则和下一层节点写入 map。
fn insertMatchedItemIntoMap(
    pattern: String,
    entity: ItemRef,
    rules: Option<&mut HashMap<String, RuleSet>>,
    nodes: Option<&mut HashMap<String, NodeRef>>,
) {
    if let Some(rules) = rules {
        if let Some(ruleSet) = entity.read().expect("item read lock").getRule() {
            rules.insert(pattern.clone(), ruleSet);
        }
    }

    if let Some(nodes) = nodes {
        if let Some(nextLevel) = entity.read().expect("item read lock").getNextLevel() {
            nodes.insert(pattern, nextLevel);
        }
    }
}

// 返回 Go `range string` 在当前字节位置的推进宽度；非法 UTF-8 按一个字节推进。
fn goUtf8RuneWidth(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.chars().next().map(char::len_utf8).unwrap_or(1),
        Err(err) if err.valid_up_to() > 0 => std::str::from_utf8(&bytes[..err.valid_up_to()])
            .expect("valid_up_to prefix must be valid UTF-8")
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1),
        Err(_) => 1,
    }
}

// quoteSchemaTable 对应 Go helper，负责把 schema/table 组合成 cache key。
pub(crate) fn quoteSchemaTable(schema: &str, table: &str) -> String {
    if schema.is_empty() {
        return String::new();
    }

    if !table.is_empty() {
        return format!("`{}`.`{}`", schema, table);
    }

    format!("`{}`", schema)
}
