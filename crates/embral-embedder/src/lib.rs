//! Local text embedding: multilingual-e5-small over ONNX Runtime. E5's
//! contract (`"query: "`/`"passage: "` prefixes, mean pooling over the
//! attention mask, L2 normalization) lives here so callers need only hand
//! over text. Loading is ~a second and the session is reusable; hold one
//! per process and evict on idle.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use embral_search::model;
use ort::session::Session;
use ort::value::Tensor;

const MAX_TOKENS: usize = 512;

pub struct Embedder {
    session: Session,
    tokenizer: tokenizers::Tokenizer,
    needs_token_type_ids: bool,
}

impl Embedder {
    /// Load from the model dir ([`model::model_path`]/[`model::tokenizer_path`]).
    pub fn load_default() -> Result<Embedder> {
        Self::load(&model::model_path(), &model::tokenizer_path())
    }

    pub fn load(model_file: &Path, tokenizer_file: &Path) -> Result<Embedder> {
        let session = Session::builder()?
            .commit_from_file(model_file)
            .with_context(|| format!("load embedding model {}", model_file.display()))?;
        let needs_token_type_ids = session
            .inputs()
            .iter()
            .any(|input| input.name() == "token_type_ids");

        let mut tokenizer = tokenizers::Tokenizer::from_file(tokenizer_file)
            .map_err(|e| anyhow!("load tokenizer {}: {e}", tokenizer_file.display()))?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| anyhow!("configure truncation: {e}"))?;
        tokenizer.with_padding(Some(tokenizers::PaddingParams::default()));

        Ok(Embedder {
            session,
            tokenizer,
            needs_token_type_ids,
        })
    }

    pub fn embed_passages(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts.iter().map(|t| format!("passage: {t}")).collect())
    }

    pub fn embed_query(&mut self, text: &str) -> Result<Vec<f32>> {
        let mut vectors = self.embed(vec![format!("query: {text}")])?;
        vectors.pop().ok_or_else(|| anyhow!("empty embedding batch"))
    }

    fn embed(&mut self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts, true)
            .map_err(|e| anyhow!("tokenize: {e}"))?;
        let batch = encodings.len();
        let seq = encodings[0].get_ids().len();

        let mut input_ids: Vec<i64> = Vec::with_capacity(batch * seq);
        let mut attention_mask: Vec<i64> = Vec::with_capacity(batch * seq);
        for enc in &encodings {
            input_ids.extend(enc.get_ids().iter().map(|&id| id as i64));
            attention_mask.extend(enc.get_attention_mask().iter().map(|&m| m as i64));
        }

        let shape = vec![batch as i64, seq as i64];
        let mut inputs: Vec<(&str, ort::session::SessionInputValue)> = vec![
            (
                "input_ids",
                Tensor::from_array((shape.clone(), input_ids))?.into(),
            ),
            (
                "attention_mask",
                Tensor::from_array((shape.clone(), attention_mask.clone()))?.into(),
            ),
        ];
        if self.needs_token_type_ids {
            inputs.push((
                "token_type_ids",
                Tensor::from_array((shape.clone(), vec![0i64; batch * seq]))?.into(),
            ));
        }

        let outputs = self.session.run(inputs)?;
        let (out_shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let dims: Vec<i64> = out_shape.iter().copied().collect();
        let [out_batch, out_seq, hidden] = dims[..] else {
            bail!("unexpected embedding output shape {dims:?}");
        };
        if out_batch as usize != batch || out_seq as usize != seq {
            bail!("embedding output shape {dims:?} does not match input {batch}x{seq}");
        }
        if hidden as usize != model::DIM {
            bail!("model produced {hidden}-dim vectors, expected {}", model::DIM);
        }
        let hidden = hidden as usize;

        // Mean-pool token states over the attention mask, then L2-normalize
        // so vec0's L2 distance ranks like cosine similarity.
        let mut vectors = Vec::with_capacity(batch);
        for b in 0..batch {
            let mut pooled = vec![0.0f32; hidden];
            let mut count = 0.0f32;
            for s in 0..seq {
                if attention_mask[b * seq + s] == 0 {
                    continue;
                }
                count += 1.0;
                let offset = (b * seq + s) * hidden;
                for (h, value) in pooled.iter_mut().enumerate() {
                    *value += data[offset + h];
                }
            }
            if count > 0.0 {
                for value in &mut pooled {
                    *value /= count;
                }
            }
            let norm = pooled.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm > 0.0 {
                for value in &mut pooled {
                    *value /= norm;
                }
            }
            vectors.push(pooled);
        }
        Ok(vectors)
    }
}

/// A real-model round trip, run manually against a downloaded model dir:
/// `EMBRAL_TEST_EMBED_DIR=%LOCALAPPDATA%\embral\models\embedding-multilingual
///  cargo test -p embral-embedder -- --ignored --nocapture`
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore = "needs EMBRAL_TEST_EMBED_DIR pointing at a downloaded model dir"]
    fn real_model_embeds_and_ranks_semantically() {
        let dir = std::path::PathBuf::from(
            std::env::var("EMBRAL_TEST_EMBED_DIR").expect("set EMBRAL_TEST_EMBED_DIR"),
        );
        let start = Instant::now();
        let mut embedder = Embedder::load(
            &dir.join(model::MODEL_FILE),
            &dir.join(model::TOKENIZER_FILE),
        )
        .unwrap();
        eprintln!("load: {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);

        let passages = [
            "We ordered sandwiches and salads for the team lunch.",
            "The quarterly budget review moved to Thursday.",
            "Deployment of the new build finished without errors.",
        ];
        let start = Instant::now();
        let vectors = embedder.embed_passages(&passages).unwrap();
        eprintln!(
            "embed 3 passages: {:.1} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(vectors.len(), 3);
        for v in &vectors {
            assert_eq!(v.len(), model::DIM);
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "not normalized: {norm}");
        }

        let start = Instant::now();
        let query = embedder.embed_query("what did we eat").unwrap();
        eprintln!(
            "embed 1 query: {:.1} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        let cosine = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let scores: Vec<f32> = vectors.iter().map(|v| cosine(&query, v)).collect();
        assert!(
            scores[0] > scores[1] && scores[0] > scores[2],
            "lunch should win: {scores:?}"
        );
    }
}
