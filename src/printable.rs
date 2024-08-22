use std::{
    cmp::Ord,
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    fmt::{self, Display, Formatter},
};

use itertools::Itertools;
use syn::Ident;

use crate::{
    flattened::{SingleUsedItem, UsedItemLeaf},
    tree::{ConfigsList, DocsList, Rooted, Visibility},
};

/// The way a name is used:
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
enum NameUse<'a> {
    Used,
    Renamed(&'a Ident),
}

/// The list of things that can happen at path `a::b`
enum PrintableChild<'a> {
    /// Just `a::b` or `a::b as c`
    Plain(NameUse<'a>),

    /// `a::b::{...}` or `a::b::c`
    Subtree(PrintableTree<'a>),
}

impl<'a> PrintableChild<'a> {
    /// If this child is already a subtree, return it. Otherwise, convert it
    /// into a subtree by adding `self` parameters based on its current value.
    ///
    /// In other words, this converts `a::b` into `a::b::{self}`, in
    /// anticipation of `self` gaining some siblings.
    pub fn become_subtree(&mut self) -> &mut PrintableTree<'a> {
        let usage = match *self {
            PrintableChild::Subtree(ref mut tree) => return tree,
            PrintableChild::Plain(usage) => usage,
        };

        *self = PrintableChild::Subtree(PrintableTree {
            this_usage: BTreeSet::from([usage]),
            wildcard: false,
            children: BTreeMap::new(),
        });

        match *self {
            PrintableChild::Subtree(ref mut tree) => tree,
            _ => unreachable!("just asssigned *self to be a Subtree"),
        }
    }

    pub fn add_usage(&mut self, usage: NameUse<'a>) {
        if let Self::Plain(current_usage) = *self {
            if current_usage == usage {
                return;
            }
        }

        let tree = self.become_subtree();
        tree.this_usage.insert(usage);
    }
}

/// A printable tree is a collection of things that can appear inside {} in
/// an import path. When printed, it will automatically include or omit the {},
/// depending on if it contains one item or more than one item.
pub struct PrintableTree<'a> {
    // Whether this tree contains a field called `self` or any fields
    // called `self as rename`
    this_usage: BTreeSet<NameUse<'a>>,

    // Whether this tree contains a field called `*`
    wildcard: bool,

    // All of the other fields in this tree
    children: BTreeMap<&'a Ident, PrintableChild<'a>>,
}

impl<'a> PrintableTree<'a> {
    // This constructor is private because we don't ever really want it to be
    // possible to create an empty tree. Locally it's okay because we always
    // take care to `.add_path()` to it immediately after creation.
    fn new() -> Self {
        Self {
            this_usage: BTreeSet::new(),
            wildcard: false,
            children: BTreeMap::new(),
        }
    }

    /// Create a new tree containing a single path
    pub fn new_from_path(
        path: impl IntoIterator<Item = &'a Ident>,
        leaf: &UsedItemLeaf<'a>,
    ) -> Self {
        let mut tree = Self::new();
        tree.add_path(path, leaf);
        tree
    }

    /// Add another path to a tree
    pub fn add_path(&mut self, path: impl IntoIterator<Item = &'a Ident>, leaf: &UsedItemLeaf<'a>) {
        let mut path = path.into_iter();

        if let Some(head) = path.next() {
            // If there is a path, add the subpath to the appropriate child
            self.children
                .entry(head)
                .or_insert_with(|| PrintableChild::Subtree(PrintableTree::new()))
                .become_subtree()
                .add_path(path, leaf);
        } else {
            let (ident, usage) = match leaf {
                // Simply add a wildcard
                UsedItemLeaf::Wildcard => {
                    self.wildcard = true;
                    return;
                }

                // Add a plain leaf resembling `::ident`
                UsedItemLeaf::Used(ident) => (ident, NameUse::Used),

                // Add a renamed leaf resembling `::original as renamed`
                UsedItemLeaf::Renamed { original, renamed } => {
                    (original, NameUse::Renamed(&renamed))
                }
            };

            match self.children.entry(ident) {
                // Add ::ident to the set of children
                Entry::Vacant(entry) => {
                    entry.insert(PrintableChild::Plain(usage));
                }
                Entry::Occupied(mut entry) => entry.get_mut().add_usage(usage),
            }
        }
    }

