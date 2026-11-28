import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import Mock, patch

from history_freshness_probe import CommandAdapter, Phase, Probe, ProbeError


class FakeClock:
    def __init__(self):
        self.value = 100_000_000_000

    def now(self):
        return self.value

    def sleep(self, seconds):
        self.value += round(seconds * 1e9)


class Adapter:
    def __init__(self, clock):
        self.clock = clock
        self.starts = {}
        self.calls = []
        self.errors = {}
        self.search_after = 0.5
        self.summary_after = 1.0
        self.stale_summary = False
        self.other_digest = False
        self.anchor = "http_request_start"

    def call(self, operation, phase, clock_id, timeout):
        self.calls.append((operation, phase.name, timeout))
        if operation in self.errors:
            self.clock.sleep(min(0.2, timeout))
            raise ProbeError(self.errors[operation])
        identity = {"source": phase.source, "session_id": phase.session_id}
        if operation == "upload":
            self.clock.sleep(0.2)  # Preparation is explicitly outside the HTTP anchor.
            start = self.clock.now()
            self.starts[phase.name] = start
            self.clock.sleep(0.1)
            return {**identity, "accepted": True, "clock_id": clock_id,
                    "first_attempt_ns": start, "ack_ns": self.clock.now(),
                    "attempt_anchor": self.anchor, "acceptance_level": "queued"}
        elapsed = (self.clock.now() - self.starts[phase.name]) / 1e9
        threshold = self.search_after if operation == "search" else self.summary_after + (0.5 if operation == "long_summary" else 0)
        ready = elapsed >= threshold
        if operation == "search":
            return {**identity, "state": "ready" if ready else "missing", "marker_sha256": phase.marker_sha256}
        binding = {"pointer_id": "pointer-" + phase.name, "physical_machine_id": "machine",
                   "physical_session_id": "physical-session", "manifest_key": "synthetic/manifest.json",
                   "revision": phase.expected_revision or "revision-2", "content_sha256": "0" * 64}
        summary_binding = dict(binding)
        if self.stale_summary:
            summary_binding["revision"] = "old"
        if self.other_digest and operation == "short_summary":
            summary_binding["content_sha256"] = "1" * 64
        return {**identity, "state": "ready" if ready else "missing", "recorded": True,
                "summary_present": True, "source_kind": "session_summary",
                "reader_path": "session_summary",
                "summary_kind": operation.removesuffix("_summary"), "unique_live_pointer": True,
                "input_manifest_verified": True, "input_marker_sha256": phase.marker_sha256,
                "input_binding": binding, "summary_binding": summary_binding}


def phase(name="initial", **kwargs):
    return Phase(name, "codex", "synthetic-session", hashlib.sha256(name.encode()).hexdigest(),
                 expected_revision=kwargs.pop("expected_revision", "revision-2"),
                 summary_supported=kwargs.pop("summary_supported", True),
                 deadline_seconds=kwargs.pop("deadline_seconds", 2), poll_seconds=0.25, **kwargs)


