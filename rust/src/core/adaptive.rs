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
}
