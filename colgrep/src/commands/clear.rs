use std::path::PathBuf;

use anyhow::Result;

use colgrep::{
    acquire_index_lock, find_parent_index, get_colgrep_data_dir,
    get_index_dir_for_project_and_model, IndexState, ProjectMetadata,
};

use crate::commands::search::resolve_model;

pub fn cmd_clear(path: &PathBuf, all: bool) -> Result<()> {
    // Resolve the active model to determine which index to clear
    let model = resolve_model(None);

    if all {
        // Clear all indexes
        let data_dir = get_colgrep_data_dir()?;
        if !data_dir.exists() {
            println!("No indexes found.");
            return Ok(());
        }

        // Collect index directories and their project paths
        let index_dirs: Vec<_> = std::fs::read_dir(&data_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        if index_dirs.is_empty() {
            println!("No indexes found.");
            return Ok(());
        }

        // Only clear indexes belonging to the active model.
        // This ensures `clear --all` with model X doesn't destroy model Y's indexes.
        let mut cleared = 0;
        for entry in &index_dirs {
            let index_path = entry.path();

            // Check if this index belongs to the active model
            if let Ok(state) = IndexState::load(&index_path) {
                if !state.model_id.is_empty() && state.model_id != model {
                    continue; // Skip indexes for other models
                }
            }

            let project_path = ProjectMetadata::load(&index_path)
                .map(|m| m.project_path.display().to_string())
                .unwrap_or_else(|_| index_path.display().to_string());

            // Acquire lock before deleting, then drop it so the lock file can be removed
            let lock = acquire_index_lock(&index_path)?;
            drop(lock);
            std::fs::remove_dir_all(&index_path)?;
            println!("🗑️  Cleared index for {} (model: {})", project_path, model);
            cleared += 1;
        }

        if cleared == 0 {
            println!("No indexes found for model: {}", model);
        } else {
            println!("\n✅ Cleared {} index(es)", cleared);
        }
    } else {
        // Clear index for current project, respecting active model
        let path = std::fs::canonicalize(path)?;

        // Always use model-aware index directory
        let index_dir = get_index_dir_for_project_and_model(&path, &model)?;

        if index_dir.exists() {
            // Exact match found - clear it
            let lock = acquire_index_lock(&index_dir)?;
            drop(lock);
            std::fs::remove_dir_all(&index_dir)?;
            println!(
                "🗑️  Cleared index for {} (model: {})",
                path.display(),
                model
            );
        } else if let Some(parent_info) = find_parent_index(&path, Some(&model))? {
            // We're in a subdirectory of an indexed project - clear the parent index
            let lock = acquire_index_lock(&parent_info.index_dir)?;
            drop(lock);
            std::fs::remove_dir_all(&parent_info.index_dir)?;
            println!(
                "🗑️  Cleared index for {} (parent of current directory, model: {})",
                parent_info.project_path.display(),
                model
            );
        } else {
            println!("No index found for {} (model: {})", path.display(), model);
            return Ok(());
        }
    }

    Ok(())
}
