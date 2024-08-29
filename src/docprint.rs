/*!
Functionality related to whether and how to print doc tags
 */

use std::fmt::{self, Display, Formatter};

use crate::tree::DocsList;

impl Display for DocsList {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.blocks()
            .iter()
            .try_for_each(|block| match categorize_doc(&block) {
                DocCategory::SingleLine => writeln!(f, "///{block}"),
                DocCategory::Block => writeln!(f, "/**{block}*/"),
                DocCategory::Attribute => writeln!(f, "#[doc = {block:?}]"),
            })
    }
}

enum DocCategory {
    /// A doc comment that is a single line, like `/// foo`
    SingleLine,

    /// A doc comment that is a block comment, like `/** foo */`
    Block,

    /// A doc comment that lives in a #[doc = "..."] attribute
    Attribute,
}

fn categorize_doc(doc: &str) -> DocCategory {
    if doc.as_bytes().contains(&b'\n') {
        // A doc comment must not have inbalanced /* */ comments
        if contains_balanced_blocks(doc) {
            DocCategory::Block
        } else {
            DocCategory::Attribute
        }
    } else {
        DocCategory::SingleLine
    }
}

/// Check if the given comment contains balanced /* */ comments
fn contains_balanced_blocks(comment: &str) -> bool {
    let mut depth: u32 = 0;
    let mut chars = comment.as_bytes().iter().copied().peekable();

    while let Some(c) = chars.next() {
        match c {
            b'/' if chars.peek() == Some(&b'*') => {
                depth = match depth.checked_add(1) {
                    Some(depth) => depth,
                    None => return false,
                };
                chars.next();
            }

            b'*' if chars.peek() == Some(&b'/') => {
                depth = match depth.checked_sub(1) {
                    Some(depth) => depth,
                    None => return false,
                };
                chars.next();
            }

            _ => {}
        }
    }

    depth == 0
}
