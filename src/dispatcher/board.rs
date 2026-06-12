use std::collections::HashMap;

use super::Column;

/// Per-column work-in-progress limits. Enforcement happens in the Dispatcher
/// when attempting to advance a task into a column.
#[derive(Debug, Clone)]
pub struct WipLimits {
    limits: HashMap<Column, u32>,
}

impl Default for WipLimits {
    fn default() -> Self {
        let mut limits = HashMap::new();
        // Work columns: 1 each (except Prioritized = unlimited backlog staging)
        limits.insert(Column::Prioritized, u32::MAX);
        limits.insert(Column::Designing, 1);
        limits.insert(Column::Coding, 1);
        limits.insert(Column::Reviewing, 1);
        limits.insert(Column::Testing, 1);
        limits.insert(Column::Auditing, 1);
        limits.insert(Column::Deploying, 1);
        // Buffer columns: 1 each
        limits.insert(Column::CodingBuffer, 1);
        limits.insert(Column::ReviewBuffer, 1);
        limits.insert(Column::TestBuffer, 1);
        limits.insert(Column::SecurityAuditBuffer, 1);
        limits.insert(Column::DeploymentBuffer, 1);
        // Done: unlimited
        limits.insert(Column::Done, u32::MAX);
        Self { limits }
    }
}

impl WipLimits {
    /// Returns true if the column can accept another task given its current count.
    pub fn allows(&self, column: Column, current_count: u32) -> bool {
        let limit = self.limits.get(&column).copied().unwrap_or(u32::MAX);
        current_count < limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_cover_all_columns() {
        let wip = WipLimits::default();
        for col in Column::all() {
            assert!(
                wip.limits.contains_key(col),
                "WipLimits missing entry for {col}"
            );
        }
    }

    #[test]
    fn work_columns_limit_is_one_except_prioritized() {
        let wip = WipLimits::default();
        let work_with_limit_1 = [
            Column::Designing,
            Column::Coding,
            Column::Reviewing,
            Column::Testing,
            Column::Auditing,
            Column::Deploying,
        ];
        for col in &work_with_limit_1 {
            assert_eq!(wip.limits[col], 1, "{col} should have limit 1");
        }
        assert_eq!(wip.limits[&Column::Prioritized], u32::MAX);
    }

    #[test]
    fn buffer_columns_limit_is_one() {
        let wip = WipLimits::default();
        let buffers = [
            Column::CodingBuffer,
            Column::ReviewBuffer,
            Column::TestBuffer,
            Column::SecurityAuditBuffer,
            Column::DeploymentBuffer,
        ];
        for col in &buffers {
            assert_eq!(wip.limits[col], 1, "{col} should have limit 1");
        }
    }

    #[test]
    fn done_is_unlimited() {
        let wip = WipLimits::default();
        assert_eq!(wip.limits[&Column::Done], u32::MAX);
    }

    #[test]
    fn allows_when_under_limit() {
        let wip = WipLimits::default();
        assert!(wip.allows(Column::Coding, 0));
    }

    #[test]
    fn blocks_when_at_limit() {
        let wip = WipLimits::default();
        assert!(!wip.allows(Column::Coding, 1));
    }

    #[test]
    fn unlimited_columns_always_allow() {
        let wip = WipLimits::default();
        assert!(wip.allows(Column::Prioritized, 1_000_000));
        assert!(wip.allows(Column::Done, 1_000_000));
    }
}
