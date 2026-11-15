#!/usr/bin/env python3
"""Pin the fast path's safety properties: approval-gated, read-only, publishes nothing."""
from pathlib import Path
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / '.github/workflows/fast-path.yml'
CACHE_ACTIONS = ('./.github/actions/rust-lane-cache', './.github/actions/swift-cache',
                 './.github/actions/windows-build-cache', './.github/actions/windows-toolchain')
PUBLISHING = ('npm publish', 'gh release', 'git push', 'git tag', 'docker push', '--push',
              'cargo publish', 'desktop-feed', 'promote')


class FastPathTests(unittest.TestCase):
    def setUp(self):
        self.text = WORKFLOW.read_text()
        self.doc = yaml.safe_load(self.text)
        self.jobs = self.doc['jobs']

    def test_workflow_dispatch_is_the_only_trigger(self):
        # PyYAML reads the bare `on:` key as True.
        triggers = self.doc.get('on', self.doc.get(True))
        self.assertEqual(list(triggers), ['workflow_dispatch'])

    def test_permissions_are_read_only_everywhere(self):
        self.assertEqual(self.doc['permissions'], {'contents': 'read'})
        for name, job in self.jobs.items():
            self.assertNotIn('permissions', job, name)

    def test_only_source_uses_the_fast_path_environment(self):
        gated = [name for name, job in self.jobs.items() if 'environment' in job]
        self.assertEqual(gated, ['source'])
        self.assertEqual(self.jobs['source']['environment'], 'fast-path')

    def test_every_other_job_needs_source(self):
        for name, job in self.jobs.items():
            if name == 'source':
                continue
            needs = job.get('needs', [])
            needs = [needs] if isinstance(needs, str) else needs
            self.assertIn('source', needs, name)

    def test_concurrency_never_replaces_a_run_waiting_for_approval(self):
        self.assertIn('github.run_id', self.doc['concurrency']['group'])

    def test_nothing_is_published_and_no_secret_is_read(self):
        self.assertNotIn('secrets.', self.text)
        for name, job in self.jobs.items():
            self.assertNotIn('uses', job, f'{name} calls a reusable workflow')
            for step in job.get('steps', []):
                run = step.get('run', '')
                for word in PUBLISHING:
                    self.assertNotIn(word, run, f'{name}: {step.get("name")}')
                self.assertFalse(step.get('uses', '').startswith('docker/'), name)

    def test_every_cache_is_restore_only(self):
        for name, job in self.jobs.items():
            for step in job.get('steps', []):
                uses = step.get('uses', '').split('@')[0]
                options = step.get('with', {})
                if uses == 'Swatinem/rust-cache':
                    self.assertEqual(options.get('save-if'), 'false', name)
                elif uses in CACHE_ACTIONS:
                    self.assertEqual(options.get('save'), 'false', f'{name}: {uses}')
                elif uses == 'actions/cache':
                    self.fail(f'{name}: actions/cache saves; use actions/cache/restore')
                if uses.startswith('actions/setup-'):
                    self.assertNotIn('cache', options, f'{name}: {uses} caches save')

    def test_later_jobs_check_out_the_approved_commit(self):
        for name, job in self.jobs.items():
            for step in job.get('steps', []):
                if step.get('uses', '').startswith('actions/checkout@'):
                    ref = step['with']['ref']
                    want = '${{ inputs.ref }}' if name == 'source' else '${{ needs.source.outputs.sha }}'
                    self.assertEqual(ref, want, name)


if __name__ == '__main__':
    unittest.main()
