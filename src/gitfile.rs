/*!
Parsers for a file containing git conflicts. The final parsed file can be
converted into the left or right version of the conflict, along with line
number mappings back to the original file.
 */

use std::{collections::HashMap, iter, num::NonZeroUsize};

use either::Either;
use nom::{
    branch::alt,
    character::complete::space0,
    combinator::eof,
    error::{ErrorKind, ParseError},
    sequence::pair,
    IResult, Parser,
};
use nom_supreme::{
    error::ErrorTree,
    final_parser::{final_parser, Location},
    tag::complete::tag,
    ParserExt,
};

/// A one-indexed line numbers. 1-indexing is what `syn` uses, so it's what
/// we'll use, too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineNumber(NonZeroUsize);

impl LineNumber {
    pub const ONE: Self = Self(NonZeroUsize::MIN);

    pub fn from_one_indexed(line: usize) -> Option<Self> {
        NonZeroUsize::new(line).map(Self)
    }

    /// Increment this value in place, then return the old value.
    pub fn get_incr(&mut self) -> Self {
        let value = *self;
        self.0 = self.0.checked_add(1).expect("line number overflow");
        value
    }

    /// Create an iterator of all line numbers, starting at 1.
    pub fn lines_iter() -> impl Iterator<Item = Self> {
        let mut line = Self::ONE;
        iter::repeat_with(move || line.get_incr())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Line<'a> {
    pub content: &'a str,
    pub line_number: LineNumber,
}

impl<'a> Line<'a> {
    /// Create a new line with a line number. Increment the line number
    /// in-place (so, if called with 1, this will return a `Line` on  line 1,
    /// and modify the argument to be 2).
    pub fn with_line_number(content: &'a str, line_number: &mut LineNumber) -> Self {
        Self {
            content,
            line_number: line_number.get_incr(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Side {
    Left,
    Right,
}

/// A parsed file containing git conflicts.
#[derive(Debug)]
pub struct GitFile<'a> {
    chunks: Vec<Chunk<'a, Line<'a>>>,
}

impl<'a> GitFile<'a> {
    pub fn from_file(file: &'a str) -> Result<GitFile<'a>, ErrorTree<Location>> {
        final_parser(parse_file)(file)
    }

    /// Get an iterator of all of the lines of a particular version of the
    /// conflicted file, along with their "real" line numbers (that is, the
    /// line numbers of the original file containing the conflicts).
    pub fn get_lines(&self, side: Side) -> impl Iterator<Item = Line<'a>> + '_ {
        self.chunks.iter().flat_map(move |chunk| match *chunk {
            Chunk::Line(line) => Either::Left(iter::once(line)),
            Chunk::Conflict(ref conflict) => {
                let half = match side {
                    Side::Left => &conflict.left,
                    Side::Right => &conflict.right,
                };

                Either::Right(half.lines.iter().copied())
            }
        })
    }

    pub fn build_derived_file(&self, side: Side) -> DerivedFile {
        let mut content = String::new();
        let mut line_mappings = HashMap::new();

        for (line, line_number) in self.get_lines(side).zip(LineNumber::lines_iter()) {
            line_mappings.insert(line_number, line.line_number);
            content.push_str(line.content);
        }

        DerivedFile {
            content,
            line_mappings,
        }
    }

    fn from_chunks(chunks: impl IntoIterator<Item = Chunk<'a, &'a str>>) -> Self {
        let mut line_number = LineNumber::ONE;

        Self {
            chunks: chunks
                .into_iter()
                .map(|chunk| chunk.with_line_number(&mut line_number))
                .collect(),
        }
    }

    pub fn chunks(&self) -> &[Chunk<'a, Line<'a>>] {
        &self.chunks
    }

    pub fn contains_conflict(&self) -> bool {
        self.chunks
            .iter()
            .any(|chunk| matches!(chunk, Chunk::Conflict(_)))
    }
}

#[derive(Debug)]
pub enum Chunk<'a, Line> {
    Line(Line),
    Conflict(Conflict<'a, Line>),
}

impl<'a> Chunk<'a, &'a str> {
    pub fn with_line_number(self, line_number: &mut LineNumber) -> Chunk<'a, Line<'a>> {
        match self {
            Chunk::Line(line) => Chunk::Line(Line::with_line_number(line, line_number)),
            Chunk::Conflict(conflict) => Chunk::Conflict(conflict.with_line_number(line_number)),
        }
    }
}

#[derive(Debug)]
pub struct Conflict<'a, L> {
    pub left: ConflictHalf<'a, L>,
    pub right: ConflictHalf<'a, L>,
}

impl<'a> Conflict<'a, &'a str> {
    pub fn with_line_number(self, line_number: &mut LineNumber) -> Conflict<'a, Line<'a>> {
        let left = self.left.with_line_number(line_number);
        let right = self.right.with_line_number(line_number);

        // Skip the final line
        line_number.get_incr();

        Conflict { left, right }
    }
}

#[derive(Debug)]
pub struct ConflictHalf<'a, L> {
    name: &'a str,
    lines: Vec<L>,
}

impl<'a, L> ConflictHalf<'a, L> {
    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn lines(&self) -> &[L] {
        &self.lines
    }
}

impl<'a> ConflictHalf<'a, &'a str> {
    pub fn with_line_number(self, line_number: &mut LineNumber) -> ConflictHalf<'a, Line<'a>> {
        // Skip the first line, since it's the header line
        line_number.get_incr();

        ConflictHalf {
            name: self.name,
            lines: self
                .lines
                .iter()
                .map(|&line| Line::with_line_number(line, line_number))
                .collect(),
        }
    }
}

