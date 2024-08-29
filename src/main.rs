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
- Convert the syn item into a local tree representation (`tree.rs`). The
  representation include import paths (including wildcards and renames),
  #[cfg] flags, visibility, and docs.
- normalize configs: Flatten the tree into a list of paths, where each path
  separately stores a mapping of config -> (visibility, docs). In any case
  where a path appears in both unconditional and conditional forms, the
  conditional forms are discarded, with their visibilities and docs merged into
  the unconditional form. Otherwise, all distinct conditional forms are
  retained; we don't make any effort to compute overlaps. If an import appears
  more than once with the same config (for instance, because it appears on both
  sides of a conflicted file), the visibilities and docs are merged.
- Normalize wildcards: group all of the items by (config -> (path -> (vis, docs))).
  Within each config, if a path exists in wildcard form, all of the paths that
  are subsumed by that wildcard are discarded and merged into the wildcard
  form. Additionally, any anonymous imports (e.g. `a::Trait as _`) are subsumed
  by a matching wildcard (`a::*`) or named import of the same path (`a::Trait`).
- We now have a canonical set of imports (`printable.rs`). Convert them into a
  series of use item trees. Much like `rust-analyzer`, we prefer to use a
  single use item for each top level imported identifier:

```
// We prefer this
use a::{b, c::d, e};
use f::g;

// Over this
use {
    a::{b, c::d, e},
    f::g,
}

// Or this
use a::b;
use a::c::d;
use a::e;
use f::g;
```

  Note that we'll have to split these into multiple use items to account for
  visibility, docs, and `#[cfg]` conditionals. In general we attempt to group
  stuff up that share any of these attributes.
- Put the use items in order, and into newline-separated groups. This section
  is nominal, as we expect the specific order and groupings to evolve for a
  while. In general:
  - Prefer `std`/`alloc`/`core`, followed by dependencies, followed by `crate`,
    `super`, and `self` imports
  - Prefer unconditional imports before conditional imports
  - The complete set of rules for grouping and ordering is in the `PrintableKey`
    type, in `printable.rs`
- Render the use items. This is mostly handled by `Display` implementations in
  `printable.rs`.
- Prettify the rendered use items. Rather than try to compete with `rustfmt`,
  we just use it directly. `rustfmt` can't be used as a library, so we offer
  two options:
  - Use `prettyplease`, a variant of `rustfmt` that is intended for use with
    macros and other codegen tools. `prettyplease` doesn't respect grouping
    of `use` items and the whitespace between them, so we have to call it
    several times, once with each grouped set of use items.
  - Call `rustfmt` as a subprocess. We expect in practice that this will be the
    typical case, but it requires `rustfmt` to be installed, so we still ask
    the user to ask for it.
- Insert the prettified use items into the original file, and remove the
  existing use items (`writefile.rs`). This is a fraught thing to try to do,
  because the original file might include git conflicts. The basic rule is to
  insert the use items at the point where the very first use item appears in
  the original file.
  - If this point is a non-conflicted line, it's easy; we just put it there.
  - If this point is a conflict, we split the conflict into two separate
    conflicts, and insert the use items in between them.
  - If there are no such points, it means that all the use items only appear
    in half of the conflicts (that is, for each conflict, it appears ONLY on
    the left or right side of the conflict). This is an awfully edge-casey
    edge case, and we insert the use items twice: once at the first use item
    in the left version of the file, and once at the first use item in the
    right version of the file. Note again that we only do this if there's no
    possible non-conflicted sites to insert these use items.
  - We assume that, in the original rust file, no lines that include a use item
    (or part of a use item) will include anything OTHER than that use item.
    No sane rust developer would do otherwise, even if they don't use rustfmt
    for some reason.
  - When writing conflicts, we check that the conflict is still a conflict: if
    its remaining lines (after excluding the use items we processed) are
    identical, we write them as a plain, non-conflicted lines. This will be
    common in the case where a conflict appears in the middle of a larger set
    of imports.
  - One odd side effect of our algorithm is that spaces between groups of use
    items in the original file are kept, "clump" together at the end of all the
    use items. To handle this, we consume all but one empty when we insert
    the formatted use items.


Sub-algorithms:
    Docs merge:
        If either set of docs are a prefix or suffix of the other, use the
        longer one. Otherwise, concatenate them. In a future version we could
        detect if either docs are a complete subset of the other, but for now
        this is fine.
    Visibility Merge
        Always prefer the "more public" visibility
 */

mod common;
mod docprint;
mod flattened;
mod gitfile;
mod pretty;
mod printable;
mod tree;
mod write_file;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, Write},
};

use anyhow::Context;

use crate::{
    flattened::{NormalizedUsedItems, SingleUsedItem, UsedItemPropertiesGroup},
    gitfile::{GitFile, LineNumber, Side},
    pretty::prettify_with_subcommand,
    printable::PrintableUseItems,
    tree::{ConfigsList, UseItem},
};

fn main() -> anyhow::Result<()> {
    let file =
        io::read_to_string(io::stdin().lock()).context("i/o error reading file from stdin")?;
    let parsed_file = GitFile::from_file(&file).context("error parsing git conflicts in file")?;

    // TODO: do these in separate threads. `proc-macro2`` stuff isn't Send,
    // unfortunately.
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

    // Render the use items to a string, complete with sorting and grouping,
    // then prettify it with rustfmt or prettyplease.
    let formatted_use_items = printable_items.to_string();
    let prettified_use_items = prettify_with_subcommand("rustfmt", &formatted_use_items)?;

    // Compute the set of lines from the ORIGINAL file that need to be
    // discarded; these are the lines in the original file that include any
    // part of a use item. There's an important assumption here that no line
    // that includes any part of a use item includes anything OTHER than that
    // use item.
    let discarded_lines = Iterator::chain(left_use_items.iter(), right_use_items.iter())
        .flat_map(|item| &item.touched_original_lines)
        .copied()
        .collect();

    // Create the final, fixed version of the file. We assume that files fit
    // neatly in memory, so to save on system calls, we just put it all in a
    // single buffer and write it at the end.
    let mut formatted_file: Vec<u8> = Vec::with_capacity(file.len());
    write_file::write_corrected_file(
        &mut formatted_file,
        &parsed_file,
        &discarded_lines,
        &prettified_use_items,
    )
    .expect("writing to a vector is infallible");

    io::stdout()
        .lock()
        .write_all(&formatted_file)
        .context("i/o error writing to stdout")?;

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

/// Group all of the flattened items by config (so that, for each unique `#[cfg]`
/// among all the use items, all of the imports associated with that config are
/// grouped together) and then normalize wildcards and
fn group_flattened_items_normalize_wildcards<'a>(
    flattened_items: &'a NormalizedUsedItems<'a>,
) -> ConfigToPathToProperties<'a> {
    let mut grouped_flattened_items = ConfigToPathToProperties::new();

    for (path, config_properties) in &flattened_items.items {
        for (&config, properties) in config_properties {
            let config_entries = grouped_flattened_items.entry(config).or_default();

            // This works because `SingleUsedItem` is sorted such that any
            // item comes *after* any other item that subsumes it.
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

/// A parsed `UseItem` (see `tree.rs`) along with all of the line numbers from
/// the original file are associated with this item.
struct AnnotatedUseItem {
    use_item: UseItem,
    touched_original_lines: HashSet<LineNumber>,
}
