use proptest::prelude::*;
use sobre::core::compressor::safeguard_ratio;
use sobre::core::patterns::compress_output;
use sobre::core::tokens::count_tokens;

proptest! {
    #[test]
    fn safeguard_never_inflates(
        original in "[a-zA-Z0-9 \n]{1,500}",
        compressed in "[a-zA-Z0-9 \n]{0,500}"
    ) {
        let result = safeguard_ratio(&original, &compressed);
        let result_tokens = count_tokens(&result);
        let orig_tokens = count_tokens(&original);
        let comp_tokens = count_tokens(&compressed);
        let max_tokens = orig_tokens.max(comp_tokens);
        prop_assert!(
            result_tokens <= max_tokens,
            "safeguard_ratio returned {} tokens, max(orig={}, comp={}) = {}",
            result_tokens, orig_tokens, comp_tokens, max_tokens
        );
    }

    #[test]
    fn safeguard_returns_one_of_inputs(
        original in "[a-zA-Z0-9 \n]{1,200}",
        compressed in "[a-zA-Z0-9 \n]{0,200}"
    ) {
        let result = safeguard_ratio(&original, &compressed);
        prop_assert!(
            result == original || result == compressed,
            "safeguard_ratio must return either original or compressed"
        );
    }

    #[test]
    fn compress_output_never_inflates_tokens(
        command in "(git status|cargo test|npm install|ls -la)",
        output in "[a-zA-Z0-9:/ \n._-]{20,1000}"
    ) {
        if let Some(compressed) = compress_output(&command, &output) {
            let orig_tokens = count_tokens(&output);
            let comp_tokens = count_tokens(&compressed);
            prop_assert!(
                comp_tokens <= orig_tokens,
                "compress_output inflated: {} > {} for command '{}'",
                comp_tokens, orig_tokens, command
            );
        }
    }
}

#[cfg(test)]
mod fuzz_no_panic {
    use super::*;

    proptest! {
        #[test]
        fn compress_output_no_panic_on_arbitrary_utf8(
            command in "\\PC{1,100}",
            output in "\\PC{0,2000}"
        ) {
            let _ = compress_output(&command, &output);
        }

        #[test]
        fn safeguard_no_panic_on_empty(
            text in "\\PC{0,100}"
        ) {
            let _ = safeguard_ratio("", &text);
            let _ = safeguard_ratio(&text, "");
        }
    }
}