/// Parse a file containing git conflicts. This is a list of chunks, terminated
/// by eof.
fn parse_file(input: &str) -> IResult<&str, GitFile<'_>, ErrorTree<&str>> {
    parse_lines_terminated(
        alt((
            parse_conflict.map(Chunk::Conflict),
            parse_any_line.map(Chunk::Line),
        )),
        eof.value(()),
    )
    .map(|(chunks, ())| GitFile::from_chunks(chunks))
    .parse(input)
}

/// Parse a git conflict, resembling:
///
/// ```text
/// <<<<<<< branch-1
/// content in branch-1
/// =======
/// content in branch-2
/// >>>>>>> branch-2
/// ```
///
/// Either or both sides of the conflict may be empty.
fn parse_conflict(input: &str) -> IResult<&str, Conflict<&str>, ErrorTree<&str>> {
    let (input, left_name) = parse_conflict_header(input)?;

    let (input, ((left_lines, ()), (right_lines, right_name))) = pair(
        parse_lines_terminated(parse_any_line, parse_conflict_separator),
        parse_lines_terminated(parse_any_line, parse_conflict_footer),
    )
    .cut()
    .parse(input)?;

    Ok((
        input,
        Conflict {
            left: ConflictHalf {
                name: left_name,
                lines: left_lines,
            },
            right: ConflictHalf {
                name: right_name,
                lines: right_lines,
            },
        },
    ))
}

fn parse_conflict_header(input: &str) -> IResult<&str, &str, ErrorTree<&str>> {
    parse_conflict_part("<<<<<<<").parse(input)
}

fn parse_conflict_footer(input: &str) -> IResult<&str, &str, ErrorTree<&str>> {
    parse_conflict_part(">>>>>>>").parse(input)
}

fn parse_conflict_separator(input: &str) -> IResult<&str, (), ErrorTree<&str>> {
    tag("=======\n").value(()).parse(input)
}

/// Parse a conflict header or a conflict footer, which is a series of chevrons
/// followed by a git ref name
fn parse_conflict_part<'a>(
    arrows: &'static str,
) -> impl Parser<&'a str, &'a str, ErrorTree<&'a str>> {
    tag(arrows)
        .terminated(space0)
        .precedes(parse_any_line)
        .map(|line| line.trim_end())
}

/// Parse a line from the input, defined as any sequence of characters
/// terminated by a newline or eof. This parser can't fail.
fn parse_any_line<E>(input: &str) -> IResult<&str, &str, E> {
    let idx = input.find("\n").map(|i| i + 1).unwrap_or(input.len());
    let (line, tail) = input.split_at(idx);
    Ok((tail, line))
}

/// Parse 0 or more lines with the line parser, terminated by the terminator
/// parser. Returns an error if the file is emptied without a terminator being
/// found.
///
/// The terminator is tried eagerly, so make sure that it can't parse a line by
/// mistake.
fn parse_lines_terminated<'a, Error, Line, Terminator>(
    mut line: impl Parser<&'a str, Line, Error>,
    mut terminator: impl Parser<&'a str, Terminator, Error>,
) -> impl Parser<&'a str, (Vec<Line>, Terminator), Error>
where
    Error: ParseError<&'a str>,
{
    move |mut input: &'a str| {
        let mut lines = Vec::new();

        loop {
            let terminator_error = match terminator.parse(input) {
                Ok((tail, terminator)) => break Ok((tail, (lines, terminator))),
                Err(nom::Err::Error(err)) => err,
                Err(err) => break Err(err),
            };

            if input.is_empty() {
                break Err(nom::Err::Error(
                    terminator_error.or(Error::from_error_kind(input, ErrorKind::Eof)),
                ));
            }

            let (tail, line) = line.parse(input).map_err(|err| match err {
                nom::Err::Error(err) => nom::Err::Error(terminator_error.or(err)),
                err => err,
            })?;

            // Technically right around here we should check that we're not
            // going to loop forever (did the `line` parser consume 0 input?).
            // In practice, all of the line parsers we use won't have this
            // problem.

            lines.push(line);
            input = tail;
        }
    }
}

#[derive(Debug, Clone)]
pub struct DerivedFile {
    content: String,

    /// Mapping from local line numbers to line numbers in the original git
    /// conflicted file. Technically this could be a `Vec`, since the local
    /// line numbers are always going precisly resemble `1..n`, but it's just
    /// a bit easier to do it this way.
    line_mappings: HashMap<LineNumber, LineNumber>,
}

impl DerivedFile {
    #[inline]
    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn get_original_line(&self, derived_line: LineNumber) -> Option<LineNumber> {
        self.line_mappings.get(&derived_line).copied()
    }
}
