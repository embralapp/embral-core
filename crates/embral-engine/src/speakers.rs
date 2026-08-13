//! Pure diarization math: no ONNX, fully unit-testable.
//!
//! The diarization models (see [`crate::Engine::diarize`] / `embed`) produce
//! per-recording clusters and voice embeddings; everything that turns those
//! into labels lives here: the live one-pass clusterer behind provisional
//! speaker labels and the mapping of transcript segments onto diarized spans.

use crate::engine::DiarizedSpan;

/// Cosine similarity in [-1, 1]; 0.0 for mismatched or empty inputs.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Similarity floor for joining an existing live cluster during a recording.
/// Loose on purpose: single-utterance embeddings are noisy, and a wrong live
/// label is a provisional preview the post-meeting pass overwrites, while a
/// spuriously split speaker is immediately visible noise.
pub const ONLINE_CLUSTER_THRESHOLD: f32 = 0.6;

/// One-pass greedy clustering over per-utterance voice embeddings: the live
/// counterpart of the recording-wide diarization pass. Each embedding joins
/// the most similar existing cluster at or above `threshold` (updating that
/// cluster's running-mean centroid) or opens a new one. Cluster indices are
/// 0-based in first-appearance order, matching the offline pipeline's
/// numbering convention.
pub struct OnlineClusterer {
    threshold: f32,
    centroids: Vec<Vec<f32>>,
    counts: Vec<f32>,
}

impl OnlineClusterer {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold,
            centroids: Vec::new(),
            counts: Vec::new(),
        }
    }

    /// Number of clusters seen so far.
    pub fn len(&self) -> usize {
        self.centroids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.centroids.is_empty()
    }

    /// Assign one embedding, returning its 0-based cluster index.
    pub fn assign(&mut self, embedding: &[f32]) -> usize {
        let best = self
            .centroids
            .iter()
            .enumerate()
            .map(|(i, c)| (i, cosine(c, embedding)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, s)) if s >= self.threshold => {
                // Running mean; magnitude drift is irrelevant to cosine.
                let n = self.counts[i];
                for (c, e) in self.centroids[i].iter_mut().zip(embedding) {
                    *c = (*c * n + e) / (n + 1.0);
                }
                self.counts[i] += 1.0;
                i
            }
            _ => {
                self.centroids.push(embedding.to_vec());
                self.counts.push(1.0);
                self.centroids.len() - 1
            }
        }
    }
}

/// For each transcript segment `(start, end)`, the diarized cluster it
/// overlaps most, or `None` when it overlaps no span at all.
pub fn label_segments(segments: &[(f64, f64)], spans: &[DiarizedSpan]) -> Vec<Option<usize>> {
    segments
        .iter()
        .map(|&(start, end)| {
            let mut per_cluster: std::collections::HashMap<usize, f64> =
                std::collections::HashMap::new();
            for span in spans {
                let overlap = span.end.min(end) - span.start.max(start);
                if overlap > 0.0 {
                    *per_cluster.entry(span.cluster).or_default() += overlap;
                }
            }
            per_cluster
                .into_iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(cluster, _)| cluster)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, end: f64, cluster: usize) -> DiarizedSpan {
        DiarizedSpan { start, end, cluster }
    }

    #[test]
    fn cosine_basics() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0, 0.0], &[-1.0, 0.0]), -1.0);
        // Scale-invariant.
        assert!((cosine(&[2.0, 2.0], &[5.0, 5.0]) - 1.0).abs() < 1e-6);
        // Degenerate inputs are 0, not NaN.
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn online_clusterer_groups_similar_voices_in_appearance_order() {
        let mut c = OnlineClusterer::new(0.6);
        // Two orthogonal "voices": every same-voice embedding rejoins its
        // cluster, and numbering follows first appearance.
        assert_eq!(c.assign(&[1.0, 0.0]), 0);
        assert_eq!(c.assign(&[0.9, 0.1]), 0);
        assert_eq!(c.assign(&[0.0, 1.0]), 1);
        assert_eq!(c.assign(&[0.1, 0.9]), 1);
        assert_eq!(c.assign(&[1.0, 0.05]), 0);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn online_clusterer_centroid_tracks_the_running_mean() {
        let mut c = OnlineClusterer::new(0.6);
        c.assign(&[1.0, 0.0]);
        c.assign(&[0.0, 1.0]); // below threshold vs [1,0] → new cluster
        assert_eq!(c.len(), 2);
        // A vector between the two leans toward whichever centroid it joins;
        // after joining cluster 0 twice, the drifted centroid still owns it.
        assert_eq!(c.assign(&[0.8, 0.6]), 0);
        assert_eq!(c.assign(&[0.75, 0.65]), 0);
        // A pure second-axis vector still belongs to cluster 1.
        assert_eq!(c.assign(&[0.0, 1.0]), 1);
    }

    #[test]
    fn label_segments_picks_max_overlap() {
        let spans = vec![span(0.0, 5.0, 0), span(5.0, 10.0, 1)];
        let labels = label_segments(
            &[
                (0.0, 4.0),  // inside cluster 0
                (4.0, 9.0),  // 1s of c0, 4s of c1
                (12.0, 13.0), // overlaps nothing
            ],
            &spans,
        );
        assert_eq!(labels, vec![Some(0), Some(1), None]);
    }

    #[test]
    fn label_segments_sums_split_spans_per_cluster() {
        // Cluster 0 speaks twice inside the segment; combined it out-overlaps
        // cluster 1's single longer turn.
        let spans = vec![span(0.0, 2.0, 0), span(3.0, 5.0, 0), span(2.0, 3.5, 1)];
        assert_eq!(label_segments(&[(0.0, 5.0)], &spans), vec![Some(0)]);
    }
}
