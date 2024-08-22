// This is only here during develop to quiet down my editor
#![allow(dead_code)]

/*
High level data model:

We need to track a lot of things about imports. Specifically:

    - The path: `a::b::c::d`
    - The imported item, which can be either a regular item (`e`), a renamed item (`e as f`),
      or a wildcard (`*`)
    - Any `#[cfg(...)]` attributes attached to the item, which we call configs. Any
      item without a config is called "unconditional"
    - The visibility of the item (`pub`, `pub(crate)`, etc)
    - Any docs attached to the item

At various points in this algorithm we'll be grouping these imports in various
ways to aid with normalization. At a very high level, the goal of usefix's
merge algorithm is to compute a union of the imports of both forms of a
conflicted file and use it as the conflict resolution.

High level algorithm:

- Load the file with git conflicts
- Split into two files, based on conflicts. Include a mapping to the line numbers
  of the original files.
- Parse the files with syn
- Extract all top-level use items from both files. Track which line numnbers
  they came from.
- Convert the syn item into a local tree representation. The representation
  include import paths (including wildcards and renames), #[cfg] flags,
  visibility, and docs.
- normalize configs: Flatten the tree into a list of paths, where each path
  separately stores a mapping of config -> (visibility, docs). In any case
  where a path appears in both unconditional and conditional forms, the
  conditional forms are discarded, with their visibilities and docs merged into
  the unconditional form.
- Normalize wildcards: group all of the items by (config -> (path -> (vis, docs))).
  Within each config, if a path exists in wildcard form, all of the paths that
  are subsumed by that wildcard are discarded and merged into the wildcard
  form


Sub-algorithms:
    Docs merge:
        If either set of docs are a prefix or suffix of the other, use the
        longer one. Otherwise, concatenate them.
    Visibility Merge
        Always prefer the "more public" visibility
 */

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
};

use anyhow::Context;
use flattened::{NormalizedUsedItems, SingleUsedItem, UsedItemPropertiesGroup};
use gitfile::{GitFile, LineNumber, Side};
use printable::PrintableUseItems;
use tree::{ConfigsList, UseItem};

mod flattened;
mod gitfile;
mod printable;
mod tree;

/*
Sort Order:

First, by LOCALITY:

[std/core/alloc]
[named imports
[crate imports]
[super imports]
[self imports]

Then, by CONDITIONALITY:

[unconditional imports]

[cfg conditional imports]

Then, by ROOTEDNESS:

[::rooted imports]
[relative imports]

Then, by NAME:

use aaa::{...};
use bbb::{...};

Then, by VISBILITY:

use aaa::{...};
pub use aaa::{...};
use bbb::{...};


*/

fn main() -> anyhow::Result<()> {
    let file =
        io::read_to_string(io::stdin().lock()).context("i/o error reading file from stdin")?;
    let parsed_file = GitFile::from_file(&file).context("error parsing git conflicts in file")?;

    // TODO: do these in separate threads. Proc macro2 stuff isn't Send, unfortunately.
    let left_use_items = extract_use_items(&parsed_file, Side::Left).unwrap();
    let right_use_items = extract_use_items(&parsed_file, Side::Right).unwrap();

    // Flatten the list into a list of paths, where each path stores all known
    // properties variants. This step normalizes the configs (any time a path
    // appears in unconditional form, it subsumes all instances of that path
    // in conditional form)
    let mut flattened_items = NormalizedUsedItems::default();
    Iterator::chain(left_use_items.iter(), right_use_items.iter())
        .for_each(|item| flattened_items.add_tree(&item.use_item));

    // Group the list by config and normalize wildcard. Any time a path appears
    // with a wildcard import, it subsumes all instances of that same path
    // importing a non-renamed item, provided they share a config
    let grouped_flattened_items = group_flattened_items_normalize_wildcards(&flattened_items);

    // We now have the final set of imports we wish to use. Convert them into
    // a form suitable for printing.
    let printable_items =
        PrintableUseItems::build_from_use_items(grouped_flattened_items.iter().flat_map(
            |(&configs, items)| {
                items.iter().map(move |(&path, properties)| {
                    (&properties.docs, configs, properties.visibility, path)
                })
            },
        ));

    let formatted_use_items = printable_items.to_string();

    // TODO: pretty print
    println!("{}", formatted_use_items);

    Ok(())
}

/// Parse a GitFile with syn, and extract its use itmes (and their spans) into
/// a list of Annotated Use Items.
fn extract_use_items(file: &GitFile<'_>, side: Side) -> anyhow::Result<Vec<AnnotatedUseItem>> {
    let derived_file = file.build_derived_file(side);

    let parsed_file =
        syn::parse_file(&derived_file.content()).context("failed to parse Rust syntax")?;

    let use_items = parsed_file
        .items
        .into_iter()
        .filter_map(|item| match item {
            syn::Item::Use(use_item) => Some(use_item),
            _ => None,
        })
        .filter_map(|use_item| UseItem::from_syn_use_item(use_item).ok())
        .map(|use_item| {
            let start = use_item.span.start().line;
            let end = use_item.span.end().line;

            let touched_original_lines = (start..=end)
                .map(|derived_line| {
                    LineNumber::from_one_indexed(derived_line).expect("line number was 0")
                })
                .map(|derived_line| {
                    derived_file
                        .get_original_line(derived_line)
                        .expect("derived line didn't exist")
                })
                .collect();

            AnnotatedUseItem {
                use_item,
                touched_original_lines,
            }
        })
        .collect();

    Ok(use_items)
}

type ConfigToPathToProperties<'a> =
    HashMap<&'a ConfigsList, BTreeMap<&'a SingleUsedItem<'a>, UsedItemPropertiesGroup<'a>>>;
fn group_flattened_items_normalize_wildcards<'a>(
    flattened_items: &'a NormalizedUsedItems<'a>,
) -> ConfigToPathToProperties<'a> {
    let mut grouped_flattened_items = ConfigToPathToProperties::new();

    for (path, config_properties) in &flattened_items.items {
        for (&config, properties) in config_properties {
            let config_entries = grouped_flattened_items.entry(config).or_default();

            match config_entries.last_entry() {
                Some(entry)
                    if path.is_subsumed_by(entry.key())
                        && entry.get().docs == properties.docs
                        && entry.get().visibility == properties.visibility => {}
                _ => {
                    config_entries.insert(path, properties.clone());
                }
            }
        }
    }

    grouped_flattened_items
}

struct AnnotatedUseItem {
    use_item: UseItem,
    touched_original_lines: HashSet<LineNumber>,
}
