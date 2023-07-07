/*!
Parsers supporting usefix functionality. While the overal parser is highly
bespoke, we can still make use of these primitive parsers for the basics.
*/

use std::{
    cmp,
    collections::BTreeSet,
    hash::{Hash, Hasher},
    num::ParseIntError,
};

use nom::{
    branch::alt,
    character::complete::{digit1, space0},
    error::{ErrorKind, FromExternalError, ParseError},
    multi::many0,
    IResult, Parser as _,
};
use nom_supreme::{
    tag::{complete::tag, TagError},
    ParserExt as _,
};
use unicode_xid::UnicodeXID as _;

use crate::tree::{SimplePath, TreeRoot};

/// Parse a normal rust identifier. This is an XID_Start character followed by
/// 0 or more XID_Continue characters, or alternatively, and underscore
/// followed by at least 1 XID_Continue character.
fn parse_normal_identifier<'i, E>(input: &'i str) -> IResult<&'i str, &'i str, E>
where
    E: ParseError<&'i str>,
{
    // This implementation depends on the property that XID_Continue is a
    // superset of XID_Start

    let split_point = input
        .find(|c: char| !c.is_xid_continue())
        .unwrap_or(input.len());

    let (ident, tail) = input.split_at(split_point);

    match ident.chars().next() {
        // Identifier is empty, that's no good
        None => Err(nom::Err::Error(E::from_error_kind(input, ErrorKind::Alpha))),

        // We have our first character, it must be an XID_Start or underscore
        Some(c) => {
            // XID_Start is fine
            if c.is_xid_start() {
                Ok((tail, ident))
            }
            // underscore is fine, IF there's at least one more character in
            // the identifier
            else if c == '_' {
                if ident[1..].is_empty() {
                    Err(nom::Err::Error(E::from_error_kind(
                        &input[1..],
                        ErrorKind::AlphaNumeric,
                    )))
                } else {
                    Ok((tail, ident))
                }
            } else {
                Err(nom::Err::Error(E::from_error_kind(input, ErrorKind::Alpha)))
            }
        }
    }
}

#[test]
fn test_parse_normal_identifier() {
    let ident = "std::";

    assert_eq!(parse_normal_identifier::<()>(ident), Ok(("::", "std")))
}

/// Parse the "r#" prefix
fn parse_raw_prefix<'i, E>(input: &'i str) -> IResult<&'i str, &'i str, E>
where
    E: TagError<&'i str, &'static str>,
{
    tag("r#").parse(input)
}

/**
A single identifier that can appear in a `use` declaration. Some notes about it:

- We assume that such identifiers are either static or sourced from the input
  file `str`, so they're represented as a `str`.
- We support `r#` identifiers, but only barely. We don't attempt to normalize
  them or anything like that.
- Technically things like `crate` and `super` are keywords, not identifiers.
  We don't make that distinction here.
 */
#[derive(Debug, Clone, Copy)]
pub struct Identifier<'a>(&'a str);

impl Identifier<'_> {
    #[inline]
    #[must_use]
    pub fn get_raw(&self) -> &str {
        self.0
    }

    /**
    Get the "correct" value of this identifier, stripping out a leading r#

    */
    #[inline]
    #[must_use]
    pub fn get(&self) -> &str {
        self.0.strip_prefix("r#").unwrap_or(self.0)
    }

    pub const CRATE: Identifier<'static> = Identifier("crate");
    pub const SUPER: Identifier<'static> = Identifier("super");
    pub const SELF: Identifier<'static> = Identifier("self");
}

impl<'a, 'b> PartialEq<Identifier<'b>> for Identifier<'a> {
    #[inline]
    #[must_use]
    fn eq(&self, other: &Identifier<'b>) -> bool {
        self.get() == other.get()
    }
}

impl Eq for Identifier<'_> {}

impl<'a, 'b> PartialOrd<Identifier<'b>> for Identifier<'a> {
    #[inline]
    #[must_use]
    fn partial_cmp(&self, other: &Identifier<'b>) -> Option<cmp::Ordering> {
        Some(Ord::cmp(self, other))
    }
}

impl Ord for Identifier<'_> {
    #[inline]
    #[must_use]
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        Ord::cmp(self.get(), other.get())
    }
}

impl Hash for Identifier<'_> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.get().hash(state)
    }
}

