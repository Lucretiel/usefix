use std::{collections::HashSet, io};

use crate::gitfile::{Chunk, Conflict, ConflictHalf, GitFile, Line, LineNumber};

struct PrintableConflict<'a, I1, I2> {
    pub left: PrintableConflictHalf<'a, I1>,
    pub right: PrintableConflictHalf<'a, I2>,
}

impl<'a, I> PrintableConflict<'a, I, I> {
    pub fn map_lines<T>(self, mut f: impl FnMut(I) -> T) -> PrintableConflict<'a, T, T> {
        PrintableConflict {
            left: PrintableConflictHalf {
                name: self.left.name,
                lines: f(self.left.lines),
            },
            right: PrintableConflictHalf {
                name: self.right.name,
                lines: f(self.right.lines),
            },
        }
    }
}

impl<'a: 'file, 'file> PrintableConflict<'file, &'a [Line<'file>], &'a [Line<'file>]> {
    pub fn from_conflict(conflict: &'a Conflict<'a, Line<'a>>) -> Self {
        PrintableConflict {
            left: PrintableConflictHalf {
                name: conflict.left.name(),
                lines: conflict.left.lines(),
            },
            right: PrintableConflictHalf {
                name: conflict.right.name(),
                lines: conflict.right.lines(),
            },
        }
    }
}

struct PrintableConflictHalf<'a, I> {
    pub name: &'a str,
    pub lines: I,
}

impl<'a, I: Iterator<Item = &'a [u8]> + Clone> PrintableConflictHalf<'a, I> {
    pub fn write_lines(mut self, dest: &mut impl io::Write) -> io::Result<()> {
        self.lines.try_for_each(|line| dest.write_all(line))
    }
}

/// Write a conflict to the destination. If the conflict halves are identical,
/// the non-conflicted version is written (usually this will happen because
/// the conflicted lines were consumed by usefix in the course of its work).
/// Otherwise, it will be written as-is, with the typical git conflict markers.
fn write_conflict<'a, I1, I2>(
    dest: &mut impl io::Write,
    conflict: PrintableConflict<'a, I1, I2>,
) -> io::Result<()>
where
    I1: Iterator<Item = &'a [u8]> + Clone,
    I2: Iterator<Item = &'a [u8]> + Clone,
{
    let left_lines = conflict.left.lines.clone();
    let right_lines = conflict.right.lines.clone();

    if Iterator::eq(left_lines, right_lines) {
        conflict.left.write_lines(dest)
    } else {
        let left_name = conflict.left.name;
        let right_name = conflict.right.name;

        write!(dest, "<<<<<<< {}\n", left_name)?;
        conflict.left.write_lines(dest)?;

        dest.write_all(b"=======\n")?;

        conflict.right.write_lines(dest)?;
        write!(dest, ">>>>>>> {}\n", right_name)
    }
}

fn filtered_lines<'file: 'a, 'a, I>(
    lines: I,
    discarded_lines: &'a HashSet<LineNumber>,
) -> impl Iterator<Item = &'file [u8]> + Clone + 'a
where
    I: IntoIterator<Item = &'a Line<'file>, IntoIter: Clone + 'a>,
{
    lines
        .into_iter()
        .filter(move |line| !discarded_lines.contains(&line.line_number))
        .map(|line| line.content.as_bytes())
}

fn filtered_lines_inject_content<'file: 'a, 'a, I>(
    lines: I,
    discarded_lines: &'a HashSet<LineNumber>,
    formatted_use_items: &'file [u8],
    insert_point: &'a InsertPoint,
) -> impl Iterator<Item = &'file [u8]> + Clone + 'a
where
    I: IntoIterator<Item = &'a Line<'file>, IntoIter: Clone + 'a>,
{
    lines.into_iter().filter_map(move |line| {
        if insert_point.contains_line(line.line_number) {
            Some(formatted_use_items)
        } else if discarded_lines.contains(&line.line_number) {
            None
        } else {
            Some(line.content.as_bytes())
        }
    })
}

