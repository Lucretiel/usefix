/// If a name is being imported, it either keeps its own name or is renamed
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NameUse<I> {
    /// `::name`
    Used,

    /// `::name as alias`
    Renamed(I),
}

impl<I> NameUse<I> {
    pub fn as_ref(&self) -> NameUse<&I> {
        match *self {
            NameUse::Used => NameUse::Used,
            NameUse::Renamed(ref renamed) => NameUse::Renamed(renamed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rooted {
    Rooted,
    Unrooted,
}
