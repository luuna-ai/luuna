//! Schema rule patterns with the meaning Go's `regexp` package gives them
//! (RE2 syntax with Perl extensions, `regexp.Compile`), run on the `regex`
//! crate (assumption AS8, GD6).
//!
//! The pattern is parsed here with Go's grammar and rewritten into an
//! equivalent `regex` pattern in which nothing depends on the two engines
//! agreeing about syntax:
//!
//! - every literal is written as `\x{..}`, so `regex`'s own escapes (`\<`,
//!   `\b{start}`, `&&` in classes, ...) can never be reached;
//! - `\d`, `\s`, `\w`, their negations and the POSIX classes become explicit
//!   ASCII ranges (Go's classes are ASCII; `\s` is `[\t\n\f\r ]` without
//!   `\v`), and `\b`, `\B` become ASCII word boundaries;
//! - flags are tracked here and applied to each item (`(?i:..)`, `(?s:.)`,
//!   `(?m:^)`); laziness is resolved here, so the `U` flag is never passed on;
//! - octal escapes, `\Q..\E` and Go's literal `{` are expanded.
//!
//! A pattern Go rejects is rejected with Go's error text
//! (`error parsing regexp: <code>: `<expr>``); callers add the
//! `invalid rule pattern: ` prefix. Matching is unanchored, as Go's
//! `MatchString`.
//!
//! Known differences, none reachable by an ordinary rule: Go 1.27 knows the
//! scripts of Unicode 17 and `regex` 1.13 those of Unicode 16, so a pattern
//! naming a script new in Unicode 17 is rejected here, and a character
//! assigned in Unicode 17 may fall into a different class; Go's nesting
//! limit (1000) is applied to an approximation of Go's tree height; more
//! than 20000 nested non-capturing groups (which Go does not limit) are
//! rejected here; and a pattern whose compiled program exceeds the size
//! limit below is rejected here although Go may accept it.

use regex::{Regex, RegexBuilder};
use std::fmt;

/// Go's limit on a repeat count and on nested repeat counts.
const MAX_REPEAT: i64 = 1000;
/// Go's `maxHeight`.
const MAX_HEIGHT: usize = 1000;
/// `regex`'s nesting limit: far above what a pattern within Go's height
/// limit produces after rewriting, and within what [`DEEP_STACK`] holds.
const NEST_LIMIT: u32 = 20_000;
/// `regex`'s limit on the compiled program, raised from its default so that
/// counted repetitions of Unicode classes that Go accepts compile here.
const SIZE_LIMIT: usize = 64 << 20;

/// A Go `regexp/syntax` error: `error parsing regexp: <code>: `<expr>``.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoSyntaxError {
    code: &'static str,
    expr: String,
}

impl fmt::Display for GoSyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error parsing regexp: {}: `{}`", self.code, self.expr)
    }
}

const INVALID_CHAR_RANGE: &str = "invalid character class range";
const INVALID_ESCAPE: &str = "invalid escape sequence";
const INVALID_NAMED_CAPTURE: &str = "invalid named capture";
const INVALID_PERL_OP: &str = "invalid or unsupported Perl syntax";
const INVALID_REPEAT_OP: &str = "invalid nested repetition operator";
const INVALID_REPEAT_SIZE: &str = "invalid repeat count";
const MISSING_BRACKET: &str = "missing closing ]";
const MISSING_PAREN: &str = "missing closing )";
const MISSING_REPEAT_ARGUMENT: &str = "missing argument to repetition operator";
const TRAILING_BACKSLASH: &str = "trailing backslash at end of expression";
const UNEXPECTED_PAREN: &str = "unexpected )";
const NESTING_DEPTH: &str = "expression nests too deeply";
const LARGE: &str = "expression too large";

fn error(code: &'static str, expr: &str) -> GoSyntaxError {
    GoSyntaxError {
        code,
        expr: expr.to_owned(),
    }
}

/// Patterns nested deeper than this are compiled on a helper thread with
/// [`DEEP_STACK`] bytes of stack: `regex` compiles recursively, and Go
/// accepts nesting (999 capture groups, any number of non-capturing groups)
/// that overflows an ordinary thread's stack.
const SHALLOW_DEPTH: usize = 64;
const DEEP_STACK: usize = 256 << 20;

/// Compiles `pattern` as Go's `regexp.Compile` would; the error text is
/// Go's for every pattern Go rejects.
pub(crate) fn compile(pattern: &str) -> Result<Regex, String> {
    let translated = translate(pattern).map_err(|error| error.to_string())?;
    let build = move |text: String| {
        RegexBuilder::new(&text)
            .nest_limit(NEST_LIMIT)
            .size_limit(SIZE_LIMIT)
            .build()
    };
    let built = if translated.depth <= SHALLOW_DEPTH {
        build(translated.text)
    } else {
        let text = translated.text;
        std::thread::Builder::new()
            .name("luuna-beacon-regex".to_owned())
            .stack_size(DEEP_STACK)
            .spawn(move || build(text))
            .map_err(|spawn| format!("error parsing regexp: {LARGE}: `{pattern}`: {spawn}"))?
            .join()
            .map_err(|_| error(LARGE, pattern).to_string())?
    };
    built.map_err(|engine| match engine {
        regex::Error::CompiledTooBig(_) => error(LARGE, pattern).to_string(),
        other => format!(
            "error parsing regexp: not supported by this implementation: `{pattern}`: {}",
            other.to_string().lines().last().unwrap_or_default()
        ),
    })
}

// ---- character data ----------------------------------------------------------

type Ranges = &'static [(u32, u32)];

const DIGIT: Ranges = &[(0x30, 0x39)];
/// Go's `\s`: `[\t\n\f\r ]`.
const PERL_SPACE: Ranges = &[(0x09, 0x0a), (0x0c, 0x0d), (0x20, 0x20)];
const WORD: Ranges = &[(0x30, 0x39), (0x41, 0x5a), (0x5f, 0x5f), (0x61, 0x7a)];

