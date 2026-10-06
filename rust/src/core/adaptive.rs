use crate::core::cache::SessionCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    Mechanical,
    Standard,
    Architectural,
}

impl TaskComplexity {
    pub fn instruction_suffix(&self) -> &'static str {
        match self {
            TaskComplexity::Mechanical => {
                "TASK COMPLEXITY: mechanical\n\
                 Minimal reasoning needed. Act immediately, report result in one line."
            }
            TaskComplexity::Standard => {
                "TASK COMPLEXITY: standard\n\
                 Brief reasoning allowed. Summarize approach in 1-2 lines, then act."
            }
            TaskComplexity::Architectural => {
                "TASK COMPLEXITY: architectural\n\
                 Full reasoning expected. Outline approach, consider edge cases, then act."
            }
        }
    }
}

pub fn classify_from_context(cache: &SessionCache) -> TaskComplexity {
    let stats = cache.get_stats();
    let unique_files = cache.get_all_entries().len();
    let total_reads = stats.total_reads();

    if unique_files <= 1 && total_reads <= 3 {
        return TaskComplexity::Mechanical;
    }

    if unique_files >= 5 || total_reads >= 15 {
        return TaskComplexity::Architectural;
    }

    TaskComplexity::Standard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_suffix_not_empty() {
        assert!(!TaskComplexity::Mechanical.instruction_suffix().is_empty());
        assert!(!TaskComplexity::Standard.instruction_suffix().is_empty());
        assert!(
            !TaskComplexity::Architectural
                .instruction_suffix()
                .is_empty()
        );
    }

    #[test]
    fn test_context_based_mechanical() {
        let cache = SessionCache::new();
        let result = classify_from_context(&cache);
        assert_eq!(result, TaskComplexity::Mechanical);
    }
}
