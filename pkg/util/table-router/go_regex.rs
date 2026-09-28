// Copyright 2026 AsterSQL.

//! Translate Go regexp syntax before handing matching to Rust's linear-time
//! regex engine. In particular, Rust's extra flags, Unicode Perl classes and
//! character-class set operations must not change the meaning of a Go pattern.

use regex::{Regex, RegexBuilder};
use regex_syntax::ast::{Ast, RepetitionKind, RepetitionRange};

fn invalid() -> regex::Error {
    regex::Error::Syntax("invalid Go regular expression".into())
}

pub(super) fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    let mut parser = Parser { rest: pattern };
    let translated = parser.translate()?;
    let ast = regex_syntax::ast::parse::ParserBuilder::new()
        .nest_limit(1000)
        .build()
        .parse(&translated)
        .map_err(|_| invalid())?;
    check_repeats(&ast)?;
    RegexBuilder::new(&translated)
        .nest_limit(1000)
        .size_limit(128 << 20)
        .build()
}

// Go checks the product of nested bounded repetitions, not just each bound.
// Validate children before multiplying: even inside {0}, an invalid inner
// repetition must fail before the outer expression is simplified.
fn check_repeats(ast: &Ast) -> Result<u32, regex::Error> {
    let weight = match ast {
        Ast::Repetition(rep) => {
            let inner = check_repeats(&rep.ast)?;
            let count = match rep.op.kind {
                RepetitionKind::Range(RepetitionRange::Exactly(n))
                | RepetitionKind::Range(RepetitionRange::AtLeast(n))
                | RepetitionKind::Range(RepetitionRange::Bounded(_, n)) => n,
                _ => 1,
            };
            inner * count
        }
        Ast::Group(group) => check_repeats(&group.ast)?,
        Ast::Concat(concat) => concat
            .asts
            .iter()
            .map(check_repeats)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .unwrap_or(1),
        Ast::Alternation(alt) => alt
            .asts
            .iter()
            .map(check_repeats)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .unwrap_or(1),
        _ => 1,
    };
    if weight > 1000 {
        Err(invalid())
    } else {
        Ok(weight)
    }
}

struct Parser<'a> {
    rest: &'a str,
}

// A single Unicode scalar or a set. Keeping this distinction is necessary for
// Go's range grammar: [a-z--b] is a-z plus the range '-'..'b', not subtraction.
enum Atom {
    Rune(u32),
    Set(String),
}

fn rune(value: u32) -> String {
    if (0xd800..=0xdfff).contains(&value) {
        "[a&&b]".into()
    } else {
        format!(r"\x{{{value:x}}}")
    }
}

fn range(lo: u32, hi: u32) -> String {
    let mut ranges = String::new();
    for (start, end) in [(lo, hi.min(0xd7ff)), (lo.max(0xe000), hi)] {
        if start <= end {
            ranges.push_str(&format!("{}-{}", rune(start), rune(end)));
        }
    }
    if ranges.is_empty() {
        "[a&&b]".into()
    } else {
        format!("[{ranges}]")
    }
}

// Set flags on atoms rather than emitting standalone flag nodes. Go permits a
// flag-only group between two repeats (a*(?i)*); its second repeat applies to
// the preceding expression, whose case semantics must remain unchanged.
fn emit(out: &mut String, atom: &str, flags: [bool; 4]) {
    if flags[..3].iter().any(|flag| *flag) {
        out.push_str("(?");
        for (idx, ch) in "ims".chars().enumerate() {
            if flags[idx] {
                out.push(ch);
            }
        }
        out.push(':');
        out.push_str(atom);
        out.push(')');
    } else {
        out.push_str(atom);
    }
}

