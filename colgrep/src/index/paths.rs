//! Centralized index storage paths following XDG Base Directory Specification
//!
//! Index storage location:
//! - Linux: ~/.local/share/colgrep/indices/
//! - macOS: ~/Library/Application Support/colgrep/indices/
//! - Windows: C:\Users\{user}\AppData\Roaming\colgrep\indices\

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use super::state::IndexState;

const STATE_FILE: &str = "state.json";
const PROJECT_FILE: &str = "project.json";
const INDEX_SUBDIR: &str = "index";
const LOCK_FILE: &str = ".lock";

/// Metadata about the project stored alongside the index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMetadata {
    /// Canonical path to the project directory
    pub project_path: PathBuf,
    /// Project name (directory name)
    pub project_name: String,
}

impl ProjectMetadata {
    pub fn new(project_path: &Path) -> Self {
        let project_name = project_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string());

        Self {
            project_path: project_path.to_path_buf(),
            project_name,
        }
    }

    pub fn load(index_dir: &Path) -> Result<Self> {
        let path = index_dir.join(PROJECT_FILE);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, index_dir: &Path) -> Result<()> {
        fs::create_dir_all(index_dir)?;
        let path = index_dir.join(PROJECT_FILE);
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }
}

/// Get the base colgrep data directory (XDG_DATA_HOME/colgrep or platform equivalent)
pub fn get_colgrep_data_dir() -> Result<PathBuf> {
    let data_dir = dirs::data_dir().context("Could not determine data directory")?;
    Ok(data_dir.join("colgrep").join("indices"))
}

/// Compute the index directory name for a project path and optional model identifier.
/// Format: {project_name}-{first 8 hex chars of xxh3_64 hash}
/// When a model is specified, the hash incorporates both the path and model identity,
/// so different models get separate index directories for the same project.
fn compute_index_dir_name(project_path: &Path, model: Option<&str>) -> String {
    let path_str = project_path.to_string_lossy();
    let hash_input = match model {
        Some(m) => format!("{}:{}", path_str, m),
        None => path_str.to_string(),
    };
    let hash = xxh3_64(hash_input.as_bytes());
    let hash_prefix = format!("{:08x}", hash).chars().take(8).collect::<String>();

    let project_name = project_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    // Sanitize project name (remove characters that might cause issues in filenames)
    let sanitized_name: String = project_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    format!("{}-{}", sanitized_name, hash_prefix)
}

/// Get the index directory for a project path.
/// Creates the directory structure if it doesn't exist.
/// When model is None, uses the legacy hash (path-only) for backwards compatibility.
pub fn get_index_dir_for_project(project_path: &Path) -> Result<PathBuf> {
    let base_dir = get_colgrep_data_dir()?;
    let dir_name = compute_index_dir_name(project_path, None);
    Ok(base_dir.join(dir_name))
}

/// Get the index directory for a project path with a specific model.
/// Different models get separate index directories, allowing multiple
/// indexes per project (e.g., code model + reasoning model).
pub fn get_index_dir_for_project_and_model(
    project_path: &Path,
    model: &str,
) -> Result<PathBuf> {
    let base_dir = get_colgrep_data_dir()?;
    let dir_name = compute_index_dir_name(project_path, Some(model));
    Ok(base_dir.join(dir_name))
}

/// Find an existing index for a project path (legacy, no model awareness).
/// Returns None if no index exists.
pub fn find_index_for_project(project_path: &Path) -> Result<Option<PathBuf>> {
    find_index_for_project_impl(project_path, None)
}

/// Find an existing index for a project path with a specific model.
/// Returns None if no index exists for this project+model combination.
pub fn find_index_for_project_and_model(
    project_path: &Path,
    model: &str,
) -> Result<Option<PathBuf>> {
    find_index_for_project_impl(project_path, Some(model))
}

