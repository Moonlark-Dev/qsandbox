//! Contract test: registry ↔ route table coherence.

use std::collections::HashSet;

use mqs::contract::{coverage_markdown, routes_table, sim_endpoints, summary, typescript};
use mqs::sim::ROUTE_TABLE;

const ALLOWED_EXTRA: &[(&str, &str)] = &[
    ("GET", "/protocol"),
    ("GET", "/openapi.json"),
    ("GET", "/avatars/{key}"),
    ("GET", "/websocket"),
    ("GET", "/websocket/"),
];

#[test]
fn every_registry_endpoint_is_implemented() {
    let routes: HashSet<(&str, &str)> = ROUTE_TABLE.iter().copied().collect();
    let missing: Vec<_> = sim_endpoints()
        .iter()
        .filter(|endpoint| !routes.contains(&(endpoint.method, endpoint.path)))
        .map(|endpoint| (endpoint.method, endpoint.path))
        .collect();
    assert!(
        missing.is_empty(),
        "registry entries without a route: {missing:?}"
    );
}

#[test]
fn every_sim_route_is_registered() {
    let registered: HashSet<(&str, &str)> = sim_endpoints()
        .iter()
        .map(|endpoint| (endpoint.method, endpoint.path))
        .collect();
    let allowed: HashSet<(&str, &str)> = ALLOWED_EXTRA.iter().copied().collect();
    let unregistered: Vec<_> = ROUTE_TABLE
        .iter()
        .filter(|route| !registered.contains(*route) && !allowed.contains(*route))
        .collect();
    assert!(
        unregistered.is_empty(),
        "routes missing from the contract registry: {unregistered:?}"
    );
}

#[test]
fn coverage_summary_is_consistent() {
    let data = summary();
    let endpoints = sim_endpoints();
    assert_eq!(data["total"].as_u64().unwrap() as usize, endpoints.len());
    let by_level: i64 = data["by_level"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|value| value.as_i64())
        .sum();
    assert_eq!(by_level as usize, endpoints.len());
    assert!(data["ascii"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{} endpoints", endpoints.len())));
}

#[test]
fn coverage_document_mentions_every_path() {
    let markdown = coverage_markdown();
    for endpoint in sim_endpoints() {
        assert!(
            markdown.contains(endpoint.path),
            "missing {}",
            endpoint.path
        );
    }
    assert!(markdown.contains("接口覆盖率"));
}

#[test]
fn no_guild_family_remains() {
    for endpoint in sim_endpoints() {
        assert!(!endpoint.path.starts_with("/guilds"), "{}", endpoint.path);
        assert!(!endpoint.path.starts_with("/channels"), "{}", endpoint.path);
        assert!(!endpoint.path.starts_with("/dms"), "{}", endpoint.path);
        assert!(
            !endpoint.path.contains("/users/@me/guilds"),
            "{}",
            endpoint.path
        );
    }
}

#[test]
fn typescript_generation() {
    let ts = typescript();
    assert!(ts.contains("export interface MessageSendRequest"));
    assert!(ts.contains("export interface GatewayFrame"));
    assert!(ts.contains("export interface ChatSend"));
    assert!(ts.contains("AUTO-GENERATED"));
    assert!(!ts.contains("$defs"));
}

#[test]
fn routes_table_matches_registry() {
    let table = routes_table();
    assert_eq!(table.len(), sim_endpoints().len());
    assert!(table.iter().any(|row| row["key"] == "group.send_message"));
    assert!(table.iter().any(|row| row["key"] == "app.access_token"));
}
