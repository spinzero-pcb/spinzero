//! A small, allocation-light reader for the S-expression dialect KiCad uses in
//! its `.kicad_sch`, `.kicad_pcb`, `.kicad_sym` and netlist files.
//!
//! This is written from the on-disk grammar (parenthesised lists, bare tokens,
//! and double-quoted strings with backslash escapes), not ported from any other
//! implementation. Bytes are scanned directly: KiCad keywords are ASCII, and the
//! only structurally significant bytes — whitespace, parens, quote, backslash —
//! never appear inside a UTF-8 multibyte sequence, so raw byte scanning is safe
//! for arbitrary UTF-8 string payloads.

use std::fmt;

/// One node of a parsed tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A bare token: a keyword, identifier, or number (e.g. `at`, `yes`, `1.27`).
    Sym(String),
    /// A double-quoted string, with escape sequences already resolved.
    Text(String),
    /// A parenthesised list. By convention `list[0]` is the tag.
    List(Vec<Node>),
}

impl Node {
    /// The string payload of an atom (`Sym` or `Text`); `None` for a list.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Sym(s) | Node::Text(s) => Some(s),
            Node::List(_) => None,
        }
    }

    /// The elements of a list; `None` for an atom.
    pub fn as_list(&self) -> Option<&[Node]> {
        match self {
            Node::List(v) => Some(v),
            _ => None,
        }
    }

    /// The head token of a list, e.g. the tag of `(at 1 2)` is `"at"`.
    pub fn tag(&self) -> Option<&str> {
        self.as_list()?.first()?.as_str()
    }

    /// Elements after the head (the arguments of a tagged list).
    pub fn tail(&self) -> &[Node] {
        match self {
            Node::List(v) if !v.is_empty() => &v[1..],
            _ => &[],
        }
    }

    /// The i-th element of a list, where index 0 is the head.
    pub fn nth(&self, i: usize) -> Option<&Node> {
        self.as_list()?.get(i)
    }

    /// Parse this atom as an `f64`. Non-finite values (`NaN`, `inf`, overflow of
    /// `1e999` → `inf`) are rejected: Rust's `f64::from_str` accepts them, but a
    /// `NaN`/`inf` coordinate would poison every bbox and SVG path it flows into
    /// with no error anywhere, so a malformed number reads as absent instead.
    pub fn as_f64(&self) -> Option<f64> {
        let v: f64 = self.as_str()?.parse().ok()?;
        v.is_finite().then_some(v)
    }

    /// Parse this atom as an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        self.as_str()?.parse().ok()
    }

    /// The first immediate child list with the given tag.
    pub fn child(&self, tag: &str) -> Option<&Node> {
        self.as_list()?.iter().find(|n| n.tag() == Some(tag))
    }

    /// All immediate child lists with the given tag.
    pub fn children<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.as_list()
            .into_iter()
            .flatten()
            .filter(move |n| n.tag() == Some(tag))
    }

    /// The first argument of a child `(tag value …)`, as a string.
    pub fn field(&self, tag: &str) -> Option<&str> {
        self.child(tag)?.nth(1)?.as_str()
    }

    /// The first argument of a child `(tag value …)`, as an `f64`.
    pub fn field_f64(&self, tag: &str) -> Option<f64> {
        self.child(tag)?.nth(1)?.as_f64()
    }

    /// True when a child flag reads as KiCad-truthy (`yes`/`true`/`1`), e.g.
    /// `(in_bom yes)`. Returns `None` when the tag is absent so callers can tell
    /// "missing" apart from "present and false".
    pub fn flag(&self, tag: &str) -> Option<bool> {
        let v = self.field(tag)?;
        Some(matches!(v, "yes" | "true" | "1"))
    }

    /// True when `tag` is asserted either as a `(tag yes)` child list (KiCad 7+)
    /// or as a bare `tag` token directly in this list (KiCad 5/6) — the two forms
    /// KiCad has used for `bold`, `italic`, `hide` and `knockout`.
    ///
    /// Only a bare [`Node::Sym`] matches the token form, so a quoted string that
    /// merely *equals* `tag` (a [`Node::Text`] — e.g. a property whose value is the
    /// literal word "hide") never trips it. This is the same discriminator the
    /// KiCad-10 `(hide yes)` handling relies on.
    pub fn has_flag(&self, tag: &str) -> bool {
        self.flag(tag) == Some(true)
            || self
                .as_list()
                .into_iter()
                .flatten()
                .any(|x| matches!(x, Node::Sym(s) if s == tag))
    }

    /// Read a child `(tag a b …)` as a numeric pair `(a, b)` — the shape KiCad uses
    /// for `(size w h)`, `(paper "User" w h)` (with an offset), and similar. `None`
    /// when the child is absent or either value is missing/non-finite. Callers apply
    /// their own default (usually `0.0`), keeping the "absent" vs "present" distinction.
    pub fn pair(&self, tag: &str) -> Option<(f64, f64)> {
        self.child(tag)?.pair_at(1)
    }

    /// Read this list's arguments at `i` and `i+1` as a numeric pair. Used for
    /// tagged lists whose numbers do not start at index 1 (e.g. `(paper "User" w h)`).
    pub fn pair_at(&self, i: usize) -> Option<(f64, f64)> {
        Some((self.nth(i)?.as_f64()?, self.nth(i + 1)?.as_f64()?))
    }

    /// The first immediate `(property "KEY" "VALUE" …)` child whose key matches
    /// `key`. Returns the whole property node so callers can read its value
    /// (`.nth(2)`) or its position/effects.
    pub fn property_named(&self, key: &str) -> Option<&Node> {
        self.children("property")
            .find(|p| p.nth(1).and_then(Node::as_str) == Some(key))
    }

    /// The value (`.nth(2)`) of the first `(property "KEY" "VALUE")` child matching
    /// `key`, as a string.
    pub fn property_value(&self, key: &str) -> Option<&str> {
        self.property_named(key)?.nth(2)?.as_str()
    }
}