impl Parser<'_> {
    fn take(&mut self) -> Result<char, regex::Error> {
        let ch = self.rest.chars().next().ok_or_else(invalid)?;
        self.rest = &self.rest[ch.len_utf8()..];
        Ok(ch)
    }

    fn translate(&mut self) -> Result<String, regex::Error> {
        let mut out = String::new();
        let mut flags = [false; 4]; // i, m, s, U
        let mut stack = Vec::new();
        let mut last_repeat = false;
        while !self.rest.is_empty() {
            let ch = self.take()?;
            let previous_repeat = last_repeat;
            last_repeat = false;
            match ch {
                '(' if self.rest.starts_with('?') => {
                    self.take()?;
                    if self.rest.starts_with("P<") || self.rest.starts_with('<') {
                        if self.rest.starts_with('P') {
                            self.take()?;
                        }
                        self.take()?;
                        let end = self.rest.find('>').ok_or_else(invalid)?;
                        let name = &self.rest[..end];
                        if name.is_empty()
                            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                        {
                            return Err(invalid());
                        }
                        self.rest = &self.rest[end + 1..];
                        // Names may repeat in Go. Captures here are indexed.
                        stack.push(flags);
                        out.push('(');
                    } else {
                        let saved = flags;
                        let mut negative = false;
                        let mut saw_negative = false;
                        loop {
                            let flag = self.take()?;
                            if flag == '-' {
                                if negative {
                                    return Err(invalid());
                                }
                                negative = true;
                            } else if let Some(idx) = "imsU".find(flag) {
                                flags[idx] = !negative;
                                saw_negative |= negative;
                            } else if matches!(flag, ':' | ')') {
                                if negative && !saw_negative {
                                    return Err(invalid());
                                }
                                if flag == ':' {
                                    stack.push(saved);
                                    out.push_str("(?:");
                                }
                                break;
                            } else {
                                return Err(invalid());
                            }
                        }
                    }
                }
                '(' => {
                    stack.push(flags);
                    out.push('(');
                }
                ')' => {
                    flags = stack.pop().ok_or_else(invalid)?;
                    out.push(')');
                }
                '|' => out.push('|'),
                '*' | '+' | '?' => {
                    if previous_repeat {
                        return Err(invalid());
                    }
                    out.push(ch);
                    self.greediness(&mut out, flags[3])?;
                    last_repeat = true;
                }
                '{' => {
                    // Go treats malformed counts, including leading zeroes,
                    // as literal text. Rust would reject or interpret them.
                    if let Some(end) = self.rest.find('}') {
                        let count = &self.rest[..end];
                        let parts: Vec<_> = count.split(',').collect();
                        let integer = |s: &str| {
                            !s.is_empty()
                                && s.bytes().all(|c| c.is_ascii_digit())
                                && (s.len() == 1 || !s.starts_with('0'))
                        };
                        if (parts.len() == 1 && integer(parts[0]))
                            || (parts.len() == 2
                                && integer(parts[0])
                                && (parts[1].is_empty() || integer(parts[1])))
                        {
                            if previous_repeat {
                                return Err(invalid());
                            }
                            for part in &parts {
                                if !part.is_empty()
                                    && part.parse::<u32>().map_or(true, |n| n > 1000)
                                {
                                    return Err(invalid());
                                }
                            }
                            out.push('{');
                            out.push_str(count);
                            out.push('}');
                            self.rest = &self.rest[end + 1..];
                            self.greediness(&mut out, flags[3])?;
                            last_repeat = true;
                            continue;
                        }
                    }
                    emit(&mut out, &rune('{' as u32), flags);
                }
                '[' => emit(&mut out, &self.class()?, flags),
                '\\' if self.rest.starts_with('Q') => {
                    self.take()?;
                    let end = self.rest.find(r"\E").unwrap_or(self.rest.len());
                    for literal in self.rest[..end].chars() {
                        emit(&mut out, &rune(literal as u32), flags);
                    }
                    self.rest = if end == self.rest.len() {
                        ""
                    } else {
                        &self.rest[end + 2..]
                    };
                }
                '\\' if self.rest.starts_with(['b', 'B', 'A', 'z']) => {
                    let ch = self.take()?;
                    if ch == 'b' || ch == 'B' {
                        emit(&mut out, &format!(r"(?-u:\{ch})"), flags);
                    } else {
                        emit(&mut out, &format!(r"\{ch}"), flags);
                    }
                }
                '\\' => match self.escape()? {
                    Atom::Rune(ch) => emit(&mut out, &rune(ch), flags),
                    Atom::Set(set) => emit(&mut out, &set, flags),
                },
                '.' | '^' | '$' => emit(&mut out, &ch.to_string(), flags),
                ch => emit(&mut out, &rune(ch as u32), flags),
            }
        }
        if !stack.is_empty() {
            return Err(invalid());
        }
        Ok(out)
    }

    fn greediness(&mut self, out: &mut String, ungreedy: bool) -> Result<(), regex::Error> {
        let explicit = self.rest.starts_with('?');
        if explicit {
            self.take()?;
        }
        if explicit != ungreedy {
            out.push('?');
        }
        Ok(())
    }

    fn class(&mut self) -> Result<String, regex::Error> {
        let mut out = String::from("[");
        if self.rest.starts_with('^') {
            self.take()?;
            out.push('^');
        }
        let mut first = true;
        loop {
            if !first && self.rest.starts_with(']') {
                self.take()?;
                break;
            }
            first = false;
            match self.class_atom()? {
                Atom::Set(set) => out.push_str(&set),
                Atom::Rune(lo) => {
                    if self.rest.starts_with('-') && !self.rest.starts_with("-]") {
                        self.take()?;
                        let Atom::Rune(hi) = self.class_atom()? else {
                            return Err(invalid());
                        };
                        if hi < lo {
                            return Err(invalid());
                        }
                        out.push_str(&range(lo, hi));
                    } else {
                        out.push_str(&rune(lo));
                    }
                }
            }
        }
        out.push(']');
        Ok(out)
    }

    fn class_atom(&mut self) -> Result<Atom, regex::Error> {
        if self.rest.starts_with("[:") {
            if let Some(end) = self.rest.find(":]") {
                let class = &self.rest[..end + 2];
                let name = &class[2..class.len() - 2];
                if ![
                    "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower",
                    "print", "punct", "space", "upper", "word", "xdigit",
                ]
                .contains(&name.trim_start_matches('^'))
                {
                    return Err(invalid());
                }
                self.rest = &self.rest[end + 2..];
                return Ok(Atom::Set(format!("[{class}]")));
            }
        }
        match self.take()? {
            '\\' => self.escape(),
            ch => Ok(Atom::Rune(ch as u32)),
        }
    }

    fn escape(&mut self) -> Result<Atom, regex::Error> {
        let ch = self.take()?;
        let set = match ch {
            'd' => Some("[0-9]"),
            'D' => Some("[^0-9]"),
            'w' => Some("[A-Za-z0-9_]"),
            'W' => Some("[^A-Za-z0-9_]"),
            's' => Some(r"[\t\n\f\r ]"),
            'S' => Some(r"[^\t\n\f\r ]"),
            _ => None,
        };
        if let Some(set) = set {
            return Ok(Atom::Set(set.into()));
        }
        let value = match ch {
            'a' => 7,
            'f' => 12,
            't' => 9,
            'n' => 10,
            'r' => 13,
            'v' => 11,
            '0'..='7' => {
                let mut value = ch.to_digit(8).unwrap();
                let mut len = 1;
                while len < 3 && self.rest.starts_with(|c| ('0'..='7').contains(&c)) {
                    value = value * 8 + self.take()?.to_digit(8).unwrap();
                    len += 1;
                }
                if ch != '0' && len == 1 {
                    return Err(invalid());
                }
                value
            }
            'x' => {
                if self.rest.starts_with('{') {
                    self.take()?;
                    let end = self.rest.find('}').ok_or_else(invalid)?;
                    let number = &self.rest[..end];
                    if number.is_empty() || !number.bytes().all(|c| c.is_ascii_hexdigit()) {
                        return Err(invalid());
                    }
                    let value = u32::from_str_radix(number, 16).map_err(|_| invalid())?;
                    self.rest = &self.rest[end + 1..];
                    if value > 0x10ffff {
                        return Err(invalid());
                    }
                    value
                } else {
                    let a = self.take()?.to_digit(16).ok_or_else(invalid)?;
                    a * 16 + self.take()?.to_digit(16).ok_or_else(invalid)?
                }
            }
            'p' | 'P' => {
                let mut negated = ch == 'P';
                let name;
                if self.rest.starts_with('{') {
                    self.take()?;
                    let end = self.rest.find('}').ok_or_else(invalid)?;
                    name = self.rest[..end].to_owned();
                    self.rest = &self.rest[end + 1..];
                } else {
                    name = self.take()?.to_string();
                }
                let name = if let Some(name) = name.strip_prefix('^') {
                    negated = !negated;
                    name
                } else {
                    &name
                };
                let canonical: String = name
                    .chars()
                    .filter(|c| !matches!(c, '_' | '-' | ' '))
                    .map(|c| c.to_ascii_lowercase())
                    .collect();
                let (_, name) = GO_UNICODE_CLASSES
                    .iter()
                    .find(|(key, _)| *key == canonical)
                    .ok_or_else(invalid)?;
                if *name == "Cs" {
                    return Ok(Atom::Set(if negated { r"[\s\S]" } else { "[a&&b]" }.into()));
                }
                return Ok(Atom::Set(format!(
                    r"\{}{{{name}}}",
                    if negated { 'P' } else { 'p' }
                )));
            }
            c if c.is_ascii() && !c.is_ascii_alphanumeric() => c as u32,
            _ => return Err(invalid()),
        };
        Ok(Atom::Rune(value))
    }
}