/// The Perl classes Go knows: `\d \D \s \S \w \W`.
fn perl_class(letter: char) -> Option<(bool, Ranges)> {
    match letter {
        'd' => Some((false, DIGIT)),
        'D' => Some((true, DIGIT)),
        's' => Some((false, PERL_SPACE)),
        'S' => Some((true, PERL_SPACE)),
        'w' => Some((false, WORD)),
        'W' => Some((true, WORD)),
        _ => None,
    }
}

/// The POSIX classes Go knows, by their bracketed name without `^`.
fn posix_class(name: &str) -> Option<Ranges> {
    Some(match name {
        "alnum" => &[(0x30, 0x39), (0x41, 0x5a), (0x61, 0x7a)],
        "alpha" => &[(0x41, 0x5a), (0x61, 0x7a)],
        "ascii" => &[(0x00, 0x7f)],
        "blank" => &[(0x09, 0x09), (0x20, 0x20)],
        "cntrl" => &[(0x00, 0x1f), (0x7f, 0x7f)],
        "digit" => DIGIT,
        "graph" => &[(0x21, 0x7e)],
        "lower" => &[(0x61, 0x7a)],
        "print" => &[(0x20, 0x7e)],
        "punct" => &[(0x21, 0x2f), (0x3a, 0x40), (0x5b, 0x60), (0x7b, 0x7e)],
        "space" => &[(0x09, 0x0d), (0x20, 0x20)],
        "upper" => &[(0x41, 0x5a)],
        "word" => WORD,
        "xdigit" => &[(0x30, 0x39), (0x41, 0x46), (0x61, 0x66)],
        _ => return None,
    })
}

/// Unicode general categories by their short names (Unicode's own names).
const CATEGORIES: &[&str] = &[
    "C", "Cc", "Cf", "Cn", "Co", "Cs", "L", "LC", "Ll", "Lm", "Lo", "Lt", "Lu", "M", "Mc", "Me",
    "Mn", "N", "Nd", "Nl", "No", "P", "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "S", "Sc", "Sk",
    "Sm", "So", "Z", "Zl", "Zp", "Zs",
];

/// The long names of the general categories (Unicode's
/// `PropertyValueAliases.txt`) and the three POSIX-style aliases Go accepts.
const CATEGORY_ALIASES: &[(&str, &str)] = &[
    ("Cased_Letter", "LC"),
    ("Close_Punctuation", "Pe"),
    ("Combining_Mark", "M"),
    ("Connector_Punctuation", "Pc"),
    ("Control", "Cc"),
    ("Currency_Symbol", "Sc"),
    ("Dash_Punctuation", "Pd"),
    ("Decimal_Number", "Nd"),
    ("Enclosing_Mark", "Me"),
    ("Final_Punctuation", "Pf"),
    ("Format", "Cf"),
    ("Initial_Punctuation", "Pi"),
    ("Letter", "L"),
    ("Letter_Number", "Nl"),
    ("Line_Separator", "Zl"),
    ("Lowercase_Letter", "Ll"),
    ("Mark", "M"),
    ("Math_Symbol", "Sm"),
    ("Modifier_Letter", "Lm"),
    ("Modifier_Symbol", "Sk"),
    ("Nonspacing_Mark", "Mn"),
    ("Number", "N"),
    ("Open_Punctuation", "Ps"),
    ("Other", "C"),
    ("Other_Letter", "Lo"),
    ("Other_Number", "No"),
    ("Other_Punctuation", "Po"),
    ("Other_Symbol", "So"),
    ("Paragraph_Separator", "Zp"),
    ("Private_Use", "Co"),
    ("Punctuation", "P"),
    ("Separator", "Z"),
    ("Space_Separator", "Zs"),
    ("Spacing_Mark", "Mc"),
    ("Surrogate", "Cs"),
    ("Symbol", "S"),
    ("Titlecase_Letter", "Lt"),
    ("Unassigned", "Cn"),
    ("Uppercase_Letter", "Lu"),
    ("cntrl", "Cc"),
    ("digit", "Nd"),
    ("punct", "P"),
];

