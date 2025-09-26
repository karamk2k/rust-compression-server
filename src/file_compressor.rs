use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use zstd::stream::{Encoder, Decoder};
use base64;

/// FileCompressor struct
#[derive(Debug, Clone)]
pub struct FileCompressor {
    pub level: i32, // default compression level
}

impl Default for FileCompressor {
    fn default() -> Self {
        Self { level: 22 } // default zstd compression level
    }
}

impl FileCompressor {
    /// Compress any file to a .zst file
    pub fn compress_file(
        &self,
        input_path: &str,
        output_path: &str,
    ) -> std::io::Result<()> {
        let input_file = File::open(input_path)?;
        let compressed_file = File::create(output_path)?;
        let mut reader = BufReader::new(input_file);
        let mut encoder = Encoder::new(BufWriter::new(compressed_file), self.level)?;
        std::io::copy(&mut reader, &mut encoder)?;
        encoder.finish()?; // flush
        Ok(())
    }

    /// Decompress a .zst file back to original
    pub fn decompress_file(
        &self,
        input_path: &str,
        output_path: &str,
    ) -> std::io::Result<()> {
        let compressed_file = File::open(input_path)?;
        let mut decoder = Decoder::new(BufReader::new(compressed_file))?;
        let mut output_file = File::create(output_path)?;
        std::io::copy(&mut decoder, &mut output_file)?;
        Ok(())
    }

    /// Compress file and return a base64 string
    pub fn compress_file_to_base64(&self, input_path: &str) -> std::io::Result<String> {
        let mut input_file = File::open(input_path)?;
        let mut buffer = Vec::new();
        input_file.read_to_end(&mut buffer)?;
        let compressed = zstd::encode_all(&buffer[..], self.level)?;
        Ok(base64::encode(&compressed))
    }

    /// Decompress a .zst file into memory
    pub fn decompress_file_to_bytes(&self, input_path: &str) -> std::io::Result<Vec<u8>> {
        // let base_dir = "/home/karamk2k/Desktop/rust/compression/storage/file_1";
        let compressed_file = File::open(format!("{}",input_path))?;
        print!("Decompressing file: {}\n", input_path);
        let mut decoder = Decoder::new(BufReader::new(compressed_file))?;
        let mut buf = Vec::new();
        decoder.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Decompress a base64 string back to bytes and write to file
    pub fn decompress_base64_to_file(
        &self,
        b64: &str,
        output_path: &str,
    ) -> std::io::Result<()> {
        let compressed = base64::decode(b64)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let decompressed = zstd::decode_all(&compressed[..])?;
        let mut output_file = File::create(output_path)?;
        output_file.write_all(&decompressed)?;
        Ok(())
    }

    /// Decompress a base64 string into memory
    pub fn decompress_base64_to_bytes(&self, b64: &str) -> std::io::Result<Vec<u8>> {
        let compressed = base64::decode(b64)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let decompressed = zstd::decode_all(&compressed[..])?;
        Ok(decompressed)
    }
}
