#!/usr/bin/env python3
"""Exercise the actual release workflow gates without a YAML dependency."""
import ast
import base64
import glob
import hashlib
import json
import os
import itertools
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import textwrap
import unittest

import yaml

WORKFLOW = Path(__file__).resolve().parents[1] / '.github/workflows/desktop-release.yml'
PROMOTE = Path(__file__).resolve().parents[1] / '.github/workflows/desktop-promote.yml'
SITE_PROFILE = Path(__file__).resolve().parents[1] / '.github/actions/site-profile/action.yml'
WINDOWS_SIGN = Path(__file__).resolve().parents[1] / '.github/actions/windows-sign/action.yml'
# The repository variables Windows signing reads (docs/windows-app.md, "Code signing").
SIGNING_VARIABLES = ('AZURE_TENANT_ID', 'AZURE_CLIENT_ID', 'AZURE_SUBSCRIPTION_ID', 'TRUSTED_SIGNING_ENDPOINT',
                     'TRUSTED_SIGNING_ACCOUNT', 'TRUSTED_SIGNING_CERT_PROFILE')
# A site profile's values, each an environment secret of desktop-<id>.
PROFILE_SECRETS = ('AGENT_RELAY_SITE_URL', 'AGENT_RELAY_URL_SCHEME', 'DESKTOP_PRODUCT_NAME',
                   'DESKTOP_BUNDLE_ID', 'RELEASES_REPOSITORY', 'DESKTOP_SUPPORT_URL',
                   'DESKTOP_POLICY_DEFAULTS', 'DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS', 'DESKTOP_COBRAND_NAME')
STATES = ('success', 'failure', 'skipped', 'cancelled')


def block(text, marker, indent):
    """Read an indentation-delimited job or step from this workflow."""
    lines = text.splitlines()
    start = next(i for i, line in enumerate(lines) if line == ' ' * indent + marker)
    end = next((i for i in range(start + 1, len(lines))
                if lines[i].strip() and not lines[i].lstrip().startswith('#')
                and len(lines[i]) - len(lines[i].lstrip()) <= indent), len(lines))
    return '\n'.join(lines[start:end])


def condition(text, indent):
    lines = text.splitlines()
    prefix = ' ' * indent + 'if: '
    start = next(i for i, line in enumerate(lines) if line.startswith(prefix))
    value = lines[start][len(prefix):]
    if value in ('>-', '>', '|', '|-'):
        parts = []
        for line in lines[start + 1:]:
            if line.strip() and len(line) - len(line.lstrip()) <= indent:
                break
            parts.append(line.strip())
        value = ' '.join(parts)
    return value.removeprefix('${{').removesuffix('}}').strip()


class Gate:
    """Compile the workflow's supported boolean expression; reject new syntax."""
    def __init__(self, expression):
        self.references = []
        expression = expression.replace('always()', 'True').replace('cancelled()', 'cancelled')
        def reference(match):
            self.references.append(match[0])
            return f'value_{len(self.references) - 1}'
        expression = re.sub(r'needs\.[\w-]+\.(?:result|outputs\.[\w-]+)|inputs\.[\w-]+|env\.[\w-]+|github(?:\.[\w-]+)+', reference, expression)
        expression = expression.replace('&&', ' and ').replace('||', ' or ')
        expression = re.sub(r'!(?!=)', ' not ', expression).strip()
        tree = ast.parse(expression, mode='eval')
        allowed = (ast.Expression, ast.BoolOp, ast.And, ast.Or, ast.UnaryOp,
                   ast.Not, ast.Compare, ast.Eq, ast.NotEq, ast.Constant, ast.Name, ast.Load)
        for node in ast.walk(tree):
            if not isinstance(node, allowed):
                raise ValueError(f'Unsupported gate syntax: {ast.dump(node)}')
            if isinstance(node, ast.Name) and node.id not in {'cancelled', *(f'value_{i}' for i in range(len(self.references)))}:
                raise ValueError(f'Unsupported gate variable: {node.id}')
        self.code = compile(tree, str(WORKFLOW), 'eval')

    def __call__(self, context, cancelled=False):
        values = {f'value_{i}': (context.get(ref, 'false') if ref == 'needs.version.outputs.skip' else context[ref])
                  for i, ref in enumerate(self.references)}
        return bool(eval(self.code, {'__builtins__': {}}, dict(values, cancelled=cancelled)))


class ReleaseWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text()
        cls.promote = PROMOTE.read_text()
        cls.publish = block(cls.workflow, 'publish:', 2)
        cls.publish_gate = Gate(condition(cls.publish, 4))

    def test_cloud_selected_gate_waits_through_a_rerun_and_never_falls_back_on_bad_evidence(self):
        job = block(self.workflow, 'ci-status:', 2)
        step = block(job, '- name: Look for a passing Desktop CI run of this commit on main', 6)
        script = textwrap.dedent(step.split('        run: |\n', 1)[1])
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'gh').write_text("""#!/bin/sh
n=$(cat "$COUNT" 2>/dev/null || echo 0)
n=$((n+1)); echo "$n" > "$COUNT"
printf '%s\\n' "$*" >> "$CALLS"
case "$n" in
  1) echo '{"status":"completed","conclusion":"failure","run_attempt":1}' ;;
  2) echo '{"status":"in_progress","conclusion":null,"run_attempt":2}' ;;
  *) echo '{"status":"completed","conclusion":"success","run_attempt":2}' ;;
esac
""")
            (root / 'sleep').write_text('#!/bin/sh\nexit 0\n')
            (root / 'python3').write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$CALLS"\nexit "$VERIFY_EXIT"\n')
            for name in ('gh', 'sleep', 'python3'): (root / name).chmod(0o755)
            for code in (0, 7):
                (root / 'count').unlink(missing_ok=True)
                (root / 'calls').write_text('')
                output = root / 'output'
                output.write_text('')
                env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}", COUNT=str(root / 'count'),
                           CALLS=str(root / 'calls'), VERIFY_EXIT=str(code), GITHUB_OUTPUT=str(output),
                           GITHUB_REPOSITORY='AgentWorkforce/relay-app', SELECTED_CI_RUN_ID='123',
                           SHA='a' * 40, FULL_CHECKS='false', CRATE_OVERRIDES='false')
                result = subprocess.run(['bash', '-c', script], cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, code, result.stdout + result.stderr)
                self.assertNotIn('branch=main', (root / 'calls').read_text())
                self.assertEqual('tested=true' in output.read_text(), code == 0)
                self.assertIn('release-ci-evidence.py --run-id 123 --source-sha ' + 'a' * 40,
                              (root / 'calls').read_text())

    def test_profile_builds_never_fall_back_to_the_repository_telemetry_key(self):
        # A profile build is a site build: it reports only to its own site's
        # analytics gateway and is given no PostHog key (the core's build
        # refuses one). Every build job gives the repository's key to the
        # default profile only, and no profile key is read at all.
        guarded = "AGENT_RELAY_POSTHOG_API_KEY: ${{ (inputs.profile == '' && secrets.POSTHOG_API_KEY) || '' }}"
        self.assertEqual(self.workflow.count(guarded), 5)
        self.assertNotIn('secrets.DESKTOP_POSTHOG_API_KEY', self.workflow)
        self.assertNotIn('AGENT_RELAY_POSTHOG_API_KEY: ${{ secrets.POSTHOG_API_KEY }}', self.workflow)
        self.assertNotIn('AGENT_RELAY_POSTHOG_API_KEY: ${{ secrets.DESKTOP_POSTHOG_API_KEY }}', self.workflow)

    def test_releases_app_pair_is_selected_by_profile_in_publish_and_promote(self):
        # An environment secret shadows a repository secret of the same name
        # but cannot hide one: a profile uses DESKTOP_RELEASES_APP_* and the
        # default RELEASES_APP_*, never the other's, and each credential use
        # prefers the minted App token over RELEASES_TOKEN.
        has_app = ("HAS_APP: ${{ (inputs.profile != '' && secrets.DESKTOP_RELEASES_APP_ID != ''"
                   " && secrets.DESKTOP_RELEASES_APP_PRIVATE_KEY != '') || (inputs.profile == ''"
                   " && secrets.RELEASES_APP_ID != '' && secrets.RELEASES_APP_PRIVATE_KEY != '') }}")
        app_id = ("app-id: ${{ (inputs.profile != '' && secrets.DESKTOP_RELEASES_APP_ID)"
                  " || (inputs.profile == '' && secrets.RELEASES_APP_ID) || '' }}")
        key = ("private-key: ${{ (inputs.profile != '' && secrets.DESKTOP_RELEASES_APP_PRIVATE_KEY)"
               " || (inputs.profile == '' && secrets.RELEASES_APP_PRIVATE_KEY) || '' }}")
        # Promote alone lets a profile without its own pair opt into the
        # organisation's pusher App (test_promote_profile_opts_into_the_org_pusher_app).
        promote_app_id = ("app-id: ${{ (inputs.profile != '' && secrets.DESKTOP_RELEASES_APP_ID)"
                          " || (inputs.profile != '' && steps.releases-app.outputs.use_org_pusher == 'true'"
                          " && secrets.GH_APP_PUSHER_ID) || (inputs.profile == '' && secrets.RELEASES_APP_ID) || '' }}")
        promote_key = ("private-key: ${{ (inputs.profile != '' && secrets.DESKTOP_RELEASES_APP_PRIVATE_KEY)"
                       " || (inputs.profile != '' && steps.releases-app.outputs.use_org_pusher == 'true'"
                       " && secrets.GH_APP_PUSHER_PRIVATE_KEY) || (inputs.profile == ''"
                       " && secrets.RELEASES_APP_PRIVATE_KEY) || '' }}")
        credential = '${{ steps.releases-token.outputs.token || secrets.RELEASES_TOKEN }}'
        # (The promote workflow's bridge job reads only its bridge
        # environment's DESKTOP_RELEASES_APP_* pair: test_release_workflow_cloud.py.)
        promote = self.promote.replace(block(self.promote, 'bridge:', 2), '')
        for name, text, job, expressions, profile_reads in (
                ('release', self.workflow, self.publish, (has_app, app_id, key), 2),
                # The opt-in also reads that the profile's own pair is unset.
                ('promote', promote, block(self.promote, 'promote:', 2), (has_app, promote_app_id, promote_key), 3)):
            with self.subTest(workflow=name):
                for expression in expressions:
                    self.assertEqual(text.count(expression), 1)
                    self.assertIn(expression, job)
                # No other use of either pair's names.
                for secret in ('RELEASES_APP_ID', 'RELEASES_APP_PRIVATE_KEY'):
                    uses = re.findall(r'secrets\.(\w*' + secret + ')', text)
                    self.assertEqual(sorted(uses), sorted([secret] * 2 + ['DESKTOP_' + secret] * profile_reads))
                mint = block(job, '- name: Mint a releases repository token', 6)
                if name == 'release':
                    # Preparation never publishes: it skips the token and the access check.
                    guard = "github.event_name != 'workflow_run' && !inputs.prepare-only"
                    self.assertIn("if: ${{ steps.releases-app.outputs.has_app == 'true' && " + guard + " }}", mint)
                    # A cloud feed publishes with its publisher token: no App, no access check.
                    self.assertIn('if: ${{ ' + guard + " && env.RELEASE_FEED != 'cloud' }}",
                                  block(job, '- name: Check publishing access', 6))
                else:
                    self.assertIn('if: steps.releases-app.outputs.has_app == \'true\'', mint)
                self.assertIn("if: env.RELEASE_FEED != 'cloud'", block(job, '- name: Check for the releases App', 6))
                self.assertIn('owner: ${{ env.RELEASES_OWNER }}', mint)
                self.assertIn('repositories: ${{ env.RELEASES_NAME }}', mint)
                self.assertIn('permission-contents: write', mint)
                self.assertNotIn('permission-', mint.replace('permission-contents: write', ''))
                # Every GH_TOKEN / token use takes the App token first.
                uses = re.findall(r'^\s+(?:GH_TOKEN|token): (.*)$', job, re.M)
                # (github.token reads of this source repository are not
                # releases-repository credentials.)
                releases_uses = [use for use in uses if use != '${{ github.token }}']
                self.assertGreaterEqual(len(releases_uses), 2)
                for use in releases_uses:
                    self.assertEqual(use, credential)
                self.assertNotIn('secrets.RELEASES_TOKEN', job.replace(credential, ''))

    def test_promote_profile_opts_into_the_org_pusher_app(self):
        # A profile whose own pair is unset uses the organisation's pusher App
        # only when its environment sets DESKTOP_RELEASES_USE_ORG_PUSHER to
        # "true"; its own pair comes first, the default profile never uses the
        # pusher and never a profile the default's pair, and the token's scope
        # is the same whichever App mints it.
        promote = block(self.promote, 'promote:', 2)
        check = block(promote, '- name: Check for the releases App', 6)
        mint = block(promote, '- name: Mint a releases repository token', 6)
        step = yaml.safe_load(check)[0]
        [mint_step] = yaml.safe_load(mint)
        self.assertEqual(mint_step['uses'],
                         'actions/create-github-app-token@64ab729a6294000f5aa4a4bc0b73d1f32bc45510')
        self.assertEqual(mint_step['continue-on-error'], "${{ env.RELEASE_FEED == 'cloud' }}")
        self.assertEqual(mint_step['if'], "steps.releases-app.outputs.has_app == 'true'")
        self.assertEqual({key: value for key, value in mint_step['with'].items() if key not in ('app-id', 'private-key')},
                         {'owner': '${{ env.RELEASES_OWNER }}', 'repositories': '${{ env.RELEASES_NAME }}',
                          'permission-contents': 'write'})
        # The pusher's names appear only in the opt-in and the mint.
        self.assertEqual(self.promote.count('secrets.GH_APP_PUSHER_ID'), 2)
        self.assertEqual(self.promote.count('secrets.GH_APP_PUSHER_PRIVATE_KEY'), 2)
        self.assertEqual(self.promote.count('secrets.DESKTOP_RELEASES_USE_ORG_PUSHER'), 1)
        opt_in = step['env']['USE_ORG_PUSHER']
        for clause in ("inputs.profile != ''", "secrets.DESKTOP_RELEASES_APP_ID == ''",
                       "secrets.DESKTOP_RELEASES_APP_PRIVATE_KEY == ''",
                       "secrets.DESKTOP_RELEASES_USE_ORG_PUSHER == 'true'"):
            self.assertIn(clause, opt_in)
        # In the mint, each pusher secret sits in a term that requires a
        # profile and the opt-in output, which only that expression sets.
        for name, secret in (('app-id', 'GH_APP_PUSHER_ID'), ('private-key', 'GH_APP_PUSHER_PRIVATE_KEY')):
            terms = mint_step['with'][name].removeprefix('${{').removesuffix('}}').split('||')
            [term] = [term.strip() for term in terms if secret in term]
            self.assertEqual(term, "(inputs.profile != '' && steps.releases-app.outputs.use_org_pusher == 'true'"
                                   " && secrets." + secret + ")")
        self.assertIn('echo "use_org_pusher=$USE_ORG_PUSHER"', check)

        def evaluate(expression, context):
            # GitHub's && and || return an operand, as Python's and/or do;
            # an unset secret is ''.
            names = {}
            def reference(match):
                names[f'value_{len(names)}'] = context.get(match[0], '')
                return f'value_{len(names) - 1}'
            source = expression.strip().removeprefix('${{').removesuffix('}}')
            source = re.sub(r'(?:inputs|secrets|steps\.[\w-]+\.outputs)\.[\w-]+', reference, source)
            # GitHub compares strings with == case-insensitively.
            source = re.sub(r'(value_\d+|\'[^\']*\')\s*(==|!=)\s*(value_\d+|\'[^\']*\')',
                            lambda m: f'{m[1]}.casefold() {m[2]} {m[3]}.casefold()', source)
            source = source.replace('&&', ' and ').replace('||', ' or ')
            tree = ast.parse(source.strip(), mode='eval')
            for node in ast.walk(tree):
                self.assertIsInstance(node, (ast.Expression, ast.BoolOp, ast.And, ast.Or, ast.Compare, ast.Call, ast.Attribute,
                                             ast.Eq, ast.NotEq, ast.Constant, ast.Name, ast.Load))
            value = eval(compile(tree, 'expression', 'eval'), {'__builtins__': {}}, names)
            return ('true' if value else 'false') if isinstance(value, bool) else value

        own = {'secrets.DESKTOP_RELEASES_APP_ID': 'own-id', 'secrets.DESKTOP_RELEASES_APP_PRIVATE_KEY': 'own-key'}
        default = {'secrets.RELEASES_APP_ID': 'default-id', 'secrets.RELEASES_APP_PRIVATE_KEY': 'default-key'}
        pusher = {'secrets.GH_APP_PUSHER_ID': 'pusher-id', 'secrets.GH_APP_PUSHER_PRIVATE_KEY': 'pusher-key'}
        for profile, has_own, has_default, has_pusher, flag in itertools.product(
                ('', 'q7m2k9'), (False, True), (False, True), (False, True), ('', 'true', 'TRUE', 'True', 'yes', '1')):
            context = {'inputs.profile': profile, 'secrets.DESKTOP_RELEASES_USE_ORG_PUSHER': flag,
                       **(own if has_own else {}), **(default if has_default else {}), **(pusher if has_pusher else {})}
            with self.subTest(context=context):
                use = evaluate(opt_in, context)
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    env = dict(os.environ, RELEASES_REPOSITORY='owner/name', RELEASE_FEED='', PROFILE=profile,
                               HAS_APP=evaluate(step['env']['HAS_APP'], context), USE_ORG_PUSHER=use,
                               GITHUB_ENV=str(root / 'env'), GITHUB_OUTPUT=str(root / 'output'))
                    result = subprocess.run(['bash', '-c', step['run']], env=env, capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    outputs = dict(line.split('=', 1) for line in (root / 'output').read_text().splitlines())
                context['steps.releases-app.outputs.use_org_pusher'] = outputs['use_org_pusher']
                app_id = evaluate(mint_step['with']['app-id'], context)
                key = evaluate(mint_step['with']['private-key'], context)
                if profile and has_own:
                    expected = ('own-id', 'own-key')
                elif profile and flag.casefold() == 'true' and has_pusher:
                    expected = ('pusher-id', 'pusher-key')
                elif not profile and has_default:
                    expected = ('default-id', 'default-key')
                else:
                    expected = None
                self.assertEqual(outputs['has_app'], 'true' if expected else 'false')
                self.assertEqual(outputs['use_org_pusher'], 'true' if expected == ('pusher-id', 'pusher-key') else 'false')
                if expected:
                    self.assertEqual((app_id, key), expected)

    def test_release_asset_names_come_from_the_publish_job_env_not_a_job_output(self):
        # A job output that contains a masked value (a profile id equal to its
        # link scheme, say) is dropped by the runner; the publish job's own
        # site-profile step exports the prefix into env instead.
        self.assertNotIn('asset-prefix: ${{ steps.resolve.outputs.asset-prefix }}', self.workflow)
        self.assertNotIn('needs.version.outputs.asset-prefix', self.workflow)
        self.assertGreater(self.workflow.count('release-assets/${{ env.ASSET_PREFIX }}-'), 40)

    def test_profile_release_notes_come_from_the_unreleased_section(self):
        # A profile's version numbers are its own repository's, so a
        # `## v<version>` section of this changelog is another release's.
        self.assertIn('if [ -n "$PROFILE" ]; then notes_args+=(--unreleased); fi', self.workflow)
        self.assertIn('python3 scripts/release-notes.py "$version" --check ${notes_args[@]+"${notes_args[@]}"}', self.workflow)
        self.assertIn('if [ -n "$PROFILE" ]; then args+=(--unreleased); fi', self.workflow)
        self.assertIn('--output release-notes.md ${args[@]+"${args[@]}"}', self.workflow)

    def test_publish_requires_every_build_and_positive_test_evidence(self):
        required = ('version', 'ci-status', 'release-core', 'build', 'build-linux', 'build-windows')
        for states in itertools.product(STATES, repeat=len(required) + 1):
            results = dict(zip((*required, 'pre-release'), states))
            for tested, cancelled in itertools.product(('true', 'false'), (True, False)):
                context = {f'needs.{job}.result': result for job, result in results.items()}
                context['needs.ci-status.outputs.tested'] = tested
                expected = (not cancelled and all(results[job] == 'success' for job in required)
                            and (results['pre-release'] == 'success'
                                 or (tested == 'true' and results['pre-release'] == 'skipped')))
                actual = self.publish_gate(context, cancelled)
                if actual != expected:
                    self.fail(f'Publication gate mismatch: {results}, tested={tested}, cancelled={cancelled}')

    def test_fallback_checks_run_only_with_ready_core_and_no_main_evidence(self):
        gate = Gate(condition(block(self.workflow, 'pre-release:', 2), 4))
        for version, ci, core, tested, cancelled in itertools.product(
                STATES, STATES, STATES, ('true', 'false'), (True, False)):
            context = {'needs.version.result': version, 'needs.ci-status.result': ci,
                       'needs.release-core.result': core, 'needs.ci-status.outputs.tested': tested}
            expected = (not cancelled and version == ci == core == 'success' and tested != 'true')
            self.assertEqual(gate(context, cancelled), expected, context)

    def test_prepare_only_cannot_publish_or_promote_changelog(self):
        release_step = block(self.publish, '- name: Create the release', 6)
        release_gate = Gate(condition(release_step, 8))
        changelog_gate = Gate(condition(block(self.workflow, 'changelog:', 2), 4))
        for prepare, cancelled, status, channel, event, profile, feed in itertools.product((True, False), (True, False), STATES, ('stable', 'beta'), ('push', 'workflow_dispatch', 'workflow_run'), ('', 'acme-7'), ('', 'github:acme/desktop-releases', 'cloud')):
            context = {'inputs.prepare-only': prepare, 'needs.publish.result': status,
                       'needs.version.outputs.channel': channel, 'github.event_name': event,
                       'inputs.profile': profile, 'env.RELEASE_FEED': feed}
            # A cloud feed never creates a GitHub release.
            self.assertEqual(release_gate(context), not prepare and event != 'workflow_run' and feed != 'cloud')
            # The changelog is the default profile's: a profile's publish never promotes it.
            self.assertEqual(changelog_gate(context, cancelled),
                             not prepare and event != 'workflow_run' and not cancelled and status == 'success' and channel == 'stable' and not profile)

    def test_only_preparation_lists_a_feed_without_its_route_as_empty(self):
        # Prepare-only and the automatic workflow_run pass --missing-feed-empty;
        # a publish run never does, so a 404 there still fails the listing.
        steps = yaml.safe_load(self.workflow)['jobs']['version']['steps']
        step = next(step for step in steps if step.get('name') == 'List the cloud feed')
        expression = step['env']['PREPARE_ONLY'].removeprefix('${{').removesuffix('}}').strip()
        gate = Gate(expression)
        for prepare, event in itertools.product((True, False, None), ('push', 'workflow_dispatch', 'workflow_run')):
            context = {'inputs.prepare-only': prepare, 'github.event_name': event}
            self.assertEqual(gate(context), event == 'workflow_run' or bool(prepare), context)
        self.assertIn('set -euo pipefail', step['run'])
        with tempfile.TemporaryDirectory() as temp:
            fake = Path(temp) / 'python3'
            fake.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
            fake.chmod(0o755)
            for value, flagged in (('true', True), ('false', False), ('', False)):
                with self.subTest(prepare_only=value):
                    env = dict(os.environ, PATH=f"{temp}:{os.environ['PATH']}", PREPARE_ONLY=value,
                               SITE_URL='https://acme.example', PROFILE='acme7', RELEASE_FEED='cloud')
                    result = subprocess.run(['bash', '-c', step['run']], env=env, capture_output=True,
                                            text=True, check=True)
                    argv = result.stdout.split()
                    self.assertEqual(argv[:2], ['scripts/desktop-feed-publish.py', 'list'])
                    self.assertEqual('--missing-feed-empty' in argv, flagged)
                    self.assertIn('feed-listing.json', argv)

    def test_resolve_logs_manifest_source_and_automatic_ci_run(self):
        resolve = block(block(self.workflow, 'version:', 2), '- name: Resolve', 6)
        self.assertIn('SOURCE_SHA: ${{ steps.source.outputs.sha }}', resolve)
        self.assertIn('CI_RUN_ID: ${{ inputs.ci-run-id || github.event.workflow_run.id }}', resolve)
        script = textwrap.dedent(resolve.split('        run: |\n', 1)[1])
        logging = script[script.index('echo "Releasing v$version"'):]
        # The lines the host's tick.sh and release.sh read stay as they are;
        # a profile adds its opaque id after the source, never its name.
        for event, ci_id, profile in [('workflow_run', '456', ''), ('workflow_dispatch', '', ''), ('push', '', ''),
                                      ('workflow_dispatch', '', 'acme-7')]:
            env = dict(os.environ, EVENT_NAME=event, SOURCE_SHA='a' * 40,
                       CI_RUN_ID=ci_id, version='2026.11.1', PROFILE=profile)
            result = subprocess.run(['bash', '-eu', '-c', logging], env=env,
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            expected = ['Releasing v2026.11.1', 'Source ' + 'a' * 40]
            if profile:
                expected.append('Profile acme-7')
            if event == 'workflow_run':
                expected.append('CI run 456')
            self.assertEqual(result.stdout.splitlines(), expected)
        preparation = block(self.publish, '- name: Prepare verified release candidate', 6)
        self.assertIn('SOURCE_SHA: ${{ needs.version.outputs.source-sha }}', preparation)
        self.assertIn('CI_RUN_ID: ${{ inputs.ci-run-id || github.event.workflow_run.id }}', preparation)

    def test_automatic_preparation_only_accepts_successful_main_pushes(self):
        gate = Gate(condition(block(self.workflow, 'version:', 2), 4))
        for event, conclusion, upstream_event, branch, repo in itertools.product(
                ('push', 'workflow_dispatch', 'workflow_run'), STATES,
                ('push', 'pull_request', 'workflow_dispatch'), ('main', 'topic'),
                ('AgentWorkforce/relay-app', 'fork/repo')):
            context = {'github.event_name': event,
                       'github.event.workflow_run.conclusion': conclusion,
                       'github.event.workflow_run.event': upstream_event,
                       'github.event.workflow_run.head_branch': branch,
                       'github.event.workflow_run.head_repository.full_name': repo,
                       'github.repository': 'AgentWorkforce/relay-app'}
            expected = (event != 'workflow_run' or
                        (conclusion == 'success' and upstream_event == 'push' and
                         branch == 'main' and repo == 'AgentWorkforce/relay-app'))
            self.assertEqual(gate(context), expected, context)

    def test_every_release_checkout_and_package_uses_selected_exact_source(self):
        refs = re.findall(r'uses: actions/checkout@[^\n]+\n        with:\n          ref: (.+)', self.workflow)
        self.assertTrue(refs)
        self.assertEqual(refs[0], '${{ steps.source.outputs.sha }}')
        self.assertEqual(refs.count('main'), 1)  # trusted evidence verifier only
        self.assertTrue(all(ref in ('main', '${{ needs.version.outputs.source-sha }}') for ref in refs[1:]))
        self.assertEqual(len(refs), self.workflow.count('uses: actions/checkout@'))
        self.assertIn('github.event.workflow_run.head_sha || github.sha', self.workflow)
        self.assertNotIn('--source-sha "$GITHUB_SHA"', self.workflow)
        for arch in ('x64', 'arm64'):
            package = block(self.workflow, f'package-windows-{arch}:', 2)
            self.assertIn("--source-sha '${{ needs.version.outputs.source-sha }}'", package)
        linux = block(self.workflow, 'build-linux:', 2)
        self.assertIn('source-sha: ${{ needs.version.outputs.source-sha }}', linux)
        core = block(self.workflow, 'release-core:', 2)
        self.assertIn('name: ci-linux-core-${{ matrix.arch }}-${{ needs.version.outputs.source-sha }}', core)

    def test_every_job_that_checks_out_code_keeps_contents_read(self):
        # A permissions block that lists scopes without contents: read leaves
        # actions/checkout unable to read this private repository (the
        # ci-status evidence verifier failed that way: "repository not found").
        for path in (WORKFLOW, PROMOTE):
            doc = yaml.safe_load(path.read_text())
            for job_id, job in doc['jobs'].items():
                steps = job.get('steps') or []
                if not any(str(step.get('uses', '')).startswith('actions/checkout')
                           for step in steps):
                    continue
                perms = job.get('permissions', doc.get('permissions'))
                if perms is None:
                    continue  # default token, which may read contents
                self.assertIsInstance(perms, dict, f'{path.name} job {job_id}')
                self.assertIn(
                    perms.get('contents'), ('read', 'write'),
                    f'{path.name} job {job_id} checks out code without contents: read')

    def test_publication_is_locked_per_profile(self):
        # One publication of a profile at a time, in Desktop Release and
        # Desktop Promote alike; another profile's never waits on it, and an
        # automatic candidate has its own lock per commit.
        release = block(self.workflow, 'concurrency:', 0).splitlines()
        self.assertEqual(release, [
            'concurrency:',
            "  group: ${{ github.event_name == 'workflow_run' && format('desktop-candidate-{0}', "
            "github.event.workflow_run.head_sha) || (inputs.request-id && inputs.prepare-only && "
            "format('desktop-cloud-candidate-{0}-{1}-{2}', inputs.profile, github.sha, inputs.channel)) "
            "|| (inputs.profile && format('desktop-release-{0}', "
            "inputs.profile)) || 'desktop-release' }}",
            '  cancel-in-progress: false',
        ])
        promote = block(self.promote, 'concurrency:', 0).splitlines()
        self.assertEqual(promote, [
            'concurrency:',
            "  group: ${{ inputs.profile && format('desktop-release-{0}', inputs.profile) || 'desktop-release' }}",
            '  cancel-in-progress: false',
        ])

    def test_automatic_preparation_does_not_share_publication_concurrency(self):
        concurrency = block(self.workflow, 'concurrency:', 0)
        self.assertIn("format('desktop-candidate-{0}'", concurrency)
        self.assertIn("|| 'desktop-release'", concurrency)
        self.assertIn("github.event.workflow_run.head_sha)", concurrency)
        self.assertIn('cancel-in-progress: false', concurrency)

    def test_no_notes_skip_gates_every_downstream_job_including_aggregates(self):
        for job in ('ci-status', 'release-core', 'build', 'build-linux',
                    'compile-windows-x64', 'compile-windows-arm64',
                    'package-windows-x64', 'package-windows-arm64',
                    'build-windows', 'pre-release', 'publish', 'changelog'):
            text = block(self.workflow, job + ':', 2)
            gate = Gate(condition(text, 4))
            context = {ref: 'success' for ref in gate.references}
            context.update({'needs.version.outputs.skip': 'true',
                            'needs.ci-status.outputs.tested': 'true',
                            'needs.version.outputs.channel': 'stable',
                            'inputs.prepare-only': False, 'github.event_name': 'workflow_dispatch'})
            self.assertFalse(gate(context), job)
        version = block(self.workflow, 'version:', 2)
        self.assertIn('name: Resolve version', version)
        self.assertIn('skip: ${{ steps.resolve.outputs.skip }}', version)
        config = block(version, '- name: Require analytics release configuration', 6)
        self.assertEqual(condition(config, 8), "steps.resolve.outputs.skip != 'true' && !inputs.profile")

    def test_resolve_skips_only_empty_notes_on_automatic_preparation(self):
        checkout = WORKFLOW.parents[2]
        resolve = block(block(self.workflow, 'version:', 2), '- name: Resolve', 6)
        script = textwrap.dedent(resolve.split('        run: |\n', 1)[1])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'scripts').mkdir()
            for name in ('release-notes.py', 'macos-release-assets.py', 'release-version.py', 'test_release_version.py'):
                shutil.copyfile(checkout / 'scripts' / name, root / 'scripts' / name)
            gh = root / 'gh'
            gh.write_text('#!/bin/sh\nprintf \'[[]]\n\'\n')
            gh.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}{os.pathsep}' + os.environ['PATH'],
                       TAG_NAME='', INPUT_VERSION='2026.11.1', INPUT_CHANNEL='stable',
                       RELEASES_REPOSITORY='distribution/repo', SOURCE_SHA='a' * 40,
                       CI_RUN_ID='456', GITHUB_OUTPUT=str(root / 'output'),
                       PROFILE='', ASSET_PREFIX='AgentRelay')
            for event, notes, expected in [
                    ('workflow_run', '## Unreleased\n\n', 0),
                    ('workflow_dispatch', '## Unreleased\n\n', 3),
                    ('workflow_run', '## Unreleased\n\n## Unreleased\n\n', 1),
                    ('workflow_run', '## Unreleased\n\n- A change.\n', 0)]:
                env['EVENT_NAME'] = event
                (root / 'CHANGELOG.md').write_text(notes)
                (root / 'output').write_text('')
                result = subprocess.run(['bash', '-c', script], cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                output = (root / 'output').read_text()
                self.assertIn('profile=\n', output)
                self.assertNotIn('asset-prefix=', output)
                if event == 'workflow_run' and notes == '## Unreleased\n\n':
                    self.assertIn('skip=true', output)
                    self.assertIn('::notice::No release note bullets for Source ' + 'a' * 40, result.stdout)
                elif expected == 0:
                    self.assertIn('skip=false', output)

    def test_branding_archive_members_must_be_regular_files(self):
        # The allow-list checks names; a symlink named icon.svg would be read
        # through, so every member must be a regular file and only the
        # allowed names are extracted.
        action = yaml.safe_load(SITE_PROFILE.read_text())
        [step] = [step for step in action['runs']['steps'] if 'BRANDING_URL' in step.get('env', {})]
        run = step['run']
        self.assertIn('tar -xzf "$archive" -C branding -- ${allowed[@]+"${allowed[@]}"}', run)
        self.assertIn('[ -L "branding/$name" ]', run)
        function = re.search(r'^branding_members_ok\(\) \{.*?^\}$', run, re.S | re.M).group(0)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            good, bad = root / 'good', root / 'bad'
            for directory in (good, bad):
                directory.mkdir()
            (good / 'icon.svg').write_text('<svg/>')
            (bad / 'target').write_text('<svg/>')
            (bad / 'icon.svg').symlink_to('target')
            archives = {}
            for name, directory, members in (('good', good, ['.']), ('bad', bad, ['icon.svg'])):
                archives[name] = root / (name + '.tar.gz')
                subprocess.run(['tar', '-czf', str(archives[name]), '-C', str(directory)] + members, check=True)
            def check(archive):
                return subprocess.run(['bash', '-c', function + '\nbranding_members_ok "$1"', 'bash', str(archive)],
                                      capture_output=True, text=True)
            result = check(archives['good'])
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = check(archives['bad'])
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('::error::The branding archive member icon.svg is not a regular file', result.stdout)

    def test_branding_token_is_minted_from_the_org_app_only_when_branding_is_wanted(self):
        # DESKTOP_BRANDING_TOKEN overrides; otherwise the organization's App
        # mints a read-only token for desktop-branding, and only for a profile
        # whose branding URL is in that repository (never sent elsewhere).
        action = yaml.safe_load(SITE_PROFILE.read_text())
        steps = action['runs']['steps']
        names = [step['name'] for step in steps]
        self.assertLess(names.index('Mint a branding token'), names.index('Fetch branding'))
        [mint] = [step for step in steps if step['name'] == 'Mint a branding token']
        self.assertEqual(mint['uses'], 'actions/create-github-app-token@64ab729a6294000f5aa4a4bc0b73d1f32bc45510')
        self.assertEqual(mint['with'], {
            'app-id': '${{ inputs.branding-app-id }}',
            'private-key': '${{ inputs.branding-app-private-key }}',
            'owner': 'AgentWorkforce', 'repositories': 'desktop-branding', 'permission-contents': 'read'})
        condition = ' '.join(mint['if'].split())
        for clause in ("inputs.profile != ''", "inputs.branding-token == ''", "inputs.branding-app-id != ''",
                       "inputs.branding-app-private-key != ''",
                       "startsWith(inputs.branding-url, 'https://raw.githubusercontent.com/AgentWorkforce/desktop-branding/')",
                       "startsWith(inputs.branding-url, 'https://api.github.com/repos/AgentWorkforce/desktop-branding/contents/')"):
            self.assertIn(clause, condition)
        [fetch] = [step for step in steps if step['name'] == 'Fetch branding']
        self.assertEqual(fetch['env']['BRANDING_TOKEN'],
                         '${{ inputs.branding-token || steps.branding-app-token.outputs.token }}')
        for name in ('branding-app-id', 'branding-app-private-key'):
            self.assertEqual(action['inputs'][name]['default'], '')
        # Every build job that fetches branding passes the App's org secrets.
        for job in ('build', 'build-linux', 'compile-windows-x64', 'compile-windows-arm64'):
            text = block(self.workflow, job + ':', 2)
            self.assertIn('branding-token: ${{ secrets.DESKTOP_BRANDING_TOKEN }}', text, job)
            self.assertIn('branding-app-id: ${{ secrets.GH_APP_PUSHER_ID }}', text, job)
            self.assertIn('branding-app-private-key: ${{ secrets.GH_APP_PUSHER_PRIVATE_KEY }}', text, job)
        self.assertEqual(self.workflow.count('secrets.GH_APP_PUSHER_ID'), 4)

    def test_branding_fetch_accepts_raw_and_contents_api_urls(self):
        action = yaml.safe_load(SITE_PROFILE.read_text())
        [fetch] = [step for step in action['runs']['steps'] if step['name'] == 'Fetch branding']
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, bin_dir, work = root / 'source', root / 'bin', root / 'work'
            for directory in (source, bin_dir, work):
                directory.mkdir()
            (source / 'icon.svg').write_text('<svg/>')
            archive = root / 'branding.tar.gz'
            subprocess.run(['tar', '-czf', str(archive), '-C', str(source), 'icon.svg'], check=True)
            # A curl that records its arguments and writes the archive to -o.
            curl = bin_dir / 'curl'
            curl.write_text('#!/usr/bin/env bash\nprintf "%s\\n" "$@" > "$CURL_ARGS"\n'
                            'while [ $# -gt 0 ]; do if [ "$1" = -o ]; then cp "$ARCHIVE" "$2"; fi; shift; done\n')
            curl.chmod(0o755)
            def fetch_with(url, token):
                env = dict(os.environ, PATH=f'{bin_dir}:{os.environ["PATH"]}', BRANDING_URL=url, BRANDING_TOKEN=token,
                           GITHUB_ENV=str(root / 'env'), CURL_ARGS=str(root / 'args'), ARCHIVE=str(archive))
                result = subprocess.run(['bash', '-c', fetch['run']], cwd=work, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual((work / 'branding' / 'icon.svg').read_text(), '<svg/>')
                return (root / 'args').read_text().splitlines()
            raw = 'https://raw.githubusercontent.com/AgentWorkforce/desktop-branding/main/p1.tar.gz'
            args = fetch_with(raw, 'minted')
            self.assertIn('Authorization: Bearer minted', args)
            self.assertNotIn('Accept: application/vnd.github.raw', args)
            self.assertEqual(args[-3], raw)
            api = 'https://api.github.com/repos/AgentWorkforce/desktop-branding/contents/p1.tar.gz'
            args = fetch_with(api, 'minted')
            self.assertIn('Authorization: Bearer minted', args)
            self.assertIn('Accept: application/vnd.github.raw', args)
            args = fetch_with('https://branding.example/p1.tar.gz', '')
            self.assertFalse(any(arg.startswith('Authorization') for arg in args))

    def resolve_site_profile(self, profile, **inputs):
        """Run the site-profile action's Resolve step as the runner would:
        its run: block under bash from the repository root (it sources
        scripts/site-origin.sh), with GITHUB_ENV and GITHUB_OUTPUT files and
        the step's PROFILE and IN_* environment. Returns the result, the
        exported lines and the masked values."""
        action = yaml.safe_load(SITE_PROFILE.read_text())
        [step] = [step for step in action['runs']['steps'] if step['name'] == 'Resolve the site profile']
        self.assertEqual(step['shell'], 'bash')
        names = {name for name in step['env'] if name.startswith('IN_')}
        self.assertEqual(names, {'IN_' + name.upper().replace('-', '_') for name in inputs} | names)
        checkout = SITE_PROFILE.parents[3]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            env = {name: value for name, value in os.environ.items()
                   if name not in step['env'] and not name.startswith('GITHUB_')}
            env.update({name: '' for name in names}, PROFILE=profile, GITHUB_WORKSPACE=str(checkout),
                       GITHUB_ENV=str(root / 'env'), GITHUB_OUTPUT=str(root / 'output'))
            env.update({'IN_' + name.upper().replace('-', '_'): value for name, value in inputs.items()})
            (root / 'env').write_text('')
            (root / 'output').write_text('')
            result = subprocess.run(['bash', '-c', step['run']], cwd=checkout, env=env, capture_output=True, text=True)
            exported = (root / 'env').read_text().splitlines()
        masks = [line.removeprefix('::add-mask::') for line in result.stdout.splitlines() if line.startswith('::add-mask::')]
        return result, exported, masks

    PROFILE_INPUTS = dict(site_url='  HTTPS://Relay.Example/ ', url_scheme=' Acme-Relay ', product_name='  Acme Desktop ',
                          bundle_id=' com.acme.desktop ', releases_repository=' acme/desktop-releases ',
                          support_url=' https://acme.example/support ', branding_url='https://acme.example/branding.tar.gz')

    def test_site_profile_action_exports_the_default_profile_unmasked(self):
        result, exported, masks = self.resolve_site_profile('')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(exported, [
            'PROFILE=', 'ASSET_PREFIX=AgentRelay', 'SITE_URL=https://agentrelay.com', 'URL_SCHEME=agentrelay',
            'PRODUCT_NAME=Agent Relay', 'BUNDLE_ID=com.agentrelay.desktop',
            'RELEASES_REPOSITORY=AgentWorkforce/relay-desktop-releases', 'PULL_RELEASES_REPOSITORY=', 'SUPPORT_URL=',
            'BRANDING_ICON=', 'BRANDING_COBRAND_MARK=', 'COBRAND_NAME=',
            'BRANDING_WANTED=', 'RELEASE_FEED=github:AgentWorkforce/relay-desktop-releases', 'FEED_PUBLIC_KEY=',
            'DESKTOP_POLICY_DEFAULTS=', 'AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS=', 'COEXIST='])
        self.assertEqual(masks, [])
        self.assertNotIn('::add-mask::', result.stdout + result.stderr)

    def test_site_profile_action_exports_a_profiles_trimmed_values_and_masks_each(self):
        result, exported, masks = self.resolve_site_profile(' acme-7 ', **self.PROFILE_INPUTS)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(exported, [
            'PROFILE=acme-7', 'ASSET_PREFIX=Desktop-acme-7', 'SITE_URL=https://relay.example', 'URL_SCHEME=acme-relay',
            'PRODUCT_NAME=Acme Desktop', 'BUNDLE_ID=com.acme.desktop', 'RELEASES_REPOSITORY=acme/desktop-releases',
            'PULL_RELEASES_REPOSITORY=', 'SUPPORT_URL=https://acme.example/support', 'BRANDING_ICON=',
            'BRANDING_COBRAND_MARK=', 'COBRAND_NAME=', 'BRANDING_WANTED=yes',
            'RELEASE_FEED=github:acme/desktop-releases', 'FEED_PUBLIC_KEY=', 'DESKTOP_POLICY_DEFAULTS=',
            'AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS=', 'COEXIST='])
        # One mask per non-empty derived value, before any line names them;
        # the log names the profile by its id only.
        self.assertEqual(sorted(masks), sorted([
            'https://relay.example', 'acme-relay', 'Acme Desktop', 'acme/desktop-releases', 'com.acme.desktop',
            'https://acme.example/support']))
        self.assertTrue(result.stdout.endswith('Profile acme-7\n'), result.stdout)
        unmasked = result.stdout.replace(''.join(f'::add-mask::{value}\n' for value in masks), '')
        self.assertEqual(unmasked, 'Profile acme-7\n')
        # The optional values unset: nothing masked for them, and no branding.
        inputs = dict(self.PROFILE_INPUTS, support_url='', branding_url='')
        result, exported, masks = self.resolve_site_profile('acme-7', **inputs)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('SUPPORT_URL=', exported)
        self.assertIn('BRANDING_WANTED=', exported)
        self.assertEqual(len(masks), 5)
        self.assertNotIn('', masks)

    def test_site_profile_action_refuses_each_bad_value_before_exporting_or_masking(self):
        for name, profile, change, error in [
                ('missing secret', 'acme-7', dict(bundle_id=''),
                 '::error::The desktop-acme-7 environment does not set the secret DESKTOP_BUNDLE_ID (docs/workspace-layout.md, Site profiles).'),
                ('bad id', 'acme--7', {}, '::error::The profile must be an opaque id of lowercase letters and digits'),
                ('non-https site', 'acme-7', dict(site_url='http://relay.example'),
                 '::error::AGENT_RELAY_SITE_URL must be an https origin such as https://relay.example.'),
                ('local http site', 'acme-7', dict(site_url='http://localhost:3100'),
                 '::error::AGENT_RELAY_SITE_URL must be an https origin; a profile is never a local site.'),
                ('id containing a value', 'acme-relay-7', {},
                 '::error::The profile id must not contain one of the profile\'s own values (its link scheme or product name, for example).'),
                ('bad bundle id', 'acme-7', dict(bundle_id='com..acme'),
                 '::error::DESKTOP_BUNDLE_ID must be a reverse-DNS identifier such as com.example.desktop.'),
                ('bundle id over 32', 'acme-7', dict(bundle_id='com.acme.' + 'd' * 24),
                 '::error::DESKTOP_BUNDLE_ID must be at most 32 characters.'),
                ('product name with a refused character', 'acme-7', dict(product_name='Acme: Desktop'),
                 '::error::DESKTOP_PRODUCT_NAME must be a name with no control characters, slashes, quotes or the characters : * ? < > | { } , (it names a Windows shortcut and installer).'),
                # A profile never reuses the default product's values, in any
                # case or spacing: with its repository it would publish into
                # the public feed.
                ('the default releases repository', 'acme-7',
                 dict(releases_repository=' agentworkforce/Relay-Desktop-Releases '),
                 "::error::RELEASES_REPOSITORY must not be the default product's releases repository; a profile publishes to its own."),
                ('the default site', 'acme-7', dict(site_url=' HTTPS://AgentRelay.com/ '),
                 "::error::AGENT_RELAY_SITE_URL must not be the default product's site; a profile signs in to its own."),
                ('the default scheme', 'acme-7', dict(url_scheme=' AgentRelay '),
                 "::error::AGENT_RELAY_URL_SCHEME must not be the default product's link scheme; a profile opens its own links."),
                ('the default bundle id', 'acme-7', dict(bundle_id='Com.AgentRelay.Desktop'),
                 "::error::DESKTOP_BUNDLE_ID must not be the default product's bundle identifier; a profile's Mac app is its own.")]:
            with self.subTest(name):
                result, exported, masks = self.resolve_site_profile(profile, **dict(self.PROFILE_INPUTS, **change))
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(any(line.startswith(error) for line in result.stdout.splitlines()), result.stdout)
                self.assertEqual(exported, [])
                self.assertEqual(masks, [])

    POLICY_DEFAULTS = ' { "upload_control": "operator", "sharing_mode": "all", "oops_window_seconds": 600,\n' \
                      '   "user_retroactive_delete": false } '
    POLICY_CANONICAL = ('{"oops_window_seconds":600,"sharing_mode":"all","upload_control":"operator",'
                        '"user_retroactive_delete":false}')

    def test_site_profile_action_exports_policy_defaults_checked_and_masked(self):
        result, exported, masks = self.resolve_site_profile(
            'acme-7', **dict(self.PROFILE_INPUTS, policy_defaults=self.POLICY_DEFAULTS))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # Compact with its keys sorted, as desktop-core's build.rs passes it on.
        self.assertEqual(exported[-3], 'DESKTOP_POLICY_DEFAULTS=' + self.POLICY_CANONICAL)
        self.assertIn(self.POLICY_CANONICAL, masks)
        self.assertEqual(len(masks), 7)
        unmasked = result.stdout.replace(''.join(f'::add-mask::{value}\n' for value in masks), '')
        self.assertEqual(unmasked, 'Profile acme-7\n')
        self.assertEqual(result.stderr, '')
        # Whitespace only is no defaults: nothing exported, nothing masked.
        result, exported, masks = self.resolve_site_profile(
            'acme-7', **dict(self.PROFILE_INPUTS, policy_defaults=' \n '))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(exported[-3], 'DESKTOP_POLICY_DEFAULTS=')
        self.assertEqual(len(masks), 6)
        # The default profile never reads it, valid or not.
        result, exported, masks = self.resolve_site_profile('', policy_defaults='{ not json')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(exported[-3], 'DESKTOP_POLICY_DEFAULTS=')
        self.assertEqual(masks, [])

    def test_site_profile_action_refuses_bad_policy_defaults_naming_only_the_secret(self):
        error = ('::error::DESKTOP_POLICY_DEFAULTS must be a JSON object setting only sharing_mode (all, new or '
                 'selected), upload_control (operator or user), oops_window_seconds (0 to 86400) and '
                 'user_retroactive_delete (true or false) (docs/workspace-layout.md, Site profiles).')
        for name, value in [
                ('invalid JSON', '{ "sharing_mode": "all"'),
                ('not an object', '["sharing_mode", "all"]'),
                ('a string', '"all"'),
                ('an unknown key', '{"sharing_mode": "all", "colour_secret": "red"}'),
                ('a bad sharing mode', '{"sharing_mode": "bogus_mode"}'),
                ('a bad upload control', '{"upload_control": "admin_only"}'),
                ('a window out of range', '{"oops_window_seconds": 86401}'),
                ('a negative window', '{"oops_window_seconds": -5}'),
                ('a fractional window', '{"oops_window_seconds": 600.5}'),
                ('a boolean window', '{"oops_window_seconds": true}'),
                ('a window as a string', '{"oops_window_seconds": "600"}'),
                ('NaN', '{"oops_window_seconds": NaN}'),
                ('a delete that is no boolean', '{"user_retroactive_delete": "false"}')]:
            with self.subTest(name):
                result, exported, masks = self.resolve_site_profile(
                    'acme-7', **dict(self.PROFILE_INPUTS, policy_defaults=value))
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(result.stdout.splitlines(), [error])
                self.assertEqual(result.stderr, '')
                self.assertEqual(exported, [])
                self.assertEqual(masks, [])

    STORAGE_ORIGINS = ' [ "HTTPS://MinIO.Corp.Example:9000/", "https://store.example:443",\n' \
                      '   "https://minio.corp.example:9000" ] '
    STORAGE_CANONICAL = '["https://minio.corp.example:9000","https://store.example"]'

    def test_site_profile_action_exports_upload_storage_origins_checked_and_masked(self):
        result, exported, masks = self.resolve_site_profile(
            'acme-7', **dict(self.PROFILE_INPUTS, history_upload_storage_origins=self.STORAGE_ORIGINS))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # Canonical, folded and sorted, as desktop-core's build.rs passes it on.
        self.assertEqual(exported[-2], 'AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS=' + self.STORAGE_CANONICAL)
        self.assertIn(self.STORAGE_CANONICAL, masks)
        self.assertEqual(len(masks), 7)
        unmasked = result.stdout.replace(''.join(f'::add-mask::{value}\n' for value in masks), '')
        self.assertEqual(unmasked, 'Profile acme-7\n')
        self.assertEqual(result.stderr, '')
        # Whitespace only, or an empty list, is none: nothing exported or masked.
        for none in [' \n ', ' [ ] ']:
            result, exported, masks = self.resolve_site_profile(
                'acme-7', **dict(self.PROFILE_INPUTS, history_upload_storage_origins=none))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(exported[-2], 'AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS=')
            self.assertEqual(len(masks), 6)
        # The default profile never reads it, valid or not.
        result, exported, masks = self.resolve_site_profile('', history_upload_storage_origins='[ not json')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(exported[-2], 'AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS=')
        self.assertEqual(masks, [])

    def test_site_profile_action_refuses_bad_upload_storage_origins_naming_only_the_secret(self):
        error = ('::error::DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS must be a JSON array of https origins on DNS names, '
                 'such as https://store.example:9000, with no user info, path, query or fragment '
                 '(docs/workspace-layout.md, Site profiles).')
        for name, value in [
                ('invalid JSON', '["https://secret-store.example"'),
                ('not an array', '{"origin": "https://secret-store.example"}'),
                ('a string', '"https://secret-store.example"'),
                ('a number', '[443]'),
                ('http', '["http://secret-store.example"]'),
                ('no scheme', '["secret-store.example"]'),
                ('user info', '["https://user@secret-store.example"]'),
                ('a path', '["https://secret-store.example/bucket"]'),
                ('a query', '["https://secret-store.example?x=1"]'),
                ('a fragment', '["https://secret-store.example#x"]'),
                ('an IPv4 literal', '["https://10.0.0.5:9000"]'),
                ('a short IPv4 literal', '["https://127.1"]'),
                ('a hex IPv4 literal', '["https://0x7f000001"]'),
                ('an IPv6 literal', '["https://[fd12::1]:9000"]'),
                ('a trailing dot', '["https://secret-store.example."]'),
                ('port 0', '["https://secret-store.example:0"]'),
                ('a port out of range', '["https://secret-store.example:65536"]'),
                ('an empty port', '["https://secret-store.example:"]')]:
            with self.subTest(name):
                result, exported, masks = self.resolve_site_profile(
                    'acme-7', **dict(self.PROFILE_INPUTS, history_upload_storage_origins=value))
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(result.stdout.splitlines(), [error])
                self.assertEqual(result.stderr, '')
                self.assertNotIn('secret-store', result.stdout)
                self.assertEqual(exported, [])
                self.assertEqual(masks, [])

    FEED_KEY = '11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo='

    def test_site_profile_action_exports_a_github_feed_naming_the_repository(self):
        # A github: feed names RELEASES_REPOSITORY, as GitHub compares names;
        # it exports the repository's own spelling.
        result, exported, masks = self.resolve_site_profile(
            'acme-7', **dict(self.PROFILE_INPUTS, release_feed=' github:Acme/Desktop-Releases '))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(exported[-6:], ['BRANDING_WANTED=yes', 'RELEASE_FEED=github:acme/desktop-releases', 'FEED_PUBLIC_KEY=',
                                         'DESKTOP_POLICY_DEFAULTS=', 'AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS=', 'COEXIST='])
        self.assertEqual(len(masks), 6)

    def test_site_profile_action_exports_a_cloud_feed_with_its_key_masked(self):
        inputs = dict(self.PROFILE_INPUTS, releases_repository='', release_feed=' cloud ',
                      feed_public_key=f' {self.FEED_KEY} ', feed_publisher_token_set='true')
        result, exported, masks = self.resolve_site_profile('acme-7', **inputs)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('RELEASES_REPOSITORY=', exported)
        self.assertEqual(exported[-5:], ['RELEASE_FEED=cloud', f'FEED_PUBLIC_KEY={self.FEED_KEY}', 'DESKTOP_POLICY_DEFAULTS=',
                                         'AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS=', 'COEXIST='])
        self.assertIn(self.FEED_KEY, masks)
        self.assertNotIn('', masks)
        # The key's fingerprint (SHA-256 of the decoded 32 bytes) is noticed
        # after the masks; the key itself appears only in its mask.
        fingerprint = hashlib.sha256(base64.b64decode(self.FEED_KEY)).hexdigest()
        unmasked = result.stdout.replace(''.join(f'::add-mask::{value}\n' for value in masks), '')
        self.assertEqual(unmasked, f'::notice::Feed public key SHA-256 fingerprint: {fingerprint}\nProfile acme-7\n')
        self.assertEqual(result.stdout.count(self.FEED_KEY), 1)

    def test_site_profile_action_refuses_each_bad_feed_combination(self):
        cloud = dict(self.PROFILE_INPUTS, releases_repository='', release_feed='cloud',
                     feed_public_key=self.FEED_KEY, feed_publisher_token_set='true')
        for name, change, error in [
                ('junk feed', dict(release_feed='https://relay.example'),
                 '::error::RELEASE_FEED must be github:<owner>/<name> (the profile\'s RELEASES_REPOSITORY) or cloud.'),
                ('github: feed naming another repository', dict(release_feed='github:acme/other'),
                 "::error::RELEASE_FEED must name the profile's RELEASES_REPOSITORY"),
                ('github: feed with a key', dict(feed_public_key=self.FEED_KEY),
                 '::error::DESKTOP_FEED_PUBLIC_KEY must be unset for a github: feed'),
                ('github: feed with a publisher token', dict(feed_publisher_token_set='true'),
                 '::error::DESKTOP_FEED_PUBLISHER_TOKEN must be unset for a github: feed'),
                ('bad token flag', dict(feed_publisher_token_set='maybe'),
                 '::error::feed-publisher-token-set must be true or false.'),
                ('cloud feed with a repository', dict(cloud, releases_repository='acme/desktop-releases'),
                 '::error::RELEASES_REPOSITORY must be unset for a cloud feed'),
                ('cloud feed without a key', dict(cloud, feed_public_key=''),
                 '::error::The desktop-acme-7 environment does not set the secret DESKTOP_FEED_PUBLIC_KEY'),
                ('cloud feed without a publisher token', dict(cloud, feed_publisher_token_set='false'),
                 '::error::The desktop-acme-7 environment does not set the secret DESKTOP_FEED_PUBLISHER_TOKEN'),
                ('cloud feed with a bad key', dict(cloud, feed_public_key='11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURp='),
                 '::error::DESKTOP_FEED_PUBLIC_KEY must be an Ed25519 public key'),
                ('cloud feed on the default site', dict(cloud, site_url='https://agentrelay.com'),
                 "::error::AGENT_RELAY_SITE_URL must not be the default product's site")]:
            with self.subTest(name):
                result, exported, masks = self.resolve_site_profile('acme-7', **dict(self.PROFILE_INPUTS, **change))
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(any(line.startswith(error) for line in result.stdout.splitlines()), result.stdout)
                self.assertEqual(exported, [])
                self.assertEqual(masks, [])

    def test_site_profile_action_exports_a_cloud_feeds_pull_repository_masked(self):
        # PULL_RELEASES_REPOSITORY: where Desktop Promote attaches the pull
        # model's attested bundle, beside a cloud feed; trimmed and masked.
        inputs = dict(self.PROFILE_INPUTS, releases_repository='', release_feed='cloud', feed_public_key=self.FEED_KEY,
                      feed_publisher_token_set='true', pull_releases_repository=' acme/desktop-pull ')
        result, exported, masks = self.resolve_site_profile('acme-7', **inputs)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('PULL_RELEASES_REPOSITORY=acme/desktop-pull', exported)
        self.assertIn('RELEASE_FEED=cloud', exported)
        self.assertIn('acme/desktop-pull', masks)
        fingerprint = hashlib.sha256(base64.b64decode(self.FEED_KEY)).hexdigest()
        unmasked = result.stdout.replace(''.join(f'::add-mask::{value}\n' for value in masks), '')
        self.assertEqual(unmasked, f'::notice::Feed public key SHA-256 fingerprint: {fingerprint}\nProfile acme-7\n')

    def test_site_profile_action_refuses_each_bad_pull_repository(self):
        cloud = dict(self.PROFILE_INPUTS, releases_repository='', release_feed='cloud',
                     feed_public_key=self.FEED_KEY, feed_publisher_token_set='true')
        for name, profile, inputs, error in [
                ('the default profile', '', dict(pull_releases_repository='acme/desktop-pull'),
                 "::error::PULL_RELEASES_REPOSITORY is a site profile's setting"),
                ('a github: feed', 'acme-7', dict(self.PROFILE_INPUTS, pull_releases_repository='acme/desktop-pull'),
                 '::error::PULL_RELEASES_REPOSITORY must be unset for a github: feed'),
                ('not owner/name', 'acme-7', dict(cloud, pull_releases_repository='https://example/acme'),
                 '::error::PULL_RELEASES_REPOSITORY must be owner/name.'),
                ('the default releases repository', 'acme-7',
                 dict(cloud, pull_releases_repository='agentworkforce/Relay-Desktop-Releases'),
                 "::error::PULL_RELEASES_REPOSITORY must not be the default product's releases repository")]:
            with self.subTest(name):
                result, exported, masks = self.resolve_site_profile(profile, **inputs)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(any(line.startswith(error) for line in result.stdout.splitlines()), result.stdout)
                self.assertNotIn(inputs['pull_releases_repository'], result.stdout)
                self.assertEqual(exported, [])
                self.assertEqual(masks, [])

    def test_site_profile_action_exports_coexist_only_for_a_profile_that_sets_it(self):
        # DESKTOP_COEXIST, trimmed: true exports COEXIST=yes (build-app.sh
        # --coexist); false or unset exports it empty. It is a variable, not a
        # secret, so it adds no mask.
        for value, want in ((' true ', 'COEXIST=yes'), ('false', 'COEXIST='), ('', 'COEXIST=')):
            with self.subTest(value=value):
                result, exported, masks = self.resolve_site_profile('acme-7', **dict(self.PROFILE_INPUTS, coexist=value))
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(exported[-1], want)
                self.assertEqual(len(masks), 6)

    def test_site_profile_action_refuses_a_bad_coexist_value_or_coexist_for_the_default_profile(self):
        for name, profile, inputs, error in [
                ('not true or false', 'acme-7', dict(self.PROFILE_INPUTS, coexist='yes'),
                 '::error::DESKTOP_COEXIST must be true, false or unset.'),
                ('the default profile', '', dict(coexist='true'),
                 '::error::DESKTOP_COEXIST is a site profile\'s setting; the default profile always replaces other Agent Relay apps.')]:
            with self.subTest(name):
                result, exported, masks = self.resolve_site_profile(profile, **inputs)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(error, result.stdout.splitlines())
                # The error names the setting only, never the value.
                self.assertNotIn(inputs['coexist'], result.stdout.replace(error, ''))
                self.assertEqual(exported, [])
                self.assertEqual(masks, [])

    def test_site_profile_action_exports_a_cobrand_name_trimmed_and_masked(self):
        # DESKTOP_COBRAND_NAME is a secret: trimmed, exported for the macOS
        # build, masked before anything names it, and never printed.
        result, exported, masks = self.resolve_site_profile(
            'acme-7', **dict(self.PROFILE_INPUTS, cobrand_name='  Acme Corp\t'))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('COBRAND_NAME=Acme Corp', exported)
        self.assertIn('BRANDING_COBRAND_MARK=', exported)
        self.assertIn('Acme Corp', masks)
        self.assertNotIn('Acme Corp', result.stdout.replace('::add-mask::Acme Corp\n', '') + result.stderr)
        # Unset, nothing is exported or masked for it.
        result, exported, masks = self.resolve_site_profile('acme-7', **self.PROFILE_INPUTS)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('COBRAND_NAME=', exported)
        self.assertEqual(len(masks), 6)

    def test_site_profile_action_refuses_a_bad_cobrand_name_or_one_for_the_default_profile(self):
        # The "for" line is a self-hosted site profile's only; the default
        # profile is the hosted app and never takes one.
        bad = '::error::DESKTOP_COBRAND_NAME must be one line of at most 80 characters with no control characters.'
        for name, profile, inputs, error in [
                ('the default profile', '', dict(cobrand_name='Acme Corp'),
                 "::error::DESKTOP_COBRAND_NAME is a site profile's setting; the default profile is Agent Relay's own."),
                ('two lines', 'acme-7', dict(self.PROFILE_INPUTS, cobrand_name='Acme\nCorp'), bad),
                ('a control character', 'acme-7', dict(self.PROFILE_INPUTS, cobrand_name='Acme\x07Corp'), bad),
                ('blank', 'acme-7', dict(self.PROFILE_INPUTS, cobrand_name='   '), bad),
                ('over 80', 'acme-7', dict(self.PROFILE_INPUTS, cobrand_name='Acme' + 'x' * 77), bad)]:
            with self.subTest(name):
                result, exported, masks = self.resolve_site_profile(profile, **inputs)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(error, result.stdout.splitlines())
                # The error names the secret only, never its value.
                self.assertNotIn('Acme', result.stdout.replace(error, '') + result.stderr)
                self.assertEqual(exported, [])
                self.assertEqual(masks, [])
        # 80 characters is the limit, not over it.
        result, exported, _ = self.resolve_site_profile('acme-7', **dict(self.PROFILE_INPUTS, cobrand_name='A' * 80))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('COBRAND_NAME=' + 'A' * 80, exported)

    def fetch_branding(self, members, profile='acme-7', cobrand_name=''):
        """Run the Fetch branding step on an archive of MEMBERS (name to
        bytes) with the Resolve step's COBRAND_NAME export. Returns the
        result and the exported lines."""
        action = yaml.safe_load(SITE_PROFILE.read_text())
        [fetch] = [step for step in action['runs']['steps'] if step['name'] == 'Fetch branding']
        self.assertEqual(fetch['env']['PROFILE'], '${{ inputs.profile }}')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, bin_dir, work = root / 'source', root / 'bin', root / 'work'
            for directory in (source, bin_dir, work):
                directory.mkdir()
            for name, content in members.items():
                (source / name).write_bytes(content)
            archive = root / 'branding.tar.gz'
            subprocess.run(['tar', '-czf', str(archive), '-C', str(source)] + sorted(members), check=True)
            curl = bin_dir / 'curl'
            curl.write_text('#!/usr/bin/env bash\n'
                            'while [ $# -gt 0 ]; do if [ "$1" = -o ]; then cp "$ARCHIVE" "$2"; fi; shift; done\n')
            curl.chmod(0o755)
            (root / 'env').write_text('')
            env = dict(os.environ, PATH=f'{bin_dir}:{os.environ["PATH"]}', PROFILE=profile,
                       BRANDING_URL='https://branding.example/p1.tar.gz', BRANDING_TOKEN='',
                       COBRAND_NAME=cobrand_name, GITHUB_ENV=str(root / 'env'), ARCHIVE=str(archive))
            result = subprocess.run(['bash', '-c', fetch['run']], cwd=work, env=env, capture_output=True, text=True)
            return result, (root / 'env').read_text().splitlines()

    SVG = b'<svg xmlns="http://www.w3.org/2000/svg"/>'
    PNG = b'\x89PNG\r\n\x1a\n' + b'\0' * 8

    def test_branding_fetch_exports_a_cobrand_mark_with_or_without_an_icon(self):
        for members, icon, mark in [
                ({'cobrand.png': self.PNG, 'colours.json': b'{}'}, None, 'branding/cobrand.png'),
                ({'cobrand.svg': self.SVG, 'icon.svg': self.SVG}, 'branding/icon.svg', 'branding/cobrand.svg')]:
            with self.subTest(mark):
                result, exported = self.fetch_branding(members, cobrand_name='Acme Corp')
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('BRANDING_COBRAND_MARK=' + mark, exported)
                self.assertEqual(icon is not None, 'BRANDING_ICON=' + str(icon) in exported)
                self.assertNotIn('Acme Corp', result.stdout + result.stderr)
        # Without a co-brand the archive is read as before: no mark exported.
        result, exported = self.fetch_branding({'icon.svg': self.SVG})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(exported, ['BRANDING_ICON=branding/icon.svg', 'BRANDING_COBRAND_MARK='])

    def test_branding_fetch_refuses_each_bad_cobrand_archive(self):
        for name, members, profile, cobrand_name, error in [
                ('the default profile', {'icon.svg': self.SVG, 'cobrand.svg': self.SVG}, '', '',
                 "::error::cobrand.svg and cobrand.png are a site profile's branding; the default profile is Agent Relay's own."),
                ('a mark without the name', {'cobrand.svg': self.SVG}, 'acme-7', '',
                 '::error::The branding archive holds a co-brand mark, but the profile\'s environment does not set the secret DESKTOP_COBRAND_NAME (docs/workspace-layout.md, Site profiles).'),
                ('the name without a mark', {'icon.svg': self.SVG}, 'acme-7', 'Acme Corp',
                 '::error::DESKTOP_COBRAND_NAME needs cobrand.svg or cobrand.png in the branding archive.'),
                ('both marks', {'cobrand.svg': self.SVG, 'cobrand.png': self.PNG}, 'acme-7', 'Acme Corp',
                 '::error::The branding archive may hold cobrand.svg or cobrand.png, not both.'),
                ('an SVG that is not', {'cobrand.svg': b'Acme'}, 'acme-7', 'Acme Corp',
                 '::error::branding/cobrand.svg is not an SVG.'),
                ('a PNG that is not', {'cobrand.png': self.SVG}, 'acme-7', 'Acme Corp',
                 '::error::branding/cobrand.png is not a PNG.'),
                ('no icon and no mark', {'colours.json': b'{}'}, 'acme-7', '', 'has no icon.svg'),
                ('another member', {'icon.svg': self.SVG, 'cobrand.jpg': b'Acme'}, 'acme-7', '',
                 'may hold only icon.svg, icon.png, colours.json, cobrand.svg and cobrand.png at its root')]:
            with self.subTest(name):
                result, exported = self.fetch_branding(members, profile=profile, cobrand_name=cobrand_name)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(any(line.startswith('::error::') and error in line
                                    for line in result.stdout.splitlines()), result.stdout + result.stderr)
                self.assertNotIn('Acme Corp', result.stdout + result.stderr)
                self.assertFalse(any(line.startswith('BRANDING_COBRAND_MARK=branding') for line in exported))

    def test_cobrand_reaches_only_the_macos_build_and_image(self):
        for text in (self.workflow, self.promote):
            uses = text.count('uses: ./.github/actions/site-profile')
            self.assertEqual(text.count('cobrand-name: ${{ secrets.DESKTOP_COBRAND_NAME }}'), uses)
            self.assertNotRegex(text, r'echo[^\n]*(COBRAND_NAME|secrets\.DESKTOP_COBRAND_NAME)\b(?!\s+needs)')
        steps = re.split(r'\n      - name: ', self.workflow)
        cobranded = [step.split('\n', 1)[0] for step in steps if '--cobrand-mark' in step]
        self.assertEqual(cobranded, ['Assemble and sign the app bundle', 'Package the disk image'])
        for step in steps:
            for script in ('build-gtk.sh', 'build-linux.sh', 'build-windows.sh'):
                if f'scripts/{script}' in step:
                    self.assertNotIn('cobrand', step.lower(), script)
        # Run as the runner would: the options only with a mark, never for
        # the default profile, and a name without a mark is refused.
        for title, script in (('Assemble and sign the app bundle', 'scripts/build-app.sh'),
                              ('Package the disk image', 'scripts/package-dmg.sh')):
            [step] = [step for step in steps if step.startswith(title + '\n')]
            run = step[step.index('run: |') + len('run: |'):]
            run = textwrap.dedent(run).replace(script, 'printf "%s\\n"').replace('${{ matrix.arch }}', 'arm64')
            base = dict(os.environ, PROFILE='acme-7', SITE_URL='https://relay.example', URL_SCHEME='acme-relay',
                        PRODUCT_NAME='Acme Desktop', BUNDLE_ID='com.acme.desktop', ASSET_PREFIX='Desktop-acme-7',
                        RELEASE_FEED='github:acme/desktop-releases', RELEASES_REPOSITORY='acme/desktop-releases',
                        FEED_PUBLIC_KEY='', BRANDING_ICON='', COEXIST='', VERSION='1.2.3', GITHUB_RUN_NUMBER='1')
            for name, change, want in [
                    ('no co-brand', dict(BRANDING_COBRAND_MARK='', COBRAND_NAME=''), None),
                    ('the default profile', dict(PROFILE='', BRANDING_COBRAND_MARK='', COBRAND_NAME=''), None),
                    ('a co-brand', dict(BRANDING_COBRAND_MARK='branding/cobrand.svg', COBRAND_NAME='Acme Corp'),
                     ['--cobrand-mark', 'branding/cobrand.svg', '--cobrand-name', 'Acme Corp'])]:
                with self.subTest(title=title, case=name):
                    with tempfile.TemporaryDirectory() as temporary:
                        # The bundle and probe the steps around them expect.
                        (Path(temporary) / 'dist' / 'Acme Desktop.app').mkdir(parents=True)
                        (Path(temporary) / 'probe-build').mkdir()
                        (Path(temporary) / 'probe-build' / 'probe-version.txt').write_text('1.2.3\n')
                        result = subprocess.run(['bash', '-c', run], cwd=temporary, env=dict(base, **change),
                                                capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    args = result.stdout.splitlines()
                    if want is None:
                        self.assertFalse(any('cobrand' in arg for arg in args), args)
                    else:
                        start = args.index('--cobrand-mark')
                        self.assertEqual(args[start:start + 4], want)
        [step] = [step for step in steps if step.startswith('Assemble and sign the app bundle\n')]
        run = textwrap.dedent(step[step.index('run: |') + len('run: |'):]).replace('scripts/build-app.sh', 'printf "%s\\n"')
        result = subprocess.run(['bash', '-c', run], env=dict(
            os.environ, PROFILE='acme-7', SITE_URL='', URL_SCHEME='', PRODUCT_NAME='', BUNDLE_ID='', ASSET_PREFIX='',
            RELEASE_FEED='', RELEASES_REPOSITORY='', FEED_PUBLIC_KEY='', BRANDING_ICON='', COEXIST='',
            BRANDING_COBRAND_MARK='', COBRAND_NAME='Acme Corp', VERSION='1.2.3', GITHUB_RUN_NUMBER='1'),
            capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('::error::DESKTOP_COBRAND_NAME needs DESKTOP_BRANDING_URL', result.stdout)
        self.assertNotIn('Acme Corp', result.stdout + result.stderr)

    def test_site_profile_feed_inputs_reach_every_use_and_the_token_only_as_presence(self):
        for text in (self.workflow, self.promote):
            uses = text.count('uses: ./.github/actions/site-profile')
            self.assertGreater(uses, 0)
            for line in ('release-feed: ${{ secrets.RELEASE_FEED }}',
                         'feed-public-key: ${{ secrets.DESKTOP_FEED_PUBLIC_KEY }}',
                         "feed-publisher-token-set: ${{ secrets.DESKTOP_FEED_PUBLISHER_TOKEN != '' }}"):
                self.assertEqual(text.count(line), uses, line)
            # The token's value is never printed, and reaches only the cloud
            # feed steps' environment (test_release_workflow_cloud.py).
            handed = 'DESKTOP_FEED_PUBLISHER_TOKEN: ${{ secrets.DESKTOP_FEED_PUBLISHER_TOKEN }}'
            self.assertEqual(text.replace(handed, '').count('DESKTOP_FEED_PUBLISHER_TOKEN'), uses)
            self.assertNotRegex(text, r'echo[^\n]*\$\{\{ *secrets\.DESKTOP_FEED_PUBLISHER_TOKEN')

    def test_site_profile_outputs_reach_the_build_steps(self):
        # Every profile build passes the exported feed, and the repository and
        # key only when the feed has them.
        for script, count in (('scripts/build-app.sh', 1), ('scripts/build-gtk.sh', 1), ('scripts/build-windows.sh', 2)):
            steps = [step for step in re.split(r'\n      - name: ', self.workflow) if f'\n          {script} ' in step]
            self.assertEqual(len(steps), count, script)
            for step in steps:
                self.assertIn('--release-feed "$RELEASE_FEED"', step)
                self.assertIn('if [ -n "$RELEASES_REPOSITORY" ]; then args+=(--releases-repository "$RELEASES_REPOSITORY"); fi', step)
                self.assertIn('if [ -n "$FEED_PUBLIC_KEY" ]; then args+=(--feed-public-key "$FEED_PUBLIC_KEY"); fi', step)
                self.assertIn('${args[@]+"${args[@]}"}', step)
        # And, run as the runner would with a cloud profile's exports, the
        # Linux step hands build-gtk.sh the feed, key and no repository.
        [step] = [step for step in re.split(r'\n      - name: ', self.workflow) if '\n          scripts/build-gtk.sh ' in step]
        run = step[step.index('run: |') + len('run: |'):]
        run = textwrap.dedent(run).replace('scripts/build-gtk.sh', 'printf "%s\\n"')
        env = dict(os.environ, PROFILE='acme-7', SITE_URL='https://relay.example', URL_SCHEME='acme-relay',
                   PRODUCT_NAME='Acme Desktop', RELEASES_REPOSITORY='', RELEASE_FEED='cloud',
                   FEED_PUBLIC_KEY=self.FEED_KEY, BRANDING_ICON='', VERSION='1.2.3')
        result = subprocess.run(['bash', '-c', run], env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        args = result.stdout.splitlines()
        self.assertEqual(args[args.index('--release-feed') + 1], 'cloud')
        self.assertEqual(args[args.index('--feed-public-key') + 1], self.FEED_KEY)
        self.assertEqual(args[args.index('--profile') + 1], 'acme-7')
        self.assertNotIn('--releases-repository', args)

    def test_automatic_ci_status_requires_success_for_selected_source(self):
        ci = block(self.workflow, 'ci-status:', 2)
        step = block(ci, '- name: Look for a passing Desktop CI run of this commit on main', 6)
        script = textwrap.dedent(step.split('        run: |\n', 1)[1])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            # Run the actual workflow shell against an API fixture. An exact
            # main push passes; a different SHA must fail rather than running
            # tests in workflow_run's newer default-branch context.
            gh = root / 'gh'
            gh.write_text('#!/bin/sh\ncat "$CI_FIXTURE"\n')
            gh.chmod(0o755)
            fixture = root / 'ci.json'
            run = {'head_sha': 'a' * 40, 'head_branch': 'main',
                   'repository': {'full_name': 'AgentWorkforce/relay-app'},
                   'head_repository': {'full_name': 'AgentWorkforce/relay-app'},
                   'path': '.github/workflows/desktop-ci.yml', 'event': 'push',
                   'status': 'completed', 'conclusion': 'success'}
            env = dict(os.environ, PATH=f'{root}{os.pathsep}' + os.environ['PATH'],
                       SHA='a' * 40, AUTO_CI_RUN_ID='456', FULL_CHECKS='',
                       GITHUB_REPOSITORY='AgentWorkforce/relay-app',
                       GITHUB_OUTPUT=str(root / 'output'), GITHUB_STEP_SUMMARY=str(root / 'summary'),
                       CI_FIXTURE=str(fixture))
            for sha, expected in [('a' * 40, 0), ('b' * 40, 1)]:
                run['head_sha'] = sha
                fixture.write_text(json.dumps(run))
                result = subprocess.run(['bash', '-c', script], cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)

    def test_automatic_ci_api_lookup_retries_transient_failures_but_is_bounded(self):
        ci = block(self.workflow, 'ci-status:', 2)
        step = block(ci, '- name: Look for a passing Desktop CI run of this commit on main', 6)
        script = textwrap.dedent(step.split('        run: |\n', 1)[1])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            gh = root / 'gh'
            gh.write_text("""#!/bin/sh
count=$(cat "$GH_COUNT")
count=$((count + 1))
echo "$count" > "$GH_COUNT"
if [ "$count" -le "$GH_FAILURES" ]; then exit 1; fi
cat "$CI_FIXTURE"
""")
            gh.chmod(0o755)
            sleep = root / 'sleep'
            sleep.write_text('#!/bin/sh\nexit 0\n')
            sleep.chmod(0o755)
            fixture = root / 'ci.json'
            fixture.write_text(json.dumps({'head_sha': 'a' * 40, 'head_branch': 'main',
                'repository': {'full_name': 'AgentWorkforce/relay-app'},
                'head_repository': {'full_name': 'AgentWorkforce/relay-app'},
                'path': '.github/workflows/desktop-ci.yml', 'event': 'push',
                'status': 'completed', 'conclusion': 'success'}))
            env = dict(os.environ, PATH=f'{root}{os.pathsep}' + os.environ['PATH'],
                       SHA='a' * 40, AUTO_CI_RUN_ID='456', FULL_CHECKS='',
                       GITHUB_REPOSITORY='AgentWorkforce/relay-app',
                       GITHUB_OUTPUT=str(root / 'output'), GITHUB_STEP_SUMMARY=str(root / 'summary'),
                       CI_FIXTURE=str(fixture), GH_COUNT=str(root / 'count'))
            for failures, expected in [(2, 0), (3, 1)]:
                (root / 'count').write_text('0')
                env['GH_FAILURES'] = str(failures)
                result = subprocess.run(['bash', '-c', script], cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                self.assertEqual((root / 'count').read_text().strip(), '3')

    def test_install_sh_is_published_for_every_profile(self):
        # Every product installs into the same prefix and user unit (one app
        # per machine), so every release publishes install.sh,
        # install-macos.sh and install.ps1: the default's as they are, a
        # profile's set to its own releases.
        stage = block(self.publish, '- name: Stage the release assets', 6)
        self.assertIn('if [ -z "$PROFILE" ]; then\n            cp linux/install.sh release-assets/install.sh\n'
                      '            cp macos/install.sh release-assets/install-macos.sh\n'
                      '            cp windows/install.ps1 release-assets/install.ps1\n', stage)
        self.assertIn('for script in install.sh install-macos.sh install.ps1; do sha256sum "$script" > "$script.sha256"', stage)
        release = block(self.publish, '- name: Create the release', 6)
        for name in ('install.sh', 'install.sh.sha256', 'install-macos.sh', 'install-macos.sh.sha256',
                     'install.ps1', 'install.ps1.sha256'):
            self.assertIn(f'            release-assets/{name}\n', release)
        self.assertNotIn("env.PROFILE == ''", release)

    def test_candidate_staging_excludes_checkout_branding_and_matches_published_files(self):
        for profile, prefix in (('', 'AgentRelay'), ('acme-7', 'Desktop-acme-7')):
            with self.subTest(profile=profile):
                self.stage_and_publish(profile, prefix)

    def stage_and_publish(self, profile, prefix, repository=None):
        version = '2026.11.1'
        checkout = WORKFLOW.parents[2]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            # A real checkout already has branding in assets/. These files must
            # survive staging without entering a candidate or a published release.
            shutil.copytree(checkout / 'assets', root / 'assets')
            branding = {path.name: path.read_bytes() for path in (root / 'assets').iterdir() if path.is_file()}
            (root / 'scripts').mkdir()
            for name in ('macos-release-assets.py', 'release-candidate.py'):
                shutil.copyfile(checkout / 'scripts' / name, root / 'scripts' / name)
            (root / 'macos').mkdir()
            # Floor 13 needs no bridge download, keeping this regression offline.
            (root / 'macos/Package.swift').write_text('platforms: [.macOS(.v13)]')
            (root / 'linux').mkdir()
            shutil.copyfile(checkout / 'linux/install.sh', root / 'linux/install.sh')
            shutil.copyfile(checkout / 'macos/install.sh', root / 'macos/install.sh')
            (root / 'windows').mkdir()
            shutil.copyfile(checkout / 'windows/install.ps1', root / 'windows/install.ps1')
            for arch in ('arm64', 'x64'):
                for directory, platform, suffixes, record in (
                    ('downloads', 'macOS', ('.dmg', '-probe.tar.gz'), f'probe-version-{arch}.txt'),
                    ('linux-downloads', 'Linux', ('.tar.gz', '.deb', '-probe.tar.gz'), f'probe-version-linux-{arch}.txt'),
                    ('windows-downloads', 'Windows', ('.zip', '-probe.zip', '-Setup.exe'), f'probe-version-windows-{arch}.txt'),
                ):
                    folder = root / directory
                    folder.mkdir(exist_ok=True)
                    for suffix in suffixes:
                        name = f'{prefix}-{version}-{platform}-{arch}{suffix}'
                        (folder / name).write_bytes(name.encode())
                    (folder / record).write_text(version + '\n')
            (root / 'release-notes.md').write_text('Prepared release notes\n')
            if repository is None:
                repository = 'acme/desktop-releases' if profile else 'AgentWorkforce/relay-desktop-releases'
            env = dict(os.environ, VERSION=version, CHANNEL='stable', GITHUB_SHA='a' * 40,
                       GITHUB_REPOSITORY='AgentWorkforce/relay-app', GITHUB_RUN_ID='123',
                       SOURCE_SHA='a' * 40, CI_RUN_ID='',
                       GITHUB_OUTPUT=str(root / 'outputs'),
                       PROFILE=profile, ASSET_PREFIX=prefix, RELEASES_REPOSITORY=repository,
                       RELEASE_FEED=f'github:{repository}', FEED_PUBLIC_KEY='',
                       SITE_URL='https://relay.acme.test' if profile else 'https://agentrelay.com',
                       PRODUCT_NAME='Acme $Desktop & Co' if profile else 'Agent Relay')
            for name in ('Stage the release assets', 'Prepare verified release candidate'):
                step = block(self.publish, f'- name: {name}', 6)
                # The run block only: block() keeps a comment of the next step,
                # which would leave the script indented and its heredoc open.
                body = step.split('        run: |\n', 1)[1].splitlines()
                script = textwrap.dedent('\n'.join(itertools.takewhile(
                    lambda line: not line.strip() or line.startswith(' ' * 10), body)))
                result = subprocess.run(['bash', '-c', script], cwd=root, env=env, text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                if name == 'Stage the release assets' and profile:
                    # The log names the profile by its opaque id only.
                    self.assertIn(f"Profile {profile}: install.sh, install-macos.sh and install.ps1 for the profile's releases.",
                                  result.stdout)
                    self.assertNotIn('Acme', result.stdout + result.stderr)
                    self.assertNotIn(repository, result.stdout + result.stderr)
            manifest = json.loads((root / 'candidate/release-candidate.json').read_text())
            self.assertEqual(manifest['profile'], profile)
            self.assertEqual(branding, {path.name: path.read_bytes() for path in (root / 'assets').iterdir() if path.is_file()})
            self.assertTrue(all(f'assets/{name}' not in manifest['files'] for name in branding))
            # Every published name starts with the profile's prefix, but
            # the installers. The default profile publishes linux/install.sh,
            # macos/install.sh (as install-macos.sh) and windows/install.ps1
            # as they are; a profile's set the
            # product, releases, asset names, feed, site and id to its own,
            # and run.
            staged = {path.name for path in (root / 'release-assets').iterdir()}
            self.assertTrue(all(name.startswith((f'{prefix}-', 'install.sh', 'install-macos.sh', 'install.ps1'))
                                for name in staged), staged)
            for name in ('install.sh', 'install-macos.sh', 'install.ps1'):
                self.assertIn(f'{name}.sha256', staged)
                self.assertIn(f'assets/{name}', manifest['files'])
            windows_source = (checkout / 'windows/install.ps1').read_text()
            windows_installer = (root / 'release-assets/install.ps1').read_text()
            source = (checkout / 'linux/install.sh').read_text()
            installer = (root / 'release-assets/install.sh').read_text()
            mac_source = (checkout / 'macos/install.sh').read_text()
            mac_installer = (root / 'release-assets/install-macos.sh').read_text()
            if profile:
                def profiled(source):
                    return (source
                            .replace("PRODUCT_NAME='Agent Relay'\n", "PRODUCT_NAME='Acme $Desktop & Co'\n", 1)
                            .replace("RELEASES_REPOSITORY='AgentWorkforce/relay-desktop-releases'\n",
                                     f"RELEASES_REPOSITORY={repository}\n", 1)
                            .replace("ASSET_PREFIX='AgentRelay'\n", f"ASSET_PREFIX={prefix}\n", 1)
                            .replace("RELEASE_FEED='github'\n", "RELEASE_FEED=github\n", 1)
                            .replace("SITE_URL=''\n", "SITE_URL=https://relay.acme.test\n", 1)
                            .replace("PROFILE_ID=''\n", f"PROFILE_ID={profile}\n", 1))
                expected = profiled(source)
                self.assertNotEqual(expected, source)
                self.assertEqual(installer, expected)
                # The macOS installer has the same seven assignments and
                # takes the same values.
                mac_expected = profiled(mac_source)
                self.assertNotEqual(mac_expected, mac_source)
                self.assertEqual(mac_installer, mac_expected)
                self.assertEqual(windows_installer, windows_source
                                 .replace("$ProductName = 'Agent Relay'\n", "$ProductName = 'Acme $Desktop & Co'\n", 1)
                                 .replace("$ReleasesRepository = 'AgentWorkforce/relay-desktop-releases'\n",
                                          f"$ReleasesRepository = '{repository}'\n", 1)
                                 .replace("$AssetPrefix = 'AgentRelay'\n", f"$AssetPrefix = '{prefix}'\n", 1)
                                 .replace("$SiteUrl = ''\n", "$SiteUrl = 'https://relay.acme.test'\n", 1)
                                 .replace("$ProfileId = ''\n", f"$ProfileId = '{profile}'\n", 1))
                for shell_installer in ('install.sh', 'install-macos.sh'):
                    probe = subprocess.run(
                        ['bash', '-c', 'eval "$(grep -E \'^(PRODUCT_NAME|RELEASES_REPOSITORY|ASSET_PREFIX)=\' "$1")"; '
                         'printf "%s|%s|%s" "$PRODUCT_NAME" "$RELEASES_REPOSITORY" "$ASSET_PREFIX"', '-',
                         str(root / 'release-assets' / shell_installer)], capture_output=True, text=True)
                    self.assertEqual(probe.stdout, f'Acme $Desktop & Co|{repository}|{prefix}', probe.stderr)
            else:
                self.assertEqual(installer, source)
                self.assertEqual(mac_installer, mac_source)
                self.assertEqual(windows_installer, windows_source)
            # Normal publication and prepare-only must select the same packages
            # and checksums, with every configured upload pattern matching files.
            release = block(self.publish, '- name: Create the release', 6)
            patterns = release.split('          files: |\n', 1)[1].split('          body_path:', 1)[0]
            published = set()
            for pattern in patterns.splitlines():
                pattern = pattern.strip()
                if pattern.startswith('#'):
                    continue
                # An expression the runner evaluates to the name or to an
                # empty line, which the release action skips.
                guarded = re.fullmatch(r"\$\{\{ env\.PROFILE == '' && '([^']+)' \|\| '' \}\}", pattern)
                if guarded:
                    if profile:
                        continue
                    pattern = guarded[1]
                pattern = (pattern.replace('${{ needs.version.outputs.version }}', version)
                           .replace('${{ env.ASSET_PREFIX }}', prefix))
                matches = glob.glob(str(root / pattern))
                self.assertTrue(matches, pattern)
                published.update(Path(path).name for path in matches)
            prepared = {Path(path).name for path in manifest['files'] if path.startswith('assets/')}
            self.assertEqual(published, prepared)
            shutil.rmtree(root / 'candidate')
            env['CI_RUN_ID'] = '456'
            step = block(self.publish, '- name: Prepare verified release candidate', 6)
            script = textwrap.dedent(step.split('        run: |\n', 1)[1])
            result = subprocess.run(['bash', '-c', script], cwd=root, env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            manifest = json.loads((root / 'candidate/release-candidate.json').read_text())
            self.assertEqual(manifest['ci_run_id'], '456')
            self.assertEqual(manifest['source_sha'], 'a' * 40)

    def test_windows_packages_are_signed_with_oidc_only_when_configured(self):
        workflow = yaml.safe_load(self.workflow)
        self.assertEqual(workflow['permissions'], {'contents': 'read'})
        for name, job in workflow['jobs'].items():
            if name.startswith('package-windows-'):
                self.assertEqual(job['permissions'], {'contents': 'read', 'id-token': 'write'}, name)
                self.assertEqual(job['environment'], "${{ inputs.profile && format('desktop-{0}', "
                                                     "inputs.profile) || 'desktop-release' }}", name)
            else:
                # No other job can mint an OIDC token.
                self.assertNotIn('id-token', job.get('permissions') or {}, name)
        self.assertEqual(self.workflow.count('id-token'), 2)
        for arch in ('x64', 'arm64'):
            text = block(self.workflow, f'package-windows-{arch}:', 2)
            steps = workflow['jobs'][f'package-windows-{arch}']['steps']
            names = [step.get('name') for step in steps]
            step = {step.get('name'): step for step in steps}
            # The steps read only the six signing variables and the site
            # profile's DESKTOP_COEXIST.
            steps_text = text.split('\n    steps:\n', 1)[1]
            self.assertEqual(set(re.findall(r'vars\.([A-Za-z0-9_]+)', steps_text)),
                             set(SIGNING_VARIABLES) | {'DESKTOP_COEXIST'}, arch)
            resolve = step['Resolve Windows signing']
            self.assertEqual(resolve['id'], 'signing')
            self.assertEqual(resolve['env'], {
                **{name: '${{ vars.%s }}' % name for name in SIGNING_VARIABLES},
                'SIGNING_REF': "${{ github.event_name != 'workflow_run' && github.ref || "
                               "(github.event.workflow_run.head_repository.full_name == github.repository && "
                               "format('refs/heads/{0}', github.event.workflow_run.head_branch)) || 'a fork' }}"})
            self.assertNotIn('if', resolve)
            run = resolve['run']
            # Only main, hotfix/ branches and desktop-v tags are signed, checked before the
            # variables: any other ref skips loudly, naming the ref.
            guard, rest = run.split('$names = ', 1)
            self.assertIn('$ref = $env:SIGNING_REF', guard)
            self.assertIn("if ($ref -cne 'refs/heads/main' -and -not $ref.StartsWith('refs/tags/desktop-v', "
                          "[StringComparison]::Ordinal) -and -not $ref.StartsWith('refs/heads/hotfix/', "
                          "[StringComparison]::Ordinal)) {", guard)
            refused = guard.split("[StringComparison]::Ordinal)) {", 1)[1].split('\n}', 1)[0]
            self.assertIn('$message = "Building unsigned: only main, hotfix/ branches and desktop-v tags are signed, '
                          'and this run builds $ref."', refused)
            self.assertIn('Write-Output "::warning title=Windows signing skipped::$message"', refused)
            self.assertIn("'sign=false' >> $env:GITHUB_OUTPUT", refused)
            self.assertIn('exit 0', refused)
            self.assertNotIn('sign=true', guard)
            # All six or unsigned: the skip warns, says so and succeeds.
            for name in SIGNING_VARIABLES:
                self.assertIn(f"'{name}'", run)
            skip = run.split('if ($missing) {', 1)[1].split('\n}', 1)[0]
            self.assertIn('Write-Output "::warning title=Windows signing skipped::$message"', skip)
            self.assertIn('Write-Output "Windows signing skipped. $message"', skip)
            self.assertIn("'sign=false' >> $env:GITHUB_OUTPUT", skip)
            self.assertIn('exit 0', skip)
            self.assertIn("'sign=true' >> $env:GITHUB_OUTPUT", run)
            # Nothing prints a value, except the endpoint's host.
            for line in run.splitlines():
                if 'Write-Output' in line:
                    self.assertNotIn('$env:', line)
            self.assertIn('Write-Output "Signing with Azure Trusted Signing at $($endpoint.Host)."', run)
            sign_in = step['Sign in for Windows signing']
            self.assertEqual(sign_in['if'], "steps.signing.outputs.sign == 'true'")
            self.assertEqual(sign_in['uses'], './.github/actions/windows-sign')
            self.assertEqual(sign_in['with'], {
                'mode': 'setup', 'tenant-id': '${{ vars.AZURE_TENANT_ID }}',
                'client-id': '${{ vars.AZURE_CLIENT_ID }}', 'subscription-id': '${{ vars.AZURE_SUBSCRIPTION_ID }}',
                'endpoint': '${{ vars.TRUSTED_SIGNING_ENDPOINT }}', 'account': '${{ vars.TRUSTED_SIGNING_ACCOUNT }}',
                'certificate-profile': '${{ vars.TRUSTED_SIGNING_CERT_PROFILE }}'})
            package = step['Assemble the captured runtimes and build packages']
            self.assertEqual(package['env']['SIGN_COMMAND'], '${{ steps.sign-in.outputs.sign-command }}')
            self.assertIn("@('--sign-command', $env:SIGN_COMMAND)", package['run'])
            verify = step['Verify Windows signatures']
            self.assertNotIn('if', verify)
            self.assertEqual(verify['with']['mode'], 'verify')
            self.assertEqual(verify['with']['sign'], '${{ steps.signing.outputs.sign }}')
            self.assertIn(f'-Windows-{arch}-probe.zip', verify['with']['archives'])
            self.assertIn(f'-Windows-{arch}.zip', verify['with']['archives'])
            self.assertTrue(verify['with']['files'].endswith(f'-Windows-{arch}-Setup.exe'))
            self.assertNotIn('continue-on-error', text)
            # Sign in, sign while packaging, verify, then install.
            order = ['Install Inno Setup', 'Resolve Windows signing', 'Sign in for Windows signing',
                     'Assemble the captured runtimes and build packages', 'Verify Windows signatures',
                     'Smoke test the packaged core', 'Install and uninstall with Setup', 'Upload the Windows package']
            self.assertEqual([name for name in names if name in order], order)
            # The installed uninstaller is verified before it runs.
            install = step['Install and uninstall with Setup']
            self.assertEqual(install['env']['SIGN'], '${{ steps.signing.outputs.sign }}')
            self.assertEqual(install['env']['SIGNTOOL'], '${{ steps.verify-signatures.outputs.signtool }}')
            before, after = install['run'].split('Start-Process "$root\\unins000.exe"', 1)
            verified = before.split("if ($env:SIGN -eq 'true')", 1)[1]
            # Both the installed uninstaller and the installed app.
            self.assertIn('& $env:SIGNTOOL verify /pa /v "$root\\unins000.exe" '
                          f'"$root\\$version\\AgentRelay-Windows-{arch}\\bin\\agent-relay.exe"\n', verified)
            self.assertIn('if ($LASTEXITCODE -ne 0) { throw', before)

    def test_windows_signing_action_pins_its_login_and_signs_sha256(self):
        text = WINDOWS_SIGN.read_text()
        action = yaml.safe_load(text)
        uses = re.findall(r'uses: (\S+)( # .*)?$', text, re.M)
        self.assertEqual(uses, [('azure/login@a641126d1b8aa4d1fa005f4f92df94a3a4c4c906', ' # v3.1.0')])
        steps = {step['name']: step for step in action['runs']['steps']}
        login = steps['Sign in to Azure with OIDC']
        self.assertEqual(login['if'], "inputs.mode == 'setup'")
        self.assertEqual(set(login['with']), {'client-id', 'tenant-id', 'subscription-id'})
        prepare = steps['Prepare the sign command']['run']
        self.assertIn("TimestampRfc3161 = 'http://timestamp.acs.microsoft.com'", prepare)
        self.assertIn("FileDigest = 'SHA256'", prepare)
        self.assertIn("TimestampDigest = 'SHA256'", prepare)
        self.assertIn('Install-Module -Name ArtifactSigning -RequiredVersion 0.1.8', prepare)
        self.assertIn('Import-Module ArtifactSigning -RequiredVersion 0.1.8', prepare)
        # Only az's OIDC sign-in can authenticate the signing.
        self.assertNotIn('ExcludeAzureCliCredential', prepare)
        self.assertIn("if ($signature.Status -ne 'Valid') { throw", prepare)
        verify = steps['Verify the signatures']
        self.assertEqual(verify['if'], "inputs.mode == 'verify'")
        skip = verify['run'].split("if ($env:SIGN -ne 'true') {", 1)[1].split('\n}', 1)[0]
        self.assertIn("'::warning title=Windows signing skipped::", skip)
        self.assertIn('exit 0', skip)
        self.assertIn('& $signtool verify /pa /v @batch', verify['run'])
        self.assertIn('if ($LASTEXITCODE -ne 0) { throw', verify['run'])

    def test_profile_jobs_run_under_their_environment_and_the_default_under_none(self):
        # Every job that builds or publishes selects the desktop-<id>
        # environment from the dispatch input; with no profile (every
        # automatic candidate and tag) the expression is empty: no
        # environment, the default profile's values, as before. The Windows
        # package jobs, which sign, fall back to desktop-release instead, so a
        # federated credential can name them; its secrets are the default
        # profile's (none).
        expression = "environment: ${{ inputs.profile && format('desktop-{0}', inputs.profile) || '' }}"
        signing = "environment: ${{ inputs.profile && format('desktop-{0}', inputs.profile) || 'desktop-release' }}"
        for job in ('version', 'release-core', 'build', 'build-linux', 'compile-windows-x64',
                    'compile-windows-arm64', 'package-windows-x64', 'package-windows-arm64', 'publish'):
            text = block(self.workflow, job + ':', 2)
            self.assertIn('    ' + (signing if job.startswith('package-windows-') else expression), text, job)
            self.assertIn('uses: ./.github/actions/site-profile', text, job)
        for job in ('ci-status', 'pre-release', 'build-windows', 'changelog'):
            self.assertNotIn('environment:', block(self.workflow, job + ':', 2), job)
        self.assertEqual(self.workflow.count(expression), 7)
        self.assertEqual(self.workflow.count(signing), 2)
        # The input exists, is optional and empty by default; workflow_run
        # and tag pushes carry no inputs, so they are the default profile's.
        inputs = block(block(self.workflow, 'on:', 0), 'workflow_dispatch:', 2)
        profile = block(inputs, 'profile:', 6)
        self.assertIn("default: ''", profile)
        self.assertIn('required: false', profile)
        self.assertIn('type: string', profile)
        # The profile's values never appear inline: they are the profile
        # environment's secrets, which only the site-profile steps pass (as
        # `input: ${{ secrets.NAME }}`), and the steps take them from the
        # environment variables that action exports. Never a variable: a
        # `vars.` value would print in the logs.
        # (The bridge job reads its own bridge environment's
        # RELEASES_REPOSITORY: test_release_workflow_cloud.py.)
        promote = self.promote.replace(block(self.promote, 'bridge:', 2), '')
        for path, workflow in ((WORKFLOW, self.workflow), (PROMOTE, promote)):
            steps = workflow.count('uses: ./.github/actions/site-profile')
            self.assertGreater(steps, 0, path.name)
            for name in PROFILE_SECRETS:
                self.assertEqual(workflow.count('secrets.' + name), steps, (path.name, name))
                for line in workflow.splitlines():
                    if 'secrets.' + name in line:
                        self.assertRegex(line, r'^\s+[a-z-]+: \$\{\{ secrets\.[A-Z_]+ \}\}$', (path.name, line))
                self.assertNotIn('vars.' + name, workflow, path.name)
        self.assertEqual(self.workflow.count('uses: ./.github/actions/site-profile'), 9)
        self.assertEqual(self.promote.count('uses: ./.github/actions/site-profile'), 1)
        self.assertNotIn('RELEASES_REPOSITORY: AgentWorkforce/relay-desktop-releases', self.workflow)
        # The candidate manifest and the Resolve log carry the opaque id.
        self.assertIn('--run-id "$GITHUB_RUN_ID" --tests-passed --profile "$PROFILE"', self.publish)
        self.assertIn('if [ -n "$PROFILE" ]; then echo "Profile $PROFILE"; fi', block(self.workflow, 'version:', 2))
        # Branding is fetched where executables and packages are built.
        for job in ('build', 'build-linux', 'compile-windows-x64', 'compile-windows-arm64'):
            self.assertIn('branding-url: ${{ secrets.DESKTOP_BRANDING_URL }}', block(self.workflow, job + ':', 2), job)

    def test_both_architectures_are_required_by_each_package_matrix(self):
        for job in ('build', 'build-linux'):
            job_text = block(self.workflow, f'{job}:', 2)
            arches = re.findall(r'^\s+arch: (arm64|x64)$', job_text, re.MULTILINE)
            self.assertCountEqual(arches, ['x64', 'arm64'], job)
            # A failed member must fail the aggregate job rather than being ignored.
            self.assertNotRegex(job_text, r'(?m)^\s+continue-on-error:', job)

    def test_windows_architectures_package_independently_without_reinstalling_runtime(self):
        for arch in ('x64', 'arm64'):
            compilation = block(self.workflow, f'compile-windows-{arch}:', 2)
            package = block(self.workflow, f'package-windows-{arch}:', 2)
            self.assertRegex(compilation, r'(?m)^        component: \[core, app\]$')
            self.assertIn('--runtime-out runtime', compilation)
            self.assertIn('          path: runtime/', compilation)
            self.assertIn('scripts/check-probe-bridge.sh target/release/agent-relay-desktop-probe.exe', compilation)
            self.assertIn(f'    needs: [version, compile-windows-{arch}]', package)
            self.assertNotIn('windows-toolchain', package)
            self.assertNotIn('msys2', package)
            self.assertIn('python scripts/package-windows.py', package)
            self.assertIn('--app-runtime compiled/app --core-runtime compiled/core', package)
            self.assertIn('Smoke test the packaged core', package)
            self.assertIn('Install and uninstall with Setup', package)
            self.assertIn(f'name: windows-{arch}', package)
            self.assertNotRegex(package, r'(?m)^\s+continue-on-error:')
        aggregate = block(self.workflow, 'build-windows:', 2)
        gate = Gate(condition(aggregate, 4))
        for x64, arm64, cancelled in itertools.product(STATES, STATES, (True, False)):
            context = {'needs.package-windows-x64.result': x64, 'needs.package-windows-arm64.result': arm64}
            self.assertEqual(gate(context, cancelled), not cancelled and x64 == arm64 == 'success')

    def test_packaging_overlaps_verification(self):
        for job in ('build', 'compile-windows-x64', 'compile-windows-arm64', 'release-core'):
            job_text = block(self.workflow, f'{job}:', 2)
            self.assertRegex(job_text, r'(?m)^    needs: version$')
        linux = block(self.workflow, 'build-linux:', 2)
        self.assertRegex(linux, r'(?m)^    needs: \[version, release-core\]$')

    def test_empty_arrays_expand_with_the_unbound_guard(self):
        # bash < 4.4 (the macOS runner's 3.2) rejects "${a[@]}" of an empty
        # array under set -u, so an array declared with =() must expand as
        # ${a[@]+"${a[@]}"}.
        root = WORKFLOW.parents[2]
        for path in (WORKFLOW, PROMOTE, SITE_PROFILE):
            declared = set()
            for number, line in enumerate(path.read_text().splitlines(), 1):
                if re.match(r'\s*(- )?run:', line):
                    declared = set()
                declared.update(re.findall(r'\b(\w+)=\(\)', line))
                for name in re.findall(r'(?<!\+)"\$\{(\w+)\[@\]\}"', line):
                    if name in declared:
                        self.fail(f'{path.relative_to(root)}:{number} expands the '
                                  f'possibly-empty array {name} without ${{{name}[@]+"${{{name}[@]}}"}}')


    def test_crate_overrides_are_internal_prepare_only_builds(self):
        # The inputs exist on workflow_dispatch only, and a tag or an
        # automatic candidate never reads them.
        workflow = yaml.safe_load(self.workflow)
        triggers = workflow[True]
        for name in ('agent-relay-node-rev', 'relaycast-version', 'relayscrubber-version'):
            spec = triggers['workflow_dispatch']['inputs'][name]
            self.assertEqual((spec['type'], spec['required'], spec['default']), ('string', False, ''))
        version = workflow['jobs']['version']
        steps = version['steps']
        guard = next(step for step in steps if step.get('id') == 'crate-guard')
        # The guard fails early: right after the source is chosen, before anything is checked out.
        self.assertEqual(steps.index(guard), 1)
        self.assertEqual(guard['env']['NODE_REV'],
                         "${{ github.event_name == 'workflow_dispatch' && inputs.agent-relay-node-rev || '' }}")
        self.assertEqual(guard['env']['RELAYCAST_VERSION'],
                         "${{ github.event_name == 'workflow_dispatch' && inputs.relaycast-version || '' }}")
        self.assertEqual(guard['env']['RELAYSCRUBBER_VERSION'],
                         "${{ github.event_name == 'workflow_dispatch' && inputs.relayscrubber-version || '' }}")
        self.assertEqual(guard['env']['PREPARE_ONLY'], "${{ github.event_name == 'workflow_dispatch' && inputs.prepare-only }}")
        self.assertEqual(guard['env']['OVERRIDES_ALLOWED'], '${{ vars.DESKTOP_CRATE_OVERRIDES_ALLOWED }}')
        self.assertEqual(version['environment'], "${{ inputs.profile && format('desktop-{0}', inputs.profile) || '' }}")

        # A stand-in git: the clone of AgentWorkforce/relay logs its
        # arguments; RELAY_HOLDS is 'none' (no such commit), 'fork' (a commit
        # no branch or tag contains) or a ref that contains it.
        stand_in_git = textwrap.dedent('''\
            #!/usr/bin/env bash
            printf '%s\\n' "$*" >> "$GIT_LOG"
            case "$*" in
              clone*) mkdir -p "${@: -1}" ;;
              *cat-file*) [ "$RELAY_HOLDS" != none ] ;;
              *for-each-ref*) [ "$RELAY_HOLDS" = fork ] || echo "$RELAY_HOLDS" ;;
            esac
        ''')

        def run(**env):
            with tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / 'output'
                output.write_text('')
                tools = Path(temporary) / 'tools'
                tools.mkdir()
                (tools / 'git').write_text(stand_in_git)
                (tools / 'git').chmod(0o755)
                git_log = Path(temporary) / 'git.log'
                git_log.write_text('')
                values = dict(NODE_REV='', RELAYCAST_VERSION='', RELAYSCRUBBER_VERSION='', PREPARE_ONLY='false', PROFILE_ID='',
                              OVERRIDES_ALLOWED='', PATH=f"{tools}:{os.environ['PATH']}", GITHUB_OUTPUT=str(output),
                              RUNNER_TEMP=temporary, GIT_LOG=str(git_log), RELAY_HOLDS='refs/heads/main')
                values.update(env)
                result = subprocess.run(['bash', '-c', guard['run']], env=values, capture_output=True, text=True)
                result.git = git_log.read_text()
                return result, dict(line.split('=', 1) for line in output.read_text().splitlines())

        # No override: nothing to allow, whatever the dispatch.
        result, outputs = run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outputs, {'overridden': 'false', 'node': 'false'})
        # The variable must name the profile: `vars` falls back to a
        # repository or organization variable, and 'true' there names none.
        allowed = dict(PREPARE_ONLY='true', PROFILE_ID='q7m2k9', OVERRIDES_ALLOWED='q7m2k9')
        refusals = {'prepare-only': dict(allowed, PREPARE_ONLY='false'),
                    'need a profile': dict(allowed, PROFILE_ID=''),
                    'need a profile:': dict(allowed, PROFILE_ID='', OVERRIDES_ALLOWED=''),
                    'not allowed for profile q7m2k9': dict(allowed, OVERRIDES_ALLOWED=''),
                    'not allowed for profile q7m2k9: its': dict(allowed, OVERRIDES_ALLOWED='true'),
                    'not allowed for profile q7m2k9: its environment': dict(allowed, OVERRIDES_ALLOWED='d4vq8z')}
        for override in (dict(NODE_REV='a' * 40), dict(RELAYCAST_VERSION='10.0.1'), dict(RELAYSCRUBBER_VERSION='0.3.0')):
            for message, env in refusals.items():
                with self.subTest(override=override, refused=message):
                    result, outputs = run(**override, **env)
                    self.assertEqual(result.returncode, 1)
                    self.assertIn('::error::Crate overrides', result.stdout)
                    self.assertIn(message, result.stdout)
                    self.assertEqual(outputs, {})
                    self.assertEqual(result.git, '')
        result, outputs = run(NODE_REV=' ' + 'a' * 40 + ' ', **allowed)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outputs, {'overridden': 'true', 'node': 'true', 'node-rev': 'a' * 40, 'relaycast-version': '',
                                   'relayscrubber-version': ''})
        self.assertIn('clone --quiet --bare --filter=blob:none https://github.com/AgentWorkforce/relay ', result.git)
        self.assertIn(f'for-each-ref --contains {"a" * 40} --format=%(refname) refs/heads refs/tags', result.git)
        # An unmerged branch of AgentWorkforce/relay is fine.
        result, outputs = run(NODE_REV='a' * 40, RELAY_HOLDS='refs/heads/feat/unmerged', **allowed)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outputs['node-rev'], 'a' * 40)
        # A commit only a fork holds, or no commit at all, is refused.
        for holds in ('fork', 'none'):
            with self.subTest(relay_holds=holds):
                result, outputs = run(NODE_REV='a' * 40, RELAY_HOLDS=holds, **allowed)
                self.assertEqual(result.returncode, 1)
                self.assertIn(f'::error::agent-relay-node rev {"a" * 40} is on no branch or tag of AgentWorkforce/relay',
                              result.stdout)
                self.assertEqual(outputs, {})
        # A tag is fetched from AgentWorkforce/relay's own tags: no clone.
        result, outputs = run(NODE_REV='v4.2.0', **allowed)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outputs['node-rev'], 'v4.2.0')
        self.assertEqual(result.git, '')
        result, outputs = run(RELAYCAST_VERSION='10.0.1', **allowed)
        self.assertEqual(outputs, {'overridden': 'true', 'node': 'false', 'node-rev': '', 'relaycast-version': '10.0.1',
                                   'relayscrubber-version': ''})
        self.assertEqual(result.git, '')
        # An internal prepare-only build passes a relayscrubber version
        # through, trimmed; nothing is cloned for it.
        result, outputs = run(RELAYSCRUBBER_VERSION=' 0.3.0 ', **allowed)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outputs, {'overridden': 'true', 'node': 'false', 'node-rev': '', 'relaycast-version': '',
                                   'relayscrubber-version': '0.3.0'})
        self.assertIn('relayscrubber 0.3.0', result.stdout)
        self.assertEqual(result.git, '')

        # The full checks never restore the overridden Linux core: with
        # overrides they build the pinned core themselves.
        self.assertEqual(workflow['jobs']['pre-release']['with']['reuse-linux-core'],
                         "${{ needs.version.outputs.crate-overrides != 'true' }}")

        # Every job that compiles the core or the app takes the resolved Cargo
        # files straight after its checkout; only the macOS build regenerates
        # the Swift contract, and the candidate records the crates.
        take = "needs.version.outputs.crate-overrides == 'true'"
        for name in ('release-core', 'build', 'build-linux', 'compile-windows-x64', 'compile-windows-arm64'):
            job_steps = workflow['jobs'][name]['steps']
            checkout = next(i for i, step in enumerate(job_steps) if step.get('name') == 'Check out')
            step = job_steps[checkout + 1]
            self.assertEqual((step.get('name'), step.get('if'), step['with']), (
                'Take the crate overrides', take, {'name': 'crate-overrides', 'path': '.'}), name)
        for name, job in workflow['jobs'].items():
            for step in job.get('steps', []):
                if 'generate-contract.sh' in str(step.get('run', '')):
                    self.assertEqual((name, step['if']), ('build', "needs.version.outputs.node-overridden == 'true'"))
        self.assertEqual(self.workflow.count('scripts/generate-contract.sh'), 1)
        prepare = next(step for step in workflow['jobs']['publish']['steps']
                       if step.get('name') == 'Prepare verified release candidate')
        self.assertEqual(prepare['env']['CRATES'], '${{ needs.version.outputs.crates }}')
        self.assertIn('args+=(--crates "$crates")', prepare['run'])


if __name__ == '__main__':
    unittest.main()
