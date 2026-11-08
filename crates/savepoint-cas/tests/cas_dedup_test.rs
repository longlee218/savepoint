use savepoint_cas::{CasConfig, CasEngine};
use tempfile::tempdir;

#[test]
fn test_cas_deduplicates_identical_data_across_versions() {
    let dir = tempdir().unwrap();
    let cas_root = dir.path().join(".sp_cas");
    let cas = CasEngine::open(&cas_root, CasConfig::default()).unwrap();

    // 1. Create a 512KB payload
    let mut large_buffer = vec![0x42u8; 512 * 1024];

    // Store version 1
    let manifest_v1 = cas.store_blob(&large_buffer).unwrap();
    let initial_storage_bytes = cas.total_disk_usage().unwrap();
    assert!(initial_storage_bytes > 0);

    // 2. Slightly mutate the end of buffer (only last 4KB modified)
    for b in &mut large_buffer[508 * 1024..] {
        *b = 0x99;
    }

    // Store version 2
    let manifest_v2 = cas.store_blob(&large_buffer).unwrap();
    let post_mutate_storage_bytes = cas.total_disk_usage().unwrap();

    // 3. Verify deduplication: incremental disk increase must be far under 128KB
    let added_bytes = post_mutate_storage_bytes - initial_storage_bytes;
    assert!(
        added_bytes < 128 * 1024,
        "FastCDC must share unmutated chunks across versions: added {} bytes",
        added_bytes
    );

    // 4. Retrieve version 1 and version 2 blobs and verify bit-for-bit integrity
    let restored_v1 = cas.read_blob(&manifest_v1).unwrap();
    assert_eq!(&restored_v1[0..100], &[0x42u8; 100]);
    assert_eq!(&restored_v1[508 * 1024..508 * 1024 + 10], &[0x42u8; 10]);

    let restored_v2 = cas.read_blob(&manifest_v2).unwrap();
    assert_eq!(&restored_v2[0..100], &[0x42u8; 100]);
    assert_eq!(&restored_v2[508 * 1024..508 * 1024 + 10], &[0x99u8; 10]);
}

#[test]
fn test_cas_garbage_collection_prunes_unreferenced_chunks() {
    let dir = tempdir().unwrap();
    let cas_root = dir.path().join(".sp_cas_gc");
    let cas = CasEngine::open(&cas_root, CasConfig::default()).unwrap();

    let blob_a = vec![0x11u8; 256 * 1024];
    let blob_b = vec![0x22u8; 256 * 1024];

    let manifest_a = cas.store_blob(&blob_a).unwrap();
    let _manifest_b = cas.store_blob(&blob_b).unwrap();

    let storage_before_gc = cas.total_disk_usage().unwrap();

    // Only manifest_a is active / pinned. manifest_b is abandoned.
    let gc_stats = cas.run_gc(&[&manifest_a]).unwrap();
    assert!(gc_stats.pruned_chunks > 0, "Must prune unreferenced chunks of blob_b");

    let storage_after_gc = cas.total_disk_usage().unwrap();
    assert!(
        storage_after_gc < storage_before_gc,
        "Storage after GC must shrink: was {}, now {}",
        storage_before_gc,
        storage_after_gc
    );

    // Blob A must still be 100% intact
    let restored_a = cas.read_blob(&manifest_a).unwrap();
    assert_eq!(restored_a, blob_a);
}
