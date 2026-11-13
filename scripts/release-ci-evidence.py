#!/usr/bin/env python3
"""Verify independently fetched CI evidence before a native release worker reuses it.

Main push runs retain the existing release contract. A review snapshot must
have the identical Git tree and positive evidence for every native test suite;
a green workflow with skipped lanes is not a substitute for release checks.
"""
import argparse
from datetime import datetime, timedelta, timezone
import json
import re
import subprocess

REPOSITORY = 'AgentWorkforce/relay-app'
WORKFLOW = '.github/workflows/desktop-ci.yml'
REQUIRED = {
    'Probe': ('Test the desktop core',),
    **{f'macOS core tests ({arch})': ('Test the desktop core',) for arch in ('x64', 'arm64')},
    **{f'macOS Swift tests ({arch})': ('Test the Swift package', 'Test the shared session overview package',
                                      'Test newly started threads in the native app') for arch in ('x64', 'arm64')},
    **{f'Windows tests ({arch})': ("Test the desktop core's platform paths and session discovery",
                                  "Test the app's wording and state", "Test the app's window",
                                  "Test the app's hosting of the core")
       for arch in ('x64', 'arm64')},
    **{f'Linux tests ({arch})': ('Build and test the native visibility host', "Test the app's wording and state",
                                "Test the app's window", 'Test the app against the real core',
                                "Smoke test the app's startup") for arch in ('x64', 'arm64')},
}


def verify(run, source, tested, jobs, now=None):
    now = now or datetime.now(timezone.utc)
    repo = {'full_name': REPOSITORY}
    if (run.get('repository', {}).get('full_name') != repo['full_name']
            or run.get('head_repository', {}).get('full_name') != repo['full_name']
            or run.get('path') != WORKFLOW or run.get('status') != 'completed'
            or run.get('conclusion') != 'success'):
        raise ValueError('CI must be a successful Desktop CI run in the source repository')
    if run.get('event') not in ('push', 'workflow_dispatch'):
        raise ValueError('pull request and reusable workflow runs are not release evidence')
    if run.get('head_sha') != tested.get('sha'):
        raise ValueError('tested commit does not match the CI run')
    tree = source.get('commit', {}).get('tree', {}).get('sha')
    if not tree or tree != tested.get('commit', {}).get('tree', {}).get('sha'):
        raise ValueError('CI tested a different source tree')
    created = datetime.fromisoformat(run['created_at'].replace('Z', '+00:00'))
    if created > now + timedelta(minutes=5) or now - created > timedelta(days=7):
        raise ValueError('CI evidence must be no more than seven days old')
    # Main already has a conservative last-green baseline selector and always
    # tests both architectures. Keep that existing contract unchanged.
    if run.get('event') == 'push' and run.get('head_branch') == 'main':
        return {'run_id': run['id'], 'source_sha': source['sha'], 'source_tree': tree, 'kind': 'main'}
    if run.get('event') != 'workflow_dispatch':
        raise ValueError('only main push or dispatched native gates may be reused')
    # The jobs endpoint defaults to the latest attempt. Never accept old green
    # jobs left over from a prior attempt, or one green duplicate beside a red.
    for name, steps in REQUIRED.items():
        matches = [job for job in jobs if job.get('name') == name]
        if len(matches) != 1 or matches[0].get('conclusion') != 'success':
            raise ValueError(f'CI did not pass the required suite: {name}')
        actual = {step['name']: step.get('conclusion') for step in matches[0].get('steps', [])}
        if any(actual.get(step) != 'success' for step in steps):
            raise ValueError(f'CI skipped a required test step in {name}')
        if name.startswith('Windows tests ('):
            entrypoints = actual.get('Check the Windows entrypoints')
            if entrypoints == 'success':
                continue
            # When packaging is selected, the workflow compiles/links these
            # entrypoints there instead. All runtime test steps above remain
            # mandatory, and only the same architecture's successful build
            # can replace the deliberately skipped standalone check.
            arch = name.removeprefix('Windows tests (').removesuffix(')')
            packages = [job for job in jobs if job.get('name') == f'Windows package ({arch})']
            if (entrypoints not in (None, 'skipped') or len(packages) != 1
                    or packages[0].get('conclusion') != 'success'
                    or not any(step.get('name') == 'Build the Windows package'
                               and step.get('conclusion') == 'success'
                               for step in packages[0].get('steps', []))):
                raise ValueError(f'CI did not compile the Windows entrypoints for {arch}')
    return {'run_id': run['id'], 'source_sha': source['sha'], 'source_tree': tree, 'kind': 'native-gate'}


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', f'repos/{REPOSITORY}/{path}'], text=True))


def check(run_id, sha):
    if not re.fullmatch(r'[1-9][0-9]*', str(run_id)) or not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('a numeric run ID and full source SHA are required')
    run = api(f'actions/runs/{run_id}')
    source = api(f'commits/{sha}')
    tested = api(f'commits/{run["head_sha"]}')
    jobs = []
    page = 1
    while True:
        batch = api(f'actions/runs/{run_id}/jobs?filter=latest&per_page=100&page={page}')['jobs']
        jobs.extend(batch)
        if len(batch) < 100:
            break
        page += 1
    return verify(run, source, tested, jobs)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-id', required=True)
    parser.add_argument('--source-sha', required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(check(args.run_id, args.source_sha)))
    except (ValueError, KeyError, subprocess.CalledProcessError) as error:
        raise SystemExit(f'release CI evidence refused: {error}') from error
