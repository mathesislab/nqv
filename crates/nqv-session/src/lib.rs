//! nqv-session: what a viewer does with the data, independent of how it is shown: the
//! commands (`:` syntax and flags), recipes and variable matching. Front ends (the
//! terminal, the desktop app, the browser) only draw and take input.

pub mod command;
pub mod fuzzy;
pub mod recipe;
