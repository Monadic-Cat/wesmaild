//! # Wesnoth Markup Language (WML) parsing and serialization.
//! This library adopts a somewhat similar approach to the `simple_wml`
//! library used in the official Wesnoth multiplayer server.
//! The tree representation is currently read-only.
//!
//! # WML Grammar
//! See <https://wiki.wesnoth.org/GrammarWML> for a fuller explanation of the WML grammar.
//! ```text
//! wml_doc := (wml_tag | wml_attribute)*
//! wml_tag := '[' wml_name ']' wml_doc '[/' wml_name ']'
//! wml_name := [a-zA-Z0-9_]+
//! wml_attribute := textdomain? wml_key_sequence '=' wml_value «nl»
//! wml_key_sequence := wml_name (',' wml_name)*
//! wml_value := wml_value_component ('+' («nl» textdomain?)? wml_value_component)*
//! wml_value_component := text | '_'? string | '_'? raw_string
//!
//! text := [^+«nl»]*
//! string := '"' ([^"] | '""')* '"'
//! raw_string := '<<' ([^>] | >[^>])* '>>'
//! textdomain = '#textdomain' [a-zA-Z0-9_-]+ «nl»
//! ```
//!
//! # Use
//! This library is currently only tested for use with the messages received and sent
//! by the Wesnoth multiplayer server.
//! As the available documentation on WML syntax is not fully unambiguous, it is plausible
//! that the Wesnoth client accepts a wider range of inputs than does this parser.
//!
//! Additionally, this parser is not hardened against inputs crafted to cause stack overflows.
mod effects;

use ::bumpalo::Bump;
use effects::Effects;

mod bump {
    pub use ::bumpalo::boxed::Box;
    pub use ::bumpalo::collections::Vec;
}

type PResult<'a, T, E, I = &'a [u8]> = Result<(I, T), E>;

fn tagged<'a>(tag: &[u8], input: &'a [u8]) -> Result<&'a [u8], ()> {
    if input.starts_with(tag) {
        Ok(&input[tag.len()..])
    } else {
        Err(())
    }
}

// TODO: add more error types, and corresponding messages

#[derive(Debug)]
struct NoWhitespace;
impl From<NoWhitespace> for () {
    fn from(_: NoWhitespace) -> Self {
        ()
    }
}

// TODO: use this more, presumably?
/// Consume whitespace however the official WML tokenizer would.
fn whitespace(input: &[u8]) -> Result<&[u8], NoWhitespace> {
    let mut cursor = input;
    while let [b' ' | b'\t', rest @ ..] = cursor {
        cursor = rest;
    }
    if cursor.as_ptr() as usize - input.as_ptr() as usize > 0 {
        Ok(cursor)
    } else {
        Err(NoWhitespace)
    }
}

struct Or<A, B> {
    a: A,
    b: B,
}

trait StripPredicate {
    fn count(&self, input: &[u8]) -> usize;
    fn or<P: StripPredicate>(self, other: P) -> Or<Self, P> where Self: Sized {
        Or { a: self, b: other }
    }
}
impl<A: StripPredicate, B: StripPredicate> StripPredicate for Or<A, B> {
    fn count(&self, input: &[u8]) -> usize {
        let count = self.a.count(input);
        if count > 0 { count } else { self.b.count(input) }
    }
}
impl StripPredicate for &[u8] {
    fn count(&self, input: &[u8]) -> usize {
        if input.starts_with(self) { self.len() } else { 0 }
    }
}
impl<const N: usize> StripPredicate for &[u8; N] {
    fn count(&self, input: &[u8]) -> usize {
        if input.starts_with(*self) { self.len() } else { 0 }
    }
}

fn tagged_many0<'a, P: StripPredicate>(tag: P, input: &'a [u8]) -> &'a [u8] {
    let mut cursor = input;
    loop {
        let count = tag.count(cursor);
        if count > 0 {
            cursor = &cursor[count..];
        } else { break cursor }
    }
}

