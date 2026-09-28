// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// MySQL 复制规则风格的库表过滤器。
//
// 对应 Go `pkg/util/filter/filter.go`。复制过滤按 DoDB/IgnoreDB、DoTable/IgnoreTable
// 决定 schema/table 是否保留；规则支持字面量与 `~` 前缀正则，并用缓存加速重复匹配。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use regex::Regex;

// ActionType is do or ignore something
// ActionType 对应 Go 的 bool 别名：true 表示允许，false 表示忽略。
/// 过滤动作：`true` 允许（Do），`false` 忽略（Ignore）。
pub type ActionType = bool;

// builtin actiontype variable
/// 允许通过（Do）。
pub const Do: ActionType = true;
/// 忽略（Ignore）。
pub const Ignore: ActionType = false;

// Table represents a table.
// Table 直接对应 Go 侧 table-filter.Table 类型别名。
/// 库表标识（schema + name），对应 Go `table-filter.Table`。
pub type Table = tfilter::Table;

// cache 对应 Go 的缓存结构，用读写锁保护 schema.table 到 ActionType 的查询结果。
/// 匹配结果缓存：`schema.table` 字符串到 `ActionType`。
pub struct cache {
    /// 受读写锁保护的命中结果映射。
    pub items: RwLock<HashMap<String, ActionType>>,
}

impl cache {
    // query 对应 Go 的 RLock/RUnlock 查询逻辑；找不到时返回 Ignore 和 false。
    /// 查询缓存；未命中返回 `(Ignore, false)`。
    pub fn query(&self, key: &str) -> (ActionType, bool) {
        let items = self.items.read().expect("cache read lock poisoned");
        match items.get(key) {
            Some(action) => (*action, true),
            None => (Ignore, false),
        }
    }

    /// 写入或覆盖缓存项。
    pub fn set(&self, key: String, action: ActionType) {
        let mut items = self.items.write().expect("cache write lock poisoned");
        items.insert(key, action);
    }
}

// Rules contains Filter rules.
// Rules 对应 Go 侧 MySQLReplicationRules 类型别名。
/// MySQL 复制风格过滤规则集合。
pub type Rules = tfilter::MySQLReplicationRules;

// Filter implements table filter in the style of MySQL replication rules.
// Filter 保留 Go 结构：selector 嵌入字段、正则缓存、规则指针、匹配缓存和大小写开关。
/// 表过滤器：持有选择器、正则缓存、规则与匹配结果缓存。
pub struct Filter {
    /// trie/选择器，存放非纯正则规则节点。
    pub Selector: Box<dyn selector::Selector>,
    /// 已编译正则缓存，键为原始模式串。
    pub patternMap: HashMap<String, Regex>,
    /// 原始规则；`None` 表示不过滤。
    pub rules: Option<Box<Rules>>,
    /// `schema.table` 匹配结果缓存。
    pub c: cache,
    /// 是否大小写敏感。
    pub caseSensitive: bool,
}

// New creates a filter use the rules.
// New 对应 Go 构造函数；大小写不敏感时先把规则转换为小写，再初始化 trie 和正则。
/// 按规则构造过滤器；大小写不敏感时先把规则转小写再初始化。
pub fn New(caseSensitive: bool, mut rules: Option<Box<Rules>>) -> Result<Box<Filter>, String> {
    // 大小写不敏感时统一把规则字符串转小写，后续匹配也用小写输入。
    if !caseSensitive {
        Rules::ToLower(rules.as_deref_mut());
    }

    let mut f = Box::new(Filter {
        Selector: selector::NewTrieSelector(),
        caseSensitive,
        rules,
        patternMap: HashMap::new(),
        c: cache {
            items: RwLock::new(HashMap::new()),
        },
    });

    let err = f.initRules();
    if let Err(err) = err {
        return Err(err);
    }
    Ok(f)
}

/// selector 节点：纯 schema 规则。
pub const dbRule: i32 = 0;
/// selector 节点：schema+table 均为字面量。
pub const tblRuleFull: i32 = 1;
/// selector 节点：仅 schema 为字面量，table 为正则。
pub const tblRuleOnlyDBPart: i32 = 2;
/// selector 节点：仅 table 为字面量，schema 为正则。
pub const tblRuleOnlyTblPart: i32 = 3;

// nodeEndRule 对应 Go 插入 selector 节点末端保存的规则负载。
/// 插入 selector 末端的规则负载（可选正则、规则种类、是否白名单）。
pub struct nodeEndRule {
    /// 表名正则（仅部分规则种类使用）。
    pub r: Option<Regex>,
    /// 规则种类：`dbRule` / `tblRuleFull` 等。
    pub kind: i32,
    /// `true` 为 Do（白名单），`false` 为 Ignore。
    pub isAllowList: bool,
}