fn find_split_point(
    conflict_half: &ConflictHalf<'_, Line<'_>>,
    line_number: LineNumber,
) -> Option<usize> {
    conflict_half
        .lines()
        .iter()
        .position(|line| line.line_number == line_number)
}

#[derive(Debug, Clone, Copy)]
enum InsertPoint {
    Nowhere,
    Once(LineNumber),
    IntoConflict(LineNumber, LineNumber),
}

impl InsertPoint {
    pub fn contains_line(&self, line: LineNumber) -> bool {
        match self {
            InsertPoint::Nowhere => false,
            InsertPoint::Once(point) => *point == line,
            InsertPoint::IntoConflict(left, right) => *left == line || *right == line,
        }
    }

    pub fn try_split_conflict<'file, 'a: 'file>(
        &self,
        conflict: &'a Conflict<'file, Line<'file>>,
    ) -> Option<(
        PrintableConflict<'file, &'a [Line<'file>], &'a [Line<'file>]>,
        PrintableConflict<'file, &'a [Line<'file>], &'a [Line<'file>]>,
    )> {
        match *self {
            InsertPoint::Nowhere | InsertPoint::Once(_) => None,
            InsertPoint::IntoConflict(left, right) => {
                let left_lines = conflict.left.lines();
                let right_lines = conflict.right.lines();

                let left_split_point = find_split_point(&conflict.left, left);
                let right_split_point = find_split_point(&conflict.right, right);

                let (Some(left_split_point), Some(right_split_point)) =
                    (left_split_point, right_split_point)
                else {
                    return None;
                };

                let left_top_lines = &left_lines[..left_split_point];
                let left_bottom_lines = &left_lines[left_split_point + 1..];

                let right_top_lines = &right_lines[..right_split_point];
                let right_bottom_lines = &right_lines[right_split_point + 1..];

                let top_conflict = PrintableConflict {
                    left: PrintableConflictHalf {
                        name: conflict.left.name(),
                        lines: left_top_lines,
                    },
                    right: PrintableConflictHalf {
                        name: conflict.right.name(),
                        lines: right_top_lines,
                    },
                };

                let bottom_conflict = PrintableConflict {
                    left: PrintableConflictHalf {
                        name: conflict.left.name(),
                        lines: left_bottom_lines,
                    },
                    right: PrintableConflictHalf {
                        name: conflict.right.name(),
                        lines: right_bottom_lines,
                    },
                };

                Some((top_conflict, bottom_conflict))
            }
        }
    }
}

fn first_matching_line_number_in_conflict_half(
    half: &ConflictHalf<'_, Line<'_>>,
    discarded_lines: &HashSet<LineNumber>,
) -> Option<LineNumber> {
    half.lines()
        .iter()
        .map(|line| line.line_number)
        .find(|line_number| discarded_lines.contains(line_number))
}

fn find_insert_point(original: &GitFile<'_>, discarded_lines: &HashSet<LineNumber>) -> InsertPoint {
    let mut left_point = None;
    let mut right_point = None;

    for chunk in original.chunks() {
        match chunk {
            Chunk::Line(line) => {
                if discarded_lines.contains(&line.line_number) {
                    return InsertPoint::Once(line.line_number);
                }
            }
            Chunk::Conflict(conflict) => {
                let local_left_point =
                    first_matching_line_number_in_conflict_half(&conflict.left, discarded_lines);
                let local_right_point =
                    first_matching_line_number_in_conflict_half(&conflict.right, discarded_lines);

                match (local_left_point, local_right_point) {
                    (Some(left), Some(right)) => return InsertPoint::IntoConflict(left, right),
                    (Some(left), None) => {
                        left_point = left_point.or(Some(left));
                    }
                    (None, Some(right)) => {
                        right_point = right_point.or(Some(right));
                    }
                    (None, None) => {}
                }
            }
        }
    }

    match (left_point, right_point) {
        (Some(left), Some(right)) => InsertPoint::IntoConflict(left, right),
        (Some(point), None) | (None, Some(point)) => InsertPoint::Once(point),
        (None, None) => InsertPoint::Nowhere,
    }
}

pub fn write_corrected_file(
    dest: &mut impl io::Write,
    original: &GitFile<'_>,
    discarded_lines: &HashSet<LineNumber>,

    // This could be a string, but sometimes the conversion process turns it
    // into a byte array, and we don't care to pay the penalty of verifying it's
    // still UTF-8 (even though it certainly is)
    formatted_use_items: &[u8],
) -> io::Result<()> {
    // First, we need to choose where to insert the formatted use items. In
    // order of preference:
    //
    // - Either the first line containing a use item that isn't part of a
    //   conflict, or the first conflict that contains use items on both sides
    //   (whichever is first)
    // - Otherwise, we need to insert the use items twice: once into the left
    //   file, and once into the right file. We do this into the first use item
    //   that appears in each conflict (if any)
    //
    // We might change these rules in the future. The advantage of these
    // rules is that they prefer to insert use items outside of any conflicts,
    // if possible; the disadvantage is that they're not totally consistent
    // with respect to the file structure: a different set of conflicts could
    // result in a different insert point.
    //
    // In practice we expect that this will basically never matter, because
    // these cases require extremely conflicted files that share hardly any
    // internal structure to create odd outputs.
    let insert_point = find_insert_point(original, discarded_lines);

    let mut chunks = original.chunks().iter();
    // This for loop is the one that's attempting to insert the use items.
    // We'll break out of it once we do that, so we can write the rest of the
    // file unconditionally.
    for chunk in chunks.by_ref() {
        match chunk {
            Chunk::Line(line) => {
                if insert_point.contains_line(line.line_number) {
                    dest.write_all(formatted_use_items)?;
                    break;
                } else if discarded_lines.contains(&line.line_number) {
                } else {
                    dest.write_all(line.content.as_bytes())?;
                }
            }
            Chunk::Conflict(conflict) => {
                if let Some((top_conflict, bottom_conflict)) =
                    insert_point.try_split_conflict(conflict)
                {
                    // Don't need to filter lines for the top conflict,
                    // because by definition it contains only all the lines
                    // before the first discarded line (the insert point)
                    let top_conflict = top_conflict
                        .map_lines(|lines| lines.iter().map(|line| line.content.as_bytes()));

                    let bottom_conflict =
                        bottom_conflict.map_lines(|lines| filtered_lines(lines, discarded_lines));

                    write_conflict(dest, top_conflict)?;
                    dest.write_all(formatted_use_items)?;
                    write_conflict(dest, bottom_conflict)?;

                    break;
                } else {
                    // At this point, we're certain that only the left or
                    // right side of the conflict (or neither) contain
                    // discarded lines where we need to insert a conflict.
                    let conflict = PrintableConflict::from_conflict(conflict).map_lines(|lines| {
                        filtered_lines_inject_content(
                            lines,
                            discarded_lines,
                            formatted_use_items,
                            &insert_point,
                        )
                    });

                    write_conflict(dest, conflict)?;
                }
            }
        }
    }

    // At this point, it's guaranteed that we've written the use items. Write
    // out the rest of the file.
    for chunk in chunks {
        match chunk {
            Chunk::Line(line) => {
                if !discarded_lines.contains(&line.line_number) {
                    dest.write_all(line.content.as_bytes())?;
                }
            }
            Chunk::Conflict(conflict) => {
                let conflict = PrintableConflict::from_conflict(conflict)
                    .map_lines(|lines| filtered_lines(lines, discarded_lines));

                write_conflict(dest, conflict)?;
            }
        }
    }

    Ok(())
}
