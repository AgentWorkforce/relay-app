#!/usr/bin/env python3
"""Exercise native Windows packaging with tiny component runtime fixtures."""

import argparse
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location('windows_package', Path(__file__).with_name('package-windows.py'))
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)


class WindowsPackageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.args = argparse.Namespace(version='2026.11.1', arch='x64', source_sha='a' * 40,
            app_runtime=self.root / 'app', core_runtime=self.root / 'core',
            out=self.root / 'out', installer=False, iscc=None)
        for component in ('core', 'app'):
            directory = getattr(self.args, component + '_runtime')
            for name in packaging.required_files(component):
                self.write(directory, name, name.encode())
            self.write(directory, 'bin/libshared.dll', b'shared native dependency')
        self.write(self.args.app_runtime, 'NOTICES.txt', 'Лицензия GTK — © 🪿\n'.encode('utf-8'))
        self.write(self.args.core_runtime, 'NOTICES.txt', 'Лицензия core — © 🪿\n'.encode('utf-8'))
        self.write(self.args.app_runtime, 'bin/libgtk-4-1.dll', b'GTK')
        self.write(self.args.core_runtime, 'bin/libcore-only.dll', b'core dependency')
        self.record()

    @staticmethod
    def write(directory, name, data):
        path = directory / 'files' / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def record(self, arch='x64'):
        for component in ('core', 'app'):
            packaging.record(getattr(self.args, component + '_runtime'), component,
                             self.args.version, arch, self.args.source_sha)

    def test_complete_runtime_and_minimal_probe_archives(self):
        packaging.package(self.args)
        prefix = 'AgentRelay-2026.11.1-Windows-x64'
        with zipfile.ZipFile(self.args.out / (prefix + '.zip')) as archive:
            names = set(archive.namelist())
            self.assertIn('AgentRelay-Windows-x64/bin/libcore-only.dll', names)
            self.assertIn('AgentRelay-Windows-x64/bin/libgtk-4-1.dll', names)
            self.assertIn('AgentRelay-Windows-x64/share/glib-2.0/schemas/gschemas.compiled', names)
        with zipfile.ZipFile(self.args.out / (prefix + '-probe.zip')) as archive:
            self.assertEqual(set(archive.namelist()), {
                'agent_relay/LICENSE.txt', 'agent_relay/NOTICES.txt',
                'agent_relay/helpers/agent-relay-probe.exe',
                'agent_relay/helpers/libshared.dll', 'agent_relay/helpers/libcore-only.dll',
            })
        for name in [prefix + '.zip', prefix + '-probe.zip']:
            self.assertEqual((self.args.out / (name + '.sha256')).read_text(),
                             f'{packaging.digest(self.args.out / name)}  {name}\n')
        self.assertIn('Лицензия core — © 🪿', (self.args.out / 'AgentRelay-Windows-x64/NOTICES.txt').read_text(encoding='utf-8'))
        self.assertIn('Core runtime notices', (self.args.out / 'AgentRelay-Windows-x64/NOTICES.txt').read_text())

    def test_arm64_packages_keep_architecture_identity(self):
        self.args.arch = 'arm64'
        self.record(arch='arm64')
        packaging.package(self.args)
        self.assertTrue((self.args.out / 'AgentRelay-2026.11.1-Windows-arm64.zip').is_file())

    def test_changed_missing_and_added_files_are_rejected_before_output(self):
        path = self.args.core_runtime / 'files/bin/libshared.dll'
        path.write_bytes(b'corrupt download')
        with self.assertRaisesRegex(ValueError, 'inventory'):
            packaging.package(self.args)
        self.assertFalse(self.args.out.exists())
        path.unlink()
        with self.assertRaisesRegex(ValueError, 'inventory'):
            packaging.package(self.args)
        self.write(self.args.core_runtime, 'unexpected', b'extra')
        with self.assertRaisesRegex(ValueError, 'inventory'):
            packaging.package(self.args)

    def test_wrong_component_architecture_version_or_source_rejected(self):
        manifest_path = self.args.core_runtime / 'runtime.json'
        original = json.loads(manifest_path.read_text())
        for key, value in [('arch', 'arm64'), ('version', '2026.11.2'),
                           ('component', 'app'), ('source_sha', 'b' * 40)]:
            manifest_path.write_text(json.dumps({**original, key: value}))
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'identity|revision'):
                packaging.package(self.args)
        manifest_path.write_text(json.dumps(original))

    def test_two_different_native_library_builds_are_not_silently_merged(self):
        self.write(self.args.core_runtime, 'bin/libshared.dll', b'newer incompatible library')
        self.record()
        with self.assertRaisesRegex(ValueError, 'Conflicting.*libshared.dll'):
            packaging.package(self.args)
        self.assertFalse(self.args.out.exists())

    def test_unmatched_sources_are_rejected_even_without_expected_sha(self):
        packaging.record(self.args.core_runtime, 'core', self.args.version, 'x64', 'b' * 40)
        self.args.source_sha = ''
        with self.assertRaisesRegex(ValueError, 'source revisions differ'):
            packaging.package(self.args)

    def test_runtimes_carry_the_site_profile_they_were_built_for(self):
        # Recorded without one, a runtime is the default profile's.
        for component in ('core', 'app'):
            manifest = json.loads((getattr(self.args, component + '_runtime') / 'runtime.json').read_text())
            self.assertEqual(manifest['build'], packaging.DEFAULT_BUILD)
        profile = dict(site_url='https://relay.example', url_scheme='acme-relay', product_name='Acme Desktop',
                       asset_prefix='Desktop-acme-7', releases_repository='acme/desktop-releases',
                       support_url='https://acme.example/support')
        # An app and a core built for different profiles are never merged.
        packaging.record(self.args.core_runtime, 'core', self.args.version, 'x64', self.args.source_sha, profile)
        with self.assertRaisesRegex(ValueError, 'different site profiles'):
            packaging.package(self.args)
        self.assertFalse(self.args.out.exists())
        # Built for the same profile, the packages take its artifact prefix
        # and the installer is compiled from its values.
        packaging.record(self.args.app_runtime, 'app', self.args.version, 'x64', self.args.source_sha, profile)
        self.args.installer = True
        self.args.iscc = str(self.root / 'ISCC.exe')
        (self.root / 'ISCC.exe').write_text('')
        compiled = []

        def iscc(command, check):
            compiled.append(command)
            defines = dict(arg[2:].split('=', 1) for arg in command if arg.startswith('/D'))
            (Path(defines['OutputDir']) / f"{defines['AssetPrefix']}-{defines['Version']}-Windows-{defines['Arch']}-Setup.exe").write_bytes(b'setup')
        with patch.object(packaging.subprocess, 'run', iscc):
            packaging.package(self.args)
        self.assertEqual(sorted(path.name for path in self.args.out.iterdir()), [
            'AgentRelay-Windows-x64',
            'Desktop-acme-7-2026.11.1-Windows-x64-Setup.exe', 'Desktop-acme-7-2026.11.1-Windows-x64-Setup.exe.sha256',
            'Desktop-acme-7-2026.11.1-Windows-x64-probe.zip', 'Desktop-acme-7-2026.11.1-Windows-x64-probe.zip.sha256',
            'Desktop-acme-7-2026.11.1-Windows-x64.zip', 'Desktop-acme-7-2026.11.1-Windows-x64.zip.sha256'])
        [command] = compiled
        self.assertEqual(command[0], self.args.iscc)
        self.assertEqual([arg for arg in command if arg.startswith(('/DProductName', '/DUrlScheme', '/DAssetPrefix', '/DSiteUrl', '/DSupportUrl'))], [
            '/DProductName=Acme Desktop', '/DUrlScheme=acme-relay', '/DAssetPrefix=Desktop-acme-7',
            '/DSiteUrl=https://relay.example', '/DSupportUrl=https://acme.example/support'])
        self.assertTrue(command[-1].endswith('AgentRelay.iss'))

    def test_an_explicit_github_feed_is_checked_against_the_repository(self):
        def given(**values):
            keys = [*packaging.DEFAULT_BUILD, *packaging.FEED_PATTERNS]
            return argparse.Namespace(**{key: values.get(key, '') for key in keys})
        # A feed naming another repository than the one given is refused.
        with self.assertRaisesRegex(ValueError, 'names the releases repository'):
            packaging.build_values(given(release_feed='github:acme/releases',
                                         releases_repository='other/releases'))
        # A feed alone names the repository; it is not the default's.
        build = packaging.build_values(given(release_feed='github:acme/releases'))
        self.assertEqual(build['releases_repository'], 'acme/releases')
        self.assertNotIn('release_feed', build)
        # A feed and the repository it names agree.
        build = packaging.build_values(given(release_feed='github:Acme/Releases',
                                             releases_repository='acme/releases'))
        self.assertEqual(build['releases_repository'], 'acme/releases')
        self.assertNotIn('release_feed', build)
        # Neither given is the default profile's.
        self.assertEqual(packaging.build_values(given()), packaging.DEFAULT_BUILD)

    def test_a_missing_or_malformed_build_profile_is_rejected(self):
        manifest_path = self.args.core_runtime / 'runtime.json'
        original = json.loads(manifest_path.read_text())
        for build in [None, {}, {**original['build'], 'asset_prefix': 'Acme Desktop'},
                      {**original['build'], 'url_scheme': 'https'}, {**original['build'], 'extra': 'x'}]:
            manifest = {**original, 'build': build}
            if build is None:
                del manifest['build']
            manifest_path.write_text(json.dumps(manifest))
            with self.subTest(build=build), self.assertRaisesRegex(ValueError, 'build profile'):
                packaging.package(self.args)
        self.assertFalse(self.args.out.exists())
        manifest_path.write_text(json.dumps(original))

    def test_asset_prefix_takes_the_mac_updaters_id_grammar(self):
        # One id grammar everywhere: build-app.sh --asset-prefix and the Mac
        # updater refuse an id ending in a hyphen or holding two together.
        pattern = packaging.BUILD_PATTERNS['asset_prefix']
        for good in ('AgentRelay', 'Desktop-acme-7', 'Desktop-' + 'a' * 40):
            with self.subTest(prefix=good):
                self.assertTrue(re.fullmatch(pattern, good))
        for bad in ('Desktop-', 'Desktop--acme', 'Desktop-acme-', 'Desktop-a--b', 'Desktop-' + 'a' * 41, 'Desktop-Acme'):
            with self.subTest(prefix=bad):
                self.assertIsNone(re.fullmatch(pattern, bad))

    def test_setup_replaces_another_product(self):
        # One Agent Relay app per machine: every profile keeps the AppId,
        # install root and logon task, and Setup installs over another
        # product as an upgrade, without refusing or asking, then removes
        # the replaced product's link handler and desktop shortcut.
        iss = (packaging.REPO / 'windows/installer/AgentRelay.iss').read_text()
        self.assertNotIn('cannot be installed over', iss)
        self.assertNotIn('MsgBox(Existing', iss)
        init = iss.split('function InitializeSetup: Boolean;', 1)[1].split('end;', 1)[0]
        self.assertIn('Result := True;', init)
        self.assertNotIn('Result := False;', init)
        self.assertNotIn('MsgBox', init)
        self.assertIn("RegQueryStringValue(HKCU, UninstallKey, 'DisplayName', Result)", iss)
        self.assertIn("(Result = '{#ProductName}')", iss)
        self.assertIn('UninstallDisplayName={#ProductName}', iss)
        self.assertIn('Parameters: "--install-launcher"', iss)
        remove = iss.split('procedure RemoveReplacedProduct;', 1)[1].split('\nend;', 1)[0]
        self.assertIn("(CompareText(Names[I], '{#UrlScheme}') <> 0)", remove)
        self.assertIn("RegValueExists(HKCU, Key, 'URL Protocol')", remove)
        self.assertIn('RunsFromRoot(Command)', remove)
        self.assertIn("DeleteFile(ExpandConstant('{userdesktop}\\') + ReplacedProduct + '.lnk')", remove)
        post = iss.split('procedure CurStepChanged(CurStep: TSetupStep);', 1)[1]
        self.assertIn('RemoveReplacedProduct;', post.split('\nend;', 1)[0])

    def test_product_name_refuses_windows_file_name_characters(self):
        pattern = packaging.BUILD_PATTERNS['product_name']
        self.assertTrue(re.fullmatch(pattern, 'Acme Relay'))
        for bad in ('Acme: Relay', 'Acme Relay {Beta}', 'A,B', 'what?', 'a*b', 'a<b', 'a>b', 'a|b'):
            with self.subTest(name=bad):
                self.assertIsNone(re.fullmatch(pattern, bad))

    def test_case_collisions_are_rejected(self):
        # Windows cannot create this fixture, but archives produced elsewhere
        # must not acquire different behavior when unpacked there.
        self.write(self.args.core_runtime, 'bin/LIBSHARED.dll', b'different')
        if len([p for p in (self.args.core_runtime / 'files/bin').iterdir()
                 if p.name.casefold() == 'libshared.dll']) < 2:
            self.skipTest('case-insensitive filesystem')
        with self.assertRaisesRegex(ValueError, 'collision'):
            self.record()

    @unittest.skipIf(os.name == 'nt', 'Windows symlink creation requires developer mode')
    def test_symlink_payload_rejected(self):
        (self.args.core_runtime / 'files/bin/link.dll').symlink_to(self.root / 'outside')
        with self.assertRaisesRegex(ValueError, 'Links'):
            self.record()

    def test_incomplete_runtime_cannot_be_recorded(self):
        (self.args.app_runtime / 'files/bin/gdbus.exe').unlink()
        with self.assertRaisesRegex(ValueError, 'Missing app runtime'):
            self.record()

    def test_installer_receives_native_paths_and_output_is_verified(self):
        self.args.installer = True
        compiler = self.root / 'Program Files/Inno Setup 6/ISCC.exe'
        compiler.parent.mkdir(parents=True)
        compiler.write_bytes(b'compiler fixture')
        self.args.iscc = str(compiler)

        def compile_installer(command, check):
            self.assertTrue(check)
            self.assertEqual(command[0], str(compiler))
            self.assertIn(f'/DSourceDir={(self.args.out / "AgentRelay-Windows-x64").resolve()}', command)
            self.assertIn('/DArch=x64', command)
            (self.args.out / 'AgentRelay-2026.11.1-Windows-x64-Setup.exe').write_bytes(b'installer')

        with patch.object(packaging.subprocess, 'run', side_effect=compile_installer):
            packaging.package(self.args)
        self.assertTrue((self.args.out / 'AgentRelay-2026.11.1-Windows-x64-Setup.exe.sha256').is_file())
        with patch.object(packaging.subprocess, 'run'):
            with self.assertRaisesRegex(ValueError, 'did not create'):
                packaging.package(self.args)

    @staticmethod
    def portable(data=b''):
        """A minimal PE image: an MZ header whose e_lfanew points at PE\\0\\0."""
        return b'MZ' + bytes(0x3A) + (0x40).to_bytes(4, 'little') + b'PE\0\0' + data

    def make_portable(self):
        """Give the fixtures real PE headers; return the staged tree's PE files."""
        names = {'app': ['bin/agent-relay.exe', 'bin/gdbus.exe', 'bin/gspawn-win64-helper.exe',
                         'bin/gspawn-win64-helper-console.exe', 'bin/libshared.dll', 'bin/libgtk-4-1.dll'],
                 'core': ['bin/agent-relay-probe.exe', 'bin/libshared.dll', 'bin/libcore-only.dll']}
        for component, files in names.items():
            for name in files:
                self.write(getattr(self.args, component + '_runtime'), name, self.portable(name.encode()))
        # A loader that is a PE file under another extension is signed too; a
        # .dll-named file that is not a PE image is not.
        self.write(self.args.app_runtime, 'lib/gdk-pixbuf-2.0/2.10.0/loaders/png.loader', self.portable(b'png'))
        self.write(self.args.app_runtime, 'share/not-a-library.dll', b'MZ but no PE header')
        self.record()
        tree = (self.args.out / 'AgentRelay-Windows-x64').resolve()
        return sorted(str(tree / name) for name in {*names['app'], *names['core'],
                                                     'lib/gdk-pixbuf-2.0/2.10.0/loaders/png.loader'})

    def test_portable_executables_are_found_by_their_header(self):
        cases = {'image.exe': (self.portable(), True), 'loader.bin': (self.portable(b'x'), True),
                 'text.dll': (b'GTK', False), 'short.exe': (b'MZ', False),
                 'dos.exe': (b'MZ' + bytes(0x3A) + (0x40).to_bytes(4, 'little') + b'NE\0\0', False),
                 'far.exe': (b'MZ' + bytes(0x3A) + (0x4000).to_bytes(4, 'little'), False)}
        for name, (data, expected) in cases.items():
            with self.subTest(name=name):
                path = self.root / name
                path.write_bytes(data)
                self.assertEqual(packaging.portable_executable(path), expected)

    def test_every_portable_executable_is_signed_before_anything_is_zipped(self):
        expected = self.make_portable()
        self.args.sign_command = '"C:\\Program Files\\PowerShell\\7\\pwsh.exe" -File "C:\\t\\sign.ps1"'
        calls = []

        def sign(command, check):
            self.assertTrue(check)
            self.assertIsInstance(command, str)
            self.assertEqual(list(self.args.out.glob('*.zip')), [], 'signed after zipping')
            calls.append(command)
            for name in expected:
                with Path(name).open('ab') as stream:
                    stream.write(b'SIGNED')

        with patch.object(packaging.subprocess, 'run', side_effect=sign):
            packaging.package(self.args)
        self.assertEqual(calls, [self.args.sign_command + ' ' + packaging.subprocess.list2cmdline(expected)])
        prefix = 'AgentRelay-2026.11.1-Windows-x64'
        with zipfile.ZipFile(self.args.out / (prefix + '.zip')) as archive:
            self.assertTrue(archive.read('AgentRelay-Windows-x64/bin/agent-relay.exe').endswith(b'SIGNED'))
            self.assertTrue(archive.read('AgentRelay-Windows-x64/bin/libgtk-4-1.dll').endswith(b'SIGNED'))
            self.assertEqual(archive.read('AgentRelay-Windows-x64/share/not-a-library.dll'), b'MZ but no PE header')
        # The probe archive carries the signed copies of the core's closure.
        with zipfile.ZipFile(self.args.out / (prefix + '-probe.zip')) as archive:
            for name in ['agent-relay-probe.exe', 'libshared.dll', 'libcore-only.dll']:
                self.assertTrue(archive.read('agent_relay/helpers/' + name).endswith(b'SIGNED'), name)
        # The hash-bound runtimes themselves are never touched.
        packaging.verify(self.args.app_runtime, 'app', self.args.version, 'x64', self.args.source_sha)
        packaging.verify(self.args.core_runtime, 'core', self.args.version, 'x64', self.args.source_sha)

    def test_unsigned_packaging_runs_no_sign_command(self):
        self.make_portable()
        with patch.object(packaging.subprocess, 'run') as run:
            packaging.package(self.args)
        run.assert_not_called()
        with zipfile.ZipFile(self.args.out / 'AgentRelay-2026.11.1-Windows-x64-probe.zip') as archive:
            self.assertEqual(archive.read('agent_relay/helpers/libcore-only.dll'),
                             self.portable(b'bin/libcore-only.dll'))

    def test_signing_batches_stay_within_the_command_line_limit(self):
        expected = self.make_portable()
        self.args.sign_command = 'sign'
        calls = []
        limit = len(expected[0]) * 3
        with patch.object(packaging, 'SIGN_COMMAND_LIMIT', limit), \
                patch.object(packaging.subprocess, 'run', side_effect=lambda command, check: calls.append(command)):
            packaging.package(self.args)
        self.assertGreater(len(calls), 1)
        for command in calls:
            self.assertTrue(command.startswith('sign '))
            self.assertLessEqual(len(command), limit, command)
        # Each batch is as full as the limit allows: the next file would not fit.
        for command, following in zip(calls, calls[1:]):
            self.assertGreater(len(command + ' ' + following.split(' ')[1]), limit)
        self.assertEqual(sorted(name for command in calls for name in command.split(' ')[1:]), expected)

    def test_a_batch_exactly_at_the_limit_runs_once(self):
        expected = self.make_portable()
        self.args.sign_command = 'sign'
        calls = []
        line = 'sign ' + ' '.join(expected)
        with patch.object(packaging, 'SIGN_COMMAND_LIMIT', len(line)), \
                patch.object(packaging.subprocess, 'run', side_effect=lambda command, check: calls.append(command)):
            packaging.package(self.args)
        self.assertEqual([sorted(command.split(' ')[1:]) for command in calls], [expected])

    def test_the_sign_command_is_one_line_and_only_for_packaging(self):
        packaging_args = ['--version', '2026.11.1', '--arch', 'x64', '--source-sha', 'a' * 40,
                          '--app-runtime', str(self.args.app_runtime),
                          '--core-runtime', str(self.args.core_runtime), '--out', str(self.args.out)]
        record_args = ['--version', '2026.11.1', '--arch', 'x64', '--source-sha', 'a' * 40,
                       '--record', str(self.args.core_runtime), '--component', 'core']
        cases = {
            'with --record': record_args + ['--sign-command', 'sign'],
            'empty': packaging_args + ['--sign-command', ''],
            'blank': packaging_args + ['--sign-command', ' \t '],
            'carriage return': packaging_args + ['--sign-command', 'sign\rother'],
            'line feed': packaging_args + ['--sign-command', 'sign\nother'],
            'trailing line feed': packaging_args + ['--sign-command', 'sign\n'],
            'NUL': packaging_args + ['--sign-command', 'sign\0other'],
        }
        for case, argv in cases.items():
            with self.subTest(case), patch.object(sys, 'argv', ['package-windows.py', *argv]), \
                    patch.object(packaging, 'package') as package, \
                    patch.object(packaging, 'record') as record, \
                    contextlib.redirect_stderr(io.StringIO()) as stderr:
                with self.assertRaises(SystemExit) as raised:
                    packaging.main()
                self.assertEqual(raised.exception.code, 2)
                self.assertIn('--sign-command is a one-line command, and only for packaging', stderr.getvalue())
                package.assert_not_called()
                record.assert_not_called()
        # The same arguments with a one-line command reach packaging.
        with patch.object(sys, 'argv', ['package-windows.py', *packaging_args, '--sign-command', 'sign /v']), \
                patch.object(packaging, 'package') as package:
            packaging.main()
        self.assertEqual(package.call_args.args[0].sign_command, 'sign /v')

    def test_a_failed_signing_stops_packaging(self):
        self.make_portable()
        self.args.sign_command = 'sign'
        error = packaging.subprocess.CalledProcessError(1, 'sign')
        with patch.object(packaging.subprocess, 'run', side_effect=error):
            with self.assertRaises(packaging.subprocess.CalledProcessError):
                packaging.package(self.args)
        self.assertEqual(list(self.args.out.glob('*.zip')), [])

    def test_setup_and_its_uninstaller_are_signed_through_inno(self):
        iss = (packaging.REPO / 'windows/installer/AgentRelay.iss').read_text()
        self.assertIn('#ifdef SignTool\nSignTool={#SignTool}\nSignedUninstaller=yes\n#endif', iss)
        self.make_portable()
        self.args.installer = True
        compiler = self.root / 'ISCC.exe'
        compiler.write_bytes(b'compiler fixture')
        self.args.iscc = str(compiler)
        self.args.sign_command = '"C:\\Program Files\\pwsh.exe" -File "C:\\a$b\\sign.ps1"'
        compiled = []

        def run(command, check):
            if isinstance(command, list):
                compiled.append(command)
                (self.args.out / 'AgentRelay-2026.11.1-Windows-x64-Setup.exe').write_bytes(b'installer')

        with patch.object(packaging.subprocess, 'run', side_effect=run):
            packaging.package(self.args)
        # Inno reads $q as a quote, $$ as a dollar sign and $f as the quoted file.
        self.assertIn('/SAgentRelaySign=$qC:\\Program Files\\pwsh.exe$q -File $qC:\\a$$b\\sign.ps1$q $f',
                      compiled[0])
        self.assertIn('/DSignTool=AgentRelaySign', compiled[0])
        self.assertFalse(any('"' in argument for argument in compiled[0][1:]))
        self.args.sign_command = None
        compiled.clear()
        with patch.object(packaging.subprocess, 'run', side_effect=run):
            packaging.package(self.args)
        self.assertFalse(any(argument.startswith(('/S', '/DSignTool')) for argument in compiled[0]))


if __name__ == '__main__':
    unittest.main()