impl Filter {
    // initRules initialize the rules to regex expr or trie node.
    // initRules 按 Go 顺序初始化 DoDB、DoTables、IgnoreDB、IgnoreTables，并在空规则时直接返回。
    /// 按 Go 顺序把 Do/Ignore 的 DB/Table 规则编译进正则或 selector。
    pub fn initRules(&mut self) -> Result<(), String> {
        let Some(rules) = self.rules.as_ref() else {
            return Ok(());
        };

        // 先克隆四类规则，避免后续可变借用冲突。
        let doDBs = rules.DoDBs.clone();
        let doTables = rules.DoTables.clone();
        let ignoreDBs = rules.IgnoreDBs.clone();
        let ignoreTables = rules.IgnoreTables.clone();

        for db in &doDBs {
            if db.is_empty() {
                return Err("DoDB rule's DB string cannot be empty".into());
            }
            self.initSchemaRule(db, true)?;
        }

        for table in &doTables {
            if table.Schema.is_empty() || table.Name.is_empty() {
                return Err("DoTables rule's DB string or Table string cannot be empty".into());
            }
            self.initTableRule(&table.Schema, &table.Name, true)?;
        }

        for db in &ignoreDBs {
            if db.is_empty() {
                return Err("IgnoreDB rule's DB string cannot be empty".into());
            }
            self.initSchemaRule(db, false)?;
        }

        for table in &ignoreTables {
            if table.Schema.is_empty() || table.Name.is_empty() {
                return Err("IgnoreTables rule's DB string or Table string cannot be empty".into());
            }
            self.initTableRule(&table.Schema, &table.Name, false)?;
        }

        Ok(())
    }

    // initOneRegex 对应 Go 的正则编译缓存；大小写不敏感时加 (?i) 前缀。
    /// 编译并缓存一条正则；大小写不敏感时加 `(?i)` 前缀。
    pub fn initOneRegex(&mut self, originStr: &str) -> Result<(), String> {
        if !self.patternMap.contains_key(originStr) {
            let mut compileStr = originStr.to_string();
            if !self.caseSensitive {
                compileStr = format!("(?i){}", compileStr);
            }
            let reg = Regex::new(&compileStr).map_err(|err| err.to_string())?;
            self.patternMap.insert(originStr.to_string(), reg);
        }
        Ok(())
    }

    // initSchemaRule 对应 Go 的 schema 规则初始化：~ 前缀走正则，否则写入 selector。
    /// 初始化库级规则：`~` 前缀走正则，否则写入 selector。
    pub fn initSchemaRule(&mut self, dbStr: &str, isAllowList: bool) -> Result<(), String> {
        if dbStr.starts_with('~') {
            return self.initOneRegex(&dbStr[1..]);
        }
        self.Selector.Insert(
            dbStr,
            "",
            Some(Arc::new(nodeEndRule {
                r: None,
                kind: dbRule,
                isAllowList,
            })),
            selector::Append,
        )
    }

    // initTableRule 对应 Go 的表规则初始化，按 DB/table 是否正则拆成四类 selector 规则。
    /// 初始化表级规则，按 DB/table 是否正则拆成四类写入 selector/正则缓存。
    pub fn initTableRule(
        &mut self,
        dbStr: &str,
        tableStr: &str,
        isAllowList: bool,
    ) -> Result<(), String> {
        let dbIsRegex = dbStr.starts_with('~');
        let tblIsRegex = tableStr.starts_with('~');
        // 四种组合：双正则、仅 DB 正则、仅表正则、双字面量。
        if dbIsRegex && tblIsRegex {
            self.initOneRegex(&dbStr[1..])?;
            self.initOneRegex(&tableStr[1..])?;
        } else if dbIsRegex && !tblIsRegex {
            self.initOneRegex(&dbStr[1..])?;
            self.Selector.Insert(
                tableStr,
                "",
                Some(Arc::new(nodeEndRule {
                    r: None,
                    kind: tblRuleOnlyTblPart,
                    isAllowList,
                })),
                selector::Append,
            )?;
        } else if !dbIsRegex && tblIsRegex {
            self.initOneRegex(&tableStr[1..])?;
            let reg = self.patternMap.get(&tableStr[1..]).cloned();
            self.Selector.Insert(
                dbStr,
                "",
                Some(Arc::new(nodeEndRule {
                    kind: tblRuleOnlyDBPart,
                    r: reg,
                    isAllowList,
                })),
                selector::Append,
            )?;
        } else {
            self.Selector.Insert(
                dbStr,
                tableStr,
                Some(Arc::new(nodeEndRule {
                    r: None,
                    kind: tblRuleFull,
                    isAllowList,
                })),
                selector::Append,
            )?;
        }
        Ok(())
    }

