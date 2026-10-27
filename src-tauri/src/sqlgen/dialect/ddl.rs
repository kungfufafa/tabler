//! The structure editor's planner of an extension dialect. The core dialects keep
//! `sqlgen::ddl`; a lane implements this trait for its dialect and sets `DialectSpec::ddl`
//! (None: its tables cannot be created or altered from Tabler).

use crate::error::AppError;
use crate::types::ddl::{DdlOp, DdlPlan};
use crate::types::schema::{TableKind, TableMeta, TableRef};

pub trait DdlPlanner: Sync {
    /// The statements of staged structure edits of the table `meta` describes (None: a new
    /// table), with `DdlPlan::transactional` saying whether they run in one transaction.
    fn plan(&self, meta: Option<&TableMeta>, ops: &[DdlOp]) -> Result<DdlPlan, AppError>;

    /// A CREATE TABLE statement that rebuilds `meta` (Copy as CREATE, export).
    fn create_table(&self, meta: &TableMeta) -> Result<String, AppError>;

    fn drop_table(
        &self,
        table: &TableRef,
        kind: TableKind,
        cascade: bool,
    ) -> Result<String, AppError>;
}