    /// Iterate over all of the items in the tree. Used during formatting.
    /// Essentially serves to unify the 3 kinds of item in the tree: regular
    /// items, the `self` item (and its renames), and the `*` item.
    fn items(&self) -> impl Iterator<Item = PrintableItem<'_>> + '_ {
        let this_usages = self
            .this_usage
            .iter()
            .map(|&this_usage| PrintableItem::Plain(BasicName::This, this_usage));

        let wildcard = if self.wildcard {
            Some(PrintableItem::Wildcard)
        } else {
            None
        };

        let children = self.children.iter().map(|(&ident, child)| match *child {
            PrintableChild::Plain(usage) => PrintableItem::Plain(BasicName::Ident(ident), usage),
            PrintableChild::Subtree(ref tree) => PrintableItem::Tree { root: ident, tree },
        });

        this_usages.chain(wildcard).chain(children)
    }
}

impl Display for PrintableTree<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let items = self.items();

        match items.exactly_one() {
            Ok(item) => item.fmt(f),
            Err(mut items) => {
                f.write_str("{")?;

                items.try_for_each(|item| {
                    item.fmt(f)?;
                    f.write_str(", ")
                })?;

                f.write_str("}")
            }
        }
    }
}

/// This is basically an `Ident`, but it can also be `self` (for which we don't
/// have a convenient `Ident` object lying around, hence this enum)
enum BasicName<'a> {
    This,
    Ident(&'a Ident),
}

impl Display for BasicName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            BasicName::This => f.write_str("self"),
            BasicName::Ident(ident) => ident.fmt(f),
        }
    }
}

enum PrintableItem<'a> {
    Wildcard,
    Plain(BasicName<'a>, NameUse<'a>),
    Tree {
        root: &'a Ident,
        tree: &'a PrintableTree<'a>,
    },
}

impl Display for PrintableItem<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            PrintableItem::Wildcard => f.write_str("*"),
            PrintableItem::Plain(name, NameUse::Used) => name.fmt(f),
            PrintableItem::Plain(name, NameUse::Renamed(renamed)) => {
                write!(f, "{name} as {renamed}")
            }
            PrintableItem::Tree { root, tree } => write!(f, "{root}::{tree}"),
        }
    }
}

/// A printable key associates a series of use paths that are grouped under
/// a single `use` item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintableKey<'a> {
    configs: &'a ConfigsList,
    docs: &'a DocsList,
    visibility: Option<&'a Visibility>,
    rooted: Rooted,
    root_ident: &'a Ident,
}

impl PrintableKey<'_> {
    fn sort_key(&self) -> UseItemSortKey<'_> {
        let locality = if self.root_ident == "std"
            || self.root_ident == "alloc"
            || self.root_ident == "core"
        {
            CrateLocalityKey::StandardLib
        } else if self.root_ident == "self" {
            CrateLocalityKey::This
        } else if self.root_ident == "super" {
            CrateLocalityKey::Super
        } else if self.root_ident == "crate" {
            CrateLocalityKey::Crate
        } else {
            CrateLocalityKey::Dependency
        };

        UseItemSortKey {
            locality,
            configs: self.configs,
            rooted: self.rooted,
            ident: self.root_ident,
        }
    }
}

impl Ord for PrintableKey<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        Ord::cmp(&self.sort_key(), &other.sort_key())
    }
}

impl PartialOrd for PrintableKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CrateLocalityKey {
    /// `std`, `alloc`, and `core`
    StandardLib,

    /// Named dependencies
    Dependency,

    /// `use crate::...`
    Crate,

    /// `use super::...`
    Super,

    /// `use ...`
    This,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
// Note that this is used as a sort key, so the order of these fields is
// very important.
struct UseItemSortKey<'a> {
    locality: CrateLocalityKey,
    configs: &'a ConfigsList,
    rooted: Rooted,
    ident: &'a Ident,
}

