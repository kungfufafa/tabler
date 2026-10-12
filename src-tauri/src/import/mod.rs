//! Import (File ▸ Import): the CSV wizard (`csvimport`) and SQL dumps (`dump`), with their
//! shared text decoding (`text`), the CSV reader (`csvread`) and type / delimiter detection
//! (`detect`).

pub mod csvimport;
pub mod csvread;
pub mod detect;
pub mod dump;
pub mod text;

pub use csvimport::{preview_csv, run_csv_import};
pub use dump::{preview_dump, run_sql_import};

#[cfg(test)]
mod tests;
