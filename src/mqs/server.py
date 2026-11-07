"""Boot one MQS process: shared world + two uvicorn listeners + runtime handshake."""

from __future__ import annotations

import asyncio
import contextlib
import json
import signal
import socket
import sys

import uvicorn

from .api.sim import create_sim_app
from .clock import Clock
from .config import (
    ProcessConfig,
    ProfileLock,
    clear_runtime,
    write_runtime,
)
from .control.app import create_control_app
from .db import Database
from .domain.world import World
from .gateway.manager import GatewayManager


def detect_lan_ip() -> str:
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            s.connect(("192.168.1.1", 80))
            return s.getsockname()[0]
        finally:
            s.close()
    except OSError:
        return "127.0.0.1"


async def _start_server(server: uvicorn.Server, task: asyncio.Task) -> int:
    for _ in range(1000):
        if server.started:
            break
        if task.done():
            await task
            raise RuntimeError("server failed to start")
        await asyncio.sleep(0.01)
    if not server.started:
        raise RuntimeError("server did not start in time")
    sockets = server.servers[0].sockets if server.servers else []
    if not sockets:
        raise RuntimeError("server has no listening socket")
    return int(sockets[0].getsockname()[1])


async def serve(config: ProcessConfig) -> None:
    paths = config.paths.ensure()
    lock = ProfileLock(paths)
    lock.acquire()
    db = Database(paths.db)
    await db.connect()
    clock = Clock("real")
    world = World(db, clock, config.profile, paths)
    await world.start()
    gateway = GatewayManager(world)
    world.gateway = gateway
    await gateway.start()

    sim_app = create_sim_app(world, config)
    sim_app.state.gateway = gateway
    control_app = create_control_app(world, config)
    control_app.state.sim_app = sim_app
    control_app.state.gateway = gateway

    if config.public_host:
        pass
    elif config.host in ("0.0.0.0", "::"):
        config.public_host = detect_lan_ip()

    sim_cfg = uvicorn.Config(
        sim_app,
        host=config.host,
        port=config.sim_port,
        log_level=config.log_level,
        access_log=False,
        lifespan="on",
    )
    control_cfg = uvicorn.Config(
        control_app,
        host=config.host,
        port=config.control_port,
        log_level=config.log_level,
        access_log=False,
        lifespan="on",
    )
    sim_server = uvicorn.Server(sim_cfg)
    control_server = uvicorn.Server(control_cfg)
    # we own signal handling for both listeners
    sim_server.capture_signals = lambda: contextlib.nullcontext()  # type: ignore[method-assign]
    control_server.capture_signals = lambda: contextlib.nullcontext()  # type: ignore[method-assign]

    stop = asyncio.Event()

    def _stop(*_: object) -> None:
        stop.set()

    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        with contextlib.suppress(NotImplementedError, RuntimeError):
            loop.add_signal_handler(sig, _stop)

    sim_task = asyncio.create_task(sim_server.serve(), name="mqs-sim")
    try:
        config.sim_port = await _start_server(sim_server, sim_task)
    except Exception:
        lock.release()
        await db.close()
        raise

    control_task = asyncio.create_task(control_server.serve(), name="mqs-control")
    try:
        config.control_port = await _start_server(control_server, control_task)
    except Exception:
        sim_server.should_exit = True
        await asyncio.gather(sim_task, return_exceptions=True)
        lock.release()
        await db.close()
        raise

    host_for_url = config.public_host or ("127.0.0.1" if config.host in ("0.0.0.0", "::") else config.host)
    config.gui_url = f"http://{host_for_url}:{config.control_port}/#t={control_app.state.login_nonce}"
    handshake = config.handshake()
    handshake["gui_url"] = config.gui_url
    handshake["started_at"] = world.clock.iso()
    write_runtime(config.home, config.profile, handshake)

    print(
        f"\n  MQS  ·  profile={config.profile}  ·  fake data only, not affiliated with Tencent\n"
        f"  Simulate API : {config.base_url}\n"
        f"  Gateway      : {config.ws_url}\n"
        f"  Control GUI  : {config.gui_url}\n"
        f"  Control token: {config.control_token}\n",
        file=sys.stderr,
        flush=True,
    )
    print(json.dumps(handshake, ensure_ascii=False), flush=True)

    try:
        await stop.wait()
    finally:
        sim_server.should_exit = True
        control_server.should_exit = True
        await asyncio.gather(sim_task, control_task, return_exceptions=True)
        await gateway.stop()
        await world.stop()
        clear_runtime(config.home, config.profile)
        await db.close()
        lock.release()


def run(config: ProcessConfig) -> None:
    try:
        asyncio.run(serve(config))
    except KeyboardInterrupt:  # pragma: no cover
        pass


async def run_headless(config: ProcessConfig) -> dict:
    """Start in-process and return the handshake (used by tests/embedding)."""
    paths = config.paths.ensure()
    db = Database(paths.db)
    await db.connect()
    clock = Clock("real")
    world = World(db, clock, config.profile, paths)
    await world.start()
    gateway = GatewayManager(world)
    world.gateway = gateway
    await gateway.start()
    sim_app = create_sim_app(world, config)
    sim_app.state.gateway = gateway
    control_app = create_control_app(world, config)
    control_app.state.sim_app = sim_app
    control_app.state.gateway = gateway
    sim_server = uvicorn.Server(
        uvicorn.Config(sim_app, host="127.0.0.1", port=0, log_level="warning", access_log=False)
    )
    control_server = uvicorn.Server(
        uvicorn.Config(control_app, host="127.0.0.1", port=0, log_level="warning", access_log=False)
    )
    sim_server.capture_signals = lambda: contextlib.nullcontext()  # type: ignore[method-assign]
    control_server.capture_signals = lambda: contextlib.nullcontext()  # type: ignore[method-assign]
    sim_task = asyncio.create_task(sim_server.serve())
    control_task = asyncio.create_task(control_server.serve())
    config.sim_port = await _start_server(sim_server, sim_task)
    config.control_port = await _start_server(control_server, control_task)
    config.gui_url = f"http://127.0.0.1:{config.control_port}/#t={control_app.state.login_nonce}"
    return {
        "config": config,
        "world": world,
        "gateway": gateway,
        "db": db,
        "sim_app": sim_app,
        "control_app": control_app,
        "sim_server": sim_server,
        "control_server": control_server,
        "sim_task": sim_task,
        "control_task": control_task,
        "handshake": config.handshake(),
    }