/// Key for retrieving a slice of bytes for a string.
#[derive(Debug, Copy, Clone)]
struct StringKey {
    // I'll add a layer of indirection if I make changes that make indices unstable,
    // like allowing mutation.
    // It may make sense to intern tag names, too.
    // But we will avoid such performance considerations until we have representative benchmarks.
    /// An index into the serialized vector.
    idx: usize,
    /// The length of the string.
    len: usize,
}

// TODO: consider storing more span information,
// so we can do the strategy `simple_wml` does with coloring
// mutated parts of the tree and adjusting only those
// when dumping output with our stored buffer.
// I'm not sure that would be a gain on small documents, though.
/// `(wml_tag | wml_attribute)` in the WML grammar.
#[derive(Debug)]
enum TagOrAttr<'a> {
    Tag(Tag<'a>),
    Attr(Attribute<'a>),
}
impl<'a> TagOrAttr<'a> {
    fn parse<'b, E: Effects>(e: &E, arena: &'a Bump, input: &'b [u8], offset: usize) -> PResult<'b, Self, ()> {
        // Right here, `Tag::parse` may recurse.
        Tag::parse(e, arena, input, offset)
            .map(|(rest, tag)| (rest, Self::Tag(tag)))
            .or_else(|()| {
                Attribute::parse(e, arena, input, offset).map(|(rest, attr)| (rest, Self::Attr(attr)))
            })
    }
}

/// `wml_tag` in the WML grammar.
///
/// ```text
/// wml_tag := '[' wml_name ']' wml_doc '[/' wml_name ']'
/// ```
#[derive(Debug)]
struct Tag<'a> {
    // TODO: consider giving Name its own type,
    // and doing string interning.
    // Whether string interning is a gain for us depends
    // on the spread of kinds of thing we do with WML, which I
    // haven't figured out yet.
    name: Name,
    content: bump::Vec<'a, TagOrAttr<'a>>,
}
// Note: `Tag`, and *only* `Tag`, is recursive.
// Alternatively, `TagOrAttr` could possibly handle the recursion?
impl<'a> Tag<'a> {
    fn parse<'b, E: Effects>(e: &E, arena: &'a Bump, input: &'b [u8], offset: usize) -> PResult<'b, Self, ()> {
        let offset = |slc: &[u8]| slc.as_ptr() as usize - input.as_ptr() as usize + offset;
        let rest = tagged(b"[", input)?;
        let (rest, name) = Name::parse(rest, offset(rest))?;
        let rest = tagged(b"]", rest)?;
        let rest = tagged_many0(b"\n".or(b"\t"), rest);
        // TODO: parse without recursing, or otherwise prevent stack overflows
        // (consider using `stacker` to be lazy)
        let mut cursor = rest;
        // TODO: this *would* benefit from using `with_capacity_in`
        let mut content = bump::Vec::<TagOrAttr>::new_in(arena);
        let mut count = 0;
        loop {
            println!("Hm: {}", count);
            count += 1;
            cursor = tagged_many0(b"\n".or(b"\t"), cursor);
            // Every single tag or attribute in here is optional.
            match TagOrAttr::parse(e, arena, cursor, offset(cursor)) {
                Ok((rest, tag_or_attr)) => {
                    content.push(tag_or_attr);
                    cursor = rest;
                },
                Err(()) => break,
            }
        }
        let rest = tagged(b"[/", cursor)?;
        let (rest, name_again) = Name::parse(rest, offset(rest))?;
        // How do we want to perform string equality checks?
        // We can just index into `input`, of course.
        let name_base = name.content.idx - offset(input);
        let name_c = &input[name_base .. name_base + name.content.len];
        let name_again_base = name_again.content.idx - offset(input);
        let name_again = &input[name_again_base .. name_again_base + name_again.content.len];
        if name_c != name_again { return Err(()) }
        let rest = tagged(b"]", rest)?;
        let rest = tagged_many0(b"\n".or(b"\t"), rest);
        Ok((rest, Self { name, content }))
    }
}

#[derive(Debug)]
struct EmptyName;
impl From<EmptyName> for () {
    fn from(_: EmptyName) -> Self { () }
}

