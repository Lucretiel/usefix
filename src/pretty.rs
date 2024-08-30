use std::{
    io::{Read, Write},
    panic,
    path::Path,
    process::{Command, Stdio},
    thread,
};

use anyhow::Context;
use itertools::Itertools;

pub fn prettify_with_prettyplease(formatted_use_items: &str) -> Vec<u8> {
    // We use prettyplease, a variant of rustfmt intended for use with macros
    // and other codegen tools. For use items, it's hopefully identical to
    // rustfmt (though it probably doesn't respect your rustfmt config)
    //
    // A note about this step: currently, we do something sort of silly: we
    // render the new use items to a string, then convert them BACK into a
    // `syn` tree, because the input to our pretty printer is a `syn` tree.
    // In principle we could just render directly to a TokenStream and skip
    // a nasty runtime parse step. I don't want to maintain two versions of
    // `printable`, though, so we don't do that.
    //
    // One thing about `prettyplease` is that it doesn't respect spaces
    // between items, because it operates only on the content of the tokens.
    // We therefore split the use items into groups ourselves, use
    // `prettyplease` on each group, and re-concatenate.

    // We'd like to use rayon here, but it actually doesn't support splitting
    // on "\n\n", for some reason.
    thread::scope(|scope| {
        formatted_use_items
            .split("\n\n")
            .map(|chunk| {
                scope.spawn(move || {
                    let parsed_chunk = syn::parse_file(chunk)
                        .expect("usefix shouldn't produce syntatically invalid rust");
                    let mut prettified_chunk = prettyplease::unparse(&parsed_chunk);

                    let len_without_trailing_space = prettified_chunk.trim_end().len();
                    prettified_chunk.truncate(len_without_trailing_space);
                    prettified_chunk.push_str("\n\n");

                    prettified_chunk
                })
            })
            // This collect_vec is very important; it ensures that all of this
            // work happens in parellel, by spawning all the threads before
            // joining any of them.
            .collect_vec()
            .into_iter()
            .map(|thread| {
                thread
                    .join()
                    .unwrap_or_else(|panic| panic::resume_unwind(panic))
            })
            .reduce(|mut left, right| {
                left.push_str(&right);
                left
            })
            .unwrap_or_default()
            .into()
    })
}

/// Sometimes you just gotta use rustfmt
pub fn prettify_with_subcommand(
    command_name: &Path,
    formatted_use_items: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut command = Command::new(command_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch formatting subcommand"))?;

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
            stdout.read_to_end(&mut output).map(move |_| {
                // Always add an extra newline at the end
                output.push(b'\n');
                output
            })
        });

        // Await the command, then join the threads.
        let status = command.wait().expect("commands can always be joined");

        if !status.success() {
            anyhow::bail!("command failed: {status}");
        }

        stdin_thread
            .join()
            .unwrap_or_else(|panic| panic::resume_unwind(panic))
            .with_context(|| {
                format!("i/o error while writing to stdin of formatting subcommand")
            })?;

        // The stdout thread will directly return the output, so just propagate
        // it directly
        stdout_thread
            .join()
            .unwrap_or_else(|panic| panic::resume_unwind(panic))
            .with_context(|| {
                format!("i/o error while reading from stdout of formatting subcommand")
            })
    })
}
