/*!
Module for data structures representing an arbitrary `use` declaration.
It is comprehensive (can losslessly handle *any* `use` item) but exists in
normalized representation (so it doesn't distinguish between `use abc::def`
and `use abc::def::self`.)
*/

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt::{self, Display},
    hash::Hash,
};

use proc_macro2::Span;
use syn::{spanned::Spanned, PathSegment};
use syn::{AttrStyle, Expr, ExprLit, Ident, Lit, Meta, Path, UseName, UseRename, UseTree};

#[derive(Debug, PartialEq, Eq)]
pub enum Visibility {
    /// `pub`
    Public,

    /// `pub(crate)`
    Crate,

    /// `pub(self)`
    This,

    /// `pub(super)`
    Super,

    /// `pub(in PATH)`
    In(Path),
}

impl Visibility {
    pub fn from_syn_vis(vis: syn::Visibility) -> Result<Option<Self>, CreateUseItemError> {
        match vis {
            syn::Visibility::Public(_) => Ok(Some(Visibility::Public)),
            syn::Visibility::Restricted(vis) => match vis.in_token {
                Some(_) => Ok(Some(Visibility::In(*vis.path))),
                None if vis.path.is_ident("crate") => Ok(Some(Visibility::Crate)),
                None if vis.path.is_ident("self") => Ok(Some(Visibility::This)),
                None if vis.path.is_ident("super") => Ok(Some(Visibility::Super)),
                None => Err(CreateUseItemError::MalformedVisibility),
            },
            syn::Visibility::Inherited => Ok(None),
        }
    }
}

fn fmt_path(path: &Path) -> impl Display + '_ {
    lazy_format::make_lazy_format!(|f| {
        if path.leading_colon.is_some() {
            write!(f, "::")?;
        }

        // We know, from the syn parser, that the path here doesn't have any
        // fucky nonsense going on, so we can just write the idents (for
        // context, check out the `PathSegment` type for the fucky nonsense
        // we're ignoring.)

        let mut idents = path.segments.iter();

        match idents.next() {
            Some(PathSegment { ident, .. }) => {
                write!(f, "{ident}")?;
                idents.try_for_each(|PathSegment { ident, .. }| write!(f, "::{ident}"))
            }
            None => Ok(()),
        }
    })
}

impl Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Visibility::Public => write!(f, "pub"),
            Visibility::Crate => write!(f, "pub(crate)"),
            Visibility::This => write!(f, "pub(self)"),
            Visibility::Super => write!(f, "pub(super)"),
            Visibility::In(path) => {
                let path = fmt_path(path);
                write!(f, "pub(in {path})")
            }
        }
    }
}

/// If a name is being imported, it either keeps its own name or is renamed
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NameUse {
    /// `::name`
    Used,

    /// `::name as alias`
    Renamed(Ident),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Copy, Hash)]
pub enum Rooted {
    Rooted,
    Unrooted,
}

#[derive(Debug, Clone)]
pub enum Leaf {
    Used(NameUse),
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
pub struct Branches {
    /// If not none, this item is itself being imported, either using its own
    /// name or a rename (or, god forbid, some combination)
    pub used: HashSet<NameUse>,

    /// If true, the * wildcard is being imported at this point
    pub wildcard: bool,

    /// The set of child paths
    pub children: HashMap<Ident, Branches>,
}

enum CleanResult {
    Alive,
    Dead,
}

impl Branches {
    pub fn insert(&mut self, mut path: impl Iterator<Item = Ident>, leaf: Leaf) {
        match path.next() {
            None => match leaf {
                Leaf::Wildcard => self.wildcard = true,
                Leaf::Used(usage) => {
                    self.used.insert(usage);
                }
            },
            Some(component) => self
                .children
                .entry(component)
                .or_default()
                .insert(path, leaf),
        }
    }