impl UseItemSortKey<'_> {
    fn is_spaced_from(&self, other: &Self) -> bool {
        // I'm expecting to mess with this a lot during testing.
        if self.locality != other.locality {
            true
        } else if self.configs.is_empty() != other.configs.is_empty() {
            true
        } else {
            false
        }
    }
}

/// Write a complete, single use item, including all of its docs, configs,
/// and visibility. Includes a trailing semicolon and newline, as well as
/// newlines as appropriate between all of the attributes, but does not
/// include indentation or interior newlines (we rely on a separate rustfmt
/// pass to correctly indent everything).
fn format_use_item(
    dest: &mut impl fmt::Write,
    key: &PrintableKey<'_>,
    tree: &PrintableChild<'_>,
) -> fmt::Result {
    // Write docs here. Need to convert back to /// or /** */ form.

    key.configs
        .configs()
        .try_for_each(|config| writeln!(dest, "{config}"))?;

    if let Some(visibility) = key.visibility {
        write!(dest, "{visibility} ")?;
    }

    write!(dest, "use ")?;

    if key.rooted == Rooted::Rooted {
        write!(dest, "::")?;
    }

    let root_ident = key.root_ident;
    let item = match *tree {
        PrintableChild::Plain(usage) => PrintableItem::Plain(BasicName::Ident(root_ident), usage),
        PrintableChild::Subtree(ref tree) => PrintableItem::Tree {
            root: root_ident,
            tree,
        },
    };

    writeln!(dest, "{item};")
}

pub struct PrintableUseItems<'a> {
    items: BTreeMap<PrintableKey<'a>, PrintableChild<'a>>,
}

impl<'a> PrintableUseItems<'a> {
    // TODO: deduplicate this and PrintableTree::add_path
    pub fn add_single_used_item(
        &mut self,
        docs: &'a DocsList,
        configs: &'a ConfigsList,
        visibility: Option<&'a Visibility>,
        item: &'a SingleUsedItem<'a>,
    ) {
        let mut path = item.path.iter().copied();

        if let Some(ident) = path.next() {
            let key = PrintableKey {
                configs,
                docs,
                visibility,
                rooted: item.rooted,
                root_ident: ident,
            };

            match self.items.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(PrintableChild::Subtree(PrintableTree::new_from_path(
                        path, &item.leaf,
                    )));
                }

                Entry::Occupied(mut entry) => {
                    entry.get_mut().become_subtree().add_path(path, &item.leaf)
                }
            }
        } else {
            let (ident, usage) = match item.leaf {
                UsedItemLeaf::Wildcard => panic!("can't add a wildcard import at the root level"),
                UsedItemLeaf::Used(ident) => (ident, NameUse::Used),
                UsedItemLeaf::Renamed { original, renamed } => {
                    (original, NameUse::Renamed(renamed))
                }
            };

            let key = PrintableKey {
                configs,
                docs,
                visibility,
                rooted: item.rooted,
                root_ident: ident,
            };

            match self.items.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(PrintableChild::Plain(usage));
                }
                Entry::Occupied(mut entry) => entry.get_mut().add_usage(usage),
            }
        }
    }

    pub fn build_from_use_items(
        items: impl Iterator<
            Item = (
                &'a DocsList,
                &'a ConfigsList,
                Option<&'a Visibility>,
                &'a SingleUsedItem<'a>,
            ),
        >,
    ) -> Self {
        let mut this = Self {
            items: BTreeMap::new(),
        };

        items
            .into_iter()
            .for_each(|(docs, configs, visibility, item)| {
                this.add_single_used_item(docs, configs, visibility, item)
            });

        this
    }
}

impl Display for PrintableUseItems<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut items = self.items.iter();

        let Some((first_key, first_child)) = items.next() else {
            return Ok(());
        };

        // We use the sort key to determine when we should add additional
        // newlines
        let mut last_sort_key = first_key.sort_key();

        format_use_item(f, first_key, first_child)?;

        items.try_for_each(|(key, child)| {
            let sort_key = key.sort_key();

            if sort_key.is_spaced_from(&last_sort_key) {
                writeln!(f)?;
            }

            last_sort_key = sort_key;

            format_use_item(f, key, child)
        })
    }
}
