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
mod state;
mod tree;

use std::{
    borrow::Cow,
    fs::{self},
    path::PathBuf,
};

use clap::Parser;
use parsers::{parse_identifier, parse_identifier_like, parse_use_prefix, snip_whitespace};
use state::{LinesBuffer, ParseFrame, ParseStack, ParseState};
use thiserror::Error;
use tree::NameUse;

struct FixedFileReport<'a> {
    /// The final, fixed content of the file
    lines: Vec<Cow<'a, str>>,

    /// Additional things we'd like to tell the user, in addition to the fixed
    /// files, which weren't important enough to be a whole error and prevent
    /// the fixed file from being produced.
    reports: Vec<()>,
}

/// Enum that tracks if we're in a git conflict right now
enum ConflictState<'a> {
    /// We're in the first half of a git conflict. We store a snapshot of the
    /// state before the conflict.
    First(ParseState<'a>),

    /// We're in the second half of a git conflict. We store the state as of
    /// the first conflict.
    Second(ParseState<'a>),
}

/// Error for unrecoverable errors while attempting to fix. Where possible we
/// should try to recover and include a report in the `FixedFileReport`, but
/// sometimes there's nothing you can do.
#[derive(Debug, Error)]
enum FixFileError<'i> {
    #[error("invalid git conflict marker on line {line_number}")]
    BadConflictMarker { line_number: u32 },

    #[error("invalid syntax on line {line_number}")]
    ParseError {
        line_number: u32,
        stack: ParseStack<'i>,
        tail: String,
    },
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
    // TODO: replace this with a type, to make it easy to flush it out to
    // `out`.
    let mut buffered_lines = LinesBuffer::default();

    // Tracking for git conflicts
    let mut conflict_state: Option<ConflictState> = None;

    // The stack of subparser's we're currently resolving. Mostly this is used
    // for tracking nested {{}} in `use` items, and nested `/* */` comments.
    // Note that the states in this stack are orthogonal to the git conflict
    // state.
    let mut parse_state = ParseState::default();

    // *************** MAIN PARSE LOOP STARTS HERE ***************

    for (line, line_number) in input_lines.zip(1u32..) {
        buffered_lines.push(line);

        // Check for git conflict markers
        if line.starts_with("<<<<<<< ") {
            if conflict_state.is_none() {
                conflict_state = Some(ConflictState::First(parse_state.clone()));
                continue;
            } else {
                return Err(FixFileError::BadConflictMarker { line_number });
            }
        } else if line == "=======" {
            if let Some(ConflictState::First(state_snapshot)) = conflict_state.take() {
                conflict_state = Some(ConflictState::Second(parse_state));
                parse_state = state_snapshot;
                continue;
            } else {
                return Err(FixFileError::BadConflictMarker { line_number });
            }
        } else if line.starts_with(">>>>>>> ") {
            if let Some(ConflictState::Second(state_snapshot)) = conflict_state.take() {
                todo!("merge the state_snapshot and parse_state");
                // TODO: reconstruct the work. We're just going to concatenate
                // the use blocks, and rely on future deduplication to fix.
                continue;
            } else {
                return Err(FixFileError::BadConflictMarker { line_number });
            }
        }

        // All other syntax is subject to indentation, record that here
        let (indent, mut line_body) = snip_whitespace(line);

        // Parse loop working through tokens in the line.
        // TODO: Find a way to structure this into a parse table instead of
        // this crazy wild west thing
        loop {
            if line_body.is_empty() {
                break;
            }

            match parse_state.stack.top() {
                ParseFrame::Top => {
                    // We're interested in ~3 things: doc comments, attributes,
                    // and `use` items. Everything else we forward.
                    // TODO: For now we totally ignore doc comments and
                    // attributes; we'll revisit them after we have a working
                    // prototype.
                    if let Some((suffix, visibility)) = match parse_use_prefix(line_body) {
                        Ok(parsed) => Some(parsed),
                        Err(nom::Err::Failure(())) => {
                            return Err(FixFileError::ParseError {
                                line_number,
                                stack: parse_state.stack,
                                tail: line_body.to_owned(),
                            })
                        }
                        Err(_) => None,
                    } {
                        parse_state.start_use_item(visibility);
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::Use);
                        continue;
                    }
                    // TODO: This is where to check for doc comments and
                    // attributes, when the time comes
                    // if line_body.is_attribute...
                    else {
                        line_body = "";
                        match conflict_state {
                            Some(_) => panic!("can't currently handle ")
                        }
                    }

                    line_body = "";

                    // Ok, the line body is something we don't care about. This
                    // means we need to compute and flush the set of use items
                    // in this block, followed by this line. UNLESS we're in
                    // a git conflict, in which case we have to give up on the
                    // current use item block. What a mess.
                    // TODO: This part right here (see comment above)
                }
                ParseFrame::Use => {
                    if let Some(suffix) = line_body.strip_prefix('{') {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::BlockStart);
                    } else if let Some(suffix) = line_body.strip_prefix("::") {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::PathSeparator);
                    } else if let Some((suffix, ident)) = match parse_identifier(line_body) {
                        Ok((suffix, ident)) => Some((suffix, ident)),
                        Err(nom::Err::Failure(())) => {
                            return Err(FixFileError::ParseError {
                                line_number,
                                stack: parse_state.stack,
                                tail: line_body.to_owned(),
                            })
                        }
                        _ => None,
                    } {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::Identifier(ident));
                    } else {
                        // We're in the middle of a `use` item, so anything
                        // else here is a parse error
                        return Err(FixFileError::ParseError {
                            line_number,
                            stack: parse_state.stack,
                            tail: line_body.to_owned(),
                        });
                    }
                }
                ParseFrame::PathSeparator => {
                    if let Some(suffix) = line_body.strip_prefix('*') {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::Wildcard);
                    } else if let Some(suffix) = line_body.strip_prefix('{') {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::BlockStart);
                    } else if let Some((suffix, ident)) = match parse_identifier(line_body) {
                        Ok((suffix, ident)) => Some((suffix, ident)),
                        Err(nom::Err::Failure(())) => {
                            return Err(FixFileError::ParseError {
                                line_number,
                                stack: parse_state.stack,
                                tail: line_body.to_owned(),
                            })
                        }
                        _ => None,
                    } {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::Identifier(ident));
                    } else {
                        // We're in the middle of a `use` item, so anything
                        // else here is a parse error
                        return Err(FixFileError::ParseError {
                            line_number,
                            stack: parse_state.stack,
                            tail: line_body.to_owned(),
                        });
                    }
                }
                ParseFrame::Wildcard => {
                    // Every possible (correct) parse at this point results in
                    // a wildcard being inserted into the tree, so do that
                    // first
                    let Some((root, path)) = parse_state.stack.rooted_path() else {
                        // We tried to parse `use ::*;` which is illegal
                        return Err(FixFileError::ParseError {
                            line_number,
                            stack: parse_state.stack,
                            tail: line_body.to_owned()
                        })
                    };

                    parse_state.current_use_item.as_mut().unwrap().insert(
                        root,
                        path,
                        tree::Leaf::Wildcard,
                    );

                    // Because of how rustfmt works, we expect the terminator
                    // to always appear on the same line as the identifier or
                    // its alias.
                    if parse_state.stack.in_block() {
                        if let Some(suffix) = line_body.strip_prefix(',') {
                            line_body = suffix.trim_start();
                            parse_state.stack.pop_to_block_start();
                        } else if let Some(suffix) = line_body.strip_prefix('}') {
                            line_body = suffix.trim_start();
                            parse_state.stack.end_block();
                            parse_state.stack.push(ParseFrame::Block);
                        } else {
                            return Err(FixFileError::ParseError {
                                line_number,
                                stack: parse_state.stack,
                                tail: line_body.to_owned(),
                            });
                        }
                    } else if let Some(suffix) = line_body.strip_prefix(';') {
                        line_body = suffix.trim_start();
                        parse_state.stack.pop_to_block_start();
                    } else {
                        return Err(FixFileError::ParseError {
                            line_number,
                            stack: parse_state.stack,
                            tail: line_body.to_owned(),
                        });
                    }
                }
                ParseFrame::Identifier(_) => {
                    if let Some(suffix) = line_body.strip_prefix("::") {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::PathSeparator);
                        continue;
                    }

                    // Check for an alias. Alias can appear before a terminator.
                    let alias: NameUse = {
                        if let Some(suffix) = line_body.strip_prefix("as") {
                            match parse_identifier_like(suffix.trim_start()) {
                                Ok((suffix, alias)) => {
                                    line_body = suffix.trim_start();
                                    NameUse::Renamed(alias)
                                }
                                // We parsed an `as`, so any parse error here
                                // is "real"
                                Err(nom::Err::Error(()) | _) => {
                                    return Err(FixFileError::ParseError {
                                        line_number,
                                        stack: parse_state.stack,
                                        tail: line_body.to_owned(),
                                    })
                                }
                            }
                        } else {
                            NameUse::Used
                        }
                    };

                    // At this point, the only valid parse is a terminator,
                    // which means that this identifier is being inserted.
                    let (root, path) = parse_state
                        .stack
                        .rooted_path()
                        .expect("identifier was parsed, so path should exist");

                    parse_state.current_use_item.as_mut().unwrap().insert(
                        root,
                        path,
                        tree::Leaf::Used(alias),
                    );

                    // Because of how rustfmt works, we expect the terminator
                    // to always appear on the same line as the identifier or
                    // its alias.
                    if parse_state.stack.in_block() {
                        if let Some(suffix) = line_body.strip_prefix(',') {
                            line_body = suffix.trim_start();
                            parse_state.stack.pop_to_block_start();
                        } else if let Some(suffix) = line_body.strip_prefix('}') {
                            line_body = suffix.trim_start();
                            parse_state.stack.end_block();
                            parse_state.stack.push(ParseFrame::Block);
                        } else {
                            return Err(FixFileError::ParseError {
                                line_number,
                                stack: parse_state.stack,
                                tail: line_body.to_owned(),
                            });
                        }
                    } else if let Some(suffix) = line_body.strip_prefix(';') {
                        line_body = suffix.trim_start();
                        parse_state.stack.pop_to_block_start();
                    } else {
                        return Err(FixFileError::ParseError {
                            line_number,
                            stack: parse_state.stack,
                            tail: line_body.to_owned(),
                        });
                    }
                }
                // Note that this can mean we just parsed a `{`, but it can
                // also mean we just parsed a `,` (as in, `{abc::def,`), which
                // are functionally the same state
                ParseFrame::BlockStart => {
                    if let Some(suffix) = line_body.strip_prefix('{') {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::BlockStart);
                    } else if let Some(suffix) = line_body.strip_prefix("::") {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::PathSeparator);
                    } else if let Some(suffix) = line_body.strip_prefix('*') {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::Wildcard);
                    } else if let Some(suffix) = line_body.strip_prefix('}') {
                        line_body = suffix.trim_start();
                        parse_state.stack.end_block();
                        parse_state.stack.push(ParseFrame::Block);
                    } else if let Some((suffix, ident)) = match parse_identifier(line_body) {
                        Ok((suffix, ident)) => Some((suffix, ident)),
                        Err(nom::Err::Failure(())) => {
                            return Err(FixFileError::ParseError {
                                line_number,
                                stack: parse_state.stack,
                                tail: line_body.to_owned(),
                            })
                        }
                        _ => None,
                    } {
                        line_body = suffix.trim_start();
                        parse_state.stack.push(ParseFrame::Identifier(ident));
                    } else {
                        // We're in the middle of a `use` item, so anything
                        // else here is a parse error
                        return Err(FixFileError::ParseError {
                            line_number,
                            stack: parse_state.stack,
                            tail: line_body.to_owned(),
                        });
                    }
                }
                ParseFrame::Block => {
                    if parse_state.stack.in_block() {
                        if let Some(suffix) = line_body.strip_prefix(',') {
                            line_body = suffix.trim_start();
                            parse_state.stack.pop_to_block_start();
                        } else if let Some(suffix) = line_body.strip_prefix('}') {
                            line_body = suffix.trim_start();
                            parse_state.stack.end_block();
                            parse_state.stack.push(ParseFrame::Block);
                        } else {
                            return Err(FixFileError::ParseError {
                                line_number,
                                stack: parse_state.stack,
                                tail: line_body.to_owned(),
                            });
                        }
                    } else if let Some(suffix) = line_body.strip_prefix(';') {
                        line_body = suffix.trim_start();
                        parse_state.stack.pop_to_block_start();
                    } else {
                        return Err(FixFileError::ParseError {
                            line_number,
                            stack: parse_state.stack,
                            tail: line_body.to_owned(),
                        });
                    }
                }
            }
        }
    }

    eprintln!("{:#?}", parse_state.uses);

    Ok(FixedFileReport {
        lines: out,
        reports,
    })
}

fn main() {
    #[derive(Parser)]
    struct Args {
        /// The rust file to operate on
        #[arg(short, long)]
        file: PathBuf,
    }

    let args = Args::parse();
    let content = fs::read_to_string(args.file).expect("file should exist");
    fix_file(&content).expect("file should parse");
}