    fn get_subtree(&mut self, location: Ident) -> &mut Self {
        if location == "self" {
            self
        } else {
            self.children.entry(location).or_default()
        }
    }
}

/// The contents of a single `#[cfg(...)]`.
#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Config(String);

impl Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let config = self.0.as_str();
        write!(f, "#[cfg({config})]")
    }
}

#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
// This should contain a list of TokenStreams, but TokenStream doesn't implement
// Ord or Hash and we want to use it as a key in a table.
pub struct ConfigsList(BTreeSet<Config>);

impl ConfigsList {
    pub const EMPTY: Self = ConfigsList(BTreeSet::new());

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn configs(&self) -> impl Iterator<Item = &Config> + '_ {
        self.0.iter()
    }
}

#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Default, Clone)]
pub struct DocsList(pub Vec<String>);

impl DocsList {
    fn bytes(&self) -> impl DoubleEndedIterator<Item = u8> + '_ {
        self.0.iter().flat_map(|s| s.bytes())
    }

    /// The total length of these docs, in bytes
    fn len(&self) -> usize {
        self.0.iter().map(|s| s.len()).sum()
    }

    /// Returns true if either `self` or `other` is a prefix of the other.
    fn either_prefix(&self, other: &Self) -> bool {
        let mut self_bytes = self.bytes();
        let mut other_bytes = other.bytes();

        loop {
            match (self_bytes.next(), other_bytes.next()) {
                (Some(a), Some(b)) if a == b => {}
                (None, _) => return true,
                (_, None) => return true,
                _ => return false,
            }
        }
    }

    /// Returns true if either `self` or `other` is a suffix of the other.
    fn either_suffix(&self, other: &Self) -> bool {
        let mut self_bytes = self.bytes().rev();
        let mut other_bytes = other.bytes().rev();

        loop {
            match (self_bytes.next(), other_bytes.next()) {
                (Some(a), Some(b)) if a == b => {}
                (None, _) => return true,
                (_, None) => return true,
                _ => return false,
            }
        }
    }

    /// Combine two docs. The algorithm here is pretty dumb: if either is a
    /// prefix or suffix of the other, we take the longer one. Otherwise, we
    /// just concatenate them.
    pub fn combine(&mut self, other: &Self) {
        if self.either_prefix(other) || self.either_suffix(other) {
            if self.len() < other.len() {
                *self = other.clone()
            }
        } else {
            self.0.extend(other.0.iter().cloned());
        }
    }
}

/**
The very top level struct for a single `use` item
*/
#[derive(Debug)]
pub struct UseItem {
    /// All of the docs for this use. This should contain the full set of lines
    /// of rustdocs attached to the item.
    pub docs: DocsList,

    /// All of the cfg items attached to this `use`. This should specifically
    /// contain the stuff inside the parenthesis, for each #[cfg(THIS_STUFF)]
    pub configs: ConfigsList,

    /// Any `pub`, `pub(crate)`, etc associated with this use
    pub visibility: Option<Visibility>,

    /// The tree of imports in the use item.
    pub children: HashMap<TreeRoot, Branches>,

    /// The span of the syn Use Item from which this was generated
    pub span: Span,
}

