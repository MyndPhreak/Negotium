use negotium_collector::{IdentityCache, ProcessReader};
use std::collections::HashSet;

fn own_create_time() -> i64 {
    let me = std::process::id();
    ProcessReader::new().read().unwrap().into_iter().find(|p| p.pid == me).unwrap().create_time
}

#[test]
fn resolves_own_image_path_and_no_package() {
    let mut cache = IdentityCache::new();
    let id = cache.get(std::process::id(), own_create_time()).clone();
    let expected = std::env::current_exe().unwrap().to_string_lossy().to_lowercase();
    assert_eq!(id.image_path.map(|p| p.to_lowercase()), Some(expected));
    assert_eq!(id.package_family, None);
}

#[test]
fn caches_until_process_is_gone() {
    let mut cache = IdentityCache::new();
    let key = (std::process::id(), own_create_time());
    cache.get(key.0, key.1);
    cache.get(key.0, key.1);
    assert_eq!(cache.opens(), 1);
    cache.retain_alive(&HashSet::new());
    cache.get(key.0, key.1);
    assert_eq!(cache.opens(), 2);
}

#[test]
fn inaccessible_pids_yield_empty_identity() {
    let mut cache = IdentityCache::new();
    assert_eq!(cache.get(0, 0).image_path, None);
    assert_eq!(cache.get(4, 0).image_path, None);
    assert_eq!(cache.get(u32::MAX - 3, 1).image_path, None);
}
