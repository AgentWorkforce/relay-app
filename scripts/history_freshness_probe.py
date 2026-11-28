#!/usr/bin/env python3
"""Offline test harness for caller-observed history freshness (Python 3.10+).

No native uploader or live Reader adapter is included. Existing Desktop builds
do not expose a qualified first-HTTP timestamp or summary lineage to this tool.
Keep those measurements unsupported until the transport/serving owner supplies
an adapter; a command-start timestamp is never HTTP-start qualification.

Config JSON contains phases and command argv arrays (upload, search, optional
summary). Each phase declares name, source, session_id, marker_sha256, optional
expected_revision/depends_on, and timing bounds. Summary support defaults off.
Validate only: history_freshness_probe.py CONFIG --output REPORT
Execute adapters only for an independently authorized target: add --execute.

Adapters receive JSON stdin with operation, phase, clock_id and remaining timeout.
Summary calls also receive summary_kind=short|long. Commands never use a shell;
credentials belong in the adapter's existing auth mechanism, never argv/config
or stdout. Response JSON is validated at 64 KiB; subprocess buffering itself is
not a hard byte bound. Stderr and raw response bodies never enter the report.
Adapters must not detach child work into another session. Timeout cleanup kills
only the owned adapter process group, with at most 500 ms cleanup grace.

Upload response: accepted=true, source/session_id, echoed clock_id,
first_attempt_ns, ack_ns, attempt_anchor=http_request_start|upload_command_start,
acceptance_level=queued|durable. Times MUST use this host's shared kernel
clock_gettime_ns(CLOCK_MONOTONIC), immediately before the first actual HTTP
attempt and after validated ACK. A private/interpreter/remote clock is refused.
Platforms lacking this shared clock are unsupported. Queued ACK is not indexing.

Search: state=ready|missing|pending|unsupported, source/session_id and the exact
marker_sha256 for a ready hit. Summary ready requires source/session_id,
source_kind=session_summary, summary_kind, reader_path=session_summary|unknown,
recorded=true, summary_present=true, unique_live_pointer=true,
input_manifest_verified=true, input_marker_sha256 and independently attested
input_binding AND summary_binding. Both bindings contain exactly pointer_id,
physical_machine_id, physical_session_id, manifest_key, revision, content_sha256.
The source manifest must contain this phase's expected event, and the authorized
summary read must be backed by the identical live pointer/digest. A bare revision
cannot qualify: re-upload may restart numbering. session_brief cannot qualify.
These are this harness's adapter fields, not a new serving API or a substitute
for serving-version evidence. Until that evidence exists, leave support off.

Short/long outcomes and each observation include requested_length and reader_path
(session_summary, session_brief or unknown), including timeout/unsupported samples.
Each phase has its own first-attempt origin on one shared clock. Elapsed times
are first OBSERVED positive upper bounds, not exact readiness times. Last valid
negative observations bound them; errors are not negative-readiness evidence.
Failures, deadlines, unsupported summaries and blocked dependents remain in each
declared phase denominator. Never turn successful-only samples into population
percentiles, or infer queue/processing causes from these intervals alone.
"""
from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import dataclass, asdict
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import time
import uuid


class ProbeError(Exception):
    """Content-free failure category; never retain command output or stderr."""


class Clock:
    def now(self):
        # Use the explicit shared kernel clock, not an interpreter's private
        # epoch. Command adapters must use this SAME API on this SAME host.
        if not hasattr(time, "clock_gettime_ns") or not hasattr(time, "CLOCK_MONOTONIC"):
            raise ValueError("shared CLOCK_MONOTONIC is unavailable on this platform")
        return time.clock_gettime_ns(time.CLOCK_MONOTONIC)

    def sleep(self, seconds):
        time.sleep(seconds)


@dataclass
class Phase:
    name: str
    source: str
    session_id: str
    marker_sha256: str
    expected_revision: str | None = None
    summary_supported: bool = False
    depends_on: str | None = None
    deadline_seconds: float = 120
    poll_seconds: float = 1
    command_timeout_seconds: float = 15
    max_observations: int = 2000

    def validate(self):
        if not all(isinstance(x, str) and 0 < len(x) <= 256
                   for x in (self.name, self.source, self.session_id)):
            raise ValueError("invalid phase identity")
        if len(self.marker_sha256) != 64 or any(x not in "0123456789abcdef" for x in self.marker_sha256):
            raise ValueError("marker_sha256 must be lowercase SHA256")
        for value in (self.deadline_seconds, self.poll_seconds, self.command_timeout_seconds):
            if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(value) or not 0 < value <= 3600:
                raise ValueError("invalid duration")
        if type(self.max_observations) is not int or not 1 <= self.max_observations <= 10000:
            raise ValueError("invalid observation bound")
        if self.expected_revision is not None and (not isinstance(self.expected_revision, str) or not 0 < len(self.expected_revision) <= 256):
            raise ValueError("revision must be an exact opaque string or null")
        if type(self.summary_supported) is not bool:
            raise ValueError("summary_supported must be a boolean")