    // ApplyOn applies filter rules on tables and convert schema/table name to lower case if not caseSensitive
    // rules like
    // https://dev.mysql.com/doc/refman/8.0/en/replication-rules-table-options.html
    // https://dev.mysql.com/doc/refman/8.0/en/replication-rules-db-options.html
    // Deprecated
    // ApplyOn 保留 Go 的废弃方法语义：返回克隆后的 Table 切片。
    /// 对表列表应用过滤，返回克隆后的匹配表（废弃路径，对齐 Go ApplyOn）。
    pub fn ApplyOn(&self, stbs: Vec<Box<Table>>) -> Vec<Box<Table>> {
        if self.rules.is_none() {
            return stbs;
        }

        let mut tbs = Vec::new();
        for tb in stbs {
            let mut newTb = tb.Clone();
            if !self.caseSensitive {
                newTb.Schema = newTb.Schema.to_lowercase();
                newTb.Name = newTb.Name.to_lowercase();
            }

            if self.Match(&newTb) {
                tbs.push(newTb);
            }
        }

        tbs
    }

    // Apply applies filter rules on tables
    // rules like
    // https://dev.mysql.com/doc/refman/8.0/en/replication-rules-table-options.html
    // https://dev.mysql.com/doc/refman/8.0/en/replication-rules-db-options.html
    // Apply 保留 Go 版本“不克隆原始返回表，只用小写副本做匹配”的差异。
    /// 对表列表应用过滤；匹配用小写副本，返回的仍是原始表对象。
    pub fn Apply(&self, stbs: Vec<Box<Table>>) -> Vec<Box<Table>> {
        if self.rules.is_none() {
            return stbs;
        }
        let mut tbs = Vec::new();
        for tb in stbs {
            let mut newTb = tb.Clone();
            if !self.caseSensitive {
                newTb = Box::new(Table {
                    Schema: newTb.Schema.to_lowercase(),
                    Name: newTb.Name.to_lowercase(),
                });
            }

            if self.Match(&newTb) {
                tbs.push(tb);
            }
        }
        tbs
    }

    // Match returns true if the specified table should not be removed.
    // Match 是过滤入口：先处理大小写，再查缓存，未命中时组合 schema 和 table 两段规则。
    /// 判断指定表是否应保留；先查缓存，未命中则组合库级与表级规则。
    pub fn Match(&self, tb: &Table) -> bool {
        if self.rules.is_none() {
            return true;
        }
        let mut newTb = tb.Clone();
        if !self.caseSensitive {
            newTb.Schema = newTb.Schema.to_lowercase();
            newTb.Name = newTb.Name.to_lowercase();
        }

        let name = newTb.to_string();
        let (mut doAction, exist) = self.c.query(&name);
        if !exist {
            // 库级与表级都通过才算 Do，并写入缓存。
            doAction = self.filterOnSchemas(&newTb) && self.filterOnTables(&newTb);
            self.c.set(newTb.to_string(), doAction);
        }
        doAction == Do
    }

    // filterOnSchemas 对应 Go 的库级规则判定：DoDB 优先，否则 IgnoreDB。
    /// 库级规则判定：有 DoDB 则必须命中；否则命中 IgnoreDB 则拒绝。
    pub fn filterOnSchemas(&self, tb: &Table) -> bool {
        let Some(rules) = self.rules.as_ref() else {
            return true;
        };
        if !rules.DoDBs.is_empty() {
            // not macthed do db rules, ignore update
            if !self.findMatchedDoDBs(tb) {
                return false;
            }
        } else if !rules.IgnoreDBs.is_empty() {
            //  macthed ignore db rules, ignore update
            if self.findMatchedIgnoreDBs(tb) {
                return false;
            }
        }

        true
    }

    // findMatchedDoDBs 保留 Go 的 DoDB 包装方法。
    /// 检查 schema 是否命中 DoDB 规则。
    pub fn findMatchedDoDBs(&self, tb: &Table) -> bool {
        let rules = self.rules.as_ref().expect("rules checked by caller");
        self.matchDB(&rules.DoDBs, &tb.Schema, true)
    }

    // findMatchedIgnoreDBs 保留 Go 的 IgnoreDB 包装方法。
    /// 检查 schema 是否命中 IgnoreDB 规则。
    pub fn findMatchedIgnoreDBs(&self, tb: &Table) -> bool {
        let rules = self.rules.as_ref().expect("rules checked by caller");
        self.matchDB(&rules.IgnoreDBs, &tb.Schema, false)
    }