/// Unicode 17 script names, as Go 1.27 knows them.
const SCRIPTS: &[&str] = &[
    "Adlam",
    "Ahom",
    "Anatolian_Hieroglyphs",
    "Arabic",
    "Armenian",
    "Avestan",
    "Balinese",
    "Bamum",
    "Bassa_Vah",
    "Batak",
    "Bengali",
    "Beria_Erfe",
    "Bhaiksuki",
    "Bopomofo",
    "Brahmi",
    "Braille",
    "Buginese",
    "Buhid",
    "Canadian_Aboriginal",
    "Carian",
    "Caucasian_Albanian",
    "Chakma",
    "Cham",
    "Cherokee",
    "Chorasmian",
    "Common",
    "Coptic",
    "Cuneiform",
    "Cypriot",
    "Cypro_Minoan",
    "Cyrillic",
    "Deseret",
    "Devanagari",
    "Dives_Akuru",
    "Dogra",
    "Duployan",
    "Egyptian_Hieroglyphs",
    "Elbasan",
    "Elymaic",
    "Ethiopic",
    "Garay",
    "Georgian",
    "Glagolitic",
    "Gothic",
    "Grantha",
    "Greek",
    "Gujarati",
    "Gunjala_Gondi",
    "Gurmukhi",
    "Gurung_Khema",
    "Han",
    "Hangul",
    "Hanifi_Rohingya",
    "Hanunoo",
    "Hatran",
    "Hebrew",
    "Hiragana",
    "Imperial_Aramaic",
    "Inherited",
    "Inscriptional_Pahlavi",
    "Inscriptional_Parthian",
    "Javanese",
    "Kaithi",
    "Kannada",
    "Katakana",
    "Kawi",
    "Kayah_Li",
    "Kharoshthi",
    "Khitan_Small_Script",
    "Khmer",
    "Khojki",
    "Khudawadi",
    "Kirat_Rai",
    "Lao",
    "Latin",
    "Lepcha",
    "Limbu",
    "Linear_A",
    "Linear_B",
    "Lisu",
    "Lycian",
    "Lydian",
    "Mahajani",
    "Makasar",
    "Malayalam",
    "Mandaic",
    "Manichaean",
    "Marchen",
    "Masaram_Gondi",
    "Medefaidrin",
    "Meetei_Mayek",
    "Mende_Kikakui",
    "Meroitic_Cursive",
    "Meroitic_Hieroglyphs",
    "Miao",
    "Modi",
    "Mongolian",
    "Mro",
    "Multani",
    "Myanmar",
    "Nabataean",
    "Nag_Mundari",
    "Nandinagari",
    "New_Tai_Lue",
    "Newa",
    "Nko",
    "Nushu",
    "Nyiakeng_Puachue_Hmong",
    "Ogham",
    "Ol_Chiki",
    "Ol_Onal",
    "Old_Hungarian",
    "Old_Italic",
    "Old_North_Arabian",
    "Old_Permic",
    "Old_Persian",
    "Old_Sogdian",
    "Old_South_Arabian",
    "Old_Turkic",
    "Old_Uyghur",
    "Oriya",
    "Osage",
    "Osmanya",
    "Pahawh_Hmong",
    "Palmyrene",
    "Pau_Cin_Hau",
    "Phags_Pa",
    "Phoenician",
    "Psalter_Pahlavi",
    "Rejang",
    "Runic",
    "Samaritan",
    "Saurashtra",
    "Sharada",
    "Shavian",
    "Siddham",
    "Sidetic",
    "SignWriting",
    "Sinhala",
    "Sogdian",
    "Sora_Sompeng",
    "Soyombo",
    "Sundanese",
    "Sunuwar",
    "Syloti_Nagri",
    "Syriac",
    "Tagalog",
    "Tagbanwa",
    "Tai_Le",
    "Tai_Tham",
    "Tai_Viet",
    "Tai_Yo",
    "Takri",
    "Tamil",
    "Tangsa",
    "Tangut",
    "Telugu",
    "Thaana",
    "Thai",
    "Tibetan",
    "Tifinagh",
    "Tirhuta",
    "Todhri",
    "Tolong_Siki",
    "Toto",
    "Tulu_Tigalari",
    "Ugaritic",
    "Vai",
    "Vithkuqi",
    "Wancho",
    "Warang_Citi",
    "Yezidi",
    "Yi",
    "Zanabazar_Square",
];

/// The lookup form of a property name: separators (`_`, `-`, space)
/// dropped, the first remaining ASCII letter upper case, later ones lower
/// case.
fn canonical_name(name: &str) -> String {
    let mut out = Vec::with_capacity(name.len());
    let mut first = true;
    for &byte in name.as_bytes() {
        match byte {
            b'_' | b'-' | b' ' => continue,
            _ if first => {
                first = false;
                out.push(byte.to_ascii_uppercase());
            }
            _ => out.push(byte.to_ascii_lowercase()),
        }
    }
    String::from_utf8(out).expect("only ASCII bytes were changed")
}

/// The `regex` property for a Go `\p` name, and whether Go inverts it.
fn unicode_property(name: &str) -> Option<(&'static str, bool)> {
    let wanted = canonical_name(name);
    match wanted.as_str() {
        "Any" => return Some(("Any", false)),
        "Assigned" => return Some(("Cn", true)),
        "Ascii" => return Some(("ASCII", false)),
        "Lc" => return Some(("LC", false)),
        _ => {}
    }
    if let Some(found) = CATEGORIES.iter().find(|key| **key == wanted) {
        return Some((found, false));
    }
    if let Some(found) = SCRIPTS.iter().find(|key| **key == wanted) {
        return Some((found, false));
    }
    if let Some((_, actual)) = CATEGORY_ALIASES
        .iter()
        .find(|(alias, _)| canonical_name(alias) == wanted)
    {
        return Some((actual, false));
    }
    SCRIPTS
        .iter()
        .find(|script| canonical_name(script) == wanted)
        .map(|script| (*script, false))
}

// ---- rewriting -----------------------------------------------------------------

/// A pattern that matches nothing: a character outside every scalar value
/// (Go accepts surrogate code points in escapes; no `&str` contains one).
const NOTHING: &str = r"[^\x{0}-\x{10FFFF}]";

fn is_surrogate(code: u32) -> bool {
    (0xd800..=0xdfff).contains(&code)
}

/// One literal code point.
fn literal(code: u32) -> String {
    if is_surrogate(code) {
        NOTHING.to_owned()
    } else {
        format!("\\x{{{code:X}}}")
    }
}

/// Appends `lo-hi` to a class body, leaving out surrogates; returns whether
/// anything was appended.
fn push_range(body: &mut String, lo: u32, hi: u32) -> bool {
    let mut pieces = Vec::new();
    if lo < 0xd800 {
        pieces.push((lo, hi.min(0xd7ff)));
    }
    if hi > 0xdfff {
        pieces.push((lo.max(0xe000), hi));
    }
    for (from, to) in &pieces {
        if from == to {
            body.push_str(&format!("\\x{{{from:X}}}"));
        } else {
            body.push_str(&format!("\\x{{{from:X}}}-\\x{{{to:X}}}"));
        }
    }
    !pieces.is_empty()
}