/**
Parse a rust identifier, optionally preceeded by `r#`. The rules are the same
for both.
*/
pub fn parse_identifier<'i, E>(input: &'i str) -> IResult<&'i str, Identifier<'i>, E>
where
    E: TagError<&'i str, &'static str>,
    E: ParseError<&'i str>,
{
    parse_normal_identifier
        .opt_preceded_by(parse_raw_prefix)
        .recognize()
        .map(Identifier)
        .parse(input)
}

#[test]
fn test_parse_identifier() {
    use cool_asserts::assert_matches;

    let ident = "std::";

    assert_matches!(parse_identifier::<()>(ident), Ok(("::", Identifier("std"))))
}

#[test]
fn test_parse_raw_identifier() {
    use cool_asserts::assert_matches;

    let ident = "r#std::";

    let (tail, ident) = parse_identifier::<()>(ident).unwrap();
    assert_eq!(tail, "::");
    assert_matches!(ident, Identifier("r#std"));
    assert_eq!(ident.get(), "std");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierLike<'a> {
    Identifier(Identifier<'a>),
    Underscore,
}

/**
Parse an identifier or an underscore
*/
pub fn parse_identifier_like<'i, E>(input: &'i str) -> IResult<&'i str, IdentifierLike<'i>, E>
where
    E: TagError<&'i str, &'static str>,
    E: ParseError<&'i str>,
{
    alt((
        parse_identifier.map(IdentifierLike::Identifier),
        tag("_").value(IdentifierLike::Underscore),
    ))
    .parse(input)
}

/**
Get and return all of the whitespace that starts a line
*/
#[must_use]
pub fn snip_whitespace(input: &str) -> (&str, &str) {
    let split_point = input
        .find(|c: char| !c.is_whitespace())
        .unwrap_or(input.len());

    input.split_at(split_point)
}

fn parse_path_separator<'i, E>(input: &'i str) -> IResult<&'i str, &'i str, E>
where
    E: TagError<&'i str, &'static str>,
{
    tag("::").parse(input)
}

pub fn parse_simple_path<'i, E>(input: &'i str) -> IResult<&'i str, SimplePath<'i>, E>
where
    E: TagError<&'i str, &'static str>,
    E: ParseError<&'i str>,
{
    parse_path_separator
        .opt()
        .and(parse_identifier)
        .map(|(root, identifier)| TreeRoot {
            identifier,
            rooted: root.is_some(),
        })
        .and(many0(
            parse_identifier.cut().preceded_by(parse_path_separator),
        ))
        .map(|(root, children)| SimplePath { root, children })
        .parse(input)
}

#[derive(Debug, Clone)]
pub enum Visibility<'a> {
    Public,
    Crate,
    This,
    Super,
    In(SimplePath<'a>),
}

/// Parse a `pub` visibility marker, such as `pub`, `pub(crate)`, or
/// `pub(in a::b)`.
pub fn parse_pub_visibility<'i, E>(input: &'i str) -> IResult<&'i str, Visibility<'i>, E>
where
    E: TagError<&'i str, &'static str> + ParseError<&'i str>,
{
    alt((
        tag("crate").value(Visibility::Crate),
        tag("super").value(Visibility::Super),
        tag("self").value(Visibility::This),
        parse_simple_path
            .cut()
            .preceded_by(tag("in "))
            .map(Visibility::In),
    ))
    .terminated(tag(")"))
    .cut()
    .preceded_by(tag("("))
    .opt()
    .preceded_by(tag("pub"))
    .map(|vis| vis.unwrap_or(Visibility::Public))
    .parse(input)
}

/// Parse `use`, optionally prefixed by a visibility specifier. Returns the
/// visibility specifier, if any.
pub fn parse_use_prefix<'i, E>(input: &'i str) -> IResult<&'i str, Option<Visibility<'i>>, E>
where
    E: TagError<&'i str, &'static str> + ParseError<&'i str>,
{
    parse_pub_visibility
        .terminated(tag(" "))
        .opt_precedes(tag("use"))
        .map(|(vis, _)| vis)
        .parse(input)
}

/// Parse an `as <IDENTIFIER` alias
pub fn parse_as_alias<'i, E>(input: &'i str) -> IResult<&'i str, Identifier<'i>, E>
where
    E: TagError<&'i str, &'static str>,
    E: ParseError<&'i str>,
{
    parse_identifier.cut().preceded_by(tag("as ")).parse(input)
}
