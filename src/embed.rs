//! BGE embedding backend (pinned candle 0.9.1 / tokenizers 0.22).

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use tokenizers::Tokenizer;

use crate::error::LibraryError;
use crate::ingest::{Embedder, INGEST_DIMENSION};
use crate::model_cache::ModelPaths;

fn io_err(message: impl Into<String>) -> LibraryError {
    LibraryError::Io {
        message: message.into(),
    }
}

pub(crate) const MAX_TOKENS: usize = 512;

pub(crate) fn check_token_len(len: usize) -> Result<(), LibraryError> {
    if len > MAX_TOKENS {
        return Err(LibraryError::InvalidInput {
            message: format!("ingest text too long: {len} tokens (max {MAX_TOKENS})"),
        });
    }
    Ok(())
}

pub(crate) struct BgeEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl BgeEmbedder {
    pub(crate) fn load(paths: &ModelPaths) -> Result<Self, LibraryError> {
        let config_raw = std::fs::read_to_string(&paths.config)
            .map_err(|e| io_err(format!("read {}: {e}", paths.config.display())))?;
        let config: Config = serde_json::from_str(&config_raw)
            .map_err(|e| io_err(format!("parse config.json: {e}")))?;
        let tokenizer = Tokenizer::from_file(&paths.tokenizer)
            .map_err(|e| io_err(format!("load tokenizer.json: {e}")))?;
        let weights = std::fs::read(&paths.weights)
            .map_err(|e| io_err(format!("read {}: {e}", paths.weights.display())))?;
        let device = Device::Cpu;
        let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)
            .map_err(|e| io_err(format!("load model.safetensors: {e}")))?;
        let model =
            BertModel::load(vb, &config).map_err(|e| io_err(format!("load bert model: {e}")))?;
        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }
}

impl Embedder for BgeEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, LibraryError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| io_err(format!("tokenize: {e}")))?;
        let ids = encoding.get_ids();
        check_token_len(ids.len())?;
        let seq = ids.len();
        if seq == 0 {
            return Err(io_err("tokenizer produced empty encoding"));
        }
        let ids_u32: Vec<u32> = ids.to_vec();
        let type_ids = vec![0u32; seq];
        let mask = vec![1u32; seq];
        let device = &self.device;
        let input_ids =
            Tensor::new(ids_u32.clone(), device).map_err(|e| io_err(format!("tensor: {e}")))?;
        let input_ids = input_ids
            .unsqueeze(0)
            .map_err(|e| io_err(format!("tensor: {e}")))?;
        let token_type_ids =
            Tensor::new(type_ids, device).map_err(|e| io_err(format!("tensor: {e}")))?;
        let token_type_ids = token_type_ids
            .unsqueeze(0)
            .map_err(|e| io_err(format!("tensor: {e}")))?;
        let attention_mask =
            Tensor::new(mask, device).map_err(|e| io_err(format!("tensor: {e}")))?;
        let attention_mask = attention_mask
            .unsqueeze(0)
            .map_err(|e| io_err(format!("tensor: {e}")))?;
        let output = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .map_err(|e| io_err(format!("bert forward: {e}")))?;
        // Masked mean-pool over sequence dim.
        let mask_f = attention_mask
            .to_dtype(DType::F32)
            .map_err(|e| io_err(format!("tensor: {e}")))?;
        let mask_exp = mask_f
            .unsqueeze(2)
            .map_err(|e| io_err(format!("tensor: {e}")))?;
        let masked = output
            .broadcast_mul(&mask_exp)
            .map_err(|e| io_err(format!("pool: {e}")))?;
        let summed = masked.sum(1).map_err(|e| io_err(format!("pool: {e}")))?;
        let count = mask_f.sum(1).map_err(|e| io_err(format!("pool: {e}")))?;
        let count = count
            .clamp(1e-9f32, f32::MAX)
            .map_err(|e| io_err(format!("pool: {e}")))?;
        let count = count
            .unsqueeze(1)
            .map_err(|e| io_err(format!("tensor: {e}")))?;
        let mean = summed
            .broadcast_div(&count)
            .map_err(|e| io_err(format!("pool: {e}")))?;
        let flat: Vec<f32> = mean
            .squeeze(0)
            .map_err(|e| io_err(format!("tensor: {e}")))?
            .to_vec1()
            .map_err(|e| io_err(format!("tensor: {e}")))?;
        if flat.len() != INGEST_DIMENSION as usize {
            return Err(io_err(format!(
                "unexpected embedding dimension {}",
                flat.len()
            )));
        }
        for v in &flat {
            if !v.is_finite() {
                return Err(io_err("non-finite embedding value"));
            }
        }
        let norm: f32 = flat.iter().map(|v| v * v).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= 0.0 {
            return Err(io_err("invalid embedding norm"));
        }
        let out: Vec<f32> = flat.iter().map(|v| v / norm).collect();
        for v in &out {
            if !v.is_finite() {
                return Err(io_err("non-finite normalized value"));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_len_512_accepted() {
        assert!(check_token_len(512).is_ok());
    }

    #[test]
    fn token_len_513_rejected() {
        match check_token_len(513) {
            Err(LibraryError::InvalidInput { .. }) => {}
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn token_len_0_accepted() {
        assert!(check_token_len(0).is_ok());
    }

    #[test]
    #[ignore]
    fn real_model_smoke() {
        let paths = crate::model_cache::ensure_model_files().expect("cache");
        let embedder = BgeEmbedder::load(&paths).expect("load");
        let vec = embedder.embed("hello world").expect("embed");
        assert_eq!(vec.len(), 384);
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "norm was {norm}");
    }
}
