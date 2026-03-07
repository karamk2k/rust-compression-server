
use notify::{Watcher, RecursiveMode, watcher, DebouncedEvent};
use std::collections::HashMap;
use std::sync::mpsc::channel;
use std::time::Duration;
use std::path::{Path, PathBuf};
use std::fs;
use crate::file_compressor::FileCompressor;
use tracing::{error, info};

/// Watches folders and compresses new files with FileCompressor
pub struct FolderWatcher {
    pub folders: HashMap<String, PathBuf>, // category name -> folder path
    pub compressor: FileCompressor,
}

impl FolderWatcher {
    /// Create a new watcher
    pub fn new(folders: HashMap<String, String>, compressor: FileCompressor) -> Self {
        let folder_paths = folders
            .into_iter()
            .map(|(key, path)| (key, PathBuf::from(path)))
            .collect();

        Self {
            folders: folder_paths,
            compressor,
        }
    }

    /// Compress and replace file with naming convention: category_filename.zst
    fn compress_and_replace(&self, category: &str, file_path: &Path) {
        let filename = file_path.file_name().unwrap().to_str().unwrap();
        let folder_path = &self.folders[category];
        
        // Get the relative path from the base folder
        let relative_path = file_path.strip_prefix(folder_path).unwrap_or(Path::new(""));
        let parent_path = relative_path.parent().unwrap_or(Path::new(""));
        
        // Create the output path maintaining the subfolder structure
        let output_path = folder_path.join(parent_path).join(format!("{}_{}.zst", category, filename));
        
        // Ensure the subfolder exists
        if let Some(parent) = output_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                error!(path = %parent.display(), ?e, "failed to create directory");
                return;
            }
        }

        if filename.ends_with(".zst") 
        {
            return;
        }

        info!(
            file = %filename,
            folder = %folder_path.display(),
            output = %output_path.display(),
            "compressing file"
        );

        if let Err(e) = self.compressor.compress_file(file_path.to_str().unwrap(), output_path.to_str().unwrap()) {
            error!(file = %filename, ?e, "failed to compress file");
            return;
        }

        if let Err(e) = fs::remove_file(file_path) {
            error!(file = %filename, ?e, "failed to remove original file");
        } else {
            info!(file = %filename, output = %output_path.display(), "compressed and replaced file");
        }
    }

    /// Start watching all folders
    pub fn watch(&self) -> notify::Result<()> {
        let (tx, rx) = channel();
        let mut watcher = watcher(tx, Duration::from_secs(2))?;

        // watch all folders recursively to detect new subfolders
        for (_category, folder_path) in &self.folders {
            fs::create_dir_all(folder_path)?; // ensure folder exists
            info!(folder = %folder_path.display(), "watching folder recursively");
            watcher.watch(folder_path, RecursiveMode::Recursive)?;
        }

        loop {
            match rx.recv() {
                Ok(event) => match event {
                    DebouncedEvent::Create(path) => {
                        if path.is_file() {
                            for (category, folder_path) in &self.folders {
                                if path.starts_with(folder_path) {
                                    self.compress_and_replace(category, &path);
                                }
                            }
                        }
                    }
                    _ => {}
                },
                Err(e) => error!(?e, "watch error"),
            }
        }
    }
}