class ProbeTests(unittest.TestCase):
    def setUp(self):
        self.clock = FakeClock()
        self.adapter = Adapter(self.clock)

    def run_probe(self, phases=None):
        return Probe(self.adapter, self.clock).run(phases or [phase()])

    def test_initial_append_have_separate_http_origins_on_one_clock(self):
        report = self.run_probe([phase(), phase("append", depends_on="initial")])
        a, b = report["phases"]
        self.assertGreater(b["first_attempt_ns"], a["first_attempt_ns"])
        self.assertEqual(a["ack_elapsed_ms"], 100)  # Not 300 ms from command invocation.
        self.assertEqual(a["search_elapsed_ms"], b["search_elapsed_ms"])
        self.assertGreater(a["search_elapsed_ms"], a["search_last_negative_ms"])
        self.assertEqual(report["metrics"]["search"]["denominator"], 2)
        self.assertEqual(report["metrics"]["short_summary"]["ready"], 2)
        self.assertEqual(report["metrics"]["long_summary"]["ready"], 2)
        self.assertLess(a["short_summary_elapsed_ms"], a["long_summary_elapsed_ms"])
        self.assertEqual(report["metrics"]["http_start_qualified"], 2)

    def test_stale_revision_never_qualifies_current_summary(self):
        self.adapter.stale_summary = True
        report = self.run_probe()
        result = report["phases"][0]
        self.assertEqual(result["search"], "ready")
        self.assertEqual(result["short_summary"], "deadline")
        self.assertEqual(result["long_summary"], "deadline")
        self.assertIsNone(result["short_summary_elapsed_ms"])
        self.assertEqual(report["metrics"]["short_summary"]["ready_rate"], 0)

    def test_legacy_revision_only_response_cannot_qualify(self):
        original = self.adapter.call
        def legacy(op, p, cid, timeout):
            body = original(op, p, cid, timeout)
            if op.endswith("_summary"):
                body.pop("input_binding")
            return body
        self.adapter.call = legacy
        report = self.run_probe([phase(expected_revision=None)])
        self.assertEqual(report["phases"][0]["short_summary"], "unsupported")
        self.assertEqual(report["phases"][0]["long_summary"], "unsupported")
        self.assertEqual(report["metrics"]["short_summary"]["denominator"], 1)

    def test_missing_summary_adapter_is_unsupported_not_dropped(self):
        report = self.run_probe([phase(expected_revision=None, summary_supported=False)])
        self.assertEqual(report["phases"][0]["short_summary"], "unsupported")
        self.assertEqual(report["phases"][0]["long_summary"], "unsupported")
        self.assertEqual(report["metrics"]["short_summary"]["denominator"], 1)
        self.assertEqual(report["metrics"]["long_summary"]["ready"], 0)
        self.assertFalse(any(op.endswith("_summary") for op, _, _ in self.adapter.calls))
        for kind, sample in report["phases"][0]["summary_samples"].items():
            self.assertEqual(sample["requested_length"], kind)
            self.assertEqual(sample["reader_path"], "unknown")
            self.assertEqual(sample["state"], "unsupported")

    def test_summary_path_and_length_survive_unsupported_brief(self):
        original = self.adapter.call
        def brief(op, p, cid, timeout):
            result = original(op, p, cid, timeout)
            if op.endswith("_summary"):
                result.update(state="ready", reader_path="session_brief")
            return result
        self.adapter.call = brief
        result = self.run_probe()["phases"][0]
        for kind in ("short", "long"):
            sample = result["summary_samples"][kind]
            self.assertEqual(sample["reader_path"], "session_brief")
            self.assertEqual(sample["requested_length"], kind)
            self.assertEqual(sample["state"], "unsupported")
        observations = [x for x in result["observations"] if x["operation"].endswith("_summary")]
        self.assertEqual({x["requested_length"] for x in observations}, {"short", "long"})
        self.assertTrue(all(x["reader_path"] == "session_brief" for x in observations))

    def test_timed_out_summary_samples_are_unknown_path_with_length(self):
        self.adapter.errors.update(short_summary="command_timeout", long_summary="command_timeout")
        result = self.run_probe()["phases"][0]
        for kind in ("short", "long"):
            sample = result["summary_samples"][kind]
            self.assertEqual(sample["reader_path"], "unknown")
            self.assertEqual(sample["requested_length"], kind)
            self.assertEqual(sample["state"], "deadline")
        observations = [x for x in result["observations"] if x["operation"].endswith("_summary")]
        self.assertTrue(observations)
        self.assertTrue(all(x["reader_path"] == "unknown" and x["requested_length"] in ("short", "long") for x in observations))

    def test_same_revision_different_digest_cannot_pass_and_kinds_are_independent(self):
        self.adapter.other_digest = True
        report = self.run_probe()
        result = report["phases"][0]
        self.assertEqual(result["short_summary"], "deadline")
        self.assertIsNone(result["short_summary_elapsed_ms"])
        self.assertEqual(result["long_summary"], "ready")
        self.assertEqual(report["metrics"]["short_summary"]["ready_rate"], 0)
        self.assertEqual(report["metrics"]["long_summary"]["ready_rate"], 1)

    def test_failure_and_blocked_append_are_both_in_denominator(self):
        self.adapter.errors["upload"] = "command_timeout"
        report = self.run_probe([phase(), phase("append", depends_on="initial")])
        self.assertEqual(report["metrics"]["declared_phases"], 2)
        self.assertEqual(report["metrics"]["upload_accepted"], 0)
        self.assertEqual(report["phases"][1]["upload"], "blocked_dependency")
        self.assertEqual(report["metrics"]["search"]["denominator"], 2)

    def test_errors_do_not_become_negative_readiness_observations(self):
        self.adapter.errors["search"] = "command_timeout"
        result = self.run_probe()["phases"][0]
        self.assertEqual(result["search"], "deadline")
        self.assertIsNone(result["search_last_negative_ms"])
        self.assertGreater(result["errors"]["command_timeout"], 0)
        self.assertEqual(result["short_summary"], "ready")
        self.assertEqual(result["long_summary"], "ready")

    def test_foreign_identity_and_wrong_marker_cannot_pass(self):
        original = self.adapter.call
        for mode in ("identity", "marker"):
            self.setUp()
            original = self.adapter.call
            def corrupted(op, p, cid, timeout):
                result = original(op, p, cid, timeout)
                if op == "search" and result.get("state") == "ready":
                    result["session_id" if mode == "identity" else "marker_sha256"] = "wrong"
                return result
            self.adapter.call = corrupted
            result = self.run_probe()["phases"][0]
            self.assertEqual(result["search"], "deadline")
            self.assertIn("identity_mismatch" if mode == "identity" else "marker_mismatch", result["errors"])

    def test_wrong_clock_or_future_ack_refused(self):
        original = self.adapter.call
        def corrupted(op, p, cid, timeout):
            result = original(op, p, cid, timeout)
            if op == "upload":
                result["clock_id"] = "another-host"
            return result
        self.adapter.call = corrupted
        report = self.run_probe()
        self.assertEqual(report["phases"][0]["upload"], "clock_mismatch")
        self.assertEqual(report["metrics"]["search"]["ready"], 0)

    def test_future_ack_refused_without_guessing_clock_offset(self):
        original = self.adapter.call
        def corrupted(op, p, cid, timeout):
            result = original(op, p, cid, timeout)
            if op == "upload":
                result["ack_ns"] = self.clock.now() + 1
            return result
        self.adapter.call = corrupted
        self.assertEqual(self.run_probe()["phases"][0]["upload"], "invalid_upload_timestamps")

    def test_late_ready_response_does_not_pass_deadline(self):
        original = self.adapter.call
        def late(op, p, cid, timeout):
            result = original(op, p, cid, timeout)
            if op == "search":
                self.clock.sleep(timeout + 1)
                result["state"] = "ready"
            return result
        self.adapter.call = late
        result = self.run_probe()["phases"][0]
        self.assertEqual(result["search"], "deadline")
        self.assertIsNone(result["search_elapsed_ms"])

    def test_command_start_is_never_http_qualified(self):
        self.adapter.anchor = "upload_command_start"
        report = self.run_probe()
        self.assertEqual(report["metrics"]["search"]["ready"], 1)
        self.assertEqual(report["metrics"]["http_start_qualified"], 0)

    def test_observation_bound_terminates_even_without_deadline(self):
        self.adapter.search_after = self.adapter.summary_after = 100
        result = self.run_probe([phase(max_observations=2)])["phases"][0]
        self.assertEqual(len(result["observations"]), 2)
        self.assertEqual(result["search"], "observation_limit")
        self.assertEqual(result["short_summary"], "observation_limit")
        self.assertEqual(result["long_summary"], "observation_limit")

    def test_invalid_durations_refused_before_upload(self):
        for duration in (0, -1, float("nan"), float("inf"), True):
            with self.assertRaises(ValueError):
                self.run_probe([phase(deadline_seconds=duration)])
        self.assertEqual(self.adapter.calls, [])

    def test_command_failures_do_not_log_sensitive_output(self):
        adapter = CommandAdapter({"upload": [sys.executable, "-c", "import sys;print('Bearer synthetic-secret');sys.exit(1)"]})
        with self.assertRaisesRegex(ProbeError, "^command_failed$"):
            adapter.call("upload", phase(), "clock", 1)

    @unittest.skipUnless(os.name == "posix", "owned process-group cleanup is POSIX-specific")
    def test_timeout_stops_descendants_holding_stdout(self):
        with tempfile.TemporaryDirectory() as directory:
            child_pid = Path(directory) / "child.pid"
            script = ("import subprocess,sys,time;from pathlib import Path;"
                      "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(10)']);"
                      "Path(sys.argv[1]).write_text(str(child.pid));time.sleep(10)")
            adapter = CommandAdapter({"upload": [sys.executable, "-c", script, str(child_pid)]})
            started = time.monotonic()
            with self.assertRaisesRegex(ProbeError, "^command_timeout$"):
                adapter.call("upload", phase(), "clock", 1)
            self.assertTrue(child_pid.exists(), "the regression must actually spawn a descendant")
            self.assertLess(time.monotonic() - started, 2)

    def test_cleanup_never_signals_an_already_reaped_pid(self):
        process = Mock(returncode=0, pid=1)
        process.communicate.return_value = ("", None)
        with patch("history_freshness_probe.os.killpg", create=True) as kill_group:
            CommandAdapter._stop(process)
        kill_group.assert_not_called()
        process.kill.assert_not_called()


