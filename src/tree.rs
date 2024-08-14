/*!
Module for data structures representing an arbitrary `use` declaration.
It is comprehensive (can losslessly handle *any* `use` item) but exists in
normalized representation (so it doesn't distinguish between `use abc::def`
and `use abc::def::self`.)
*/

use std::collections::BTreeMap;
use std::hash::Hash;

use crate::parsers::{Identifier, IdentifierLike, Visibility};

/// If a name is being imported, it either keeps its own name or is renamed
#[derive(Debug, Clone, Copy)]
pub enum NameUse<'a> {
    /// `::name`
    Used,

    /// `::name as alias`
    Renamed(IdentifierLike<'a>),
}

#[derive(Debug, Clone, Copy)]
pub enum Leaf<'a> {
    Used(NameUse<'a>),
    Wildcard,
}

/**
Collection of things that can be associated with a subtree in a use declaration.

Specifically, given `use abc::def...`, this is all of the things that "belong"
to "def".

Note that at least one of these fields must be non-empty in order for this
to be valid
 */
#[derive(Debug, Clone, Default)]
pub struct Branches<'a> {
    /// If not none, this item is itself being imported, either using its own
    /// name or a rename.
    used: Option<NameUse<'a>>,

    /// If true, the * wildcard is being imported at this point
    wildcard: bool,

    /// The set of child paths
    children: BTreeMap<Identifier<'a>, Branches<'a>>,
}

enum CleanResult {
    Alive,
    Dead,
}

impl<'a> Branches<'a> {
    pub fn insert(&mut self, mut path: impl Iterator<Item = Identifier<'a>>, leaf: Leaf<'a>) {
        match path.next() {
            None => match leaf {
                Leaf::Wildcard => self.wildcard = true,
                Leaf::Used(usage) => self.used = Some(usage),
            },
            Some(component) => self
                .children
                .entry(component)
                .or_default()
                .insert(path, leaf),
        }
    }

    /// Clean these branches: remove any empty children, and additionally remove
    /// any imports that are a direct sibling to a wildcard
    pub fn clean(&mut self) -> bool {
        self.children.retain(|_, branches| {
            if self.wildcard && matches!(branches.used, Some(NameUse::Used)) {
                branches.used = None;
            }

            branches.clean()
        });

        self.used.is_some() || self.wildcard || !self.children.is_empty()
    }
}

/**
The very top level struct for a single `use` item
*/
#[derive(Debug, Clone)]
pub struct UseItem<'a> {
    /// All of the docs for this use. This should contain the full set of lines
    /// of rustdocs attached to the item.
    pub docs: Vec<&'a str>,

    /// All of the cfg items attached to this `use`. This should specifically
    /// contain the stuff inside the parenthesis, for each #[cfg()]
    pub configs: Vec<&'a str>,

    /// Any `pub`, `pub(crate)`, etc associated with this use
    pub visibility: Option<Visibility<'a>>,

    /// The tree of imports in the use item.
    pub children: BTreeMap<TreeRoot<'a>, Branches<'a>>,
}

impl<'a> UseItem<'a> {
    pub fn insert(
        &mut self,
        root: TreeRoot<'a>,
        path: impl Iterator<Item = Identifier<'a>>,
        leaf: Leaf<'a>,
    ) {
        self.children.entry(root).or_default().insert(path, leaf)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimplePath<'a> {
    pub root: TreeRoot<'a>,
    pub children: Vec<Identifier<'a>>,
}

/// An identifier that might be prefixed with `::`. The very root of a tree is
/// an identifier like this (so `::core::iter` is different than `core::iter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TreeRoot<'a> {
    /// If true, this identifier was prefixed with `::`.
    pub rooted: bool,

    // The identifier itself
    pub identifier: Identifier<'a>,
}