/// A class item from fixed ranges: inline when positive, nested when
/// negated.
fn ranges_item(negated: bool, ranges: Ranges) -> String {
    let mut body = String::new();
    for &(lo, hi) in ranges {
        push_range(&mut body, lo, hi);
    }
    if negated {
        format!("[^{body}]")
    } else {
        body
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    fold: bool,
    multi_line: bool,
    dot_nl: bool,
    ungreedy: bool,
}

/// A counted repetition inside an item, for Go's nested-count check.
struct Repeat {
    min: i64,
    max: i64,
    inner: Vec<Repeat>,
}

/// Go's `repeatIsValid`: the product of nested counts stays within `n`.
fn repeats_valid(repeats: &[Repeat], n: i64) -> bool {
    repeats.iter().all(|repeat| {
        if repeat.max == 0 {
            return true;
        }
        let bound = if repeat.max < 0 {
            repeat.min
        } else {
            repeat.max
        };
        if bound > n {
            return false;
        }
        let inner = if bound > 0 { n / bound } else { n };
        repeats_valid(&repeat.inner, inner)
    })
}

/// The last complete item of a group, which a repetition applies to.
struct Item {
    out_start: usize,
    repeated: bool,
    repeats: Vec<Repeat>,
    height: usize,
}

/// An open group.
struct Frame {
    flags: Flags,
    capture: bool,
    /// Where the group's opening was written in the output.
    out_open: usize,
    repeats: Vec<Repeat>,
    height: usize,
    /// Items in the current branch, and whether the group is more than one
    /// item (a concatenation or an alternation adds a level in Go's tree).
    branch_items: usize,
    compound: bool,
    last: Option<Item>,
}

impl Frame {
    fn new(flags: Flags, capture: bool, out_open: usize) -> Self {
        Frame {
            flags,
            capture,
            out_open,
            repeats: Vec::new(),
            height: 0,
            branch_items: 0,
            compound: false,
            last: None,
        }
    }

    fn finish_last(&mut self) {
        if let Some(item) = self.last.take() {
            self.repeats.extend(item.repeats);
            self.height = self.height.max(item.height);
        }
    }

    /// The height of the group's content in Go's tree.
    fn content_height(&self) -> usize {
        self.height + usize::from(self.compound)
    }
}

struct Translator<'p> {
    pattern: &'p str,
    bytes: &'p [u8],
    pos: usize,
    out: String,
    frames: Vec<Frame>,
    /// The deepest nesting seen: open groups plus stacked repetitions.
    depth: usize,
}

/// A rewritten pattern and how deeply it nests.
pub(crate) struct Translated {
    pub(crate) text: String,
    pub(crate) depth: usize,
}

/// Rewrites a Go pattern into a `regex` pattern with the same meaning.
pub(crate) fn translate(pattern: &str) -> Result<Translated, GoSyntaxError> {
    let mut translator = Translator {
        pattern,
        bytes: pattern.as_bytes(),
        pos: 0,
        out: String::with_capacity(pattern.len() * 4),
        frames: vec![Frame::new(Flags::default(), false, 0)],
        depth: 0,
    };
    translator.run()?;
    Ok(Translated {
        text: translator.out,
        depth: translator.depth,
    })
}

