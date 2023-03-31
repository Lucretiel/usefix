// This is only here during develop to quiet down my editor
#![allow(dead_code)]

/*
Design notes from myself:

- syn is appealing, but it likely doesn't work with the git conflict tags,
  which is the entire idea
- You've had some neat ideas related to parser state forking and git conflict
  tags, where a parser can be made agnostic towards conflict tags, and instead
  is state-forked and merged when a tag is found. Not clear if this is possible
  with nested parsers and conflicts that interfere with nesting / repetition:

use aaa::{
    bbb,
>>>>>>>> CONFLICT
    ccc,
}

use zzz::{
========
    dddd,
<<<<<<<<
    eeee,
}

- Probably will need an incredibly bespoke parser here
- Need to support:
    - #[cfg]
    - doc comments
    - non-cfg attributes?
    - use itself
        - wildcards
        - nesting
        - `as` renames
        - self

 */

mod parsers;
mod tree;

use std::borrow::Cow;

use parsers::snip_whitespace;
use thiserror::Error;

struct FixedFileReport<'a> {
    /// The final, fixed content of the file
    lines: Vec<Cow<'a, str>>,

    /// Additional things we'd like to tell the user, in addition to the fixed
    /// files, which weren't important enough to be a whole error and prevent
    /// the fixed file from being produced.
    reports: Vec<()>,
}

/// Enum that tracks if we're in a git conflict right now
enum ConflictState {
    First,
    Second,
}

/// Error for unrecoverable errors while attempting to fix. Where possible we
/// should try to recover and include a report in the `FixedFileReport`, but
/// sometimes there's nothing you can do.
#[derive(Debug, Error)]
enum FixFileError {
    #[error("invalid git conflict marker on line {line_number}")]
    BadConflictMarker { line_number: u32 },
}

/**
Main entry point. This function performs all the work of fixing a file:
normalizing imports
 */
fn fix_file(input: &str) -> Result<FixedFileReport<'_>, FixFileError> {
    let mut out = Vec::with_capacity(input.as_bytes().iter().filter(|&&b| b == b'\n').count());
    let mut reports = Vec::new();
    let input_lines = input.lines();

    /*
    High level implementation notes

    The parser is a doozy. Fundamentally, we have two extremely contradictory
    syntaxes we're trying to deal with: Rust, which is structured, tree
    oriented, and whitespace insensitive; and git conflicts, which are line
    oriented and don't really care about rust's structure.

    There are some things that help us. Most importantly, we ONLY care about
    `use` items, which are syntatically pretty straightforward. Everything else
    we just pass through verbatim.

    The basic process is to *start* with the git conflicts (parsing the file
    line by line) and reconstruct the underlying rust structure as best we can.
    This ends up being easier than trying to write a whole recursive parser
    that's capable of handling git conflict markers at almost any point.

    We do require completely (or most) rustfmt'd code. We don't handle
    degenerate syntax, like when use items are spead too widely over newlines
    or stuff like that. We expect the use items to most be "normal".

    We do additionally have to handle `#[cfg(...)]` directives and doc comments.
    Other attributes are rejected (when attached to `use` items)
    */

    // *************** PARSER STATE LIVE HERE ***************

    // Frequently during processing, we *might* want to do something with a
    // line (for instance, if we hit a #[cfg]). However, if it turns out we
    // can't do anything with it (for instance, the #[cfg] was attached to a
    // struct definition, not a use item), we want to forward verbatim. Those
    // lines that we are *maybe* interested in are stored here.
    let mut buffered_lines: Vec<&str> = Vec::new();

    // Tracking for git conflicts
    let mut conflict_state: Option<ConflictState> = None;

    input_lines
        .zip(1u32..)
        .try_for_each(|(line, line_number)| {
            let (leading_whitespace, body) = snip_whitespace(line);

            if line.starts_with("<<<<<<< ") {
                if conflict_state.is_none() {
                    conflict_state = Some(ConflictState::First);
                    // TODO: create a parser snapshot here to return to later
                    return Ok(());
                } else {
                    return Err(FixFileError::BadConflictMarker { line_number });
                }
            } else if line == "=======" {
                if matches!(conflict_state, Some(ConflictState::First)) {
                    conflict_state = Some(ConflictState::Second);
                    // TODO: parser snapshot management
                    return Ok(());
                } else {
                    return Err(FixFileError::BadConflictMarker { line_number });
                }
            } else if line.starts_with(">>>>>>> ") {
                if matches!(conflict_state, Some(ConflictState::Second)) {
                    conflict_state = None;
                    // TODO: reconstruct the work
                }
            }

            Ok(())
        })?;

    Ok(FixedFileReport {
        lines: out,
        reports,
    })
}

fn main() {
    println!("Hello, world!");
}
