/*!
Parsers supporting usefix functionality. While the overal parser is highly
bespoke, we can still make use of these primitive parsers for the basics.
*/

use nom::{
    character::complete::alpha1,
    error::{ErrorKind, ParseError},
    IResult, Parser as _,
};
use nom_supreme::{
    tag::{complete::tag, TagError},
    ParserExt as _,
};
use unicode_xid::UnicodeXID as _;

use crate::tree::{self, Identifier};

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

/// Parse the "r#" prefix
fn parse_raw_prefix<'i, E>(input: &'i str) -> IResult<&'i str, &'i str, E>
where
    E: TagError<&'i str, &'static str>,
{
    tag("r#").parse(input)
}

/**
Parse a rust identifier, optionally preceeded by `r#`. The rules are the same
for both.
*/
pub fn parse_identifier<'i, E>(input: &'i str) -> IResult<&'i str, &'i str, E>
where
    E: TagError<&'i str, &'static str>,
    E: ParseError<&'i str>,
{
    parse_normal_identifier
        .opt_preceded_by(parse_raw_prefix)
        .recognize()
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