// Go 1.25 regexp property names, including unicode.CategoryAliases.
const GO_UNICODE_CLASSES: &[(&str, &str)] = &[
    ("adlam", "Adlam"),
    ("ahom", "Ahom"),
    ("anatolianhieroglyphs", "Anatolian_Hieroglyphs"),
    ("any", "Any"),
    ("arabic", "Arabic"),
    ("armenian", "Armenian"),
    ("ascii", "ASCII"),
    ("assigned", "Assigned"),
    ("avestan", "Avestan"),
    ("balinese", "Balinese"),
    ("bamum", "Bamum"),
    ("bassavah", "Bassa_Vah"),
    ("batak", "Batak"),
    ("bengali", "Bengali"),
    ("bhaiksuki", "Bhaiksuki"),
    ("bopomofo", "Bopomofo"),
    ("brahmi", "Brahmi"),
    ("braille", "Braille"),
    ("buginese", "Buginese"),
    ("buhid", "Buhid"),
    ("c", "C"),
    ("canadianaboriginal", "Canadian_Aboriginal"),
    ("carian", "Carian"),
    ("casedletter", "LC"),
    ("caucasianalbanian", "Caucasian_Albanian"),
    ("cc", "Cc"),
    ("cf", "Cf"),
    ("chakma", "Chakma"),
    ("cham", "Cham"),
    ("cherokee", "Cherokee"),
    ("chorasmian", "Chorasmian"),
    ("closepunctuation", "Pe"),
    ("cn", "Cn"),
    ("cntrl", "Cc"),
    ("co", "Co"),
    ("combiningmark", "M"),
    ("common", "Common"),
    ("connectorpunctuation", "Pc"),
    ("control", "Cc"),
    ("coptic", "Coptic"),
    ("cs", "Cs"),
    ("cuneiform", "Cuneiform"),
    ("currencysymbol", "Sc"),
    ("cypriot", "Cypriot"),
    ("cyprominoan", "Cypro_Minoan"),
    ("cyrillic", "Cyrillic"),
    ("dashpunctuation", "Pd"),
    ("decimalnumber", "Nd"),
    ("deseret", "Deseret"),
    ("devanagari", "Devanagari"),
    ("digit", "Nd"),
    ("divesakuru", "Dives_Akuru"),
    ("dogra", "Dogra"),
    ("duployan", "Duployan"),
    ("egyptianhieroglyphs", "Egyptian_Hieroglyphs"),
    ("elbasan", "Elbasan"),
    ("elymaic", "Elymaic"),
    ("enclosingmark", "Me"),
    ("ethiopic", "Ethiopic"),
    ("finalpunctuation", "Pf"),
    ("format", "Cf"),
    ("georgian", "Georgian"),
    ("glagolitic", "Glagolitic"),
    ("gothic", "Gothic"),
    ("grantha", "Grantha"),
    ("greek", "Greek"),
    ("gujarati", "Gujarati"),
    ("gunjalagondi", "Gunjala_Gondi"),
    ("gurmukhi", "Gurmukhi"),
    ("han", "Han"),
    ("hangul", "Hangul"),
    ("hanifirohingya", "Hanifi_Rohingya"),
    ("hanunoo", "Hanunoo"),
    ("hatran", "Hatran"),
    ("hebrew", "Hebrew"),
    ("hiragana", "Hiragana"),
    ("imperialaramaic", "Imperial_Aramaic"),
    ("inherited", "Inherited"),
    ("initialpunctuation", "Pi"),
    ("inscriptionalpahlavi", "Inscriptional_Pahlavi"),
    ("inscriptionalparthian", "Inscriptional_Parthian"),
    ("javanese", "Javanese"),
    ("kaithi", "Kaithi"),
    ("kannada", "Kannada"),
    ("katakana", "Katakana"),
    ("kawi", "Kawi"),
    ("kayahli", "Kayah_Li"),
    ("kharoshthi", "Kharoshthi"),
    ("khitansmallscript", "Khitan_Small_Script"),
    ("khmer", "Khmer"),
    ("khojki", "Khojki"),
    ("khudawadi", "Khudawadi"),
    ("l", "L"),
    ("lao", "Lao"),
    ("latin", "Latin"),
    ("lc", "LC"),
    ("lepcha", "Lepcha"),
    ("letter", "L"),
    ("letternumber", "Nl"),
    ("limbu", "Limbu"),
    ("lineara", "Linear_A"),
    ("linearb", "Linear_B"),
    ("lineseparator", "Zl"),
    ("lisu", "Lisu"),
    ("ll", "Ll"),
    ("lm", "Lm"),
    ("lo", "Lo"),
    ("lowercaseletter", "Ll"),
    ("lt", "Lt"),
    ("lu", "Lu"),
    ("lycian", "Lycian"),
    ("lydian", "Lydian"),
    ("m", "M"),
    ("mahajani", "Mahajani"),
    ("makasar", "Makasar"),
    ("malayalam", "Malayalam"),
    ("mandaic", "Mandaic"),
    ("manichaean", "Manichaean"),
    ("marchen", "Marchen"),
    ("mark", "M"),
    ("masaramgondi", "Masaram_Gondi"),
    ("mathsymbol", "Sm"),
    ("mc", "Mc"),
    ("me", "Me"),
    ("medefaidrin", "Medefaidrin"),
    ("meeteimayek", "Meetei_Mayek"),
    ("mendekikakui", "Mende_Kikakui"),
    ("meroiticcursive", "Meroitic_Cursive"),
    ("meroitichieroglyphs", "Meroitic_Hieroglyphs"),
    ("miao", "Miao"),
    ("mn", "Mn"),
    ("modi", "Modi"),
    ("modifierletter", "Lm"),
    ("modifiersymbol", "Sk"),
    ("mongolian", "Mongolian"),
    ("mro", "Mro"),
    ("multani", "Multani"),
    ("myanmar", "Myanmar"),
    ("n", "N"),
    ("nabataean", "Nabataean"),
    ("nagmundari", "Nag_Mundari"),
    ("nandinagari", "Nandinagari"),
    ("nd", "Nd"),
    ("newa", "Newa"),
    ("newtailue", "New_Tai_Lue"),
    ("nko", "Nko"),
    ("nl", "Nl"),
    ("no", "No"),
    ("nonspacingmark", "Mn"),
    ("number", "N"),
    ("nushu", "Nushu"),
    ("nyiakengpuachuehmong", "Nyiakeng_Puachue_Hmong"),
    ("ogham", "Ogham"),
    ("olchiki", "Ol_Chiki"),
    ("oldhungarian", "Old_Hungarian"),
    ("olditalic", "Old_Italic"),
    ("oldnortharabian", "Old_North_Arabian"),
    ("oldpermic", "Old_Permic"),
    ("oldpersian", "Old_Persian"),
    ("oldsogdian", "Old_Sogdian"),
    ("oldsoutharabian", "Old_South_Arabian"),
    ("oldturkic", "Old_Turkic"),
    ("olduyghur", "Old_Uyghur"),
    ("openpunctuation", "Ps"),
    ("oriya", "Oriya"),
    ("osage", "Osage"),
    ("osmanya", "Osmanya"),
    ("other", "C"),
    ("otherletter", "Lo"),
    ("othernumber", "No"),
    ("otherpunctuation", "Po"),
    ("othersymbol", "So"),
    ("p", "P"),
    ("pahawhhmong", "Pahawh_Hmong"),
    ("palmyrene", "Palmyrene"),
    ("paragraphseparator", "Zp"),
    ("paucinhau", "Pau_Cin_Hau"),
    ("pc", "Pc"),
    ("pd", "Pd"),
    ("pe", "Pe"),
    ("pf", "Pf"),
    ("phagspa", "Phags_Pa"),
    ("phoenician", "Phoenician"),
    ("pi", "Pi"),
    ("po", "Po"),
    ("privateuse", "Co"),
    ("ps", "Ps"),
    ("psalterpahlavi", "Psalter_Pahlavi"),
    ("punct", "P"),
    ("punctuation", "P"),
    ("rejang", "Rejang"),
    ("runic", "Runic"),
    ("s", "S"),
    ("samaritan", "Samaritan"),
    ("saurashtra", "Saurashtra"),
    ("sc", "Sc"),
    ("separator", "Z"),
    ("sharada", "Sharada"),
    ("shavian", "Shavian"),
    ("siddham", "Siddham"),
    ("signwriting", "SignWriting"),
    ("sinhala", "Sinhala"),
    ("sk", "Sk"),
    ("sm", "Sm"),
    ("so", "So"),
    ("sogdian", "Sogdian"),
    ("sorasompeng", "Sora_Sompeng"),
    ("soyombo", "Soyombo"),
    ("spaceseparator", "Zs"),
    ("spacingmark", "Mc"),
    ("sundanese", "Sundanese"),
    ("surrogate", "Cs"),
    ("sylotinagri", "Syloti_Nagri"),
    ("symbol", "S"),
    ("syriac", "Syriac"),
    ("tagalog", "Tagalog"),
    ("tagbanwa", "Tagbanwa"),
    ("taile", "Tai_Le"),
    ("taitham", "Tai_Tham"),
    ("taiviet", "Tai_Viet"),
    ("takri", "Takri"),
    ("tamil", "Tamil"),
    ("tangsa", "Tangsa"),
    ("tangut", "Tangut"),
    ("telugu", "Telugu"),
    ("thaana", "Thaana"),
    ("thai", "Thai"),
    ("tibetan", "Tibetan"),
    ("tifinagh", "Tifinagh"),
    ("tirhuta", "Tirhuta"),
    ("titlecaseletter", "Lt"),
    ("toto", "Toto"),
    ("ugaritic", "Ugaritic"),
    ("unassigned", "Cn"),
    ("uppercaseletter", "Lu"),
    ("vai", "Vai"),
    ("vithkuqi", "Vithkuqi"),
    ("wancho", "Wancho"),
    ("warangciti", "Warang_Citi"),
    ("yezidi", "Yezidi"),
    ("yi", "Yi"),
    ("z", "Z"),
    ("zanabazarsquare", "Zanabazar_Square"),
    ("zl", "Zl"),
    ("zp", "Zp"),
    ("zs", "Zs"),
];
