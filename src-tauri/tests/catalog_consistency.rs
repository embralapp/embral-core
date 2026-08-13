//! The embedding model's identity lives twice on purpose: the download
//! side in embral-engine's catalog (which can't depend on ort) and the
//! inference side in embral-search's model.rs (which can't depend on
//! sherpa). This test is what keeps the pair from drifting, byte sizes
//! included, so a silently re-uploaded HF file fails here, not at query
//! time.

use embral_engine::catalog;

#[test]
fn the_catalog_and_embral_search_agree_on_the_embedding_model() {
    let entry = catalog::find(embral_search::model::MODEL_ID)
        .expect("the embedding model is in the catalog");
    assert!(matches!(entry.kind, catalog::ModelKind::Embedding));

    let files = entry.expected_files();
    assert_eq!(files.len(), 2);

    let model_path = entry.role_path(catalog::FileRole::TextEmbedding);
    let tokenizer_path = entry.role_path(catalog::FileRole::TokenizerJson);
    assert_eq!(model_path, Some(embral_search::model::model_path()));
    assert_eq!(tokenizer_path, Some(embral_search::model::tokenizer_path()));

    let by_role = |role: catalog::FileRole| -> u64 {
        match &entry.source {
            catalog::ModelSource::Files(files) => files
                .iter()
                .find(|f| f.role == role)
                .expect("role present")
                .bytes,
            _ => panic!("the embedding model downloads as plain files"),
        }
    };
    assert_eq!(
        by_role(catalog::FileRole::TextEmbedding),
        embral_search::model::MODEL_BYTES
    );
    assert_eq!(
        by_role(catalog::FileRole::TokenizerJson),
        embral_search::model::TOKENIZER_BYTES
    );
}
