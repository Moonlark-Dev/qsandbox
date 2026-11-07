"""Contract test: registry ↔ routes, generated artifacts are coherent."""

from fastapi.routing import APIRoute

from mqs.api.sim import create_sim_app
from mqs.config import ProcessConfig
from mqs.contract import REGISTRY, coverage_markdown, routes_table, sim_endpoints, summary, typescript

ALLOWED_EXTRA = {
    ("GET", "/docs"),
    ("GET", "/docs/oauth2-redirect"),
    ("GET", "/openapi.json"),
    ("GET", "/redoc"),
    ("GET", "/protocol"),
    ("GET", "/avatars/{key}.svg"),
}


def _walk(routes) -> list[APIRoute]:
    found: list[APIRoute] = []
    for route in routes:
        if isinstance(route, APIRoute):
            found.append(route)
        else:
            nested = getattr(route, "original_router", None)
            if nested is not None:
                found.extend(_walk(nested.routes))
    return found


def _routes() -> set[tuple[str, str]]:
    app = create_sim_app(None, ProcessConfig(profile="contract"))  # type: ignore[arg-type]
    routes: set[tuple[str, str]] = set()
    for route in _walk(app.routes):
        for method in route.methods:
            routes.add((method, route.path))
    return routes


def test_every_registry_endpoint_is_implemented() -> None:
    routes = _routes()
    missing = {(e.method, e.path) for e in sim_endpoints()} - routes
    assert not missing, f"registry entries without a route: {sorted(missing)}"


def test_every_sim_route_is_registered() -> None:
    routes = _routes()
    registered = {(e.method, e.path) for e in sim_endpoints()}
    unregistered = routes - registered - ALLOWED_EXTRA
    assert not unregistered, f"routes missing from the contract registry: {sorted(unregistered)}"


def test_coverage_summary_is_consistent() -> None:
    data = summary()
    endpoints = sim_endpoints()
    assert data["total"] == len(endpoints)
    assert sum(data["by_level"].values()) == len(endpoints)
    assert data["ascii"].startswith(f"{len(endpoints)} endpoints")


def test_coverage_document_mentions_every_path() -> None:
    markdown = coverage_markdown()
    for endpoint in sim_endpoints():
        assert endpoint.path in markdown
    assert "接口覆盖率" in markdown


def test_typescript_generation() -> None:
    ts = typescript()
    assert "export interface MessageSendRequest" in ts
    assert "export interface GatewayFrame" in ts
    assert "export interface ChatSend" in ts
    assert "AUTO-GENERATED" in ts
    # no unresolved pydantic refs
    assert "$defs" not in ts
    assert "#/" not in ts


def test_routes_table_matches_registry() -> None:
    table = routes_table()
    assert len(table) == len(REGISTRY)
    keys = {row["key"] for row in table}
    assert "group.send_message" in keys
    assert "app.access_token" in keys
