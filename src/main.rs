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
mod tree;

use std::{
    borrow::Cow,
    fs::{self},
    path::PathBuf,
};

fn main() {}
