"""Exercise both workflow entry points using an offline, stateful gh stub."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from test_release_workflow import block

ROOT = Path(__file__).resolve().parents[1]
VERSION = "2026.11.99"
REPOSITORY = "AgentWorkforce/relay-desktop-releases"
_spec = importlib.util.spec_from_file_location("release_candidate", ROOT / "scripts/release-candidate.py")
candidate = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(candidate)
FAKE_GH = """#!/usr/bin/env python3
import json
from pathlib import Path
import sys

args = sys.argv[1:]
state_file = Path('state.json')
state = json.loads(state_file.read_text())
repo = 'AgentWorkforce/relay-desktop-releases'
tag = 'v2026.11.99'
if args[0] == 'api' and args[1] == f'repos/{repo}/releases?per_page=100':
    print(json.dumps([[{'tag_name': 'v2026.10.66', 'draft': False}] + state['releases']]))
    sys.exit()
if 'example/custom' in args:
    assert all('#' not in arg for arg in args)
    assert args[2] == tag
    if args[:2] == ['release', 'create']:
        assert '--draft' in args
        state['events'].append('custom create and upload')
    else:
        assert args[:2] == ['release', 'edit'] and '--draft=false' in args
        state['events'].append('custom publish')
elif args[:2] == ['release', 'create']:
    assert args[2:7] == [tag, '--repo', repo, '--target', 'main']
    assert '--draft' in args and not state['releases']
    state['releases'].append({'tag_name': tag, 'id': 134, 'draft': True,
                              'name': args[args.index('--title') + 1]})
    state['events'].append('create')
elif args[:2] == ['release', 'upload']:
    assert args[2:6] == [tag, '--repo', repo, '--clobber']
    assert state['releases'][0]['draft']
    state['events'].append('upload')
    first_upload = state['events'].count('upload') == 1
    uploads = args[6:]
    if first_upload:
        uploads = [next(arg for arg in uploads if '-macOS-arm64.dmg#Compatibility bridge' in arg)]
    for arg in uploads:
        path, _, label = arg.partition('#')
        name = Path(path).name
        state['assets'][name] = [label, f'https://github.com/{repo}/releases/download/{tag}/{name}',
                                 Path(path).read_bytes().hex()]
    if first_upload:
        state_file.write_text(json.dumps(state))
        sys.exit(1)
else:
    assert args[:4] == ['api', '--method', 'PATCH', f'repos/{repo}/releases/134'], args
    assert 'draft=false' in args and f'make_latest={state["latest"]}' in args
    assert state['releases'][0]['draft']
    state['releases'][0]['draft'] = False
    state['events'].append('publish')
state_file.write_text(json.dumps(state))
"""


def workflow_command(promotion):
    kind = "promote" if promotion else "release"
    workflow = (ROOT / f".github/workflows/desktop-{kind}.yml").read_text()
    name = "Publish prepared assets" if promotion else "Publish the default release with labeled assets"
    step = block(workflow, f"- name: {name}", 6)
    if promotion:
        lines = step.split("        run: |\n")[1].splitlines()
        return "\n".join(line[10:] for line in lines if not line or line.startswith(" " * 10))
    return step.split("        run: ")[1].strip()


class ReleaseAssetLabelTests(unittest.TestCase):
    def run_workflow(self, promotion, *, repository=REPOSITORY, releases=(), profile=""):
        with tempfile.TemporaryDirectory() as temp:
            work = Path(temp)
            (work / "scripts").symlink_to(ROOT / "scripts")
            gh = work / "gh"
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            (work / "state.json").write_text(json.dumps(
                dict(releases=list(releases), events=[], assets={}, latest="true")))
            assets = work / ("candidate/assets" if promotion else "release-assets")
            assets.mkdir(parents=True)
            notes = work / ("candidate/release-notes.md" if promotion else "release-notes.md")
            notes.write_text("Synthetic notes\n")
            packages = candidate.required_assets(VERSION) | {"install-macos.sh"}
            expected = dict.fromkeys(packages | {f"{name}.sha256" for name in packages}, "")
            for arch, processor in (("arm64", "Apple silicon"), ("x64", "Intel")):
                pairs = {
                    f"AgentRelay-{VERSION}-macOS-{arch}.dmg": (
                        f"Compatibility bridge for older updaters ({processor}, macOS 13+)"),
                    f"AgentRelay-{VERSION}-macOS15-{arch}.dmg": (
                        f"Agent Relay {VERSION} installer ({processor}, macOS 15+)"),
                    f"AgentRelay-macOS-{arch}.dmg": f"Current installer ({processor}, macOS 15+)",
                }
                pairs.update({f"{name}.sha256": "" for name in pairs})
                expected.update(pairs)
            for name in expected:
                (assets / name).write_bytes(name.encode())
            env = dict(os.environ, PATH=f"{work}:{os.environ['PATH']}", PROFILE=profile,
                       RELEASES_REPOSITORY=repository, VERSION=VERSION, CHANNEL="stable",
                       TITLE=f"Stable v{VERSION}", GITHUB_RUN_ID="7", CANDIDATE_RUN_ID="6",
                       GITHUB_REPOSITORY="example/source",
                       GITHUB_STEP_SUMMARY=str(work / "summary"))
            result = subprocess.run(["bash", "-c", workflow_command(promotion)], cwd=work,
                                    env=env, text=True, capture_output=True, timeout=15)
            return result, json.loads((work / "state.json").read_text()), expected

    def test_both_paths_label_draft_assets_and_retry_before_publication(self):
        for promotion in (False, True):
            with self.subTest(promotion=promotion):
                result, state, expected = self.run_workflow(promotion)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(state["events"], ["create", "upload", "upload", "publish"])
                self.assertEqual(state["assets"], {
                    name: [label, f"https://github.com/{REPOSITORY}/releases/download/v{VERSION}/{name}",
                           name.encode().hex()] for name, label in expected.items()
                })

    def test_custom_promotion_keeps_unlabeled_upload(self):
        result, state, _ = self.run_workflow(True, repository="example/custom", profile="custom")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(state["events"], ["custom create and upload", "custom publish"])

    def test_same_run_resumes_its_draft(self):
        draft = {"tag_name": f"v{VERSION}", "draft": True, "id": 134,
                 "name": "Desktop release draft from example/source run 7"}
        result, state, _ = self.run_workflow(True, releases=[draft])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(state["events"], ["upload", "upload", "publish"])

    def test_refuses_published_release_foreign_draft_and_custom_repository(self):
        for settings in (
            {"repository": "example/custom"},
            {"profile": "custom"},
            {"releases": [{"tag_name": f"v{VERSION}", "draft": False}]},
            {"releases": [{"tag_name": f"v{VERSION}", "draft": True, "name": "Another run"}]},
        ):
            with self.subTest(settings=settings):
                result, state, _ = self.run_workflow(False, **settings)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(state["events"], [])


if __name__ == "__main__":
    unittest.main()