class OfflineHttpTests(unittest.TestCase):
    @unittest.skipUnless(hasattr(time, "clock_gettime_ns") and hasattr(time, "CLOCK_MONOTONIC"),
                         "shared kernel CLOCK_MONOTONIC unavailable on this platform")
    def test_commands_share_actual_http_attempt_clock_and_cli(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_POST(self):
                self.server.requests += 1
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                p = request["phase"]
                binding = {"pointer_id": "pointer-" + p["name"], "physical_machine_id": "machine",
                           "physical_session_id": "session", "manifest_key": "synthetic/manifest.json",
                           "revision": p["expected_revision"], "content_sha256": "0" * 64}
                result = {"source": p["source"], "session_id": p["session_id"], "state": "ready",
                          "marker_sha256": p["marker_sha256"], "recorded": True,
                          "summary_present": True, "source_kind": "session_summary",
                          "reader_path": "session_summary",
                          "summary_kind": request.get("summary_kind"), "unique_live_pointer": True,
                          "input_manifest_verified": True, "input_marker_sha256": p["marker_sha256"],
                          "input_binding": binding, "summary_binding": dict(binding)}
                body = json.dumps(result).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.requests = 0
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                adapter = root / "adapter.py"
                adapter.write_text("""import json,sys,time,urllib.request
r=json.load(sys.stdin); start=time.clock_gettime_ns(time.CLOCK_MONOTONIC)
q=urllib.request.Request('http://127.0.0.1:'+sys.argv[1]+'/', data=json.dumps(r).encode(),headers={'Content-Type':'application/json'})
with urllib.request.urlopen(q,timeout=r['timeout_seconds']) as response: result=json.load(response)
ack=time.clock_gettime_ns(time.CLOCK_MONOTONIC)
if r['operation']=='upload': result.update(accepted=True,clock_id=r['clock_id'],first_attempt_ns=start,ack_ns=ack,attempt_anchor='http_request_start',acceptance_level='queued')
print(json.dumps(result))
""")
                command = [sys.executable, str(adapter), str(server.server_port)]
                config = {"phases": [vars(phase()), vars(phase("append", depends_on="initial"))],
                          "commands": {op: command for op in ("upload", "search", "summary")}}
                source = root / "config.json"
                source.write_text(json.dumps(config))
                output = root / "result.json"
                argv = [sys.executable, str(Path(__file__).with_name("history_freshness_probe.py")), str(source), "--output", str(output)]
                dry = subprocess.run(argv, capture_output=True, text=True, timeout=5)
                self.assertEqual(dry.returncode, 0, dry.stderr)
                self.assertFalse(output.exists())
                self.assertEqual(server.requests, 0)
                source.write_text(json.dumps({**config, "phases": [config["phases"][0], config["phases"][0]]}))
                invalid = subprocess.run(argv, capture_output=True, text=True, timeout=5)
                self.assertNotEqual(invalid.returncode, 0)
                self.assertEqual(server.requests, 0)
                source.write_text(json.dumps(config))
                run = subprocess.run(argv + ["--execute"], capture_output=True, text=True, timeout=10)
                self.assertEqual(run.returncode, 0, run.stderr + run.stdout + (output.read_text() if output.exists() else ""))
                result = json.loads(output.read_text())
                self.assertEqual(result["metrics"]["http_start_qualified"], 2)
                self.assertEqual(result["metrics"]["search"]["ready"], 2)
                self.assertEqual(result["metrics"]["short_summary"]["ready"], 2)
                self.assertEqual(result["metrics"]["long_summary"]["ready"], 2)
                self.assertEqual(server.requests, 8)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    unittest.main()