/// `wml_name` in the WML grammar.
///
/// ```text
/// wml_name := [a-zA-Z0-9_]+
/// ```
#[derive(Debug)]
struct Name {
    content: StringKey,
}
impl Name {
    fn parse(input: &[u8], offset: usize) -> PResult<Name, EmptyName> {
        let mut cursor = input;
        while let [b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_', rest @ ..] = cursor {
            cursor = rest;
        }
        let name_len = cursor.as_ptr() as usize - input.as_ptr() as usize;
        if name_len > 0 {
            let name = Name { content: StringKey {
                len: name_len,
                idx: offset,
            }};
            Ok((cursor, name))
        } else {
            Err(EmptyName)
        }
    }
}

/// `wml_attribute` in the WML grammar.
///
/// ```text
/// wml_attribute := textdomain? wml_key_sequence '=' wml_value «nl»
/// ```
#[derive(Debug)]
struct Attribute<'a> {
    domain: Option<TextDomain>,
    key_sequence: KeySequence<'a>,
    value: Value<'a>,
}
impl<'a> Attribute<'a> {
    fn parse<'b, E: Effects>(e: &E, arena: &'a Bump, input: &'b [u8], offset: usize) -> PResult<'b, Self, ()> {
        let (rest, domain) = TextDomain::parse(e, input, offset)
            .map(|(rest, domain)| (rest, Some(domain)))
            .unwrap_or_else(|()| (input, None));
        let offset = |slc: &[u8]| slc.as_ptr() as usize - input.as_ptr() as usize + offset;
        let (rest, key_sequence) = KeySequence::parse(arena, rest, offset(rest))?;
        println!("{}", e.get_str(key_sequence.first.content).unwrap());
        let rest = tagged(b"=", rest)?;
        let (rest, value) = Value::parse(e, arena, rest, offset(rest))?;
        let rest = tagged(b"\n", rest)?;
        Ok((rest, Self { domain, key_sequence, value }))
    }
}

/// `wml_key_sequence` in the WML grammar.
///
/// ```text
/// wml_key_sequence := wml_name (',' wml_name)*
/// ```
#[derive(Debug)]
struct KeySequence<'a> {
    first: Name,
    names: bump::Vec<'a, Name>,
}
impl<'a> KeySequence<'a> {
    fn parse<'b>(arena: &'a Bump, input: &'b [u8], offset: usize) -> PResult<'b, Self, ()> {
        let (rest, first) = Name::parse(input, offset)?;
        let offset = |slc: &[u8]| slc.as_ptr() as usize - input.as_ptr() as usize + offset;
        let mut cursor = rest;
        let mut names = bump::Vec::new_in(arena);
        loop {
            match tagged(b",", cursor) {
                Ok(rest) => {
                    let (rest, name) = Name::parse(rest, offset(rest))?;
                    names.push(name);
                    cursor = rest;
                },
                Err(()) => break,
            }
        }
        Ok((cursor, Self { first, names }))
    }
}

/// `wml_value` in the WML grammar.
///
/// ```text
/// wml_value := wml_value_component ('+' («nl» textdomain?)? wml_value_component)*
/// ```
#[derive(Debug)]
struct Value<'a> {
    first: ValueComponent,
    rest: bump::Vec<'a, (Option<TextDomain>, ValueComponent)>,
}
impl<'a> Value<'a> {
    fn parse<'b, E: Effects>(e: &E, arena: &'a Bump, input: &'b [u8], offset: usize) -> PResult<'b, Self, ()> {
        let (rest, first) = ValueComponent::parse(e, input, offset)?;
        let offset = |slc: &[u8]| slc.as_ptr() as usize - input.as_ptr() as usize + offset;
        let mut cursor = rest;
        // Note: Since `Value` has no children that use the arena allocator,
        // we don't need to worry about fragmentation here, regardless of whether
        // we appropriately do `with_capacity_in` or not.
        let mut vec = bump::Vec::new_in(arena);
        loop {
            match tagged(b"+", cursor) {
                Ok(rest) => {
                    let (rest, domain) = match tagged(b"\n", rest) {
                        Ok(rest) => {
                            // Check for textdomain, which is still optional at this point
                            match TextDomain::parse(e, input, offset(rest)) {
                                Ok((rest, domain)) => {
                                    (rest, Some(domain))
                                },
                                Err(()) => (rest, None),
                            }
                        },
                        Err(()) => (rest, None),
                    };
                    // Consume value component, not optional at this point
                    let (rest, next) = ValueComponent::parse(e, rest, offset(rest))?;
                    vec.push((domain, next));
                    cursor = rest;
                },
                Err(()) => {
                    break
                }
            }
        }
        Ok((cursor, Self { first, rest: vec }))
    }
}

