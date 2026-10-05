//! Pooling strategies for transformer hidden states.
//!
//! Converts per-token hidden states `[seq_len × dim]` into a single
//! fixed-size embedding vector `[dim]`.

use super::model_registry::Pooling;

/// Mean pooling over token positions, weighted by attention mask.
///
/// Takes the raw hidden state output `[1 × seq_len × dim]` flattened to a Vec,
/// and produces a single embedding by averaging across attended positions.
pub fn mean_pool(
    hidden_states: &[f32],
    attention_mask: &[i32],
    seq_len: usize,
    dim: usize,
) -> Vec<f32> {
    let mut sum = vec![0.0f32; dim];
    let mut count = 0.0f32;

    for pos in 0..seq_len {
        if attention_mask.get(pos).copied().unwrap_or(0) > 0 {
            let offset = pos * dim;
            for (d, sum_val) in sum.iter_mut().enumerate().take(dim) {
                if let Some(&val) = hidden_states.get(offset + d) {
                    *sum_val += val;
                }
            }
            count += 1.0;
        }
    }

    if count > 0.0 {
        for val in &mut sum {
            *val /= count;
        }
    }

    sum
}

/// Pool one sequence `[seq_len × dim]` with the model's pooling strategy.
/// Sequences are right-padded, so the last attended position is the last token.
pub fn pool(
    hidden_states: &[f32],
    attention_mask: &[i32],
    seq_len: usize,
    dim: usize,
    pooling: Pooling,
) -> Vec<f32> {
    let row = |pos: usize| {
        hidden_states
            .get(pos * dim..(pos + 1) * dim)
            .map_or_else(|| vec![0.0; dim], <[f32]>::to_vec)
    };
    match pooling {
        Pooling::Mean => mean_pool(hidden_states, attention_mask, seq_len, dim),
        Pooling::Cls => row(0),
        Pooling::LastToken => {
            let attended = attention_mask
                .iter()
                .take(seq_len)
                .filter(|&&m| m > 0)
                .count();
            row(attended.saturating_sub(1))
        }
    }
}

/// Batched pooling over one output tensor `[batch, max_seq_len, dim]`
/// (flattened row-major); each sequence uses its own attention mask.
pub fn pool_batch(
    hidden_states: &[f32],
    masks: &[&[i32]],
    max_seq_len: usize,
    dim: usize,
    pooling: Pooling,
) -> Vec<Vec<f32>> {
    let batch = masks.len();
    if hidden_states.len() < batch * max_seq_len * dim {
        return vec![vec![0.0; dim]; batch];
    }
    masks
        .iter()
        .enumerate()
        .map(|(b, m)| {
            let h = &hidden_states[b * max_seq_len * dim..][..max_seq_len * dim];
            pool(h, m, max_seq_len, dim, pooling)
        })
        .collect()
}

/// L2-normalize a vector in-place.
pub fn normalize_l2(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in vec.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_pool_basic() {
        // 2 tokens, 3 dimensions, all attended
        let hidden = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mask = vec![1, 1];
        let result = mean_pool(&hidden, &mask, 2, 3);
        assert_eq!(result.len(), 3);
        assert!((result[0] - 2.5).abs() < 1e-6);
        assert!((result[1] - 3.5).abs() < 1e-6);
        assert!((result[2] - 4.5).abs() < 1e-6);
    }

    #[test]
    fn mean_pool_with_padding() {
        // 3 tokens, 2 dimensions, last token is padding
        let hidden = vec![1.0, 2.0, 3.0, 4.0, 0.0, 0.0];
        let mask = vec![1, 1, 0];
        let result = mean_pool(&hidden, &mask, 3, 2);
        assert!((result[0] - 2.0).abs() < 1e-6);
        assert!((result[1] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn mean_pool_single_token() {
        let hidden = vec![5.0, 10.0];
        let mask = vec![1];
        let result = mean_pool(&hidden, &mask, 1, 2);
        assert!((result[0] - 5.0).abs() < 1e-6);
        assert!((result[1] - 10.0).abs() < 1e-6);
    }

    #[test]
    fn mean_pool_all_masked() {
        let hidden = vec![1.0, 2.0, 3.0, 4.0];
        let mask = vec![0, 0];
        let result = mean_pool(&hidden, &mask, 2, 2);
        assert!(result.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn cls_and_last_token_pick_one_row() {
        // 3 tokens, 2 dimensions, last token is padding
        let hidden = vec![1.0, 2.0, 3.0, 4.0, 9.0, 9.0];
        let mask = vec![1, 1, 0];
        assert_eq!(pool(&hidden, &mask, 3, 2, Pooling::Cls), vec![1.0, 2.0]);
        assert_eq!(
            pool(&hidden, &mask, 3, 2, Pooling::LastToken),
            vec![3.0, 4.0]
        );
        assert_eq!(pool(&hidden, &mask, 3, 2, Pooling::Mean), vec![2.0, 3.0]);
    }

    #[test]
    fn batch_pooling_uses_each_mask() {
        // batch 2, max_seq_len 2, dim 1
        let hidden = vec![1.0, 3.0, 5.0, 7.0];
        let masks: Vec<&[i32]> = vec![&[1, 1], &[1, 0]];
        assert_eq!(
            pool_batch(&hidden, &masks, 2, 1, Pooling::LastToken),
            vec![vec![3.0], vec![5.0]]
        );
        assert_eq!(
            pool_batch(&hidden, &masks, 2, 1, Pooling::Mean),
            vec![vec![2.0], vec![5.0]]
        );
    }

    #[test]
    fn normalize_l2_basic() {
        let mut vec = vec![3.0, 4.0];
        normalize_l2(&mut vec);
        assert!((vec[0] - 0.6).abs() < 1e-6);
        assert!((vec[1] - 0.8).abs() < 1e-6);

        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn normalize_l2_already_normalized() {
        let mut vec = vec![1.0, 0.0, 0.0];
        normalize_l2(&mut vec);
        assert!((vec[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_l2_zero_vector() {
        let mut vec = vec![0.0, 0.0, 0.0];
        normalize_l2(&mut vec);
        assert!(vec.iter().all(|&v| v == 0.0));
    }
}
