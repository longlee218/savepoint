use fastcdc::v2020::FastCDC;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkMeta {
    pub hash: String,
    pub offset: usize,
    pub length: usize,
}

pub struct Chunker {
    min_size: usize,
    avg_size: usize,
    max_size: usize,
}

impl Default for Chunker {
    fn default() -> Self {
        Self {
            min_size: 16 * 1024,
            avg_size: 64 * 1024,
            max_size: 128 * 1024,
        }
    }
}

impl Chunker {
    pub fn new(min_size: usize, avg_size: usize, max_size: usize) -> Self {
        Self {
            min_size,
            avg_size,
            max_size,
        }
    }

    pub fn chunk<'a>(&self, data: &'a [u8]) -> Vec<(&'a [u8], ChunkMeta)> {
        if data.is_empty() {
            return Vec::new();
        }

        // If data is smaller than min_size, treat entire data as 1 chunk
        if data.len() <= self.min_size {
            let hash = blake3::hash(data).to_hex().to_string();
            let meta = ChunkMeta {
                hash,
                offset: 0,
                length: data.len(),
            };
            return vec![(data, meta)];
        }

        let chunker = FastCDC::new(
            data,
            self.min_size,
            self.avg_size,
            self.max_size,
        );

        let mut chunks = Vec::new();
        for chunk in chunker {
            let slice = &data[chunk.offset..chunk.offset + chunk.length];
            let hash = blake3::hash(slice).to_hex().to_string();
            chunks.push((
                slice,
                ChunkMeta {
                    hash,
                    offset: chunk.offset,
                    length: chunk.length,
                },
            ));
        }

        chunks
    }
}