/// Errors that can arise while reading a tree.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    /// Input ended while a list or string was still open.
    UnexpectedEof,
    /// A `)` appeared with no matching `(`.
    UnbalancedClose(usize),
    /// Input contained no expression at all.
    Empty,
    /// A token was not valid UTF-8.
    InvalidUtf8(usize),
    /// List nesting exceeded `MAX_DEPTH` — guards against a stack overflow on a
    /// deeply-nested or maliciously-crafted file (an overflow can't be caught).
    TooDeep,
    /// Non-whitespace content followed the root expression — e.g. a truncated file
    /// concatenated with another (a real sync-conflict artifact). Carries the byte
    /// offset where the trailing content begins.
    TrailingGarbage(usize),
    /// The root list's tag was not the one this file kind requires (e.g. a
    /// `.kicad_sym` fed to the `.kicad_pcb` parser). Distinguishes a wrong/corrupt
    /// file from a genuinely empty board.
    WrongRoot { expected: &'static str, found: Option<String> },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnexpectedEof => write!(f, "unexpected end of input"),
            ParseError::UnbalancedClose(at) => write!(f, "unbalanced ')' at byte {at}"),
            ParseError::Empty => write!(f, "no expression found"),
            ParseError::InvalidUtf8(at) => write!(f, "invalid UTF-8 near byte {at}"),
            ParseError::TooDeep => write!(f, "input nested too deeply"),
            ParseError::TrailingGarbage(at) => {
                write!(f, "unexpected content after root expression at byte {at}")
            }
            ParseError::WrongRoot { expected, found } => match found {
                Some(tag) => write!(f, "expected a ({expected} …) root, found ({tag} …)"),
                None => write!(f, "expected a ({expected} …) root"),
            },
        }
    }
}

impl std::error::Error for ParseError {}

/// Maximum list-nesting depth. Real KiCad files nest only ~10-20 deep; this bound is
/// generous for valid input yet small enough that the worst-case recursion stays well
/// within a 1 MB thread stack, so a pathological file errors instead of overflowing.
const MAX_DEPTH: usize = 256;