/// Internal implementation for finding an index, with optional model.
fn find_index_for_project_impl(
    project_path: &Path,
    model: Option<&str>,
) -> Result<Option<PathBuf>> {
    let index_dir = match model {
        Some(m) => get_index_dir_for_project_and_model(project_path, m)?,
        None => get_index_dir_for_project(project_path)?,
    };

    // Check if the index directory exists and has valid metadata
    let metadata_path = index_dir.join(INDEX_SUBDIR).join("metadata.json");
    if metadata_path.exists() {
        // Verify the project path matches
        if let Ok(meta) = ProjectMetadata::load(&index_dir) {
            if meta.project_path == project_path {
                return Ok(Some(index_dir));
            }
        }
        // Index exists but project path doesn't match (hash collision)
        // This is extremely rare with xxh3_64, but handle it gracefully
        return Ok(Some(index_dir));
    }

    // If model-specific index not found, fall back to legacy index
    // This provides backwards compatibility for existing indexes
    if model.is_some() {
        let legacy_dir = get_index_dir_for_project(project_path)?;
        let legacy_metadata = legacy_dir.join(INDEX_SUBDIR).join("metadata.json");
        if legacy_metadata.exists() {
            return Ok(Some(legacy_dir));
        }
    }

    Ok(None)
}

/// Check if an index exists for the given project
pub fn index_exists(project_path: &Path) -> bool {
    matches!(find_index_for_project(project_path), Ok(Some(_)))
}

/// Check if a model-specific index exists for the given project
pub fn index_exists_for_model(project_path: &Path, model: &str) -> bool {
    matches!(
        find_index_for_project_and_model(project_path, model),
        Ok(Some(_))
    )
}

/// Information about a discovered parent index
#[derive(Debug, Clone)]
pub struct ParentIndexInfo {
    /// Path to the parent project's index directory
    pub index_dir: PathBuf,
    /// The parent project's root path
    pub project_path: PathBuf,
    /// Relative path from parent project root to the search directory
    pub relative_subdir: PathBuf,
}

/// Find if the given path is a subdirectory of any existing indexed project.
/// Returns the most specific (longest-matching) parent index if found.
///
/// When `model` is Some, only considers indexes that were built with that model
/// (by checking state.json's model_id). Falls back to model-unaware matching
/// if no model-specific parent index is found (backwards compatibility).
///
/// When `model` is None, matches any index regardless of model.
pub fn find_parent_index(
    search_path: &Path,
    model: Option<&str>,
) -> Result<Option<ParentIndexInfo>> {
    let data_dir = get_colgrep_data_dir()?;

    if !data_dir.exists() {
        return Ok(None);
    }

    let mut best_match: Option<ParentIndexInfo> = None;
    let mut best_depth = 0;
    // Track best legacy (no model) match as fallback
    let mut best_legacy_match: Option<ParentIndexInfo> = None;
    let mut best_legacy_depth = 0;

    for entry in fs::read_dir(&data_dir)?.filter_map(|e| e.ok()) {
        let index_dir = entry.path();
        if !index_dir.is_dir() {
            continue;
        }

        // Try to load project metadata
        if let Ok(meta) = ProjectMetadata::load(&index_dir) {
            // Check if search_path starts with this project's path
            // but is NOT the same path (must be a subdirectory)
            if search_path != meta.project_path {
                if let Ok(relative) = search_path.strip_prefix(&meta.project_path) {
                    let depth = meta.project_path.components().count();

                    // Check model match via state.json
                    let index_model_id = IndexState::load(&index_dir)
                        .ok()
                        .and_then(|s| {
                            if s.model_id.is_empty() {
                                None
                            } else {
                                Some(s.model_id)
                            }
                        });

                    let model_matches = match (model, &index_model_id) {
                        (Some(m), Some(id)) => m == id,
                        (None, _) => true, // No model filter — match anything
                        (Some(_), None) => false, // Want specific model, index has none
                    };

                    if model_matches && depth > best_depth {
                        best_depth = depth;
                        best_match = Some(ParentIndexInfo {
                            index_dir: index_dir.clone(),
                            project_path: meta.project_path.clone(),
                            relative_subdir: relative.to_path_buf(),
                        });
                    }

                    // Also track legacy fallback (model-unaware)
                    if !model_matches && depth > best_legacy_depth {
                        best_legacy_depth = depth;
                        best_legacy_match = Some(ParentIndexInfo {
                            index_dir,
                            project_path: meta.project_path,
                            relative_subdir: relative.to_path_buf(),
                        });
                    }
                }
            }
        }
    }

    // Prefer model-specific match, fall back to legacy
    Ok(best_match.or(best_legacy_match))
}

