use savepoint_cas::{CasConfig, CasEngine, Chunker};
use tempfile::tempdir;

#[test]
fn test_cas_empty_blob() {
    let dir = tempdir().unwrap();
    let cas = CasEngine::open(dir.path(), CasConfig::default()).unwrap();

    let manifest = cas.store_blob(&[]).unwrap();
    assert_eq!(manifest.total_size, 0);
    assert!(manifest.chunks.is_empty());

    let restored = cas.read_blob(&manifest).unwrap();
    assert!(restored.is_empty());
}

#[test]
fn test_cas_tiny_blob_and_blake3_match() {
    let dir = tempdir().unwrap();
    let cas = CasEngine::open(dir.path(), CasConfig::default()).unwrap();

    let data = b"Hello, Savepoint CAS engine!";
    let manifest = cas.store_blob(data).unwrap();
    assert_eq!(manifest.total_size, data.len());
    assert_eq!(manifest.chunks.len(), 1);

    let expected_hash = blake3::hash(data).to_hex().to_string();
    assert_eq!(manifest.chunks[0], expected_hash);

    let restored = cas.read_blob(&manifest).unwrap();
    assert_eq!(restored, data);
}

#[test]
fn test_cas_exact_min_size_boundary() {
    let dir = tempdir().unwrap();
    let cas = CasEngine::open(dir.path(), CasConfig::default()).unwrap();

    // 16KB exact boundary
    let data = vec![0x77u8; 16 * 1024];
    let manifest = cas.store_blob(&data).unwrap();
    assert_eq!(manifest.chunks.len(), 1);

    let restored = cas.read_blob(&manifest).unwrap();
    assert_eq!(restored, data);
}

#[test]
fn test_cas_corrupt_chunk_detection() {
    let dir = tempdir().unwrap();
    let cas_root = dir.path().join(".sp_cas");
    let cas = CasEngine::open(&cas_root, CasConfig::default()).unwrap();

    let data = vec![0x33u8; 32 * 1024];
    let manifest = cas.store_blob(&data).unwrap();
    let chunk_hash = &manifest.chunks[0];

    // Tamper with the compressed chunk file by writing invalid decompressed content
    let prefix = &chunk_hash[..2];
    let suffix = &chunk_hash[2..];
    let chunk_path = cas_root
        .join("objects")
        .join(prefix)
        .join(format!("{}.zst", suffix));
    assert!(chunk_path.exists());

    // Overwrite with compressed bad data
    let bad_data = vec![0x99u8; 32 * 1024];
    let compressed_bad = zstd::encode_all(bad_data.as_slice(), 1).unwrap();
    std::fs::write(&chunk_path, compressed_bad).unwrap();

    // Reading should detect integrity hash mismatch
    let err = cas.read_blob(&manifest).unwrap_err();
    match err {
        savepoint_cas::CasError::CorruptChunk { hash, .. } => {
            assert_eq!(&hash, chunk_hash);
        }
        other => panic!("Expected CorruptChunk, got {:?}", other),
    }
}

#[test]
fn test_cas_missing_chunk_error() {
    let dir = tempdir().unwrap();
    let cas_root = dir.path().join(".sp_cas");
    let cas = CasEngine::open(&cas_root, CasConfig::default()).unwrap();

    let data = vec![0x55u8; 32 * 1024];
    let manifest = cas.store_blob(&data).unwrap();
    let chunk_hash = &manifest.chunks[0];

    let prefix = &chunk_hash[..2];
    let suffix = &chunk_hash[2..];
    let chunk_path = cas_root
        .join("objects")
        .join(prefix)
        .join(format!("{}.zst", suffix));
    std::fs::remove_file(&chunk_path).unwrap();

    let err = cas.read_blob(&manifest).unwrap_err();
    match err {
        savepoint_cas::CasError::ChunkNotFound(hash) => {
            assert_eq!(&hash, chunk_hash);
        }
        other => panic!("Expected ChunkNotFound, got {:?}", other),
    }
}

#[test]
fn test_cas_custom_chunker_sizes() {
    let chunker = Chunker::new(1024, 2048, 4096);
    let data = vec![0xAAu8; 8 * 1024];
    let chunks = chunker.chunk(&data);
    assert!(chunks.len() >= 2);

    let mut total_len = 0;
    for (slice, meta) in &chunks {
        assert_eq!(slice.len(), meta.length);
        total_len += meta.length;
    }
    assert_eq!(total_len, data.len());
}

#[test]
fn test_cas_gc_retains_shared_chunks_across_multiple_active_manifests() {
    let dir = tempdir().unwrap();
    let cas = CasEngine::open(dir.path(), CasConfig::default()).unwrap();

    let data1 = vec![0xAAu8; 128 * 1024];
    let mut data2 = data1.clone();
    data2.extend_from_slice(&[0xBBu8; 64 * 1024]);
    let data3 = vec![0xCCu8; 64 * 1024]; // Orphaned

    let m1 = cas.store_blob(&data1).unwrap();
    let m2 = cas.store_blob(&data2).unwrap();
    let _m3 = cas.store_blob(&data3).unwrap();

    // Run GC retaining m1 and m2
    let stats = cas.run_gc(&[&m1, &m2]).unwrap();
    assert!(stats.pruned_chunks > 0);
    assert!(stats.freed_bytes > 0);

    // Both m1 and m2 must still be fully readable
    let r1 = cas.read_blob(&m1).unwrap();
    assert_eq!(r1, data1);
    let r2 = cas.read_blob(&m2).unwrap();
    assert_eq!(r2, data2);
}
