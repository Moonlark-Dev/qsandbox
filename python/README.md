# mqs-compat

Python-only companion of [QSandbox](https://github.com/Moonlark-Dev/qsandbox).

The sandbox itself is a Rust binary; only the *SDK shim* stays in Python, because
`botpy` hardcodes its endpoints and monkeypatching it is a Python-only job.

```python
import mqs.compat.botpy  # noqa: F401 — before creating botpy.Client
import botpy

client = botpy.Client(intents=botpy.Intents(public_messages=True))
client.run(appid="10000", secret="mqs-secret-10000")
```

`MQS_BASE_URL` (default `http://127.0.0.1:5700`) or `patch("http://host:port")`
selects the sandbox. `patch_from_runtime()` reads `profiles/<name>/runtime.json`
written by `mqs serve`.