class CommandAdapter:
    """Commands receive metadata on stdin. Credentials stay in their own auth mechanism."""
    def __init__(self, commands):
        self.commands = commands
        for argv in commands.values():
            if not isinstance(argv, list) or not argv or not all(isinstance(x, str) for x in argv):
                raise ValueError("commands must be nonempty argv arrays, never shell text")

    @staticmethod
    def _stop(process):
        # Only the new adapter process group, never a pre-existing
        # uploader/core. Cleanup has at most 500 ms grace beyond the deadline.
        if process.returncode is None:  # The child PID has not been reaped/reused.
            try:
                if os.name == "posix":
                    os.killpg(process.pid, signal.SIGKILL)
                else:
                    process.kill()
            except ProcessLookupError:
                pass
        try:
            process.communicate(timeout=0.25)
        except (subprocess.TimeoutExpired, UnicodeError, OSError):
            for stream in (process.stdin, process.stdout):
                if stream:
                    try:
                        stream.close()
                    except OSError:
                        pass
            try:
                process.wait(timeout=0.25)
            except subprocess.TimeoutExpired:
                pass

    def call(self, operation, phase, clock_id, timeout):
        command = "summary" if operation in ("short_summary", "long_summary") else operation
        request = {"operation": command, "phase": asdict(phase), "clock_id": clock_id,
                   "timeout_seconds": timeout, "clock": "clock_gettime_ns(CLOCK_MONOTONIC) on this host"}
        if command == "summary":
            request["summary_kind"] = operation.removesuffix("_summary")
        try:
            process = subprocess.Popen(self.commands[command], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                       text=True, start_new_session=os.name == "posix")
        except OSError:
            raise ProbeError("command_unavailable") from None
        try:
            stdout, _ = process.communicate(json.dumps(request), timeout=timeout)
        except subprocess.TimeoutExpired:
            self._stop(process)
            raise ProbeError("command_timeout") from None
        except (UnicodeError, OSError):
            self._stop(process)
            raise ProbeError("invalid_response") from None
        if process.returncode:
            raise ProbeError("command_failed")
        if len(stdout.encode()) > 65536:
            raise ProbeError("response_too_large")
        try:
            body = json.loads(stdout)
        except (ValueError, TypeError):
            raise ProbeError("invalid_json") from None
        if not isinstance(body, dict):
            raise ProbeError("invalid_response")
        return body


