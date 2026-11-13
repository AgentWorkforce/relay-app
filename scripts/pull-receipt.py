#!/usr/bin/env python3
"""The pull model's publication receipt (docs/enterprise-distribution.md,
Pull model).

Desktop Promote publishes an unlisted candidate's version to the cloud feed
once: a second create of that version is refused (desktop-feed-publish.py
--unlisted). So that a failure after that publish (the attestation, the pull
release) can be retried for the same candidate, the promote job keeps, right
after the cloud publish, the workflow artifact pull-receipt-<profile>-v<version>:
publish-request.json, the exact create body sent, and receipt.json, which
binds that publication to the candidate (profile, version, channel,
candidate run id, source commit), to the promote run that kept it, and to
the sha256 of publish-request.json.

  write   writes the receipt directory after a cloud publish.
  find    looks for the receipt kept by an earlier Desktop Promote run of
          this repository dispatched on main. The same candidate's receipt
          resumes (resume=true; the receipt is copied to --output); a
          receipt of any other candidate is refused (exit 1); none is a
          first publication (resume=false), which keeps the feed's
          collision check. An artifact of the same name kept by any other
          workflow or ref is ignored.
  check   checks a kept publish-request.json against the candidate's cloud
          assets (cloud-assets.txt) before a resumed run attaches it.

Exit status: 0 done; 1 refused, or a GitHub API call failed.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import zipfile

WORKFLOW = ".github/workflows/desktop-promote.yml"
TRUSTED_BRANCH = "main"
RECEIPT = "receipt.json"
REQUEST = "publish-request.json"
SCHEMA = 1
MAX_BYTES = 1 << 20
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
SHA256 = re.compile(r"[0-9a-f]{64}")


class Refused(Exception):
    pass


def artifact_name(profile, version):
    return f"pull-receipt-{profile}-v{version}"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    hasher = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def binding(args):
    """What ties a receipt to one candidate."""
    return {"profile": args.profile, "version": args.version, "channel": args.channel,
            "candidate_run_id": args.candidate_run_id, "source_sha": args.source_sha}


def write(args):
    request = args.request.read_bytes()
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / REQUEST).write_bytes(request)
    receipt = dict(binding(args), schema=SCHEMA, promote_run_id=args.run_id, request_sha256=digest(request))
    (args.output / RECEIPT).write_text(json.dumps(receipt, sort_keys=True) + "\n")
    print(f"Kept the pull receipt of candidate run {args.candidate_run_id} for v{args.version}.")


def gh(*args):
    result = subprocess.run(["gh", "api", *args], capture_output=True)
    if result.returncode != 0:
        path = next(arg for arg in args if arg.startswith("repos/"))
        raise Refused(f"gh api {path} failed: {result.stderr.decode(errors='replace').strip()}")
    return result.stdout


def artifacts(repository, name):
    pages = json.loads(gh("-X", "GET", f"repos/{repository}/actions/artifacts", "-f", f"name={name}",
                          "-f", "per_page=100", "--paginate", "--slurp"))
    return [artifact for page in pages for artifact in (page.get("artifacts") or [])]


def trusted_run(repository, run_id):
    """A Desktop Promote run of this repository dispatched on main."""
    run = json.loads(gh(f"repos/{repository}/actions/runs/{run_id}"))
    return (run.get("path") == WORKFLOW and run.get("head_branch") == TRUSTED_BRANCH
            and run.get("event") == "workflow_dispatch"
            and (run.get("repository") or {}).get("full_name") == repository
            and (run.get("head_repository") or {}).get("full_name") == repository)


def download(repository, artifact_id, run_id):
    data = gh(f"repos/{repository}/actions/artifacts/{artifact_id}/zip")
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            members = archive.infolist()
            names = sorted(member.filename for member in members)
            if names != sorted([RECEIPT, REQUEST]) or any(member.file_size > MAX_BYTES for member in members):
                raise Refused(f"the pull receipt kept by Desktop Promote run {run_id} holds other files")
            return {name: archive.read(name) for name in names}
    except zipfile.BadZipFile as error:
        raise Refused(f"the pull receipt kept by Desktop Promote run {run_id} is not a zip archive") from error


def find(args):
    name = artifact_name(args.profile, args.version)
    want = binding(args)
    same, other = None, []
    for artifact in sorted(artifacts(args.repository, name), key=lambda item: item.get("id") or 0):
        if artifact.get("name") != name or artifact.get("expired"):
            continue
        run_id = (artifact.get("workflow_run") or {}).get("id")
        if not isinstance(run_id, int) or not trusted_run(args.repository, run_id):
            print(f"::warning::Ignoring the artifact {name} of run {run_id}: Desktop Promote on"
                  f" {TRUSTED_BRANCH} did not keep it.")
            continue
        files = download(args.repository, artifact.get("id"), run_id)
        try:
            receipt = json.loads(files[RECEIPT])
        except ValueError as error:
            raise Refused(f"the pull receipt kept by Desktop Promote run {run_id} is not JSON") from error
        if (not isinstance(receipt, dict) or receipt.get("schema") != SCHEMA
                or receipt.get("promote_run_id") != run_id
                or receipt.get("request_sha256") != digest(files[REQUEST])):
            raise Refused(f"the pull receipt kept by Desktop Promote run {run_id} is inconsistent")
        if {key: receipt.get(key) for key in want} == want:
            same = (run_id, files)
        else:
            other.append((run_id, receipt.get("candidate_run_id")))
    if other:
        run_id, candidate = other[0]
        raise Refused(f"v{args.version} was already published to the cloud feed by Desktop Promote run {run_id}"
                      f" from candidate run {candidate}; promote this candidate under a new version")
    resume = same is not None
    if resume:
        run_id, files = same
        args.output.mkdir(parents=True, exist_ok=True)
        for file_name, data in files.items():
            (args.output / file_name).write_bytes(data)
        print(f"Desktop Promote run {run_id} published candidate run {args.candidate_run_id} as v{args.version}"
              " to the cloud feed; this run resumes after that publish.")
    else:
        print(f"No pull receipt for v{args.version}: a first publication.")
    if args.github_output:
        with open(args.github_output, "a") as out:
            out.write(f"resume={'true' if resume else 'false'}\n")


def check(args):
    try:
        request = json.loads((args.receipt / REQUEST).read_bytes())
    except ValueError as error:
        raise Refused("the kept publish-request.json is not JSON") from error
    if not isinstance(request, dict) or request.get("version") != args.version or request.get("channel") != args.channel:
        raise Refused(f"the kept publish-request.json does not declare v{args.version} on the {args.channel} channel")
    names = [line.strip() for line in args.assets.read_text().splitlines() if line.strip()]
    declared = request.get("expected_assets")
    if not isinstance(declared, list) or not all(isinstance(entry, dict) for entry in declared):
        raise Refused("the kept publish-request.json has no expected_assets")
    declared_names = [entry.get("name") for entry in declared]
    if sorted(map(str, declared_names)) != sorted(names) or len(set(names)) != len(names):
        raise Refused("the kept publish-request.json does not declare exactly the candidate's cloud assets")
    for entry in declared:
        name = entry["name"]
        if not isinstance(name, str) or not NAME.fullmatch(name):
            raise Refused("the kept publish-request.json declares an asset that is not a bare file name")
        path = args.directory / name
        sha256, size = entry.get("sha256"), entry.get("size")
        if not isinstance(sha256, str) or not SHA256.fullmatch(sha256) or file_digest(path) != sha256:
            raise Refused(f"the kept publish-request.json declares {name} with another sha256")
        if isinstance(size, bool) or not isinstance(size, int) or path.stat().st_size != size:
            raise Refused(f"the kept publish-request.json declares {name} with another size")
    print(f"The kept publish-request.json declares the candidate's {len(names)} cloud assets.")


def parse(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)

    def bound(command):
        command.add_argument("--profile", required=True)
        command.add_argument("--version", required=True)
        command.add_argument("--channel", required=True, choices=["stable", "beta"])
        command.add_argument("--candidate-run-id", required=True)
        command.add_argument("--source-sha", required=True)
        command.add_argument("--output", type=Path, required=True)

    writing = commands.add_parser("write")
    bound(writing)
    writing.add_argument("--request", type=Path, required=True)
    writing.add_argument("--run-id", type=int, required=True)

    finding = commands.add_parser("find")
    bound(finding)
    finding.add_argument("--repository", required=True)
    finding.add_argument("--github-output", default=os.environ.get("GITHUB_OUTPUT", ""))

    checking = commands.add_parser("check")
    checking.add_argument("--receipt", type=Path, required=True)
    checking.add_argument("--assets", type=Path, required=True)
    checking.add_argument("--directory", type=Path, required=True)
    checking.add_argument("--version", required=True)
    checking.add_argument("--channel", required=True, choices=["stable", "beta"])
    return parser.parse_args(argv)


def main(argv=None):
    args = parse(sys.argv[1:] if argv is None else argv)
    try:
        {"write": write, "find": find, "check": check}[args.command](args)
    except (Refused, OSError) as error:
        print(f"::error::{error}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