/// `wml_value_component` in the WML grammar.
///
/// ```text
/// wml_value_component := text | '_'? string | '_'? raw_string
/// ```
#[derive(Debug)]
enum ValueComponent {
    Text(Text),
    String(WString),
    RawString(RawString),
}
impl ValueComponent {
    fn parse<'a, E: Effects>(e: &E, input: &'a [u8], offset: usize) -> PResult<'a, Self, ()> {
        // TODO: fix order these are checked?
        let mut underscored = false;
        let (rest, offset) = match tagged(b"_", input) {
            Ok(rest) => {
                underscored = true;
                (rest, offset + 1)
            },
            Err(()) => (input, offset),
        };
        WString::parse(rest, offset).map(|(rest, s)| {
            println!("String: {}", e.get_str(s.content).unwrap());
            (rest, Self::String(s))
        }).or_else(|()| {
            RawString::parse(rest, offset).map(|(rest, r)| {
                println!("Raw String: {}", e.get_str(r.content).unwrap());
                (rest, Self::RawString(r))
            })
        }).or_else(|()| {
            if !underscored {
                Text::parse(input, offset).map(|(rest, txt)| {
                    println!("Text: {}", e.get_str(txt.content).unwrap());
                    (rest, Self::Text(txt))
                })
            } else {
                Err(())
            }
        })
    }
}

/// `text` in the WML grammar.
///
/// ```text
/// text := [^+«nl»]*
/// ```
#[derive(Debug)]
struct Text {
    content: StringKey,
}
impl Text {
    fn parse(input: &[u8], offset: usize) -> PResult<Self, ()> {
        let mut cursor = input;
        while let &[a, ref rest @ ..] = cursor {
            if a == b'+' || a == b'\n' {
                break
            } else {
                cursor = rest;
            }
        }
        let len = cursor.as_ptr() as usize - input.as_ptr() as usize;
        let content = StringKey {
            idx: offset,
            len,
        };
        Ok((cursor, Self { content } ))
    }
}

/// `string` in the WML grammar.
///
/// ```text
/// string := '"' ([^"] | '""')* '"'
/// ```
#[derive(Debug)]
struct WString {
    content: StringKey,
}
impl WString {
    fn parse(input: &[u8], offset: usize) -> PResult<Self, ()> {
        let rest = tagged(b"\"", input)?;
        let mut cursor = rest;
        while let &[a, b, ref rest @ ..] = cursor {
            if a != b'"' || (a == b'"' && b == b'"') {
                cursor = &cursor[1..];
            } else {
                break
            }
        }
        let len = cursor.as_ptr() as usize - rest.as_ptr() as usize;
        let content = StringKey {
            idx: rest.as_ptr() as usize - input.as_ptr() as usize + offset,
            len,
        };
        let rest = tagged(b"\"", cursor)?;
        Ok((rest, Self { content }))
    }
}

/// `raw_string` in the WML grammar.
///
/// ```text
/// raw_string := '<<' ([^>] | >[^>])* '>>'
/// ```
#[derive(Debug)]
struct RawString {
    content: StringKey,
}
impl RawString {
    fn parse(input: &[u8], offset: usize) -> PResult<Self, ()> {
        let rest = tagged(b"<<", input)?;
        let mut cursor = rest;
        while let &[a, b, ref rest @ ..] = cursor {
            if a != b'>' || b != b'>' {
                cursor = &cursor[1..];
            } else {
                break
            }
        }
        let len = cursor.as_ptr() as usize - rest.as_ptr() as usize;
        let content = StringKey {
            idx: rest.as_ptr() as usize - input.as_ptr() as usize + offset,
            len,
        };
        // Note: Either this is true, or we hit EOF.
        let rest = tagged(b">>", cursor)?;
        Ok((rest, Self { content }))
    }
}