class Probe:
    def __init__(self, adapter, clock=None):
        self.adapter = adapter
        self.clock = clock or Clock()
        self.clock_id = str(uuid.uuid4())

    def _invoke(self, operation, phase, deadline):
        remaining = (deadline - self.clock.now()) / 1e9
        if remaining <= 0:
            raise ProbeError("deadline")
        return self.adapter.call(operation, phase, self.clock_id,
                                 min(remaining, phase.command_timeout_seconds))

    @staticmethod
    def _identity(body, phase):
        return body.get("source") == phase.source and body.get("session_id") == phase.session_id

    @staticmethod
    def _summary_labels(operation, body=None):
        if operation not in ("short_summary", "long_summary"):
            return {}
        path = (body or {}).get("reader_path", "unknown")
        return {"requested_length": operation.removesuffix("_summary"),
                "reader_path": path if path in ("session_summary", "session_brief") else "unknown"}

    @staticmethod
    def _binding(value):
        keys = {"pointer_id", "physical_machine_id", "physical_session_id", "manifest_key", "revision", "content_sha256"}
        if not isinstance(value, dict) or set(value) != keys:
            return False
        for key in keys - {"revision", "content_sha256"}:
            if not isinstance(value[key], str) or not 0 < len(value[key]) <= 2048:
                return False
        if type(value["revision"]) not in (str, int) or value["revision"] == "":
            return False
        digest = value["content_sha256"]
        return isinstance(digest, str) and len(digest) == 64 and all(c in "0123456789abcdef" for c in digest)

    def _summary_state(self, body, phase, operation):
        # These are the probe's adapter fields, NOT a new public serving API.
        # Both operands require independent serving-contract evidence.
        if (body.get("reader_path") == "session_brief" or body.get("source_kind") != "session_summary"
                or body.get("summary_kind") != operation.removesuffix("_summary")):
            return "unsupported"
        if (body.get("recorded") is not True or body.get("summary_present") is not True
                or body.get("unique_live_pointer") is not True or body.get("input_manifest_verified") is not True
                or body.get("input_marker_sha256") != phase.marker_sha256
                or not self._binding(body.get("input_binding")) or not self._binding(body.get("summary_binding"))):
            return "unsupported"
        if phase.expected_revision is not None and body["input_binding"]["revision"] != phase.expected_revision:
            return "stale_binding"
        return "ready" if body["input_binding"] == body["summary_binding"] else "stale_binding"

    def phase(self, phase):
        invoked = self.clock.now()
        invocation_deadline = invoked + round(phase.deadline_seconds * 1e9)
        result = {"phase": phase.name, "source": phase.source, "session_id": phase.session_id,
                  "expected_revision": phase.expected_revision,
                  "upload": "failed", "search": "not_attempted", "short_summary": "not_attempted", "long_summary": "not_attempted",
                  "attempt_anchor": None, "first_attempt_ns": None, "ack_elapsed_ms": None,
                  "search_elapsed_ms": None, "short_summary_elapsed_ms": None, "long_summary_elapsed_ms": None,
                  "search_last_negative_ms": None, "short_summary_last_negative_ms": None, "long_summary_last_negative_ms": None,
                  "observations": [], "errors": {}}
        errors = Counter()
        try:
            receipt = self._invoke("upload", phase, invocation_deadline)
            finished = self.clock.now()
            if finished > invocation_deadline:
                raise ProbeError("upload_deadline")
            if not self._identity(receipt, phase) or receipt.get("accepted") is not True:
                raise ProbeError("upload_not_accepted")
            start, ack = receipt.get("first_attempt_ns"), receipt.get("ack_ns")
            if receipt.get("clock_id") != self.clock_id:
                raise ProbeError("clock_mismatch")
            if type(start) is not int or type(ack) is not int or not invoked <= start <= ack <= finished:
                raise ProbeError("invalid_upload_timestamps")
            if receipt.get("attempt_anchor") not in ("http_request_start", "upload_command_start"):
                raise ProbeError("unknown_upload_anchor")
            if ack > start + round(phase.deadline_seconds * 1e9):
                raise ProbeError("upload_deadline")
            result.update(upload="accepted", first_attempt_ns=start,
                          attempt_anchor=receipt["attempt_anchor"],
                          ack_elapsed_ms=(ack - start) / 1e6,
                          acceptance_level=receipt.get("acceptance_level") if receipt.get("acceptance_level") in ("queued", "durable") else "unknown")
        except ProbeError as error:
            result["upload"] = str(error)
            result["errors"] = {str(error): 1}
            return result

        # One host's monotonic clock throughout. Wall time and server timestamps
        # never enter subtraction. Revision alone can never qualify a summary.
        deadline = start + round(phase.deadline_seconds * 1e9)
        summary_state = "pending" if phase.summary_supported else "unsupported"
        states = {"search": "pending", "short_summary": summary_state, "long_summary": summary_state}
        while "pending" in states.values() and self.clock.now() < deadline:
            for operation in ("search", "short_summary", "long_summary"):
                if states[operation] != "pending":
                    continue
                if len(result["observations"]) >= phase.max_observations:
                    states[operation] = "observation_limit"
                    continue
                try:
                    body = self._invoke(operation, phase, deadline)
                    observed = self.clock.now()
                    if observed > deadline:
                        raise ProbeError("deadline")
                    labels = self._summary_labels(operation, body)
                    if labels:
                        result[operation + "_reader_path"] = labels["reader_path"]
                    state = body.get("state")
                    if state not in ("ready", "pending", "missing", "unsupported"):
                        raise ProbeError("invalid_observation")
                    if state == "ready":
                        if not self._identity(body, phase):
                            raise ProbeError("identity_mismatch")
                        if operation == "search" and body.get("marker_sha256") != phase.marker_sha256:
                            raise ProbeError("marker_mismatch")
                        if operation in ("short_summary", "long_summary"):
                            state = self._summary_state(body, phase, operation)
                    elapsed = (observed - start) / 1e6
                    result["observations"].append({"operation": operation, "elapsed_ms": elapsed, "state": state, **labels})
                    if state == "ready":
                        states[operation] = "ready"
                        result[operation + "_elapsed_ms"] = elapsed
                        if operation in ("short_summary", "long_summary"):
                            result[operation + "_binding_sha256"] = hashlib.sha256(json.dumps(body["input_binding"], sort_keys=True, separators=(",", ":")).encode()).hexdigest()
                    elif state == "unsupported":
                        states[operation] = state
                    else:
                        result[operation + "_last_negative_ms"] = elapsed
                except ProbeError as error:
                    errors[str(error)] += 1
                    labels = self._summary_labels(operation)
                    if labels:
                        result[operation + "_reader_path"] = "unknown"
                    result["observations"].append({"operation": operation,
                        "elapsed_ms": (self.clock.now() - start) / 1e6, "state": "error", "category": str(error), **labels})
                if self.clock.now() >= deadline:
                    break
            if "pending" in states.values():
                self.clock.sleep(min(phase.poll_seconds, max(0, (deadline - self.clock.now()) / 1e9)))
        result.update({key: "deadline" if value == "pending" else value for key, value in states.items()})
        result["errors"] = dict(errors)
        result["http_start_qualified"] = result["attempt_anchor"] == "http_request_start"
        return result

    @staticmethod
    def validate_phases(phases):
        names = set()
        for phase in phases:
            phase.validate()
            if phase.name in names or phase.depends_on and phase.depends_on not in names:
                raise ValueError("phase names must be unique and dependencies precede dependents")
            names.add(phase.name)
        if not phases:
            raise ValueError("at least one declared phase is required")

    def run(self, phases):
        self.validate_phases(phases)
        results = []
        for phase in phases:
            dependency = next((x for x in results if x["phase"] == phase.depends_on), None)
            if dependency and dependency["upload"] != "accepted":
                results.append({"phase": phase.name, "upload": "blocked_dependency", "search": "not_attempted",
                                "short_summary": "not_attempted", "long_summary": "not_attempted", "attempt_anchor": None})
            else:
                results.append(self.phase(phase))
        # Includes unsupported adapters, upload failures and blocked phases:
        # an outcome sample is not an assertion that a Reader was called.
        for result in results:
            result["summary_samples"] = {kind: {
                "requested_length": kind,
                "reader_path": result.get(kind + "_summary_reader_path", "unknown"),
                "state": result[kind + "_summary"],
                "elapsed_ms": result.get(kind + "_summary_elapsed_ms"),
            } for kind in ("short", "long")}
        total = len(phases)
        metrics = {"declared_phases": total,
                   "upload_accepted": sum(x["upload"] == "accepted" for x in results),
                   "http_start_qualified": sum(x.get("http_start_qualified", False) for x in results)}
        for operation in ("search", "short_summary", "long_summary"):
            ready = sum(x[operation] == "ready" for x in results)
            metrics[operation] = {"ready": ready, "denominator": total, "ready_rate": ready / total,
                                  "outcomes": dict(Counter(x[operation] for x in results))}
        return {"contract": "hosted-freshness-probe/1", "clock": "host-local CLOCK_MONOTONIC nanoseconds",
                "clock_id": self.clock_id, "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
                "metrics": metrics, "phases": results,
                "interpretation": "Readiness times are first observed upper bounds; last negative observations bound them. Errors are not negative readiness observations. Never infer queue or processing latency from these alone."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--execute", action="store_true", help="Execute configured adapters; omitted means validate only")
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    phases = [Phase(**value) for value in config["phases"]]
    Probe.validate_phases(phases)
    adapter = CommandAdapter(config["commands"])
    if not {"upload", "search"}.issubset(adapter.commands) or set(adapter.commands) - {"upload", "search", "summary"}:
        parser.error("upload and search commands are required; summary is optional only when unsupported")
    if any(p.summary_supported for p in phases) and "summary" not in adapter.commands:
        parser.error("a summary command is required unless every phase declares summary_supported=false")
    if not args.execute:
        print(json.dumps({"validated": True, "executed": False, "phases": len(phases)}))
        return 0
    report = Probe(adapter).run(phases)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"output": str(args.output), "metrics": report["metrics"]}))
    complete = all(report["metrics"][x]["ready"] == len(phases) for x in ("search", "short_summary", "long_summary"))
    return 0 if complete and report["metrics"]["http_start_qualified"] == len(phases) else 1


if __name__ == "__main__":
    raise SystemExit(main())
