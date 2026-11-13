#!/usr/bin/env python3
import copy
from datetime import datetime, timezone
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('evidence', Path(__file__).with_name('release-ci-evidence.py'))
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)
NOW = datetime(2026, 11, 11, tzinfo=timezone.utc)


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.run = dict(id=42, repository={'full_name': evidence.REPOSITORY},
                        head_repository={'full_name': evidence.REPOSITORY}, path=evidence.WORKFLOW,
                        status='completed', conclusion='success', event='workflow_dispatch',
                        head_branch='ci/snapshot', head_sha='b' * 40, created_at='2026-11-10T00:00:00Z')
        self.source = {'sha': 'a' * 40, 'commit': {'tree': {'sha': 'c' * 40}}}
        self.tested = copy.deepcopy(self.source)
        self.tested['sha'] = self.run['head_sha']
        self.jobs = [dict(name=name, conclusion='success', steps=[dict(name=s, conclusion='success') for s in steps])
                     for name, steps in evidence.REQUIRED.items()]
        for job in self.jobs:
            if job['name'].startswith('Windows tests ('):
                job['steps'].append(dict(name='Check the Windows entrypoints', conclusion='success'))

    def check(self):
        return evidence.verify(self.run, self.source, self.tested, self.jobs, NOW)

    def test_snapshot_and_squash_with_identical_trees(self):
        self.assertEqual(self.check()['source_sha'], 'a' * 40)

    def test_refuses_wrong_tree_fork_workflow_and_old_run(self):
        for field, value in [('head_repository', {'full_name': 'attacker/fork'}),
                             ('path', '.github/workflows/other.yml'), ('conclusion', 'failure'),
                             ('status', 'in_progress'), ('event', 'pull_request'),
                             ('created_at', '2026-10-01T00:00:00Z')]:
            with self.subTest(field=field):
                before = self.run[field]
                self.run[field] = value
                with self.assertRaises(ValueError): self.check()
                self.run[field] = before
        self.tested['commit']['tree']['sha'] = 'd' * 40
        with self.assertRaises(ValueError): self.check()

    def test_skipped_architecture_or_step_never_counts_as_green(self):
        self.jobs[-1]['conclusion'] = 'skipped'
        with self.assertRaises(ValueError): self.check()
        self.jobs[-1]['conclusion'] = 'success'
        self.jobs[-1]['steps'][-1]['conclusion'] = 'skipped'
        with self.assertRaises(ValueError): self.check()

    def test_duplicate_or_missing_jobs_are_refused(self):
        self.jobs.append(copy.deepcopy(self.jobs[0]))
        with self.assertRaises(ValueError): self.check()
        self.jobs = self.jobs[2:]
        with self.assertRaises(ValueError): self.check()

    def package(self, arch):
        test_job = next(job for job in self.jobs if job['name'] == f'Windows tests ({arch})')
        test_job['steps'][-1]['conclusion'] = 'skipped'
        package = dict(name=f'Windows package ({arch})', conclusion='success',
                       steps=[dict(name='Build the Windows package', conclusion='success')])
        self.jobs.append(package)
        return package, test_job

    def test_packaging_can_supply_both_windows_entrypoint_builds(self):
        for arch in ('x64', 'arm64'):
            self.package(arch)
        self.assertEqual(self.check()['kind'], 'native-gate')

    def test_packaging_never_substitutes_for_windows_runtime_tests(self):
        _, test_job = self.package('x64')
        test_job['steps'][1]['conclusion'] = 'skipped'
        with self.assertRaises(ValueError): self.check()

    def test_missing_failed_or_other_arch_packages_do_not_supply_entrypoints(self):
        for arch in ('x64', 'arm64'):
            for change in ('missing', 'wrong-arch', 'failed-job', 'skipped-build', 'missing-build', 'duplicate'):
                with self.subTest(arch=arch, change=change):
                    self.setUp()
                    package, _ = self.package(arch)
                    if change == 'missing': self.jobs.remove(package)
                    elif change == 'wrong-arch': package['name'] = f'Windows package ({"arm64" if arch == "x64" else "x64"})'
                    elif change == 'failed-job': package['conclusion'] = 'failure'
                    elif change == 'skipped-build': package['steps'][0]['conclusion'] = 'skipped'
                    elif change == 'missing-build': package['steps'] = []
                    elif change == 'duplicate': self.jobs.append(copy.deepcopy(package))
                    with self.assertRaises(ValueError): self.check()

    def test_main_push_keeps_the_existing_conservative_baseline_contract(self):
        self.run.update(event='push', head_branch='main')
        self.jobs = []
        self.assertEqual(self.check()['kind'], 'main')


if __name__ == '__main__': unittest.main()
