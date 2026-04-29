use spall_config::registry::{ApiEntry, ApiRegistry, ProfileConfig};

#[test]
fn registry_find_existing() {
    let entry = ApiEntry {
        name: "petstore".into(),
        source: "/tmp/petstore.json".into(),
        config_path: None,
        base_url: Some("https://example.com".into()),
        default_headers: vec![("X-Custom".into(), "val".into())],
        auth: None,
        proxy: None,
        profiles: std::collections::HashMap::new(),
    };
    let registry = ApiRegistry::from_entries(vec![entry], Default::default());
    assert!(registry.find("petstore").is_some());
}

#[test]
fn registry_find_missing() {
    let registry = ApiRegistry::from_entries(vec![], Default::default());
    assert!(registry.find("missing").is_none());
}

#[test]
fn resolve_profile_applies_overlay() {
    let profile = ProfileConfig {
        base_url: Some("https://staging.example.com".into()),
        headers: vec![("X-Env".into(), "staging".into())],
        auth: None,
        proxy: None,
    };
    let mut profiles = std::collections::HashMap::new();
    profiles.insert("staging".into(), profile);
    let entry = ApiEntry {
        name: "api".into(),
        source: "/tmp/api.json".into(),
        config_path: None,
        base_url: Some("https://prod.example.com".into()),
        default_headers: vec![],
        auth: None,
        proxy: None,
        profiles,
    };
    let registry = ApiRegistry::from_entries(vec![entry], Default::default());
    let resolved = registry.resolve_profile("api", Some("staging")).unwrap();
    assert_eq!(resolved.base_url.as_deref(), Some("https://staging.example.com"));
}

#[test]
fn resolve_profile_no_profile() {
    let entry = ApiEntry {
        name: "api".into(),
        source: "/tmp/api.json".into(),
        config_path: None,
        base_url: Some("https://prod.example.com".into()),
        default_headers: vec![],
        auth: None,
        proxy: None,
        profiles: std::collections::HashMap::new(),
    };
    let registry = ApiRegistry::from_entries(vec![entry], Default::default());
    let resolved = registry.resolve_profile("api", None).unwrap();
    assert_eq!(resolved.base_url.as_deref(), Some("https://prod.example.com"));
}
