
mod file_compressor;
mod folder_watcher;
mod server;
mod routes;

use std::collections::HashMap;
use file_compressor::FileCompressor;
use folder_watcher::FolderWatcher;
use server::Server;



#[tokio::main]

async fn main() -> notify::Result<()> {
    let compressor = FileCompressor::default();

    let mut folders = HashMap::new();
    folders.insert("file1".to_string(), "/home/karamk2k/Desktop/rust/compression/storage/file_1");
    // folders.insert("file2".to_string(), "storage/file2");
    // folders.insert("file3".to_string(), "storage/file3");

    let watcher = FolderWatcher::new(folders, compressor.clone());
    std::thread::spawn(move || {
        watcher.watch().unwrap();
    });

    // Start HTTP server
    let server = Server::new(compressor);
    server.run().await;
    Ok(())
}
