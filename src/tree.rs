/*!
Module for data structures representing an arbitrary `use` declaration. It is comprehensive
(can losslessly handle *any* `use` item) but exists in normalized
representation (so it doesn't distinguish between `use abc::def` and
`use abc::def::self`.)
*/

use std::cmp;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialIdentifier {
    Super,
    /// Equivelent to `self`
    This,
    Crate,
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

/// If a name is being imported, it either keeps its own name or is renamed
#[derive(Debug, Clone, Copy)]
pub enum NameUse<'a> {
    Used,
    Renamed(Identifier<'a>),
}

#[derive(Debug, Clone)]
pub enum Children<'a> {
    Wildcard,
    Subtrees(BTreeMap<Identifier<'a>, Branches<'a>>),
}

/**
Collection of things that can be associated with a subtree in a use declaration.

Specifically, given `use abc::def...`, this is all of the things that "belong"
to "def".
 */
#[derive(Debug, Clone)]
pub struct Branches<'a> {
    /// If not none, this item is itself being imported, either using its own
    /// name or a rename.
    used: Option<NameUse<'a>>,

    /// The set of child paths
    children: Children<'a>,
}

#[derive(Debug, Clone)]
pub enum Visibility<'a> {
    Public,
    Crate,
    This,
    Super,
    In(SimplePath<'a>),
}

/**
The very top level struct for a single `use` item
*/
pub struct UseItem<'a> {
    docs: Vec<&'a str>,
    configs: Vec<&'a str>,
    visibility: Option<Visibility<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimplePath<'a> {
    root: TreeRoot<'a>,
    children: Vec<Identifier<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TreeRoot<'a> {
    /// If true, this identifier was prefixed with `::`.
    rooted: bool,
    identifier: Identifier<'a>,
}
