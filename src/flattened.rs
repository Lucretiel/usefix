/*!
A flattened representation of use items. Essentially this means replacing a
tree of use items:

```dont_compile
use {
    a::aa::{aaa, aab},
    b::{
        bb::{bb1, bb2, self},
        bbb::{bbb1, bbb2 as bbb3},
    }
}
```

With a flat list of each actual imported item:

```dont_compile
use a::aa::aaa;
use a::aa::aab;
use b::bb;
use b::bb::bb1;
use b::bb::bb2;
use b::bbb::bbb1;
use b::bbb::bbb2 as bbb3;
```

Once in this form, it's easier to reason about certain normalizations.
 */

use std::collections::BTreeMap;

use syn::Ident;

use crate::{
    common::{NameUse, Rooted},
    tree::{Branches, ConfigsList, DocsList, UseItem, Visibility},
};

/// The very last item of a flattened import: either an identifier, a renamed
/// identifier, or a wildcard.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UsedItemLeaf<'a> {
    // Note: it is important for correctness that `Wildcard` is the first
    // item in this list. It needs to be sorted earlier, so that it can be
    // merged with other items correctly during iteration.
    //
    // Similarly, it is important that `Used` is before rename, because renames
    // towards `_` can be subsumed by identical uses or wildcards
    Wildcard,
    Plain(&'a Ident, NameUse<&'a Ident>),
}

impl UsedItemLeaf<'_> {
    /// Check if this leaf is subsumed by another leaf. If it is, this leaf
    /// can be safely discarded (assuming that everything else lines up; ie,
    /// they both have identical visibilities, configs, etc). There are
    /// three cases where a leaf is subsumed:
    ///
    /// - A wildcard subsumes all non-renamed items (::{*, a, b, c::{d, self}} -> ::{*, c::d})
    /// - A wildcard subsumes an item that has been renamed to `_` (::{*, a as _} -> ::*).
    ///   This is because `_` renames only serve to bring trait methods into scope.
    /// - An item subsumes a rename of that same item to `_` (::{a, a as _} -> ::a),
    ///   for the same reason that the wildcard does.
    pub fn is_subsumed_by(&self, possible_parent: &Self) -> bool {
        match (possible_parent, self) {
            (UsedItemLeaf::Wildcard, UsedItemLeaf::Plain(_, usage)) => match usage {
                NameUse::Used => true,
                NameUse::Renamed(renamed) => *renamed == "_",
            },
            (
                UsedItemLeaf::Plain(name1, NameUse::Used),
                UsedItemLeaf::Plain(name2, NameUse::Renamed(renamed)),
            ) => name1 == name2 && *renamed == "_",
            _ => false,
        }
    }
}

/// A complete path of a flattened import.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SingleUsedItem<'a> {
    /// If Rooted, there is a leading `::`
    pub rooted: Rooted,

    /// The path segments preceding the leaf
    pub path: Vec<&'a Ident>,

    /// The actual item being imoported
    pub leaf: UsedItemLeaf<'a>,
}

impl SingleUsedItem<'_> {
    /// Check if this path is subsumed by another path. One path subsumes
    /// if the prefix is identical (rooted with the same path) and one leaf
    /// subsumes the other. See `UsedItemLeaf::is_subsumed_by` for more details.
    ///
    /// If a path is subsumed, it can be safely discarded, assuming that
    /// everything else lines up (identical visibilities, configs, etv).
    pub fn is_subsumed_by(&self, possible_parent: &Self) -> bool {
        self.rooted == possible_parent.rooted
            && self.path == possible_parent.path
            && self.leaf.is_subsumed_by(&possible_parent.leaf)
    }
}

/// The set of properties that can be associated with an imported item. These
/// properties exlude the configs, because a particular (path, configs) pair
/// can only ever have a single set of properties. More than one set of
/// properties will be merged as a normalization step during construction.
#[derive(Debug, Clone, Default)]
pub struct UsedItemPropertiesGroup<'a> {
    pub visibility: Option<&'a Visibility>,
    pub docs: DocsList,
}

impl<'a> UsedItemPropertiesGroup<'a> {
    pub fn merge(&mut self, visibility: Option<&'a Visibility>, docs: &DocsList) {
        self.visibility = merge_visibilities(self.visibility, visibility);
        self.docs.combine(docs);
    }
}