/// `textdomain` in the WML grammar.
///
/// ```text
/// textdomain = '#textdomain' [a-zA-Z0-9_-]+ «nl»
/// ```
#[derive(Debug)]
struct TextDomain {
    name: StringKey,
}
impl TextDomain {
    fn parse<'a, E: Effects>(e: &E, input: &'a [u8], offset: usize) -> PResult<'a, Self, ()> {
        let rest = tagged(b"#textdomain", input)?;
        // TODO: verify that this is needed here,
        // or if we should just scroll over whitespace or something
        let rest = whitespace(rest)?;
        let mut cursor = rest;
        while let [b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-', rest @ ..] = cursor {
            cursor = rest;
        }
        let len = cursor.as_ptr() as usize - rest.as_ptr() as usize;
        if len > 0 {
            let name = StringKey {
                idx: rest.as_ptr() as usize - input.as_ptr() as usize + offset,
                len,
            };
            println!("Textdomain: {}", e.get_str(name).unwrap());
            let rest = tagged(b"\n", cursor)?;
            Ok((rest, Self { name }))
        } else {
            Err(())
        }
    }
}

/// A WML document.
#[derive(Debug)]
pub struct Doc<'a> {
    top: bump::Vec<'a, TagOrAttr<'a>>,
    text: Vec<u8>,
}

// Parsing of a single document is inherently single threaded, so
// parallelism would be introduced by creating a `DocProcessor` for each thread,
// with one thread per core we're willing to consume.
/// A document processor, meant to process WML documents serially and reuse memory between them.
#[derive(Debug)]
pub struct DocProcessor {
    // The lifetime carrying collections of `bumpalo` are what forced me to
    // introduce this `DocProcessor` struct.
    arena: Bump,
    // TODO: consider adding interner
}

impl DocProcessor {
    pub fn new() -> Self {
        Self {
            arena: Bump::new(),
        }
    }
    /// Nuke all parsed stuff.
    /// See [`Bump::reset`].
    pub fn reset(&mut self) {
        self.arena.reset()
    }
    pub fn parse(&self, buf: Vec<u8>) -> Result<Doc<'_>, ()> {
        // TODO: this would benefit from `with_capacity_in`
        let mut top = bump::Vec::new_in(&self.arena);
        let mut cursor = &*buf;
        let printer = effects::Printer::new(cursor);
        let offset = |slc: &[u8]| slc.as_ptr() as usize - buf.as_ptr() as usize;
        while let Ok((rest, tag_or_attr)) = TagOrAttr::parse(&printer, &self.arena, cursor, offset(cursor)) {
            cursor = rest;
            top.push(tag_or_attr);
        }
        // Check if there's input we failed to parse.
        if offset(cursor) == buf.len() {
            Ok(Doc {
                top,
                text: buf,
            })
        } else {
            dbg!(offset(cursor), buf.len());
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::array::IntoIter;
    use crate::DocProcessor;

    #[test]
    fn parse_attr() {
        let processor = DocProcessor::new();
        let input = IntoIter::new(*b"lol=\"hello\"\n").collect::<Vec<u8>>();
        let _doc = processor.parse(input).unwrap();
    }

    #[test]
    fn parse_users() {
        let processor = DocProcessor::new();
        let users = Vec::from("[user]\navailable=\"yes\"\nforum_id=\"0\"\ngame_id=\"0\"\nlocation=\"\"\nmoderator=\"no\"\nname=\"lol\"\nregistered=\"no\"\nstatus=\"lobby\"\n[/user]\n[user]\navailable=\"yes\"\nforum_id=\"0\"\ngame_id=\"0\"\nlocation=\"\"\nmoderator=\"no\"\nname=\"haha\"\nregistered=\"no\"\nstatus=\"lobby\"\n[/user]\n");
        let _doc = processor.parse(users).unwrap();
    }

    #[test]
    fn parse_empty_tag() {
        let processor = DocProcessor::new();
        let game_list = Vec::from("[gamelist]\n\n[/gamelist]");
        let _doc = processor.parse(game_list).unwrap();
    }
}