    // filterOnTables 对应 Go 的表级规则判定；schema statement 没有表名时直接允许。
    /// 表级规则判定；无表名（如 CREATE DATABASE）直接允许。
    pub fn filterOnTables(&self, tb: &Table) -> bool {
        let Some(rules) = self.rules.as_ref() else {
            return true;
        };
        // schema statement like create/drop/alter database
        if tb.Name.is_empty() {
            return true;
        }

        if !rules.DoTables.is_empty() && self.matchTable(&rules.DoTables, tb, true) {
            return true;
        }

        if !rules.IgnoreTables.is_empty() && self.matchTable(&rules.IgnoreTables, tb, false) {
            return false;
        }

        // 无 DoTables 时默认放行（仅被 IgnoreTables 否决）。
        rules.DoTables.is_empty()
    }

    // matchDB 先检查正则 DB 规则，再查 selector 中的普通 DB 规则。
    /// 匹配库名：先扫正则 Do/Ignore 列表，再查 selector 中的 `dbRule`。
    pub fn matchDB(&self, patternDBS: &[String], a: &str, isAllowListCheck: bool) -> bool {
        for b in patternDBS {
            let isRegex = b.starts_with('~');
            if isRegex && self.matchString(&b[1..], a) {
                return true;
            }
            // The selector stores literal paths by UTF-8 bytes but advances by
            // rune width while matching. Preserve Go's exact-string behavior
            // for non-ASCII schema names without changing wildcard handling.
            if !isRegex && !b.contains(['*', '?', '[']) && b == a {
                return true;
            }
        }
        let ruleSet = self.Selector.Match(a, "");
        for r in ruleSet.0 {
            let rule = r
                .downcast_ref::<nodeEndRule>()
                .expect("selector payload is nodeEndRule");
            if rule.kind == dbRule && rule.isAllowList == isAllowListCheck {
                return true;
            }
        }
        false
    }

    // matchTable 保留 Go 四种组合规则：DB/TBL 同为正则、仅 DB 正则、仅 TBL 正则、完整普通规则。
    /// 匹配表：按四种 DB/TBL 正则组合检查规则列表与 selector。
    pub fn matchTable(
        &self,
        patternTBS: &[Box<Table>],
        tb: &Table,
        isAllowListCheck: bool,
    ) -> bool {
        for ptb in patternTBS {
            let dbIsRegex = ptb.Schema.starts_with('~');
            let tblIsRegex = ptb.Name.starts_with('~');
            if !dbIsRegex
                && !tblIsRegex
                && !ptb.Schema.contains(['*', '?', '['])
                && !ptb.Name.contains(['*', '?', '['])
                && ptb.Schema == tb.Schema
                && ptb.Name == tb.Name
            {
                return true;
            }
            if dbIsRegex && tblIsRegex {
                if self.matchString(&ptb.Schema[1..], &tb.Schema)
                    && self.matchString(&ptb.Name[1..], &tb.Name)
                {
                    return true;
                }
            } else if dbIsRegex && !tblIsRegex {
                if !self.matchString(&ptb.Schema[1..], &tb.Schema) {
                    continue;
                }
                let ruleSet = self.Selector.Match(&tb.Name, "");
                for r in ruleSet.0 {
                    let rule = r
                        .downcast_ref::<nodeEndRule>()
                        .expect("selector payload is nodeEndRule");
                    if rule.kind == tblRuleOnlyTblPart && rule.isAllowList == isAllowListCheck {
                        return true;
                    }
                }
            }
            let ruleSet = self.Selector.Match(&tb.Schema, "");
            for r in ruleSet.0 {
                let rule = r
                    .downcast_ref::<nodeEndRule>()
                    .expect("selector payload is nodeEndRule");
                if rule.kind == tblRuleOnlyDBPart
                    && rule.isAllowList == isAllowListCheck
                    && rule.r.as_ref().is_some_and(|re| re.is_match(&tb.Name))
                {
                    return true;
                }
            }
            let ruleSet = self.Selector.Match(&tb.Schema, &tb.Name);
            for r in ruleSet.0 {
                let rule = r
                    .downcast_ref::<nodeEndRule>()
                    .expect("selector payload is nodeEndRule");
                if rule.kind == tblRuleFull && rule.isAllowList == isAllowListCheck {
                    return true;
                }
            }
        }

        false
    }

    // matchString 优先使用 patternMap 中已编译正则；没有正则时退回普通字符串比较。
    /// 用已编译正则匹配；无缓存时退回普通字符串相等比较。
    pub fn matchString(&self, pattern: &str, t: &str) -> bool {
        if let Some(re) = self.patternMap.get(pattern) {
            return re.is_match(t);
        }
        pattern == t
    }
}
