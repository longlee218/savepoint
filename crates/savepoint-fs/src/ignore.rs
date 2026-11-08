use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};
use walkdir::DirEntry;

pub const ALWAYS_IGNORED_NAMES: &[&str] = &[
    ".git",
    ".savepoint",
    ".claude",
    ".superpowers",
    ".impeccable",
    ".playwright-mcp",
    ".sp-tmp",
    ".DS_Store",
];

pub const HEAVY_CACHE_DIRS: &[&str] = &[
    "target",
    ".next",
    "__pycache__",
    ".gradle",
    ".turbo",
    ".cache",
    ".output",
    "dist",
    "build",
];

pub const HEAVY_DEP_DIRS: &[&str] = &["node_modules", ".venv", "venv"];

#[derive(Clone, Debug)]
pub struct SavepointIgnore {
    workspace_root: PathBuf,
    gitignore: Gitignore,
}

impl SavepointIgnore {
    pub fn for_workspace(workspace_root: &Path) -> Self {
        let mut builder = GitignoreBuilder::new(workspace_root);
        let gitignore_path = workspace_root.join(".gitignore");
        if gitignore_path.is_file() {
            let _ = builder.add(gitignore_path);
        }
        let spignore_path = workspace_root.join(".spignore");
        if spignore_path.is_file() {
            let _ = builder.add(spignore_path);
        }
        let gitignore = builder.build().unwrap_or_else(|_| Gitignore::empty());
        Self {
            workspace_root: workspace_root.to_path_buf(),
            gitignore,
        }
    }

    /// Checks if a file/dir path should be ignored during workspace cloning or delta checks.
    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => return false,
        };

        // 1. Always ignore internal agent directories, git, and savepoint metadata
        if ALWAYS_IGNORED_NAMES.contains(&file_name) {
            return true;
        }

        // 2. Heavy cache directories
        if is_dir && (HEAVY_CACHE_DIRS.contains(&file_name) || file_name == "__pycache__") {
            if file_name != "target" {
                return true;
            }
            // For target/: ignore at workspace root or cargo crate root
            if path.parent() == Some(&self.workspace_root)
                || path
                    .parent()
                    .map(|p| p.join("Cargo.toml").exists())
                    .unwrap_or(false)
            {
                return true;
            }
        }

        // 3. Gitignore / Spignore rules
        if let Ok(rel) = path.strip_prefix(&self.workspace_root) {
            if self
                .gitignore
                .matched_path_or_any_parents(rel, is_dir)
                .is_ignore()
            {
                return true;
            }
        } else if self
            .gitignore
            .matched_path_or_any_parents(path, is_dir)
            .is_ignore()
        {
            return true;
        }

        false
    }

    /// Filter for walkdir to skip traversing heavy/ignored subtrees.
    pub fn is_entry_ignored(&self, entry: &DirEntry, root: &Path) -> bool {
        if entry.depth() == 0 {
            return false;
        }
        let is_dir = entry.file_type().is_dir();
        let file_name = entry.file_name().to_string_lossy();

        // 1. Agent internals, git, savepoint
        if ALWAYS_IGNORED_NAMES.contains(&file_name.as_ref()) {
            return true;
        }

        // 2. Heavy dependency directories (node_modules, .venv, venv) at ANY depth
        // are collapsed or ignored from deep recursive walking
        if is_dir && HEAVY_DEP_DIRS.contains(&file_name.as_ref()) {
            return true;
        }

        // 3. Heavy cache directories
        if is_dir
            && HEAVY_CACHE_DIRS.contains(&file_name.as_ref())
            && (entry.depth() == 1 || file_name != "target")
        {
            return true;
        }

        // 4. Gitignore match against root
        if let Ok(rel) = entry.path().strip_prefix(root) {
            if self
                .gitignore
                .matched_path_or_any_parents(rel, is_dir)
                .is_ignore()
            {
                return true;
            }
        }

        false
    }
}
