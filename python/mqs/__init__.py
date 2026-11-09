"""QSandbox Python companion — the SDK shim only.

The sandbox server, CLI and control API are the Rust ``mqs`` binary; this
package exists so that QQ SDKs which hardcode their endpoints (botpy) can be
pointed at a running sandbox without editing the SDK.
"""

__version__ = "0.3.0"