/// Merge a pair of visibilities. The "more public" visibility takes priority.
fn merge_visibilities<'a>(
    vis1: Option<&'a Visibility>,
    vis2: Option<&'a Visibility>,
) -> Option<&'a Visibility> {
    use Visibility::*;

    match (vis1, vis2) {
        (None, vis) | (vis, None) => vis,
        (Some(vis1), Some(vis2)) => Some(match (vis1, vis2) {
            (This, vis) | (vis, This) => vis,
            (Super, vis) | (vis, Super) => vis,

            // Paths are always an ancestor module, so whichever one is shorter
            // is more public. We assume (technically incorrectly) that a
            // `self` and `super` path is always more private than an `in` path.
            (vis1 @ In(ref path1), vis2 @ In(ref path2)) => {
                match path1.segments.len() < path2.segments.len() {
                    true => vis1,
                    false => vis2,
                }
            }

            (In(_), vis) | (vis, In(_)) => vis,
            (Crate, vis) | (vis, Crate) => vis,
            (Public, Public) => &Public,
        }),
    }
}

/// Add the properties of a use item to the set of groups associated with
/// a particular path. In addition to an insertion, this function takes care
/// of:
///
/// - merging properties that exist under identical configs
/// - merging ALL properties if ANY unconditional properties exist. We do this
///   because we should never perform a conditional import and an unconditional
///   import of the same item.
fn add_properties<'a>(
    properties_groups: &mut BTreeMap<&'a ConfigsList, UsedItemPropertiesGroup<'a>>,
    item: &'a UseItem,
) {
    // If there's an unconditional group, merge into it
    let group = if let Some(unconditional_group) = properties_groups.get_mut(&ConfigsList::EMPTY) {
        unconditional_group
    }
    // If the incoming item is unconditional, merge ALL groups and replace
    // with a new unconditional group
    else if item.configs.is_empty() {
        let merged = properties_groups.values().fold(
            UsedItemPropertiesGroup::default(),
            |mut merged, props| {
                merged.merge(props.visibility, &props.docs);
                merged
            },
        );

        properties_groups.clear();
        properties_groups
            .entry(const { &ConfigsList::EMPTY })
            .or_insert(merged)
    }
    // Otherwise, merge into the existing group
    else {
        properties_groups.entry(&item.configs).or_default()
    };

    group.merge(item.visibility.as_ref(), &item.docs);
}

/// A flattened list of import paths, associated with all of the properties
/// for each path. Properties consist of visibility, documentation, and configs.
/// Properties are grouped by config to assist with certain normalizations.
#[derive(Default)]
pub struct NormalizedUsedItems<'a> {
    pub items: BTreeMap<SingleUsedItem<'a>, BTreeMap<&'a ConfigsList, UsedItemPropertiesGroup<'a>>>,
}

impl<'a> NormalizedUsedItems<'a> {
    /// Add the entire tree of a `UseItem` to this list.
    pub fn add_tree(&mut self, items: &'a UseItem) {
        for (root, branches) in &items.children {
            self.add_branches(
                root.rooted,
                PathChain {
                    prev: None,
                    ident: &root.identifier,
                },
                &items,
                branches,
            )
        }
    }

    /// Add a set of branches, at a path prefix, to this list.
    fn add_branches(
        &mut self,
        rooted: Rooted,
        prefix: PathChain<'_, 'a>,
        use_item: &'a UseItem,
        branches: &'a Branches,
    ) {
        if branches.wildcard {
            let item = SingleUsedItem {
                rooted,
                path: prefix.to_list(),
                leaf: UsedItemLeaf::Wildcard,
            };

            let entry = self.items.entry(item).or_default();
            add_properties(entry, use_item);
        }

        let leaf = prefix.ident;
        let path = prefix.prev;

        for usage in &branches.used {
            let item = SingleUsedItem {
                rooted,
                path: path.map(PathChain::to_list).unwrap_or_default(),
                leaf: UsedItemLeaf::Plain(leaf, usage.as_ref()),
            };

            let entry = self.items.entry(item).or_default();
            add_properties(entry, use_item);
        }

        for (child, subtree) in &branches.children {
            let prefix = PathChain {
                prev: Some(&prefix),
                ident: child,
            };

            self.add_branches(rooted, prefix, use_item, subtree)
        }
    }
}

/// Linked list structure describing the path of a set of branches.
struct PathChain<'s, 'ident> {
    prev: Option<&'s PathChain<'s, 'ident>>,
    ident: &'ident Ident,
}

impl<'s, 'ident> PathChain<'s, 'ident> {
    /// Recursive helper for `to_list` that builds up the capacity of the
    /// vector with each step.
    fn to_list_capacity(&self, capacity: usize) -> Vec<&'ident Ident> {
        let mut vec = self
            .prev
            .map(|prev| prev.to_list_capacity(capacity + 1))
            .unwrap_or_else(|| Vec::with_capacity(capacity));

        vec.push(self.ident);
        vec
    }

    /// Convert this path chain into a vector of identifiers.
    pub fn to_list(&self) -> Vec<&'ident Ident> {
        self.to_list_capacity(1)
    }
}
