"""Scenario engine: builtin templates, JUnit output, assertion failures."""

import yaml

from mqs.scenario import BUILTINS, builtin_scenarios
from mqs.scenario.engine import run_scenario_payload


async def test_builtin_catalog() -> None:
    names = {item["name"] for item in builtin_scenarios()}
    assert {"smoke", "c2c", "rate_limit", "passive_window", "duplicate_seq"} <= names
    for name, script in BUILTINS.items():
        doc = yaml.safe_load(script)
        assert doc["name"] == name
        assert doc["steps"]


async def test_smoke_scenario_with_echo_bot(mqs, echo_bot) -> None:
    result = await run_scenario_payload(mqs.world, BUILTINS["smoke"], None, sim_app=mqs.ctx["sim_app"])
    assert result.ok, [step.as_dict() for step in result.steps]
    assert len(result.steps) == 3
    assert "<testsuite" in result.junit_xml()
    assert "TAP version 13" in result.tap()


async def test_failure_reports_step_and_junit(mqs) -> None:
    script = """\
name: fail_demo
steps:
  - expect_event:
      type: NEVER_HAPPENS
      timeout: 300ms
"""
    result = await run_scenario_payload(mqs.world, script, None, sim_app=mqs.ctx["sim_app"])
    assert not result.ok
    assert result.steps[-1].ok is False
    assert "timeout" in result.steps[-1].detail
    assert 'failures="1"' in result.junit_xml()


async def test_http_assertion_step(mqs) -> None:
    script = """\
name: http_demo
setup:
  settings:
    rate_mode: "off"
    require_online: false
steps:
  - http:
      bot: "10000"
      method: POST
      target: group
      group: "{{__none__}}"
      path: /v2/groups/{openid}/messages
      json: {content: hi}
      expect_status: 200
"""
    # group alias is unknown on purpose: setup error should be reported, not crash
    result = await run_scenario_payload(mqs.world, script, None, sim_app=mqs.ctx["sim_app"])
    assert not result.ok
    assert result.steps[0].detail


async def test_passive_window_template_without_bot(mqs) -> None:
    result = await run_scenario_payload(
        mqs.world, BUILTINS["passive_window"], None, sim_app=mqs.ctx["sim_app"]
    )
    assert result.ok, [step.as_dict() for step in result.steps]