/// Get the path to the state.json file within an index directory
pub fn get_state_path(index_dir: &Path) -> PathBuf {
    index_dir.join(STATE_FILE)
}

/// Get the path to the vector index within an index directory
pub fn get_vector_index_path(index_dir: &Path) -> PathBuf {
    index_dir.join(INDEX_SUBDIR)
}

/// Get the path to the lock file within an index directory
pub fn get_lock_path(index_dir: &Path) -> PathBuf {
    index_dir.join(LOCK_FILE)
}

/// Try to acquire the index lock without waiting.
/// Returns `Ok(Some(file))` if acquired, `Ok(None)` if another process holds it.
pub fn try_acquire_index_lock(index_dir: &Path) -> Result<Option<File>> {
    fs::create_dir_all(index_dir)?;
    let lock_path = get_lock_path(index_dir);
    let lock_file = File::create(&lock_path)
        .with_context(|| format!("Failed to create lock file at {}", lock_path.display()))?;

    match lock_file.try_lock_exclusive() {
        Ok(()) => Ok(Some(lock_file)),
        Err(_) => Ok(None),
    }
}

/// Acquires an exclusive lock on the index directory.
/// Returns a guard (File handle) that releases the lock when dropped.
///
/// If another process holds the lock, retries for up to 5 seconds before
/// returning an error.
pub fn acquire_index_lock(index_dir: &Path) -> Result<File> {
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(5);
    const RETRY_INTERVAL: Duration = Duration::from_millis(500);

    fs::create_dir_all(index_dir)?;
    let lock_path = get_lock_path(index_dir);
    let lock_file = File::create(&lock_path)
        .with_context(|| format!("Failed to create lock file at {}", lock_path.display()))?;

    let start = Instant::now();
    loop {
        match lock_file.try_lock_exclusive() {
            Ok(()) => return Ok(lock_file),
            Err(_) if start.elapsed() < TIMEOUT => {
                std::thread::sleep(RETRY_INTERVAL);
            }
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Timed out waiting for index lock after 5 seconds. \
                     Another colgrep instance may be updating this index."
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_index_dir_name() {
        let path = PathBuf::from("/Users/foo/myproject");
        let name = compute_index_dir_name(&path, None);
        // Should be format: myproject-{8 hex chars}
        assert!(name.starts_with("myproject-"));
        assert_eq!(name.len(), "myproject-".len() + 8);
    }

    #[test]
    fn test_compute_index_dir_name_with_special_chars() {
        let path = PathBuf::from("/Users/foo/my project (1)");
        let name = compute_index_dir_name(&path, None);
        // Special chars should be replaced with underscores
        assert!(name.starts_with("my_project__1_-"));
    }

    #[test]
    fn test_different_paths_different_hashes() {
        let path1 = PathBuf::from("/Users/foo/project1");
        let path2 = PathBuf::from("/Users/foo/project2");
        let name1 = compute_index_dir_name(&path1, None);
        let name2 = compute_index_dir_name(&path2, None);
        assert_ne!(name1, name2);
    }

    #[test]
    fn test_same_path_different_models_different_hashes() {
        let path = PathBuf::from("/Users/foo/myproject");
        let name_default = compute_index_dir_name(&path, None);
        let name_code = compute_index_dir_name(&path, Some("lightonai/LateOn-Code-edge"));
        let name_reason = compute_index_dir_name(&path, Some("lightonai/Reason-ModernColBERT"));
        // All three should be different
        assert_ne!(name_default, name_code);
        assert_ne!(name_default, name_reason);
        assert_ne!(name_code, name_reason);
        // But all should start with myproject-
        assert!(name_default.starts_with("myproject-"));
        assert!(name_code.starts_with("myproject-"));
        assert!(name_reason.starts_with("myproject-"));
    }

    #[test]
    fn test_same_path_same_model_same_hash() {
        let path = PathBuf::from("/Users/foo/myproject");
        let name1 = compute_index_dir_name(&path, Some("lightonai/Reason-ModernColBERT"));
        let name2 = compute_index_dir_name(&path, Some("lightonai/Reason-ModernColBERT"));
        assert_eq!(name1, name2);
    }
}