impl UseItem {
    pub fn from_syn_use_item(item: syn::ItemUse) -> Result<UseItem, CreateUseItemError> {
        let span = item.span();

        let mut docs = Vec::new();
        let mut configs = BTreeSet::new();

        // Handle all attributes. Collect doc and cfg attributes, and reject
        // items that have other attributes.
        for attr in item.attrs {
            if matches!(attr.style, AttrStyle::Inner(_)) {
                return Err(CreateUseItemError::InnerAttributes);
            }

            match attr.meta {
                Meta::List(attr) => {
                    if !matches!(attr.delimiter, syn::MacroDelimiter::Paren(_)) {
                        return Err(CreateUseItemError::UnrecognizedAttribute);
                    }

                    if attr.path.is_ident("cfg") {
                        configs.insert(Config(attr.tokens.to_string()));
                    } else {
                        return Err(CreateUseItemError::UnrecognizedAttribute);
                    }
                }
                Meta::NameValue(attr) => {
                    if attr.path.is_ident("doc") {
                        // Doc attributes should contain precisely a single string
                        match attr.value {
                            Expr::Lit(ExprLit {
                                attrs,
                                lit: Lit::Str(content),
                            }) if attrs.is_empty() => {
                                docs.push(content.value());
                            }
                            _ => return Err(CreateUseItemError::MalformedDocAttribute),
                        }
                    } else {
                        return Err(CreateUseItemError::UnrecognizedAttribute);
                    }
                }
                Meta::Path(_) => return Err(CreateUseItemError::UnrecognizedAttribute),
            }
        }

        let visibility = Visibility::from_syn_vis(item.vis)?;

        let mut children = HashMap::new();
        build_use_item_children_root(
            item.tree,
            match item.leading_colon {
                Some(_) => Rooted::Rooted,
                None => Rooted::Unrooted,
            },
            &mut children,
        )?;

        Ok(Self {
            docs: DocsList(docs),
            configs: ConfigsList(configs),
            visibility,
            children,
            span,
        })
    }
}

fn build_use_item_children_root(
    tree: UseTree,
    rooted: Rooted,
    children: &mut HashMap<TreeRoot, Branches>,
) -> Result<(), CreateUseItemError> {
    match tree {
        UseTree::Path(path) => {
            let subtree = children
                .entry(TreeRoot {
                    rooted,
                    identifier: path.ident,
                })
                .or_default();

            build_use_item_children_branches(*path.tree, subtree);
            Ok(())
        }
        UseTree::Name(UseName { ident }) => {
            let subtree = children
                .entry(TreeRoot {
                    rooted,
                    identifier: ident,
                })
                .or_default();

            subtree.used.insert(NameUse::Used);

            Ok(())
        }
        UseTree::Rename(rename) => {
            let subtree = children
                .entry(TreeRoot {
                    rooted,
                    identifier: rename.ident,
                })
                .or_default();

            subtree.used.insert(NameUse::Renamed(rename.rename));

            Ok(())
        }
        UseTree::Glob(_) => Err(CreateUseItemError::UseStar),
        UseTree::Group(group) => group
            .items
            .into_iter()
            .try_for_each(|tree| build_use_item_children_root(tree, rooted, children)),
    }
}

fn build_use_item_children_branches(tree: UseTree, branches: &mut Branches) {
    match tree {
        UseTree::Path(path) => {
            let subtree = branches.get_subtree(path.ident);
            build_use_item_children_branches(*path.tree, subtree)
        }
        UseTree::Name(UseName { ident }) => {
            let subtree = branches.get_subtree(ident);
            subtree.used.insert(NameUse::Used);
        }
        UseTree::Rename(UseRename { ident, rename, .. }) => {
            let subtree = branches.get_subtree(ident);
            subtree.used.insert(NameUse::Renamed(rename));
        }
        UseTree::Glob(_) => {
            branches.wildcard = true;
        }
        UseTree::Group(group) => group
            .items
            .into_iter()
            .for_each(|item| build_use_item_children_branches(item, branches)),
    }
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum CreateUseItemError {
    #[error("use item has inner attributes")]
    InnerAttributes,

    #[error("use item has an attribute we didn't recognize. Only `cfg` and `doc` are supported.")]
    UnrecognizedAttribute,

    #[error("found a doc attribute, but it was malformed in some way")]
    MalformedDocAttribute,

    #[error("the visibility of the use item was malformed")]
    MalformedVisibility,

    #[error("tried to use the whole universe (`use *`) or something like that")]
    UseStar,
}

/// An identifier that might be prefixed with `::`. The very root of a tree is
/// an identifier like this (so `::core::iter` is different than `core::iter`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TreeRoot {
    /// If true, this identifier was prefixed with `::`.
    pub rooted: Rooted,

    // The identifier itself
    pub identifier: Ident,
}
