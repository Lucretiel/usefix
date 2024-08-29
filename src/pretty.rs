use std::{
    io::{Read, Write},
    panic,
    process::{Command, Stdio},
    thread,
};

use anyhow::Context;

fn prettify(formatted_use_items: &str) -> String {
    // A note about this step: currently, we do something sort of silly: we
    // render the new use items to a string, then convert them BACK into a
    // `syn` tree, because the input to our pretty printer is a `syn` tree.
    // In principle we could just render directly to a TokenStream and skip
    // a nasty runtime parse step. The main reason we don't do this is that
    // the spans of the tree include line numbers, and there's no way to
    // manually create a span with line numbers. The line numbers are necessary
    // to preserve grouping of use items.
    // We use prettyplease, a variant of rustfmt intended for use with macros
    // and other codegen tools. For use items, it should be identical to

    /*
      // A note about this step: currently, we do something sort of silly: we
    // render the new use items to a string, then convert them BACK into a
    // `syn` tree, because the input to our pretty printer is a `syn` tree.
    // In principle we could just render directly to a TokenStream and skip
    // a nasty runtime parse step. The main reason we don't do this is that
    // the spans of the tree include line numbers, and there's no way to
    // manually create a span with line numbers. The line numbers are necessary
    // to preserve grouping of use items.
    let parsed_formatted_use_items = syn::parse_file(&formatted_use_items)
        .expect("usefix shouldn't ever produce malformed use items");

    let prettified_use_items = prettyplease::unparse(&parsed_formatted_use_items);
     */
    todo!()
}

/// Sometimes you just gotta use rustfmt
pub fn prettify_with_subcommand(
    command_name: &str,
    formatted_use_items: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut command = Command::new(command_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch formatting subcommand '{command_name}'"))?;

    let mut stdin = command
        .stdin
        .take()
        .expect("stdin was piped, it shouldn't be None");

    let mut stdout = command
        .stdout
        .take()
        .expect("stdout was piped, it shouldn't be None");

    // Prevent deadlocks: use some threads to handle reading and writing in
    // parallel.

    thread::scope(move |scope| {
        // stdin thread
        let stdin_thread = scope.spawn(move || stdin.write_all(formatted_use_items.as_bytes()));

        // stdout thread
        let stdout_thread = scope.spawn(move || {
            let mut output = Vec::with_capacity(formatted_use_items.len());
            stdout.read_to_end(&mut output).map(|_| output)
        });

        // Await the command, then join the threads.
        let status = command.wait().expect("commands can always be joined");

        if !status.success() {
            anyhow::bail!("formatting subcommand '{command_name}' failed: {status}");
        }

        stdin_thread
            .join()
            .unwrap_or_else(|panic| panic::resume_unwind(panic))
            .with_context(|| {
                format!("error while writing to stdin of formatting subcommand '{command_name}'")
            })?;

        // The stdout thread will directly return the output, so just propagate
        // it directly
        stdout_thread
            .join()
            .unwrap_or_else(|panic| panic::resume_unwind(panic))
            .with_context(|| {
                format!("error while reading from stdout of formatting subcommand '{command_name}'")
            })
    })
}
