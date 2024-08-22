use crate::tree::{ConfigsList, DocsList, Visibility};

pub struct PrintReadyTree<'a> {
    /// All of the docs for this use. This should contain the full set of lines
    /// of rustdocs attached to the item.
    pub docs: &'a DocsList,

    /// All of the cfg items attached to this `use`. This should specifically
    /// contain the stuff inside the parenthesis, for each #[cfg(THIS_STUFF)]
    pub configs: &'a ConfigsList,

    /// Any `pub`, `pub(crate)`, etc associated with this use
    pub visibility: Option<&'a Visibility>,

    /// The tree of imports in the use item.
    pub children: BTreeMap<TreeRoot, Branches>,
}
