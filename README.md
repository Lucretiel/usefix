# usefix

`usefix` is an opinionated tool for formatting the `use` in your Rust code. It reorders and groups all of your imports, using a style largely inspired by how `rustfmt` automatically inserts new imports. It's even capable of automatically fixing git conflicts that include `use` items!

## Usage

`usefix` currently operates only on a single file, which it reads from stdin, and writes the fixed file to stdout:

```bash
usefix -c rustfmt < old_file.rs > fixed_file.rs
```

The `-c` parameter tells `usefix` to use a particular `rustfmt` to format the use items that it emits; by default it uses [`prettyplease`](https://docs.rs/prettyplease/latest/prettyplease/).

If you want to rewrite a file in-place, consider using a tool like [`rewrite`](https://github.com/Lucretiel/rewrite) or [`sponge`](https://linux.die.net/man/1/sponge):

```bash
rewrite lib.rs -- usefix -c rustfmt
```

## Conflict fixing

`usefix` is capable of handling git conflicts in your use items (in fact, this is the original purpose for which it was created). If the input file includes any git conflict markers that include `use` items, the output file will include the union of the use items in the two conflicting versions of the file, with the conflict markers removed, if possible. It doesn't touch conflicts for anything other than `use` items.
