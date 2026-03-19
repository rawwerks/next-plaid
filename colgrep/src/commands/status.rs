use std::path::PathBuf;

use anyhow::Result;

use colgrep::{get_index_dir_for_project_and_model, index_exists_for_model, IndexState};

use crate::commands::search::resolve_model;

pub fn cmd_status(path: &PathBuf) -> Result<()> {
    let path = std::fs::canonicalize(path)?;
    let model = resolve_model(None);

    if !index_exists_for_model(&path, &model) {
        println!("No index found for {} (model: {})", path.display(), model);
        println!("Run `colgrep <query>` to create one.");
        return Ok(());
    }

    let index_dir = get_index_dir_for_project_and_model(&path, &model)?;

    // Show model from state.json
    let model_display = IndexState::load(&index_dir)
        .ok()
        .and_then(|s| {
            if s.model_id.is_empty() {
                None
            } else {
                Some(s.model_id)
            }
        })
        .unwrap_or_else(|| model.clone());

    println!("Project: {}", path.display());
    println!("Model:   {}", model_display);
    println!("Index:   {}", index_dir.display());
    println!();
    println!("Run any search to update the index, or `colgrep clear` to rebuild from scratch.");

    Ok(())
}