impl Translator<'_> {
    fn frame(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .expect("the top-level frame is never popped")
    }

    fn flags(&self) -> Flags {
        self.frames.last().expect("a frame").flags
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.pattern[at..].chars().next()
    }

    fn whole(&self) -> GoSyntaxError {
        error(NESTING_DEPTH, self.pattern)
    }

    /// Ends the previous item and starts a new one with `text`.
    fn push_item(&mut self, text: &str, height: usize) -> Result<(), GoSyntaxError> {
        if height > MAX_HEIGHT {
            return Err(self.whole());
        }
        let out_start = self.out.len();
        self.out.push_str(text);
        let frame = self.frame();
        frame.finish_last();
        frame.branch_items += 1;
        if frame.branch_items > 1 {
            frame.compound = true;
        }
        frame.last = Some(Item {
            out_start,
            repeated: false,
            repeats: Vec::new(),
            height,
        });
        Ok(())
    }

    /// An item that case folding applies to.
    fn push_folded(&mut self, text: &str) -> Result<(), GoSyntaxError> {
        if self.flags().fold {
            self.push_item(&format!("(?i:{text})"), 1)
        } else {
            self.push_item(text, 1)
        }
    }

    fn run(&mut self) -> Result<(), GoSyntaxError> {
        // The start of the previous token when it was a repetition operator.
        let mut last_repeat: Option<usize> = None;
        while self.pos < self.bytes.len() {
            let start = self.pos;
            let mut this_repeat = None;
            match self.bytes[start] {
                b'(' => {
                    if self.bytes.get(start + 1) == Some(&b'?') {
                        self.group_with_flags()?;
                    } else {
                        self.open(self.flags(), true, "(");
                        self.pos += 1;
                    }
                }
                b'|' => {
                    let frame = self.frame();
                    frame.finish_last();
                    frame.branch_items = 0;
                    frame.compound = true;
                    self.out.push('|');
                    self.pos += 1;
                }
                b')' => {
                    self.close()?;
                    self.pos += 1;
                }
                b'^' => {
                    let text = if self.flags().multi_line {
                        "(?m:^)"
                    } else {
                        r"(?:\A)"
                    };
                    self.push_item(text, 1)?;
                    self.pos += 1;
                }
                b'$' => {
                    let text = if self.flags().multi_line {
                        "(?m:$)"
                    } else {
                        r"(?:\z)"
                    };
                    self.push_item(text, 1)?;
                    self.pos += 1;
                }
                b'.' => {
                    let text = if self.flags().dot_nl { "(?s:.)" } else { "." };
                    self.push_item(text, 1)?;
                    self.pos += 1;
                }
                b'[' => self.class()?,
                b'*' | b'+' | b'?' => {
                    let op = match self.bytes[start] {
                        b'*' => "*",
                        b'+' => "+",
                        _ => "?",
                    };
                    self.repeat(start, start + 1, op, None, last_repeat)?;
                    this_repeat = Some(start);
                }
                b'{' => match self.counted(start) {
                    Some((min, max, end)) => {
                        if !(0..=MAX_REPEAT).contains(&min)
                            || max > MAX_REPEAT
                            || (max >= 0 && min > max)
                        {
                            return Err(error(INVALID_REPEAT_SIZE, &self.pattern[start..end]));
                        }
                        let op = if max == min {
                            format!("{{{min}}}")
                        } else if max < 0 {
                            format!("{{{min},}}")
                        } else {
                            format!("{{{min},{max}}}")
                        };
                        self.repeat(start, end, &op, Some((min, max)), last_repeat)?;
                        this_repeat = Some(start);
                    }
                    None => {
                        self.push_folded(&literal(u32::from('{')))?;
                        self.pos += 1;
                    }
                },
                b'\\' => self.escape()?,
                _ => {
                    let character = self.char_at(start).expect("a character");
                    self.push_folded(&literal(u32::from(character)))?;
                    self.pos += character.len_utf8();
                }
            }
            last_repeat = this_repeat;
        }
        if self.frames.len() > 1 {
            return Err(error(MISSING_PAREN, self.pattern));
        }
        Ok(())
    }

    fn open(&mut self, flags: Flags, capture: bool, text: &str) {
        self.frame().finish_last();
        let out_open = self.out.len();
        self.out.push_str(text);
        self.frames.push(Frame::new(flags, capture, out_open));
        self.depth = self.depth.max(self.frames.len());
    }

    fn close(&mut self) -> Result<(), GoSyntaxError> {
        if self.frames.len() == 1 {
            return Err(error(UNEXPECTED_PAREN, self.pattern));
        }
        let mut frame = self.frames.pop().expect("an open group");
        frame.finish_last();
        let height = frame.content_height() + usize::from(frame.capture);
        if height > MAX_HEIGHT {
            return Err(self.whole());
        }
        let out_start = frame.out_open;
        self.out.push(')');
        let parent = self.frame();
        parent.branch_items += 1;
        if parent.branch_items > 1 {
            parent.compound = true;
        }
        parent.last = Some(Item {
            out_start,
            repeated: false,
            repeats: frame.repeats,
            height,
        });
        Ok(())
    }

    /// `(?P<name>`, `(?<name>`, `(?flags)` or `(?flags:`.
    fn group_with_flags(&mut self) -> Result<(), GoSyntaxError> {
        let start = self.pos;
        let rest = &self.pattern[start..];
        let bytes = rest.as_bytes();
        let with_p = bytes.len() > 4 && bytes[2] == b'P' && bytes[3] == b'<';
        let bare = bytes.len() > 3 && bytes[2] == b'<';
        if with_p || bare {
            let name_start = if with_p { 4 } else { 3 };
            let Some(end) = rest.find('>') else {
                return Err(error(INVALID_NAMED_CAPTURE, rest));
            };
            let capture = &rest[..=end];
            let name = &rest[name_start..end];
            if name.is_empty() || !name.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric()) {
                return Err(error(INVALID_NAMED_CAPTURE, capture));
            }
            self.open(self.flags(), true, "(");
            self.pos = start + end + 1;
            return Ok(());
        }
        let mut flags = self.flags();
        let mut negative = false;
        let mut saw_flag = false;
        let mut at = start + 2;
        while let Some(character) = self.char_at(at) {
            at += character.len_utf8();
            let set = !negative;
            match character {
                'i' => flags.fold = set,
                'm' => flags.multi_line = set,
                's' => flags.dot_nl = set,
                'U' => flags.ungreedy = set,
                '-' if !negative => {
                    negative = true;
                    saw_flag = false;
                    continue;
                }
                ':' | ')' if !negative || saw_flag => {
                    if character == ':' {
                        self.open(flags, false, "(?:");
                    } else {
                        // Flags change for the rest of the enclosing group;
                        // the previous item stays the one a repetition takes.
                        self.frame().flags = flags;
                    }
                    self.pos = at;
                    return Ok(());
                }
                _ => return Err(error(INVALID_PERL_OP, &self.pattern[start..at])),
            }
            saw_flag = true;
        }
        Err(error(INVALID_PERL_OP, rest))
    }

    /// Applies a repetition operator spanning `start..end` (before any lazy
    /// `?`) to the previous item.
    fn repeat(
        &mut self,
        start: usize,
        end: usize,
        op: &str,
        counted: Option<(i64, i64)>,
        last_repeat: Option<usize>,
    ) -> Result<(), GoSyntaxError> {
        let mut after = end;
        let mut lazy = false;
        if self.bytes.get(after) == Some(&b'?') {
            after += 1;
            lazy = true;
        }
        if let Some(previous) = last_repeat {
            return Err(error(INVALID_REPEAT_OP, &self.pattern[previous..after]));
        }
        let ungreedy = self.flags().ungreedy;
        let Some(mut item) = self.frame().last.take() else {
            return Err(error(MISSING_REPEAT_ARGUMENT, &self.pattern[start..after]));
        };
        if item.repeated {
            self.out.insert_str(item.out_start, "(?:");
            self.out.push(')');
        }
        self.out.push_str(op);
        if lazy != ungreedy {
            self.out.push('?');
        }
        item.repeated = true;
        item.height += 1;
        self.depth = self.depth.max(self.frames.len() + item.height);
        if item.height > MAX_HEIGHT {
            return Err(self.whole());
        }
        if let Some((min, max)) = counted {
            let inner = std::mem::take(&mut item.repeats);
            item.repeats = vec![Repeat { min, max, inner }];
            if (min >= 2 || max >= 2) && !repeats_valid(&item.repeats, MAX_REPEAT) {
                return Err(error(INVALID_REPEAT_SIZE, &self.pattern[start..end]));
            }
        }
        self.frame().last = Some(item);
        self.pos = after;
        Ok(())
    }

    /// A decimal count as Go reads it: no leading zero, and a value that
    /// reaches 10^8 becomes -1.
    fn count(&self, mut at: usize) -> Option<(i64, usize)> {
        let first = *self.bytes.get(at)?;
        if !first.is_ascii_digit() {
            return None;
        }
        if first == b'0' && self.bytes.get(at + 1).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        let mut value: i64 = 0;
        while let Some(digit) = self.bytes.get(at).filter(|b| b.is_ascii_digit()) {
            if value >= 0 {
                value = if value >= 100_000_000 {
                    -1
                } else {
                    value * 10 + i64::from(digit - b'0')
                };
            }
            at += 1;
        }
        Some((value, at))
    }

    /// `{n}`, `{n,}` or `{n,m}` at `start`, as `(min, max, end)` with
    /// `max == -1` for no upper bound; `None` when `{` is a literal.
    fn counted(&self, start: usize) -> Option<(i64, i64, usize)> {
        let (mut min, mut at) = self.count(start + 1)?;
        let max;
        if *self.bytes.get(at)? == b',' {
            at += 1;
            if *self.bytes.get(at)? == b'}' {
                max = -1;
            } else {
                let (value, next) = self.count(at)?;
                at = next;
                max = value;
                if value < 0 {
                    min = -1;
                }
            }
        } else {
            max = min;
        }
        if *self.bytes.get(at)? != b'}' {
            return None;
        }
        Some((min, max, at + 1))
    }

    /// A backslash outside a class.
    fn escape(&mut self) -> Result<(), GoSyntaxError> {
        let start = self.pos;
        if let Some(&next) = self.bytes.get(start + 1) {
            let assertion = match next {
                b'A' => Some(r"(?:\A)"),
                b'z' => Some(r"(?:\z)"),
                b'b' => Some(r"(?-u:\b)"),
                b'B' => Some(r"(?-u:\B)"),
                _ => None,
            };
            if let Some(text) = assertion {
                self.push_item(text, 1)?;
                self.pos += 2;
                return Ok(());
            }
            if next == b'C' {
                return Err(error(INVALID_ESCAPE, &self.pattern[start..start + 2]));
            }
            if next == b'Q' {
                let body_start = start + 2;
                let (body, resume) = match self.pattern[body_start..].find(r"\E") {
                    Some(end) => (
                        &self.pattern[body_start..body_start + end],
                        body_start + end + 2,
                    ),
                    None => (&self.pattern[body_start..], self.bytes.len()),
                };
                for character in body.chars() {
                    self.push_folded(&literal(u32::from(character)))?;
                }
                self.pos = resume;
                return Ok(());
            }
            if next == b'p' || next == b'P' {
                let (text, end) = self.unicode_class(start)?;
                self.push_folded(&text)?;
                self.pos = end;
                return Ok(());
            }
            if let Some((negated, ranges)) = perl_class(char::from(next)) {
                let body = ranges_item(false, ranges);
                let text = if negated {
                    format!("[^{body}]")
                } else {
                    format!("[{body}]")
                };
                self.push_folded(&text)?;
                self.pos += 2;
                return Ok(());
            }
        }
        let (code, end) = self.escaped_char(start)?;
        self.push_folded(&literal(code))?;
        self.pos = end;
        Ok(())
    }

    /// `\pX`, `\p{Name}`, `\PX`, `\P{Name}` or `\p{^Name}` at `start`, as a
    /// `regex` class item.
    fn unicode_class(&self, start: usize) -> Result<(String, usize), GoSyntaxError> {
        let mut negated = self.bytes[start + 1] == b'P';
        let after = start + 2;
        let (sequence_end, name) = match self.char_at(after) {
            None => (after, ""),
            Some('{') => {
                let Some(close) = self.pattern[start..].find('}') else {
                    return Err(error(INVALID_CHAR_RANGE, &self.pattern[start..]));
                };
                (start + close + 1, &self.pattern[start + 3..start + close])
            }
            Some(character) => {
                let end = after + character.len_utf8();
                (end, &self.pattern[after..end])
            }
        };
        let name = match name.strip_prefix('^') {
            Some(rest) => {
                negated = !negated;
                rest
            }
            None => name,
        };
        let Some((property, inverted)) = unicode_property(name) else {
            return Err(error(
                INVALID_CHAR_RANGE,
                &self.pattern[start..sequence_end],
            ));
        };
        let letter = if negated != inverted { 'P' } else { 'p' };
        Ok((format!("\\{letter}{{{property}}}"), sequence_end))
    }

    /// A single escaped character at `start` (Go's `parseEscape`), as a
    /// code point (possibly a surrogate) and the position after it.
    fn escaped_char(&self, start: usize) -> Result<(u32, usize), GoSyntaxError> {
        let mut at = start + 1;
        let Some(character) = self.char_at(at) else {
            return Err(error(TRAILING_BACKSLASH, ""));
        };
        at += character.len_utf8();
        let is_octal = |at: usize| {
            self.bytes
                .get(at)
                .is_some_and(|b| (b'0'..=b'7').contains(b))
        };
        let invalid = |at: usize| error(INVALID_ESCAPE, &self.pattern[start..at]);
        match character {
            '1'..='7' if !is_octal(at) => Err(invalid(at)),
            '0'..='7' => {
                let mut value = u32::from(character) - u32::from('0');
                for _ in 1..3 {
                    if !is_octal(at) {
                        break;
                    }
                    value = value * 8 + u32::from(self.bytes[at] - b'0');
                    at += 1;
                }
                Ok((value, at))
            }
            'x' => {
                let Some(first) = self.char_at(at) else {
                    return Err(invalid(at));
                };
                at += first.len_utf8();
                if first == '{' {
                    let mut value: u32 = 0;
                    let mut digits = 0;
                    loop {
                        let Some(next) = self.char_at(at) else {
                            return Err(invalid(at));
                        };
                        at += next.len_utf8();
                        if next == '}' {
                            break;
                        }
                        let Some(digit) = next.to_digit(16) else {
                            return Err(invalid(at));
                        };
                        value = value * 16 + digit;
                        if value > 0x10_ffff {
                            return Err(invalid(at));
                        }
                        digits += 1;
                    }
                    if digits == 0 {
                        return Err(invalid(at));
                    }
                    return Ok((value, at));
                }
                let second = self.char_at(at);
                if let Some(second) = second {
                    at += second.len_utf8();
                }
                match (first.to_digit(16), second.and_then(|c| c.to_digit(16))) {
                    (Some(high), Some(low)) => Ok((high * 16 + low, at)),
                    _ => Err(invalid(at)),
                }
            }
            'a' => Ok((0x07, at)),
            'f' => Ok((0x0c, at)),
            'n' => Ok((0x0a, at)),
            'r' => Ok((0x0d, at)),
            't' => Ok((0x09, at)),
            'v' => Ok((0x0b, at)),
            _ if character.is_ascii() && !character.is_ascii_alphanumeric() => {
                Ok((u32::from(character), at))
            }
            _ => Err(invalid(at)),
        }
    }

    /// A bracketed class at `self.pos`.
    fn class(&mut self) -> Result<(), GoSyntaxError> {
        let start = self.pos;
        let missing = || error(MISSING_BRACKET, &self.pattern[start..]);
        let mut at = start + 1;
        let negated = self.bytes.get(at) == Some(&b'^');
        if negated {
            at += 1;
        }
        let mut body = String::new();
        let mut items = 0usize;
        let mut first = true;
        loop {
            let Some(&next) = self.bytes.get(at) else {
                return Err(missing());
            };
            if next == b']' && !first {
                break;
            }
            first = false;
            let rest = &self.pattern[at..];
            if rest.len() > 2 && rest.starts_with("[:") {
                if let Some(close) = rest[2..].find(":]") {
                    let whole = &rest[..close + 4];
                    let inner = &whole[2..whole.len() - 2];
                    let (negative, name) = match inner.strip_prefix('^') {
                        Some(name) => (true, name),
                        None => (false, inner),
                    };
                    let Some(ranges) = posix_class(name) else {
                        return Err(error(INVALID_CHAR_RANGE, whole));
                    };
                    body.push_str(&ranges_item(negative, ranges));
                    items += 1;
                    at += whole.len();
                    continue;
                }
            }
            if next == b'\\' && matches!(self.bytes.get(at + 1), Some(b'p' | b'P')) {
                let (text, end) = self.unicode_class(at)?;
                body.push_str(&text);
                items += 1;
                at = end;
                continue;
            }
            if next == b'\\' {
                if let Some((negative, ranges)) = self
                    .bytes
                    .get(at + 1)
                    .and_then(|b| perl_class(char::from(*b)))
                {
                    body.push_str(&ranges_item(negative, ranges));
                    items += 1;
                    at += 2;
                    continue;
                }
            }
            let range_start = at;
            let (lo, after_lo) = self.class_char(at, start)?;
            at = after_lo;
            let mut hi = lo;
            if self.bytes.get(at) == Some(&b'-')
                && self.bytes.get(at + 1).is_some_and(|b| *b != b']')
            {
                let (value, after_hi) = self.class_char(at + 1, start)?;
                at = after_hi;
                hi = value;
                if hi < lo {
                    return Err(error(INVALID_CHAR_RANGE, &self.pattern[range_start..at]));
                }
            }
            if push_range(&mut body, lo, hi) {
                items += 1;
            }
        }
        self.pos = at + 1;
        let text = match (items, negated) {
            (0, false) => NOTHING.to_owned(),
            (0, true) => r"[\x{0}-\x{10FFFF}]".to_owned(),
            (_, false) => format!("[{body}]"),
            (_, true) => format!("[^{body}]"),
        };
        self.push_folded(&text)
    }

    /// One class character at `at` (Go's `parseClassChar`).
    fn class_char(&self, at: usize, class_start: usize) -> Result<(u32, usize), GoSyntaxError> {
        match self.char_at(at) {
            None => Err(error(MISSING_BRACKET, &self.pattern[class_start..])),
            Some('\\') => self.escaped_char(at),
            Some(character) => Ok((u32::from(character), at + character.len_utf8())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_match(pattern: &str, text: &str) -> bool {
        compile(pattern)
            .unwrap_or_else(|error| panic!("{pattern:?}: {error}"))
            .is_match(text)
    }

    #[test]
    fn patterns_go_rejects_fail_with_go_error_text() {
        // Each expected text is what Go 1.27's regexp.Compile reports.
        let cases = [
            ("(", "missing closing ): `(`"),
            ("(?=x)", "invalid or unsupported Perl syntax: `(?=`"),
            ("a{1001}", "invalid repeat count: `{1001}`"),
            ("[z-a]", "invalid character class range: `z-a`"),
            ("\\", "trailing backslash at end of expression: ``"),
            ("a**", "invalid nested repetition operator: `**`"),
            ("a{2}{3}", "invalid nested repetition operator: `{2}{3}`"),
            ("a|*", "missing argument to repetition operator: `*`"),
            ("(?i)*", "missing argument to repetition operator: `*`"),
            ("(x{100}){100}", "invalid repeat count: `{100}`"),
            ("((x{10}){10}){11}", "invalid repeat count: `{11}`"),
            ("(a{0}){1001}", "invalid repeat count: `{1001}`"),
            ("\\Ccc", "invalid escape sequence: `\\C`"),
            ("\\1", "invalid escape sequence: `\\1`"),
            ("[\\0-\\7]", "invalid escape sequence: `\\7`"),
            ("\\x{110000}", "invalid escape sequence: `\\x{110000`"),
            ("\\Z", "invalid escape sequence: `\\Z`"),
            (
                "\\p{Alphabetic}",
                "invalid character class range: `\\p{Alphabetic}`",
            ),
            ("\\p{L", "invalid character class range: `\\p{L`"),
            ("[[:foo:]]", "invalid character class range: `[:foo:]`"),
            ("[a--b]", "invalid character class range: `a--`"),
            ("(?P<n-x>a)", "invalid named capture: `(?P<n-x>`"),
            ("(?i-)", "invalid or unsupported Perl syntax: `(?i-)`"),
            ("(?x)a", "invalid or unsupported Perl syntax: `(?x`"),
            ("a)", "unexpected ): `a)`"),
            ("[a", "missing closing ]: `[a`"),
        ];
        for (pattern, expected) in cases {
            assert_eq!(
                compile(pattern).err().as_deref(),
                Some(format!("error parsing regexp: {expected}").as_str()),
                "{pattern:?}"
            );
        }
    }

    #[test]
    fn patterns_go_accepts_compile() {
        for pattern in [
            "a*?",
            "(a*)*",
            "a(?i)*",
            "^*",
            "x{01}",
            "x{,5}",
            "x{",
            "x*(?i)*",
            "((x{10}){10}){10}",
            r"\Qa.b\E+",
            r"\12",
            r"\0",
            r"\x{D800}",
            r"\<",
            r"\_",
            r"\p{greek}",
            r"\p{Letter}",
            r"\p{Old Italic}",
            r"\p{^L}",
            r"\p{Assigned}",
            "[]a]",
            "[^]a]",
            "[a-b-c]",
            r"[\d-z]",
            "[a&&b]",
            "[[a]]",
            "(?P<1>a)",
            "(?P<n>a)(?P<n>b)",
            "(?)",
            "(?s-m:a)",
            r"\b{start}",
            r"\pL{1000}",
            r"[^\x00-\x{10FFFF}]",
            r"[\x{D800}-\x{DFFF}]",
            "",
        ] {
            assert!(
                compile(pattern).is_ok(),
                "{pattern:?}: {:?}",
                compile(pattern).err()
            );
        }
    }

    #[test]
    fn matching_follows_go_regexp() {
        // (pattern, text, Go's MatchString), observed with Go 1.27.
        let cases = [
            (r"\d", "\u{663}", false),
            (r"\w", "\u{e9}", false),
            (r"\s", "\u{b}", false),
            ("[[:space:]]", "\u{b}", true),
            (r"\bfoo\b", "foo\u{e9}", true),
            (r"\B", "\u{e9}", true),
            ("(?i)k", "\u{212a}", true),
            (r"(?i)\w", "\u{212a}", true),
            (r"(?i)\W", "\u{212a}", false),
            ("(?i)[[:upper:]]", "k", true),
            (r"(?i)\p{Lu}", "a", true),
            (r"(?i)\P{Lu}", "A", false),
            ("(?i)[^k]", "\u{212a}", false),
            ("^a$", "a\n", false),
            ("(?m)^b", "a\nb", true),
            ("(?m)a$", "a\nb", true),
            ("(?s)a.b", "a\nb", true),
            ("a.b", "a\nb", false),
            ("[^a]", "\n", true),
            (r"[\D]", "\u{663}", true),
            (r"\x{D800}|z", "z", true),
            (r"\x{D800}", "x", false),
            (r"[\x{D7FF}-\x{E000}]", "\u{e000}", true),
            (r"\101", "A", true),
            (r"\Qa.b\E", "axb", false),
            ("x{,5}", "x{,5}", true),
            (r"\<a\>", "<a>", true),
            ("^a(?i)*$", "AA", false),
            ("(?i)a(?-i)b", "AB", false),
            ("(?i)a(?-i)b", "Ab", true),
            ("(?U)^a+?$", "aaa", true),
            (r"^\s$", "\u{85}", false),
            (r"\p{Assigned}", "\u{10ffff}", false),
            (r"\p{ascii}", "\u{e9}", false),
        ];
        for (pattern, text, expected) in cases {
            assert_eq!(is_match(pattern, text), expected, "{pattern:?} on {text:?}");
        }
    }

    #[test]
    fn deep_nesting_compiles_without_overflowing_the_stack() {
        // 999 capture groups: Go's limit; 1000 is refused as Go refuses it.
        let deep = format!("{}a{}", "(".repeat(999), ")".repeat(999));
        assert!(compile(&deep).unwrap().is_match("a"));
        let too_deep = format!("{}a{}", "(".repeat(1000), ")".repeat(1000));
        assert!(compile(&too_deep)
            .unwrap_err()
            .starts_with("error parsing regexp: expression nests too deeply: `(((("));
        // Go does not limit non-capturing groups.
        let groups = format!("{}a{}", "(?:".repeat(5000), ")".repeat(5000));
        assert!(compile(&groups).unwrap().is_match("a"));
    }

    #[test]
    fn property_names_are_matched_loosely_like_go() {
        assert_eq!(canonical_name("old_italic"), "Olditalic");
        assert_eq!(canonical_name("  lU-"), "Lu");
        assert_eq!(unicode_property("cased letter"), Some(("LC", false)));
        assert_eq!(unicode_property("ASCII"), Some(("ASCII", false)));
        assert_eq!(unicode_property("assigned"), Some(("Cn", true)));
        assert_eq!(
            unicode_property("signwriting"),
            Some(("SignWriting", false))
        );
        assert_eq!(unicode_property("Alphabetic"), None);
    }
}