/// Parse the first top-level expression in `input`. Trailing whitespace is
/// allowed and ignored; KiCad files contain a single root list.
pub fn parse(input: &str) -> Result<Node, ParseError> {
    let bytes = input.as_bytes();
    // Skip a UTF-8 BOM if present.
    let start = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    let mut p = Reader { b: bytes, i: start };
    p.skip_ws();
    if p.i >= p.b.len() {
        return Err(ParseError::Empty);
    }
    let root = p.node(0)?;
    // A KiCad file is exactly one root list. Anything but trailing whitespace after
    // it means the input is truncated-then-concatenated (a sync-conflict artifact) or
    // otherwise malformed — reject it rather than silently returning just the first
    // expression.
    p.skip_ws();
    if p.i < p.b.len() {
        return Err(ParseError::TrailingGarbage(p.i));
    }
    Ok(root)
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b' ' | b'\t' | b'\r' | b'\n' => self.i += 1,
                _ => break,
            }
        }
    }

    fn node(&mut self, depth: usize) -> Result<Node, ParseError> {
        match self.b.get(self.i) {
            None => Err(ParseError::UnexpectedEof),
            Some(b'(') => self.list(depth),
            Some(b'"') => self.string().map(Node::Text),
            Some(b')') => Err(ParseError::UnbalancedClose(self.i)),
            Some(_) => self.symbol(),
        }
    }

    fn list(&mut self, depth: usize) -> Result<Node, ParseError> {
        if depth >= MAX_DEPTH {
            return Err(ParseError::TooDeep);
        }
        self.i += 1; // consume '('
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.b.get(self.i) {
                None => return Err(ParseError::UnexpectedEof),
                Some(b')') => {
                    self.i += 1;
                    return Ok(Node::List(items));
                }
                Some(_) => items.push(self.node(depth + 1)?),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        let open = self.i;
        self.i += 1; // consume opening quote
        let mut out: Vec<u8> = Vec::new();
        while let Some(&c) = self.b.get(self.i) {
            match c {
                b'"' => {
                    self.i += 1;
                    return String::from_utf8(out).map_err(|_| ParseError::InvalidUtf8(open));
                }
                b'\\' => {
                    self.i += 1;
                    match self.b.get(self.i) {
                        None => return Err(ParseError::UnexpectedEof),
                        Some(b'n') => out.push(b'\n'),
                        Some(b'r') => out.push(b'\r'),
                        Some(b't') => out.push(b'\t'),
                        Some(&other) => out.push(other), // \" \\ and any other: literal
                    }
                    self.i += 1;
                }
                _ => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
        Err(ParseError::UnexpectedEof)
    }

    fn symbol(&mut self) -> Result<Node, ParseError> {
        let start = self.i;
        while let Some(&c) = self.b.get(self.i) {
            match c {
                b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b'"' => break,
                _ => self.i += 1,
            }
        }
        let s = std::str::from_utf8(&self.b[start..self.i])
            .map_err(|_| ParseError::InvalidUtf8(start))?;
        Ok(Node::Sym(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atom_and_list() {
        let n = parse("(version 20240101)").unwrap();
        assert_eq!(n.tag(), Some("version"));
        assert_eq!(n.field("version"), None); // version has no child named "version"
        assert_eq!(n.nth(1).unwrap().as_i64(), Some(20240101));
    }

    #[test]
    fn quoted_string_with_escapes() {
        let n = parse(r#"(property "Ref" "a\"b\\c\nd")"#).unwrap();
        assert_eq!(n.tag(), Some("property"));
        assert_eq!(n.nth(1).unwrap().as_str(), Some("Ref"));
        assert_eq!(n.nth(2).unwrap().as_str(), Some("a\"b\\c\nd"));
    }

    #[test]
    fn literal_newline_inside_quotes() {
        let n = parse("(text \"line1\nline2\")").unwrap();
        assert_eq!(n.nth(1).unwrap().as_str(), Some("line1\nline2"));
    }

    #[test]
    fn nested_and_accessors() {
        let src = r#"
            (symbol
                (lib_id "Device:R")
                (at 25.4 50.8 90)
                (in_bom yes)
                (dnp no)
                (property "Reference" "R1")
                (property "Value" "10k"))
        "#;
        let n = parse(src).unwrap();
        assert_eq!(n.tag(), Some("symbol"));
        assert_eq!(n.field("lib_id"), Some("Device:R"));
        let at = n.child("at").unwrap();
        assert_eq!(at.nth(1).unwrap().as_f64(), Some(25.4));
        assert_eq!(at.nth(2).unwrap().as_f64(), Some(50.8));
        assert_eq!(at.nth(3).unwrap().as_i64(), Some(90));
        assert_eq!(n.flag("in_bom"), Some(true));
        assert_eq!(n.flag("dnp"), Some(false));
        assert_eq!(n.flag("missing"), None);
        let props: Vec<_> = n
            .children("property")
            .map(|p| (p.nth(1).unwrap().as_str().unwrap(), p.nth(2).unwrap().as_str().unwrap()))
            .collect();
        assert_eq!(props, vec![("Reference", "R1"), ("Value", "10k")]);
    }

    #[test]
    fn bom_and_trailing_ws() {
        let n = parse("\u{feff}(a 1)   \n").unwrap();
        assert_eq!(n.tag(), Some("a"));
    }

    #[test]
    fn errors() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert_eq!(parse("(a (b)"), Err(ParseError::UnexpectedEof));
        assert!(matches!(parse(")"), Err(ParseError::UnbalancedClose(_))));
    }

    #[test]
    fn trailing_content_after_root_is_rejected() {
        // A truncated-then-concatenated file must not misread as just its first list.
        assert!(matches!(
            parse("(kicad_pcb (version 1)) extra junk ("),
            Err(ParseError::TrailingGarbage(_))
        ));
        assert!(matches!(parse("(a)(b)"), Err(ParseError::TrailingGarbage(_))));
        // Trailing whitespace/newline is still fine.
        assert!(parse("(a 1)\n\n").is_ok());
    }

    #[test]
    fn non_finite_numbers_are_rejected() {
        let n = parse("(at NaN inf 1e999)").unwrap();
        assert_eq!(n.nth(1).unwrap().as_f64(), None, "NaN is not a coordinate");
        assert_eq!(n.nth(2).unwrap().as_f64(), None, "inf is not a coordinate");
        assert_eq!(n.nth(3).unwrap().as_f64(), None, "1e999 overflows to inf");
        // A finite value still parses.
        assert_eq!(parse("(at 1.5)").unwrap().nth(1).unwrap().as_f64(), Some(1.5));
    }

    #[test]
    fn has_flag_distinguishes_token_from_quoted_value() {
        // Bare token, `(tag yes)`, and `(tag no)`.
        let n = parse(r#"(font bold (italic yes) (hide no))"#).unwrap();
        assert!(n.has_flag("bold"), "bare token asserts the flag");
        assert!(n.has_flag("italic"), "(italic yes) asserts the flag");
        assert!(!n.has_flag("hide"), "(hide no) does not");
        assert!(!n.has_flag("missing"), "absent tag is false");
        // A quoted string equal to the tag must NOT count as the flag.
        let p = parse(r#"(property "Visible" "hide")"#).unwrap();
        assert!(!p.has_flag("hide"), "a quoted value of \"hide\" is not the hide flag");
    }

    #[test]
    fn pair_and_property_helpers() {
        let n = parse(r#"(text_box (size 20 5) (property "Netclass" "HV"))"#).unwrap();
        assert_eq!(n.pair("size"), Some((20.0, 5.0)));
        assert_eq!(n.pair("missing"), None);
        assert_eq!(n.property_value("Netclass"), Some("HV"));
        assert!(n.property_named("Netclass").is_some());
        assert_eq!(n.property_value("Nope"), None);
    }

    #[test]
    fn deep_nesting_is_bounded_not_a_stack_overflow() {
        // A pathological run of open parens must error cleanly, never overflow the stack.
        let bomb = "(".repeat(100_000);
        assert_eq!(parse(&bomb), Err(ParseError::TooDeep));
        // A legitimately-nested tree well under the bound still parses.
        let ok = format!("{}{}", "(a ".repeat(50), ")".repeat(50));
        assert!(parse(&ok).is_ok());
    }
}
