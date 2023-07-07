#![allow(unused_imports)]

use std::borrow::Cow;

use crate::{
    parsers::{Identifier, Visibility},
    tree::{TreeRoot, UseItem},
};

/// State to record lines of the file as they go by, and then either flush them
/// to an output buffer or discard them (if they've been rewritten)
#[derive(Default)]
pub struct LinesBuffer<'a> {
    lines: Vec<&'a str>,
}

impl<'a> LinesBuffer<'a> {
    pub fn push(&mut self, line: &'a str) {
        self.lines.push(line)
    }

    pub fn flush_to(&mut self, dest: &mut Vec<Cow<'a, str>>) {
        dest.extend(self.lines.iter().copied().map(Cow::Borrowed));
        self.lines.clear();
    }

    pub fn discard(&mut self) {
        self.lines.clear()
    }

    /// Discard all the lines, except for any suffix of empty / whitespace lines
    pub fn discard_block(&mut self) {
        let point = self
            .lines
            .iter()
            .map(|line| line.trim())
            .rposition(|line| !line.is_empty())
            .unwrap_or(self.lines.len());

        self.lines.drain(..point);
    }
}

/// States that can live in a ParseStack. In general these are named after the
/// most recent thing that *was* parsed and is still expecting more after it
#[derive(Debug, Clone, Copy)]
pub enum ParseFrame<'a> {
    /// We are at the very top level, looking for `use` items, or forwarding
    /// uniniteresting rust code verbatim.
    Top,

    /// We just parsed `use` or `pub use` or something like that. We're looking
    /// for an identifier, path root, or block starter.
    Use,

    /// We just parsed the path separator ::. We're looking for an identifier,
    /// block starter, or `*`.
    PathSeparator,

    /// We just parsed an identifer. We're looking for a path separator, `as`,
    /// or certain terminators (`}` or `,` if we're in a block, `;` otherwise).
    Identifier(Identifier<'a>),

    /// We just parsed a `*`. Similar to an identifier, but we only want a
    /// terminator or separator
    Wildcard,

    /// We just parsed a `{`. We're looking for an identifier, path root,
    /// wildcard, block starter, or block terminator.
    BlockStart,

    /// We just parsed a whole block (specifically, the terminating }). We
    /// want some kind of separator or another terminator.
    Block,
}

/// In the course of recursively parsing things (especially either doc comments
/// or use items), this tracks the stack of parser states.
#[derive(Debug, Default, Clone)]
pub struct ParseStack<'a> {
    stack: Vec<ParseFrame<'a>>,
}

impl<'a> ParseStack<'a> {
    #[inline]
    #[must_use]
    pub fn top(&self) -> ParseFrame<'a> {
        self.stack.last().copied().unwrap_or(ParseFrame::Top)
    }

    /// Add a new state to the stack.
    pub fn push(&mut self, state: ParseFrame<'a>) {
        self.stack.push(state)
    }

    #[inline]
    #[must_use]
    pub fn in_block(&self) -> bool {
        self.stack
            .iter()
            .any(|state| matches!(*state, ParseFrame::BlockStart))
    }

    /// Check if the current path is rooted (that is, it starts with a ::).
    /// Returns false if the path is empty.
    #[inline]
    #[must_use]
    pub fn rooted(&self) -> bool {
        self.stack
            .iter()
            .find_map(|state| match state {
                ParseFrame::PathSeparator => Some(true),
                ParseFrame::Identifier(_) => Some(false),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Get the current path represented by this parse stack. This indicates
    /// where in the tree additionally parsed items should be added.
    #[inline]
    pub fn path(&self) -> impl Iterator<Item = Identifier<'a>> + '_ {
        // TODO: figure out how smart to make this (how to handle crate, super,
        // self, etc)
        self.stack
            .iter()
            .filter_map(|state| match *state {
                ParseFrame::Identifier(ident) => Some(ident),
                _ => None,
            })
            .filter(|ident| ident.get() != "self")
    }

    pub fn rooted_path(&self) -> Option<(TreeRoot<'a>, impl Iterator<Item = Identifier<'a>> + '_)> {
        let mut path = self.path();
        let rooted = self.rooted();
        path.next()
            .map(|identifier| (TreeRoot { rooted, identifier }, path))
    }

    /// Check if there's at least one identifier in the path for this parse
    /// stack
    #[inline]
    #[must_use]
    pub fn in_path(&self) -> bool {
        self.path().next().is_some()
    }

    /// Pop all elements from the stack up to (but excluding) the nearest
    /// BlockStart, or pop the whole stack if we're not in a block.
    pub fn pop_to_block_start(&mut self) {
        let point = self
            .stack
            .iter()
            .rposition(|frame| matches!(frame, ParseFrame::BlockStart))
            .map(|idx| idx + 1)
            .unwrap_or(0);

        self.stack.truncate(point);
    }

    /// Pop all elements from a stack up to (and including) the nearest
    /// BlockStart, or pop the whole stack if we're not in a block.

    pub fn end_block(&mut self) {
        let point = self
            .stack
            .iter()
            .rposition(|frame| matches!(frame, ParseFrame::BlockStart))
            .unwrap_or(0);

        self.stack.truncate(point)
    }
}

/// The *overall* set of state for an ongoing parse. This type must be
/// cloneable, since it is saved, restored, and merged when git conflicts are
/// discovered.
#[derive(Default, Debug, Clone)]
pub struct ParseState<'a> {
    /// The set of `use` items in the current set
    pub uses: Vec<UseItem<'a>>,

    /// The `use` item being updated in place
    pub current_use_item: Option<UseItem<'a>>,

    /// The current recursive parser stack
    pub stack: ParseStack<'a>,
}

impl<'a> ParseState<'a> {
    pub fn start_use_item(&mut self, visibility: Option<Visibility<'a>>) {
        self.finish_use_item();
        self.current_use_item = Some(UseItem::new(visibility))
    }

    pub fn finish_use_item(&mut self) {
        if let Some(use_item) = self.current_use_item.take() {
            self.uses.push(use_item)
        }
    }
}
