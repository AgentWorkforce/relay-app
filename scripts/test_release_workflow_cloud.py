#!/usr/bin/env python3
"""The release and promote workflows' cloud feed branches (enterprise phase 2).

The publisher bridge follows the proposed Cloud contract (relay-control
docs/desktop-feed.md); these tests pin the workflow side only.
"""
import hashlib
import io
import itertools
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import textwrap
import unittest
import zipfile

from test_release_workflow import PROMOTE, WORKFLOW, Gate, block, condition

HANDED = 'DESKTOP_FEED_PUBLISHER_TOKEN: ${{ secrets.DESKTOP_FEED_PUBLISHER_TOKEN }}'
BRIDGE_SECRETS = {'RELEASES_REPOSITORY', 'DESKTOP_BRIDGE_APP_ID', 'DESKTOP_BRIDGE_APP_PRIVATE_KEY'}
FAKE_GH = '''#!/bin/sh
case "$2" in
  */git/ref/tags/*) echo 'gh: Not Found (HTTP 404)' >&2; exit 1 ;;
  */commits/main) echo 0123456789abcdef ;;
  */releases\\?*) cat "$FAKE_RELEASES" ;;
  *) echo '{"private": false}' ;;
esac
'''


def steps(text):
    return re.split(r'\n      - name: ', text)


def code(text):
    return '\n'.join(line for line in text.splitlines() if line.strip() and not line.lstrip().startswith('#'))


def run_block(step):
    return textwrap.dedent(step.split('        run: |\n', 1)[1])


class CloudWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text()
        cls.promote = PROMOTE.read_text()
        cls.version = block(cls.workflow, 'version:', 2)
        cls.publish = block(cls.workflow, 'publish:', 2)
        cls.promote_job = block(cls.promote, 'promote:', 2)
        cls.bridge = block(cls.promote, 'bridge:', 2)

    def test_no_cloud_refusal_remains(self):
        for text in (self.workflow, self.promote):
            self.assertNotIn('Refuse a cloud feed', text)
            self.assertNotIn('publishing to a cloud feed is not available yet', text)

    def test_publisher_token_reaches_only_cloud_gated_bridge_steps(self):
        for text, names in ((self.workflow, ['List the cloud feed', 'Publish to the cloud feed']),
                            (self.promote, ['Publish prepared assets to the cloud feed'])):
            handed = [step for step in steps(text) if HANDED in step]
            self.assertEqual([step.split('\n', 1)[0] for step in handed], names)
            self.assertEqual(text.count(HANDED), len(names))
            for step in handed:
                self.assertIn('          ' + HANDED, step)
                self.assertIn("env.RELEASE_FEED == 'cloud'", step.split('\n', 2)[1])
                self.assertIn('python3 scripts/desktop-feed-publish.py ', step)
                self.assertIn('--site "$SITE_URL" --profile "$PROFILE"', step)
                self.assertIn('--feed "$RELEASE_FEED"', step)
                self.assertNotIn('--token', step)
                self.assertNotRegex(step, r'(?m)^\s+(echo|printf|cat)\b.*DESKTOP_FEED_PUBLISHER_TOKEN')
        self.assertNotIn('DESKTOP_FEED_PUBLISHER_TOKEN', self.bridge)

    def test_github_publishing_steps_skip_a_cloud_feed(self):
        for job, names in ((self.publish, ('Check for the releases App', 'Check publishing access', 'Create the release')),
                           (self.promote_job, ('Check for the releases App', 'Check publishing access and reserve exact version',
                                               'Publish prepared assets'))):
            for name in names:
                self.assertIn("env.RELEASE_FEED != 'cloud'", condition(block(job, '- name: ' + name, 6), 8), name)
            # The mint step needs the App check's has_app, which a skipped check never sets.
            self.assertIn("steps.releases-app.outputs.has_app == 'true'", block(job, '- name: Mint a releases repository token', 6))

    def test_release_publishes_to_the_cloud_feed_only_when_github_would_publish(self):
        cloud = Gate(condition(block(self.publish, '- name: Publish to the cloud feed', 6), 8))
        github = Gate(condition(block(self.publish, '- name: Create the release', 6), 8))
        prepare = Gate(condition(block(self.publish, '- name: Prepare verified release candidate', 6), 8))
        for done, event, feed in itertools.product((True, False), ('push', 'workflow_dispatch', 'workflow_run'),
                                                   ('', 'github:acme/desktop-releases', 'cloud')):
            context = {'inputs.prepare-only': done, 'github.event_name': event, 'env.RELEASE_FEED': feed}
            publishes = not done and event != 'workflow_run'
            self.assertEqual(cloud(context), publishes and feed == 'cloud', context)
            self.assertEqual(github(context), publishes and feed != 'cloud', context)
            # A cloud release publishes the verified candidate's bytes.
            self.assertEqual(prepare(context), event == 'workflow_run' or done or feed == 'cloud', context)
        step = block(self.publish, '- name: Publish to the cloud feed', 6)
        self.assertIn('release-candidate.py cloud-assets --directory candidate --feed cloud', step)
        self.assertIn('--notes candidate/release-notes.md --directory candidate/assets', step)
        self.assertIn('if [ "${RELEASE_FEED:-}" = cloud ]; then args+=(--feed cloud); fi',
                      block(self.publish, '- name: Prepare verified release candidate', 6))

    def resolve(self, root, listing, version, channel='stable', unlisted='false'):
        resolve = block(self.version, '- name: Resolve', 6)
        if listing is None:
            (root / 'feed-listing.json').unlink(missing_ok=True)
        else:
            (root / 'feed-listing.json').write_text(json.dumps(listing))
        (root / 'output').write_text('')
        env = dict(os.environ, PATH=f'{root}{os.pathsep}' + os.environ['PATH'], RELEASE_FEED='cloud',
                   UNLISTED_CANDIDATE=unlisted,
                   EVENT_NAME='workflow_dispatch', TAG_NAME='', INPUT_VERSION=version, INPUT_CHANNEL=channel,
                   RELEASES_REPOSITORY='', SOURCE_SHA='a' * 40, CI_RUN_ID='', GITHUB_OUTPUT=str(root / 'output'),
                   PROFILE='x7k2q', ASSET_PREFIX='Desktop-x7k2q', GITHUB_STEP_SUMMARY=str(root / 'summary'))
        (root / 'summary').write_text('')
        result = subprocess.run(['bash', '-c', run_block(resolve)], cwd=root, env=env, capture_output=True, text=True)
        return result, (root / 'output').read_text()

    def resolve_root(self, root):
        checkout = WORKFLOW.parents[2]
        (root / 'scripts').mkdir()
        for name in ('release-notes.py', 'macos-release-assets.py', 'release-version.py', 'test_release_version.py'):
            shutil.copyfile(checkout / 'scripts' / name, root / 'scripts' / name)
        gh = root / 'gh'
        gh.write_text('#!/bin/sh\necho "gh called" >&2\nexit 9\n')
        gh.chmod(0o755)
        (root / 'CHANGELOG.md').write_text('## Unreleased\n\n- A change.\n')

    def test_only_a_prepare_only_profile_dispatch_with_a_version_skips_the_feed_listing(self):
        unlisted = re.search(r"(?m)^    env:\n      UNLISTED_CANDIDATE: \$\{\{ (.*) \}\}$", self.version)
        self.assertIsNotNone(unlisted)
        unlisted = Gate(unlisted[1])
        listed = Gate(condition(block(self.version, '- name: List the cloud feed', 6), 8))
        dispatch = itertools.product(('workflow_dispatch',), (True, False), ('', 'x7k2q'), ('', '2026.3.2'))
        automatic = ((event, False, '', '') for event in ('push', 'workflow_run'))
        for (event, done, profile, version), feed in itertools.product(
                itertools.chain(dispatch, automatic), ('', 'github:acme/desktop-releases', 'cloud')):
            context = {'github.event_name': event, 'inputs.prepare-only': done, 'inputs.profile': profile,
                       'inputs.version': version}
            given = unlisted(context)
            self.assertEqual(given, event == 'workflow_dispatch' and done and bool(profile) and bool(version), context)
            context = {'env.RELEASE_FEED': feed, 'env.UNLISTED_CANDIDATE': 'true' if given else 'false'}
            self.assertEqual(listed(context), feed == 'cloud' and not given, (event, done, profile, version, feed))
        # Promotion still publishes through the feed itself, never as given.
        self.assertNotIn('UNLISTED_CANDIDATE', self.promote)
        self.assertNotIn('--as-given', self.promote)

    def test_an_unlisted_candidate_uses_its_version_as_given_and_still_checks_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.resolve_root(root)
            for version, channel in (('2026.3.2', 'stable'), ('2026.3.2-beta.3', 'beta')):
                result, output = self.resolve(root, None, version, channel, unlisted='true')
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(f'version={version}\n', output)
                self.assertIn(f'channel={channel}\n', output)
                self.assertIn('skip=false', output)
                self.assertNotIn('gh called', result.stderr)
                self.assertFalse((root / 'feed-listing.json').exists())
                # Its candidate records it, and the run says it was not checked.
                self.assertIn('unlisted=true\n', output)
                given = f'v{version} was taken as given and not checked against the cloud feed'
                self.assertIn(f'::warning::{given}', result.stdout)
                self.assertIn(given, (root / 'summary').read_text())
            # The version regex and the channel rule still hold.
            for version, channel in (('2026.3.2', 'beta'), ('2026.3.2-beta.1', 'stable'), ('2026.03.2', 'stable'),
                                     ('2026.3', 'stable'), ('latest', 'stable')):
                result, output = self.resolve(root, None, version, channel, unlisted='true')
                self.assertNotEqual(result.returncode, 0, (version, channel))
                self.assertNotIn('version=', output)
            # Any other run still needs the listing: without one it fails closed.
            for unlisted in ('false', ''):
                result, output = self.resolve(root, None, '2026.3.2', unlisted=unlisted)
                self.assertNotEqual(result.returncode, 0, unlisted)
                self.assertNotIn('version=', output)

    def test_cloud_resolve_allocates_against_the_feed_listing_without_github(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.resolve_root(root)
            draft = {'version': '2026.3.1', 'channel': 'stable', 'state': 'draft', 'origin': 'publish',
                     'published_at': None, 'current': False}
            listing = {'versions': [draft], 'next_page': None}
            result, output = self.resolve(root, listing, '2026.3.2')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('version=2026.3.2\n', output)
            self.assertIn('skip=false', output)
            self.assertNotIn('gh called', result.stderr)
            self.assertNotIn('unlisted=', output)
            self.assertEqual((root / 'summary').read_text(), '')
            result, output = self.resolve(root, listing, '2026.3.2', 'beta')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('version=2026.3.2-beta.1\n', output)
            # A draft's version is taken, and a padded month is not a cloud version.
            for version in ('2026.3.1', '2026.03.2'):
                result, output = self.resolve(root, listing, version)
                self.assertNotEqual(result.returncode, 0, version)
                self.assertNotIn('version=', output)

    def test_promote_gates_the_bridge_to_its_own_environment(self):
        inputs = block(block(self.promote, 'on:', 0), 'workflow_dispatch:', 2)
        bridge_input = block(inputs, 'bridge-environment:', 6)
        self.assertIn("default: ''", bridge_input)
        self.assertIn('required: false', bridge_input)
        self.assertIn('needs: promote', self.bridge)
        self.assertIn('    environment: ${{ inputs.bridge-environment }}', self.bridge)
        self.assertIn("if: inputs.bridge-environment != '' && inputs.profile != '' && "
                      "inputs.bridge-environment == format('desktop-{0}-bridge', inputs.profile)", self.bridge)
        step = block(self.promote_job, '- name: Check the bridge environment', 6)
        for feed, profile, bridge, ok in [('cloud', 'x7k2q', '', True),
                                          ('github:acme/desktop-releases', 'x7k2q', '', True),
                                          ('cloud', 'x7k2q', 'desktop-x7k2q-bridge', True),
                                          ('cloud', 'x7k2q', 'desktop-other-bridge', False),
                                          ('cloud', 'x7k2q', 'desktop-x7k2q', False),
                                          ('github:acme/desktop-releases', 'x7k2q', 'desktop-x7k2q-bridge', False)]:
            env = dict(os.environ, RELEASE_FEED=feed, PROFILE=profile, BRIDGE_ENVIRONMENT=bridge)
            result = subprocess.run(['bash', '-c', run_block(step)], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode == 0, ok, (feed, bridge, result.stdout))

    def test_bridge_never_builds_and_reads_only_its_publishing_secrets(self):
        self.assertEqual(set(re.findall(r'secrets\.(\w+)', self.bridge)), BRIDGE_SECRETS)
        for word in ('site-profile', 'cargo', 'swift', 'build-', 'actions/cache', 'setup-', 'RELEASE_FEED'):
            self.assertNotIn(word, self.bridge, word)
        # The same producer and candidate verification as the promote job,
        # and only a cloud candidate is accepted.
        for name in ('Fetch producer provenance', 'Download immutable prepared candidate'):
            bridge, promote = (block(job, '- name: ' + name, 6) for job in (self.bridge, self.promote_job))
            self.assertEqual(code(bridge), code(promote), name)
        verify = block(self.bridge, '- name: Verify identity and every prepared byte', 6)
        self.assertIn('args=(--run-metadata producer-run.json --feed cloud)', verify)
        self.assertIn('--profile "$PROFILE" "${args[@]}"', verify)
        # Only the bridge's own App: never the repository-level RELEASES_TOKEN
        # or releases App pair, which every environment would inherit.
        credential = '${{ steps.releases-token.outputs.token }}'
        mint = block(self.bridge, '- name: Mint a releases repository token', 6)
        self.assertNotIn('if:', mint)
        # Never the profile's releases App, nor the organisation's pusher App.
        self.assertEqual(mint.rstrip(), '\n'.join([
            '      - name: Mint a releases repository token',
            '        id: releases-token',
            '        uses: actions/create-github-app-token@64ab729a6294000f5aa4a4bc0b73d1f32bc45510 # v3.3.0',
            '        with:',
            '          app-id: ${{ secrets.DESKTOP_BRIDGE_APP_ID }}',
            '          private-key: ${{ secrets.DESKTOP_BRIDGE_APP_PRIVATE_KEY }}',
            '          owner: ${{ env.RELEASES_OWNER }}',
            '          repositories: ${{ env.RELEASES_NAME }}',
            '          permission-contents: write',
        ]))
        for word in ('GH_APP_PUSHER', 'USE_ORG_PUSHER', 'use_org_pusher'):
            self.assertNotIn(word, self.bridge, word)
        for use in re.findall(r'^\s+GH_TOKEN: (.*)$', self.bridge, re.M):
            self.assertIn(use, (credential, '${{ github.token }}'))
        publish = block(self.bridge, '- name: Publish prepared assets', 6)
        self.assertIn('--latest=false', publish)
        self.assertIn('--prerelease', publish)

    def test_promote_verifies_and_publishes_a_cloud_candidate_through_the_bridge(self):
        verify = block(self.promote_job, '- name: Verify identity and every prepared byte', 6)
        self.assertIn('if [ "${RELEASE_FEED:-}" = cloud ]; then args+=(--feed cloud); fi', verify)
        step = block(self.promote_job, '- name: Publish prepared assets to the cloud feed', 6)
        self.assertEqual(condition(step, 8), "env.RELEASE_FEED == 'cloud'")
        self.assertIn('release-candidate.py cloud-assets --directory candidate --feed cloud', step)
        self.assertIn('--run-id "$CANDIDATE_RUN_ID"', step)
        self.assertNotIn('gh release', step)

    def test_an_unlisted_candidate_is_recorded_and_promote_checks_the_feed_for_it(self):
        self.assertIn('      unlisted: ${{ steps.resolve.outputs.unlisted }}\n', self.version)
        prepare = block(self.publish, '- name: Prepare verified release candidate', 6)
        self.assertIn('UNLISTED: ${{ needs.version.outputs.unlisted }}', prepare)
        self.assertIn('if [ "${UNLISTED:-}" = true ]; then args+=(--unlisted); fi', prepare)
        # The release's own publish never runs for a prepare-only candidate.
        self.assertNotIn('--unlisted', block(self.publish, '- name: Publish to the cloud feed', 6))
        step = block(self.promote_job, '- name: Publish prepared assets to the cloud feed', 6)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'candidate').mkdir()
            python = root / 'python3'
            python.write_text('#!/bin/sh\necho "$*" >> "$CALLS"\n')
            python.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}{os.pathsep}' + os.environ['PATH'], CALLS=str(root / 'calls'),
                       GITHUB_STEP_SUMMARY=str(root / 'summary'), RELEASE_FEED='cloud', PROFILE='x7k2q',
                       VERSION='2026.3.1', CHANNEL='stable', SITE_URL='https://feed.invalid',
                       ASSET_PREFIX='Desktop-x7k2q', SOURCE_SHA='a' * 40, CANDIDATE_RUN_ID='7',
                       GITHUB_REPOSITORY='AgentWorkforce/relay-app')
            for manifest, unlisted in (({'feed': 'cloud', 'unlisted': True}, True), ({'feed': 'cloud'}, False),
                                       ({'feed': 'cloud', 'unlisted': 'true'}, False)):
                (root / 'candidate' / 'release-candidate.json').write_text(json.dumps(manifest))
                (root / 'calls').write_text('')
                result = subprocess.run(['bash', '-c', run_block(step)], cwd=root, env=env,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                publish = [call for call in (root / 'calls').read_text().splitlines() if ' publish ' in call]
                self.assertEqual(len(publish), 1, manifest)
                self.assertEqual(publish[0].endswith(' --unlisted'), unlisted, manifest)

    def test_bridge_refuses_the_default_repository_and_any_other_credential(self):
        step = block(self.bridge, '- name: Check the bridge repository', 6)
        for repository, has_app, ok in [('acme/desktop-x7k2q-releases', 'true', True),
                                        ('AgentWorkforce/relay-desktop-releases', 'true', False),
                                        ('agentworkforce/Relay-Desktop-Releases', 'true', False),
                                        ('acme/desktop-x7k2q-releases', 'false', False),
                                        ('', 'true', False)]:
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                env = dict(os.environ, PROFILE='x7k2q', RELEASES_REPOSITORY=repository, HAS_APP=has_app,
                           GITHUB_ENV=str(root / 'env'), GITHUB_OUTPUT=str(root / 'output'))
                result = subprocess.run(['bash', '-c', run_block(step)], cwd=WORKFLOW.parents[2], env=env,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, ok, (repository, has_app, result.stdout))
                if not ok and repository:
                    self.assertIn('::error::', result.stdout)

    def test_bridge_publishes_only_a_version_the_updaters_rank_after_every_bridge_release(self):
        step = block(self.bridge, '- name: Check publishing access and reserve exact version', 6)
        checkout = WORKFLOW.parents[2]
        for tags, version, channel, ok in [(['2026.03.2'], '2026.3.1', 'stable', False),
                                           (['2026.03.2'], '2026.3.2', 'stable', False),
                                           (['2026.03.2', '2026.4.1-beta.2'], '2026.4.1-beta.1', 'beta', False),
                                           (['2026.03.2'], '2026.3.3', 'stable', True),
                                           (['2026.03.2', '2026.4.1-beta.2'], '2026.4.1', 'stable', True)]:
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / 'scripts').mkdir()
                shutil.copyfile(checkout / 'scripts' / 'release-version.py', root / 'scripts' / 'release-version.py')
                gh = root / 'gh'
                gh.write_text(FAKE_GH)
                gh.chmod(0o755)
                (root / 'releases').write_text(json.dumps([[{'tag_name': f'v{tag}'} for tag in tags]]))
                env = dict(os.environ, PATH=f'{root}{os.pathsep}' + os.environ['PATH'], GH_TOKEN='app-token',
                           RELEASES_REPOSITORY='acme/desktop-x7k2q-releases', VERSION=version, CHANNEL=channel,
                           FAKE_RELEASES=str(root / 'releases'), GITHUB_OUTPUT=str(root / 'output'))
                result = subprocess.run(['bash', '-c', run_block(step)], cwd=root, env=env,
                                        capture_output=True, text=True)
                context = (tags, version, result.stdout + result.stderr)
                self.assertEqual(result.returncode == 0, ok, context)
                if ok:
                    self.assertIn(f'version={version}\n', (root / 'output').read_text())
                else:
                    self.assertIn(f'::error::v{version} does not sort after every release', result.stdout, context)

    def stage_installers(self, root, **values):
        # The Stage step's substitution, run on the checkout's installers.
        stage = run_block(block(self.publish, '- name: Stage the release assets', 6))
        script = re.split(r'\n *PY\n', stage.split("<<'PY'\n", 1)[1], maxsplit=1)[0]
        (root / 'stage.py').write_text(textwrap.dedent(script))
        (root / 'out').mkdir()
        checkout = WORKFLOW.parents[2]
        env = {'PATH': os.environ['PATH'], 'PROFILE': 'x7k2q', 'PRODUCT_NAME': "Acme's Caf\u00e9",
               'ASSET_PREFIX': 'Desktop-x7k2q', 'RELEASES_REPOSITORY': '', 'RELEASE_FEED': 'cloud',
               'SITE_URL': 'https://relay.acme.test', 'FEED_PUBLIC_KEY': 'A' * 43 + '='}
        env.update(values)
        return subprocess.run(['python3', '-I', str(root / 'stage.py'), str(checkout / 'linux/install.sh'),
                               str(checkout / 'macos/install.sh'), str(checkout / 'windows/install.ps1'),
                               str(root / 'out')],
                              env=env, capture_output=True, text=True)

    def test_a_cloud_profile_stages_every_installer_for_its_site(self):
        # install-macos.sh takes the same seven values as install.sh; a
        # cloud copy refuses to install (scripts/test-macos-install.sh), and
        # the feed never carries it (release-candidate.py cloud_assets).
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = self.stage_installers(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout + result.stderr, '')
            for name in ('install.sh', 'install-macos.sh'):
                shell = subprocess.run(
                    ['bash', '-c', 'eval "$(grep -E \'^(PRODUCT_NAME|RELEASES_REPOSITORY|ASSET_PREFIX|RELEASE_FEED|SITE_URL'
                     '|PROFILE_ID|FEED_PUBLIC_KEY)=\' "$1")"; printf "%s|" "$PRODUCT_NAME" "$RELEASES_REPOSITORY" '
                     '"$ASSET_PREFIX" "$RELEASE_FEED" "$SITE_URL" "$PROFILE_ID" "$FEED_PUBLIC_KEY"', '-',
                     str(root / 'out' / name)], capture_output=True, text=True)
                self.assertEqual(shell.stdout, "Acme's Caf\u00e9||Desktop-x7k2q|cloud|https://relay.acme.test|x7k2q|"
                                 + 'A' * 43 + '=|', name + shell.stderr)
            powershell = (root / 'out/install.ps1').read_text()
            self.assertTrue(powershell.isascii())
            for line in ("$ProductName = 'Acme''s Caf' + [string][char]0x00E9\n", "$ReleasesRepository = ''\n",
                         "$AssetPrefix = 'Desktop-x7k2q'\n", "$ReleaseFeed = 'cloud'\n",
                         "$SiteUrl = 'https://relay.acme.test'\n", "$ProfileId = 'x7k2q'\n",
                         f"$FeedPublicKey = '{'A' * 43}='\n"):
                self.assertEqual(powershell.count(line), 1, line)

    def test_staging_fails_closed_without_a_place_to_install_from(self):
        for values in ({'SITE_URL': ''}, {'FEED_PUBLIC_KEY': ''}, {'RELEASES_REPOSITORY': 'acme/releases'},
                       {'PROFILE': 'acme-7'}, {'RELEASE_FEED': ''}, {'RELEASE_FEED': 'gitlab:acme/releases'},
                       {'RELEASE_FEED': 'github:acme/releases'}, {'PRODUCT_NAME': 'Acme\nCo'}):
            with self.subTest(values=values), tempfile.TemporaryDirectory() as temporary:
                result = self.stage_installers(Path(temporary), **values)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(list((Path(temporary) / 'out').iterdir()), [])
                self.assertNotIn('relay.acme.test', result.stdout + result.stderr)
                self.assertNotIn('Acme', result.stdout + result.stderr)


    def test_pull_bundle_steps_run_only_for_a_cloud_profile_with_a_pull_repository(self):
        gate = "if: env.RELEASE_FEED == 'cloud' && env.PULL_RELEASES_REPOSITORY != ''"
        parts = {part.split('\n', 1)[0]: part for part in steps(self.promote_job)}
        names = ['Check pull repository access', 'Prepare the pull bundle', 'Attest the pull bundle',
                 'Publish the pull bundle']
        for name in names:
            self.assertIn(gate, parts[name], name)
        self.assertIn("if: env.RELEASE_FEED == 'cloud' && env.PULL_RELEASES_REPOSITORY == ''",
                      parts['Skip the pull bundle'])
        order = list(parts)
        cloud = order.index('Publish prepared assets to the cloud feed')
        self.assertLess(order.index('Check pull repository access'), cloud)
        for name in names[1:]:
            self.assertGreater(order.index(name), cloud, name)
        self.assertIn('--request-out publish-request.json', parts['Publish prepared assets to the cloud feed'])
        # The receipt is looked for before the cloud publish and kept right
        # after it, before anything else in the pull bundle can fail.
        self.assertIn(gate, parts['Find the pull receipt'])
        self.assertLess(order.index('Find the pull receipt'), cloud)
        self.assertEqual(order.index('Keep the pull receipt'), cloud + 1)
        self.assertIn(gate + " && steps.pull-receipt.outputs.resume != 'true'", parts['Keep the pull receipt'])

    def test_pull_receipt_is_found_with_the_job_token_and_kept_by_a_pinned_action(self):
        parts = {part.split('\n', 1)[0]: part for part in steps(self.promote_job)}
        find = parts['Find the pull receipt']
        self.assertIn('id: pull-receipt', find)
        self.assertIn('GH_TOKEN: ${{ github.token }}', find)
        self.assertIn('python3 scripts/pull-receipt.py find --repository "$GITHUB_REPOSITORY"', find)
        self.assertIn('--candidate-run-id "$CANDIDATE_RUN_ID"', find)
        self.assertIn('--source-sha "$SOURCE_SHA"', find)
        keep = parts['Keep the pull receipt']
        self.assertRegex(keep, r'uses: actions/upload-artifact@[0-9a-f]{40} # v\d')
        self.assertIn('name: pull-receipt-${{ inputs.profile }}-v${{ inputs.version }}', keep)
        self.assertIn('if-no-files-found: error', keep)
        self.assertIn('overwrite: true', keep)
        self.assertIn('PULL_RESUME: ${{ steps.pull-receipt.outputs.resume }}',
                      parts['Publish prepared assets to the cloud feed'])

    def run_cloud_publish(self, root, resume):
        step = block(self.promote_job, '- name: Publish prepared assets to the cloud feed', 6)
        (root / 'candidate').mkdir(exist_ok=True)
        (root / 'candidate' / 'release-candidate.json').write_text(json.dumps({'feed': 'cloud', 'unlisted': True}))
        (root / 'pull-receipt').mkdir(exist_ok=True)
        (root / 'pull-receipt' / 'publish-request.json').write_text('{"kept": true}\n')
        python = root / 'python3'
        python.write_text('#!/bin/sh\necho "$*" >> "$CALLS"\n')
        python.chmod(0o755)
        (root / 'calls').write_text('')
        env = dict(os.environ, PATH=f'{root}{os.pathsep}' + os.environ['PATH'], CALLS=str(root / 'calls'),
                   GITHUB_STEP_SUMMARY=str(root / 'summary'), RELEASE_FEED='cloud', PROFILE='x7k2q',
                   VERSION='2026.3.1', CHANNEL='stable', SITE_URL='https://feed.invalid',
                   ASSET_PREFIX='Desktop-x7k2q', SOURCE_SHA='a' * 40, CANDIDATE_RUN_ID='7',
                   GITHUB_REPOSITORY='AgentWorkforce/relay-app', GITHUB_RUN_ID='99',
                   PULL_RELEASES_REPOSITORY='example-org/desktop-pull', PULL_RESUME=resume)
        result = subprocess.run(['bash', '-c', run_block(step)], cwd=root, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return (root / 'calls').read_text().splitlines()

    def test_a_resumed_promote_never_publishes_the_unlisted_version_again(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            calls = self.run_cloud_publish(root, 'true')
            self.assertFalse([call for call in calls if ' publish ' in call], calls)
            self.assertFalse([call for call in calls if 'pull-receipt.py write' in call], calls)
            check = [call for call in calls if 'pull-receipt.py check' in call]
            self.assertEqual(len(check), 1, calls)
            self.assertIn('--receipt pull-receipt --assets cloud-assets.txt --directory candidate/assets', check[0])
            self.assertEqual((root / 'publish-request.json').read_text(), '{"kept": true}\n')

    def test_a_first_promote_publishes_then_writes_the_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            calls = self.run_cloud_publish(root, 'false')
            publish = [index for index, call in enumerate(calls) if ' publish ' in call]
            write = [index for index, call in enumerate(calls) if 'pull-receipt.py write' in call]
            self.assertEqual(len(publish), 1, calls)
            self.assertIn('--request-out publish-request.json', calls[publish[0]])
            self.assertTrue(calls[publish[0]].endswith(' --unlisted'), calls)
            self.assertEqual(len(write), 1, calls)
            self.assertGreater(write[0], publish[0])
            for argument in ('--candidate-run-id 7', f'--source-sha {"a" * 40}', '--run-id 99',
                             '--profile x7k2q', '--version 2026.3.1', '--channel stable'):
                self.assertIn(argument, calls[write[0]])
            self.assertFalse([call for call in calls if 'pull-receipt.py check' in call], calls)

    def test_pull_bundle_lists_its_profile_binding(self):
        prepare = block(self.promote_job, '- name: Prepare the pull bundle', 6)
        self.assertIn("'{profile: $profile, version: $version, channel: $channel}' > pull/pull-bundle.json", prepare)
        self.assertIn('sha256sum -- pull-bundle.json publish-request.json', prepare)

    def test_pull_bundle_is_attested_by_a_pinned_action_in_the_promote_job_only(self):
        attest = [part for part in steps(self.promote_job) if part.startswith('Attest the pull bundle')][0]
        self.assertRegex(attest, r'uses: actions/attest-build-provenance@[0-9a-f]{40} # v\d')
        self.assertIn('subject-path: pull/SHA256SUMS', attest)
        for permission in ('id-token: write', 'attestations: write'):
            self.assertIn(permission, self.promote_job)
            self.assertNotIn(permission, self.bridge)
            self.assertNotIn(permission, self.promote.split('\njobs:', 1)[0])

    def test_pull_bundle_never_reads_the_publisher_token(self):
        for part in steps(self.promote_job):
            if 'pull' in part.split('\n', 1)[0]:
                self.assertNotIn('DESKTOP_FEED_PUBLISHER_TOKEN', part)


RECEIPT_TOOL = Path(__file__).resolve().parent / 'pull-receipt.py'
REPOSITORY = 'AgentWorkforce/relay-app'
FAKE_RECEIPT_GH = """#!/bin/sh
echo "$*" >> "$FAKE_DIR/gh.log"
for a in "$@"; do
  case "$a" in
    repos/*/actions/artifacts) cat "$FAKE_DIR/artifacts.json"; exit 0 ;;
    repos/*/actions/artifacts/*/zip) id="${a%/zip}"; cat "$FAKE_DIR/zip-${id##*/}"; exit 0 ;;
    repos/*/actions/runs/*) cat "$FAKE_DIR/run-${a##*/}.json"; exit 0 ;;
  esac
done
echo 'gh: Not Found (HTTP 404)' >&2
exit 1
"""


class PullReceiptTests(unittest.TestCase):
    """scripts/pull-receipt.py: an unlisted candidate's cloud publication is
    resumed only for the same candidate, and only from Desktop Promote on main."""

    BINDING = ['--profile', 'x7k2q', '--version', '2026.3.1', '--channel', 'stable',
               '--candidate-run-id', '7', '--source-sha', 'a' * 40]

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.fake = self.root / 'fake'
        self.fake.mkdir()
        gh = self.fake / 'gh'
        gh.write_text(FAKE_RECEIPT_GH)
        gh.chmod(0o755)
        self.artifacts = []
        self.request = b'{"version": "2026.3.1", "channel": "stable", "expected_assets": []}\n'

    def tearDown(self):
        self.temporary.cleanup()

    def tool(self, *args):
        env = dict(os.environ, PATH=f'{self.fake}{os.pathsep}' + os.environ['PATH'], FAKE_DIR=str(self.fake))
        env.pop('GITHUB_OUTPUT', None)
        return subprocess.run(['python3', '-I', str(RECEIPT_TOOL), *args], cwd=self.root, env=env,
                              capture_output=True, text=True)

    def keep(self, run_id, artifact_id, binding=None, run=None, request=None, receipt=None):
        """A receipt artifact as an earlier promote run kept it."""
        written = self.root / f'written-{artifact_id}'
        (self.root / 'request.json').write_bytes(request or self.request)
        result = self.tool('write', *(binding or self.BINDING), '--request', str(self.root / 'request.json'),
                           '--output', str(written), '--run-id', str(run_id))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        if receipt is not None:
            receipt(written)
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, 'w') as zipped:
            for path in sorted(written.iterdir()):
                zipped.write(path, path.name)
        (self.fake / f'zip-{artifact_id}').write_bytes(archive.getvalue())
        trusted = {'path': '.github/workflows/desktop-promote.yml', 'head_branch': 'main',
                   'event': 'workflow_dispatch', 'repository': {'full_name': REPOSITORY},
                   'head_repository': {'full_name': REPOSITORY}}
        (self.fake / f'run-{run_id}.json').write_text(json.dumps(dict(trusted, **(run or {}))))
        self.artifacts.append({'id': artifact_id, 'name': 'pull-receipt-x7k2q-v2026.3.1', 'expired': False,
                               'workflow_run': {'id': run_id}})
        return written

    def find(self):
        (self.fake / 'artifacts.json').write_text(json.dumps([{'artifacts': self.artifacts}]))
        output = self.root / 'github-output'
        output.write_text('')
        result = self.tool('find', '--repository', REPOSITORY, *self.BINDING, '--output', 'resumed',
                           '--github-output', str(output))
        return result, output.read_text()

    def test_no_receipt_is_a_first_publication(self):
        result, output = self.find()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output, 'resume=false\n')
        self.assertFalse((self.root / 'resumed').exists())
        self.assertIn('-f name=pull-receipt-x7k2q-v2026.3.1', (self.fake / 'gh.log').read_text())

    def test_the_same_candidates_receipt_resumes_with_the_request_sent(self):
        written = self.keep(41, 501)
        result, output = self.find()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output, 'resume=true\n')
        for name in ('publish-request.json', 'receipt.json'):
            self.assertEqual((self.root / 'resumed' / name).read_bytes(), (written / name).read_bytes())
        receipt = json.loads((written / 'receipt.json').read_text())
        self.assertEqual(receipt['candidate_run_id'], '7')
        self.assertEqual(receipt['promote_run_id'], 41)
        self.assertEqual(receipt['request_sha256'], hashlib.sha256(self.request).hexdigest())

    def test_another_candidates_receipt_is_refused(self):
        other = list(self.BINDING)
        other[other.index('--candidate-run-id') + 1] = '8'
        self.keep(41, 501, binding=other)
        result, output = self.find()
        self.assertEqual(result.returncode, 1)
        self.assertIn('::error::v2026.3.1 was already published to the cloud feed by Desktop Promote run 41'
                      ' from candidate run 8', result.stdout)
        self.assertEqual(output, '')
        self.assertFalse((self.root / 'resumed').exists())

    def test_another_source_commit_is_refused_even_with_the_same_run_id(self):
        other = list(self.BINDING)
        other[other.index('--source-sha') + 1] = 'b' * 40
        self.keep(41, 501, binding=other)
        result, _ = self.find()
        self.assertEqual(result.returncode, 1)
        self.assertIn('promote this candidate under a new version', result.stdout)

    def test_a_receipt_kept_anywhere_but_desktop_promote_on_main_is_ignored(self):
        for index, run in enumerate(({'head_branch': 'feature'}, {'event': 'push'},
                                     {'path': '.github/workflows/other.yml'},
                                     {'head_repository': {'full_name': 'example-org/relay-desktop'}})):
            self.keep(41 + index, 501 + index, run=run)
        result, output = self.find()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output, 'resume=false\n')
        self.assertEqual(result.stdout.count('::warning::Ignoring the artifact'), 4)

    def test_an_untrusted_receipt_for_another_candidate_does_not_block(self):
        other = list(self.BINDING)
        other[other.index('--candidate-run-id') + 1] = '8'
        self.keep(41, 501, binding=other, run={'head_branch': 'feature'})
        self.keep(42, 502)
        result, output = self.find()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output, 'resume=true\n')

    def test_a_changed_request_is_refused(self):
        self.keep(41, 501, receipt=lambda written: (written / 'publish-request.json').write_text('{}\n'))
        result, output = self.find()
        self.assertEqual(result.returncode, 1)
        self.assertIn('::error::the pull receipt kept by Desktop Promote run 41 is inconsistent', result.stdout)
        self.assertEqual(output, '')

    def test_a_receipt_naming_another_promote_run_is_refused(self):
        self.keep(41, 501, receipt=lambda written: self.rewrite(written, promote_run_id=40))
        result, _ = self.find()
        self.assertEqual(result.returncode, 1)
        self.assertIn('is inconsistent', result.stdout)

    def test_a_receipt_with_extra_files_is_refused(self):
        self.keep(41, 501, receipt=lambda written: (written / 'extra').write_text('x'))
        result, _ = self.find()
        self.assertEqual(result.returncode, 1)
        self.assertIn('holds other files', result.stdout)

    @staticmethod
    def rewrite(written, **fields):
        receipt = json.loads((written / 'receipt.json').read_text())
        receipt.update(fields)
        (written / 'receipt.json').write_text(json.dumps(receipt))

    def candidate(self):
        assets = self.root / 'assets'
        assets.mkdir(exist_ok=True)
        entries = []
        for name, data in (('Desktop-x7k2q-2026.3.1-Linux-x64.deb', b'deb'), ('install.sh', b'#!/bin/sh\n')):
            (assets / name).write_bytes(data)
            entries.append({'name': name, 'sha256': hashlib.sha256(data).hexdigest(), 'size': len(data)})
        (self.root / 'cloud-assets.txt').write_text(''.join(entry['name'] + '\n' for entry in entries))
        kept = self.root / 'kept'
        kept.mkdir(exist_ok=True)
        request = {'version': '2026.3.1', 'channel': 'stable', 'expected_assets': entries}
        (kept / 'publish-request.json').write_text(json.dumps(request))
        return assets, kept

    def check(self):
        return self.tool('check', '--receipt', 'kept', '--assets', 'cloud-assets.txt', '--directory', 'assets',
                         '--version', '2026.3.1', '--channel', 'stable')

    def test_check_accepts_the_candidates_own_assets(self):
        self.candidate()
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("declares the candidate's 2 cloud assets", result.stdout)

    def test_check_refuses_a_changed_asset(self):
        assets, _ = self.candidate()
        (assets / 'install.sh').write_bytes(b'#!/bin/sh\nexit 1\n')
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn('::error::the kept publish-request.json declares install.sh with another sha256', result.stdout)

    def test_check_refuses_a_missing_or_extra_asset(self):
        self.candidate()
        (self.root / 'cloud-assets.txt').write_text('install.sh\n')
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("does not declare exactly the candidate's cloud assets", result.stdout)

    def test_check_refuses_another_version(self):
        self.candidate()
        result = self.tool('check', '--receipt', 'kept', '--assets', 'cloud-assets.txt', '--directory', 'assets',
                           '--version', '2026.3.2', '--channel', 'stable')
        self.assertEqual(result.returncode, 1)
        self.assertIn('does not declare v2026.3.2 on the stable channel', result.stdout)


if __name__ == '__main__':
    unittest.main()
