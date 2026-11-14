#!/usr/bin/env python3
"""Run the Windows runtime staging shell with a tiny mocked native toolchain.

Only the MSYS2 prefix is relocated. The real packaging shell, assets and Python
manifest writer run, including DLL discovery, loader filtering and schema copy.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

REPO = Path(__file__).resolve().parents[1]


class WindowsRuntimeStagingTests(unittest.TestCase):
    def fixture(self, root):
        """The mocked MSYS2 prefix and tools under `root`; returns the environment to run the script with."""
        tools = root / 'tools'
        tools.mkdir()
        prefix = root / 'clang64'
        (root / 'scripts').mkdir()
        for name in ['LICENSE', 'assets', 'gtk']:
            source = REPO / name
            if source.is_dir():
                shutil.copytree(source, root / name)
            else:
                shutil.copyfile(source, root / name)
        shutil.copyfile(REPO / 'scripts/package-windows.py', root / 'scripts/package-windows.py')
        shutil.copyfile(REPO / 'scripts/site-origin.sh', root / 'scripts/site-origin.sh')
        source = (REPO / 'scripts/build-windows.sh').read_text()
        source = source.replace('prefix=/clang64;', f'prefix="{prefix}";')
        (root / 'scripts/build-windows.sh').write_text(source)

        def write(relative, contents='runtime file'):
            path = prefix / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents)

        for name in ['gdbus.exe', 'gspawn-win64-helper.exe', 'gspawn-win64-helper-console.exe', 'libshared.dll']:
            write('bin/' + name)
        pixbuf = 'lib/gdk-pixbuf-2.0/2.10.0'
        write(pixbuf + '/loaders/pixbufloader_svg.dll')
        write(pixbuf + '/loaders.cache', '"' + str(prefix / pixbuf) + '/loaders/pixbufloader_svg.dll"\n"svg" 0 "gdk-pixbuf" "SVG"\n')
        write('share/glib-2.0/schemas/org.gtk.gtk4.test.gschema.xml')
        write('share/icons/Adwaita/index.theme')
        write('share/icons/Adwaita/symbolic/test.svg')
        write('share/icons/hicolor/index.theme')
        write('share/gtk-4.0/emoji/en.gvariant')
        commands = {
            'cygpath': "if '-f' in args: print(sys.stdin.read(), end='')\nelse: print(args[-1])",
            'rustc': "print('host: x86_64-pc-windows-gnullvm')",
            'git': "print(os.environ.get('MOCK_GIT_REVISION', 'a' * 40))\nraise SystemExit(int(os.environ.get('MOCK_GIT_STATUS', '0')))",
            'cargo': """site = [name + '=' + os.environ[name] for name in ('AGENT_RELAY_SITE_URL', 'AGENT_RELAY_URL_SCHEME',
    'DESKTOP_PRODUCT_NAME', 'RELEASES_REPOSITORY', 'DESKTOP_ASSET_PREFIX', 'DESKTOP_ICON',
    'DESKTOP_COBRAND_MARK', 'DESKTOP_COBRAND_NAME') if name in os.environ]
with open(os.environ['COMPILE_LOG'], 'a') as log: log.write(' '.join(args) + ' env=' + ','.join(site) + '\\n')
target = pathlib.Path(args[args.index('--target-dir') + 1]) / 'release'
target.mkdir(parents=True, exist_ok=True)
(target / (args[args.index('--bin') + 1] + '.exe')).write_bytes(b'compiled executable')""",
            'rsvg-convert': """pathlib.Path(args[args.index('-o') + 1]).write_bytes(b'PNG')
with open(os.environ['RSVG_LOG'], 'a') as log: log.write(pathlib.Path(args[-3]).read_text() + '\\n')""",
            'ntldd': "print('libshared.dll => ' + os.environ['MOCK_PREFIX'] + '/bin/libshared.dll (0x1234)')",
            'glib-compile-schemas': "pathlib.Path(args[0].split('=', 1)[1], 'gschemas.compiled').write_bytes(b'schemas')",
            'zip': "raise SystemExit('staging must not archive')",
            'sha256sum': "raise SystemExit('staging hashes with Python')",
            'xargs': """if '-d' in args:
    args = args[args.index('-d') + 2:]
    files = [line for line in sys.stdin.read().splitlines() if line]
    if files:
        assert args[:2] == ['cp', '-t'], args
        subprocess.run(['/bin/cp', *files, args[2]], check=True)
else:
    raise SystemExit('unexpected xargs invocation')""",
        }
        for name, body in commands.items():
            tool = tools / name
            tool.write_text('#!' + sys.executable + '\nimport os, pathlib, subprocess, sys\nargs=sys.argv[1:]\n' + body + '\n')
            tool.chmod(0o755)
        env = os.environ.copy()
        env.update(PATH=str(tools) + os.pathsep + '/usr/bin:/bin', MSYSTEM='CLANG64',
                   MOCK_PREFIX=str(prefix), CARGO_TARGET_DIR=str(root / 'target'),
                   COMPILE_LOG=str(root / 'compile.log'), RSVG_LOG=str(root / 'rsvg.log'), GITHUB_SHA='b' * 40,
                   # A release is for production whatever the builder's shell
                   # exports: neither variable reaches either cargo build.
                   AGENT_RELAY_SITE_URL='http://localhost:3100',
                   AGENT_RELAY_URL_SCHEME='agentrelay-dev')
        # Native Python is available independently of the mocked MSYS2 tools.
        (tools / 'python3').symlink_to(sys.executable)
        return env

    def test_independent_native_components_stage_and_package_without_toolchain(self):
        with tempfile.TemporaryDirectory(prefix='relay-stage-windows-') as temporary:
            root = Path(temporary)
            env = self.fixture(root)
            for component in ['core', 'app']:
                result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                    '--version', '2026.11.1', '--build-only', component,
                    '--runtime-out', str(root / component)], env=env, text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            # --build-only without --runtime-out stages the executable with
            # its records under --out, where it outlives the script, and
            # prints that path; the records let a later run take it back.
            for component, name, option in [('core', 'probe', '--probe'), ('app', 'agent-relay', '--app')]:
                with self.subTest(component=component, runtime_out=None):
                    result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                        '--version', '2026.11.1', '--build-only', component, '--out', str(root / 'dist')],
                        env=env, text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    ready = [line for line in result.stdout.splitlines() if f'{component} executable ready: ' in line]
                    self.assertEqual(len(ready), 1, result.stdout)
                    built = Path(ready[0].split(' ready: ', 1)[1])
                    self.assertEqual(built.parent, root / 'dist/built' / name)
                    self.assertEqual(built.read_bytes(), b'compiled executable')
                    self.assertEqual((built.parent / f'{name}-site.txt').read_text(), 'https://agentrelay.com\n')
                    self.assertEqual((built.parent / f'{name}-url-scheme.txt').read_text(), 'agentrelay\n')
                    taken = root / f'{component}-taken'
                    result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                        '--version', '2026.11.1', '--build-only', component, option, str(built),
                        '--runtime-out', str(taken)], env=env, text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertTrue((taken / 'runtime.json').is_file())
            core_manifest = json.loads((root / 'core/runtime.json').read_text())
            self.assertEqual(set(core_manifest['files']), {
                'LICENSE.txt', 'agent-icons-LICENSE.txt', 'NOTICES.txt',
                'bin/agent-relay-probe.exe', 'bin/libshared.dll',
            })
            self.assertEqual(core_manifest['source_sha'], 'a' * 40)
            calls = (root / 'compile.log').read_text().splitlines()
            # Two builds each: the runtime staging, then the plain --build-only;
            # a run given --probe or --app builds nothing.
            self.assertEqual(len(calls), 4)
            for call in calls[0::2]:
                self.assertIn('/desktop-core/Cargo.toml', call)
            for call in calls[1::2]:
                self.assertIn('/gtk/Cargo.toml', call)
            for call in calls:
                self.assertTrue(call.endswith(' env='), call)
            # What the script built was staged with its site and scheme
            # recorded in the script's own work directory, which is gone;
            # nothing of the kind is left beside cargo's shared target, where
            # a record would outlive the build it described.
            self.assertEqual(sorted(path.name for path in (root / 'target/release').iterdir()),
                             ['agent-relay-desktop-probe.exe', 'agent-relay.exe'])
            # A core or app built elsewhere for another site or scheme, or an
            # app that says nothing about either, is not packaged.
            elsewhere = root / 'elsewhere'
            elsewhere.mkdir()
            for component, exe, name in [('core', 'agent-relay-probe.exe', 'probe'), ('app', 'agent-relay.exe', 'agent-relay')]:
                (elsewhere / exe).write_bytes(b'compiled elsewhere')
                for site, scheme, expected in [
                    ('http://localhost:3100', 'agentrelay', 'is built for http://localhost:3100, not https://agentrelay.com'),
                    ('https://agentrelay.com', 'agentrelay-dev', 'opens agentrelay-dev:// links, not agentrelay://'),
                ]:
                    with self.subTest(component=component, site=site, scheme=scheme):
                        (elsewhere / f'{name}-site.txt').write_text(site + '\n')
                        (elsewhere / f'{name}-url-scheme.txt').write_text(scheme + '\n')
                        refused = root / 'refused-runtime'
                        result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                            '--version', '2026.11.1', '--build-only', component, f'--{component.replace("core", "probe")}',
                            str(elsewhere / exe), '--runtime-out', str(refused)], env=env, text=True, capture_output=True)
                        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                        self.assertIn(expected, result.stderr)
                        self.assertFalse(refused.exists())
                (elsewhere / f'{name}-site.txt').unlink()
                (elsewhere / f'{name}-url-scheme.txt').unlink()
            with self.subTest(component='app', sidecars='none'):
                result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                    '--version', '2026.11.1', '--build-only', 'app', '--app', str(elsewhere / 'agent-relay.exe'),
                    '--runtime-out', str(root / 'refused-runtime')], env=env, text=True, capture_output=True)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('has no agent-relay-site.txt and agent-relay-url-scheme.txt beside it', result.stderr)
                self.assertIn('build it with this script (--build-only app, or omit --app)', result.stderr)
            self.assertEqual((root / 'compile.log').read_text().splitlines(), calls)
            # Package with the host Python and a PATH without the fake compiler
            # tools. Explicit expected source differs from workflow event SHA.
            result = subprocess.run([sys.executable, str(root / 'scripts/package-windows.py'),
                '--version', '2026.11.1', '--arch', 'x64', '--source-sha', 'a' * 40,
                '--app-runtime', str(root / 'app'), '--core-runtime', str(root / 'core'),
                '--out', str(root / 'out')], text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            with zipfile.ZipFile(root / 'out/AgentRelay-2026.11.1-Windows-x64.zip') as archive:
                self.assertIn('AgentRelay-Windows-x64/bin/agent-relay.exe', archive.namelist())
                self.assertIn('AgentRelay-Windows-x64/bin/agent-relay-probe.exe', archive.namelist())
                self.assertIn('AgentRelay-Windows-x64/lib/gdk-pixbuf-2.0/2.10.0/loaders/pixbufloader_svg.dll', archive.namelist())
            with zipfile.ZipFile(root / 'out/AgentRelay-2026.11.1-Windows-x64-probe.zip') as archive:
                self.assertEqual(set(archive.namelist()), {
                    'agent_relay/LICENSE.txt', 'agent_relay/NOTICES.txt',
                    'agent_relay/helpers/agent-relay-probe.exe',
                    'agent_relay/helpers/libshared.dll',
                })
            for revision, status in [('', '0'), ('a' * 39, '0'), ('z' * 40, '0'), ('a' * 40, '1')]:
                with self.subTest(revision=revision, status=status):
                    failed_runtime = root / 'failed-runtime'
                    failed_env = {**env, 'MOCK_GIT_REVISION': revision, 'MOCK_GIT_STATUS': status}
                    result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                        '--version', '2026.11.1', '--build-only', 'core',
                        '--runtime-out', str(failed_runtime)], env=failed_env,
                        text=True, capture_output=True)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn('source revision', result.stderr)
                    self.assertFalse(failed_runtime.exists())
            self.assertEqual((root / 'compile.log').read_text().splitlines(), calls)

    def test_a_core_that_needs_no_msys2_dll_states_it_in_the_notices(self):
        # Rust 1.100 links libunwind statically on the windows-gnullvm
        # targets: the core's closure can be empty, and so is the bundle.
        with tempfile.TemporaryDirectory(prefix='relay-stage-windows-') as temporary:
            root = Path(temporary)
            env = self.fixture(root)
            (root / 'tools/ntldd').write_text('#!' + sys.executable + "\nprint('kernel32.dll => C:/Windows/System32/kernel32.dll (0x1234)')\n")
            script = root / 'scripts/build-windows.sh'
            script.write_text(script.read_text().replace('prefix=/clangarm64;', f'prefix="{env["MOCK_PREFIX"]}";'))
            for arch, msystem, triple in [('x64', 'CLANG64', 'x86_64-pc-windows-gnullvm'),
                                          ('arm64', 'CLANGARM64', 'aarch64-pc-windows-gnullvm')]:
                with self.subTest(arch=arch):
                    (root / 'tools/rustc').write_text('#!' + sys.executable + f"\nprint('host: {triple}')\n")
                    result = subprocess.run(['bash', str(script), '--version', '2026.11.1', '--arch', arch,
                        '--build-only', 'core', '--runtime-out', str(root / arch)],
                        env={**env, 'MSYSTEM': msystem}, text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn('no MSYS2 files are bundled', result.stdout)
                    self.assertEqual([path.name for path in (root / arch / 'files/bin').iterdir()], ['agent-relay-probe.exe'])
                    notices = (root / arch / 'files/NOTICES.txt').read_text()
                    self.assertIn('No MSYS2 files are bundled with this component.', notices)
                    self.assertNotIn('unmodified builds from the MSYS2 project', notices)

    def test_a_relative_icon_reaches_cargo_as_an_absolute_path(self):
        # gtk/build.rs runs with gtk/ as its working directory, so a
        # workspace-relative --icon (what the site-profile action passes)
        # must be absolute by the time cargo sees it.
        with tempfile.TemporaryDirectory(prefix='relay-stage-windows-') as temporary:
            root = Path(temporary)
            env = self.fixture(root)
            (root / 'branding').mkdir()
            icon = root / 'branding/icon.svg'
            icon.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8"/></svg>')
            result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                '--version', '2026.11.1', '--build-only', 'app', '--runtime-out', str(root / 'app'),
                '--profile', 'acme-7', '--icon', 'branding/icon.svg'], cwd=root, env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            calls = (root / 'compile.log').read_text().splitlines()
            self.assertEqual(len(calls), 1)
            self.assertIn(f'DESKTOP_ICON={icon.resolve()}', calls[0])
            self.assertNotIn('DESKTOP_ICON=branding/', calls[0])

    def test_a_cobranded_app_reaches_cargo_and_its_record_and_a_refused_one_stops_first(self):
        # The co-brand reaches the app's cargo build with an absolute mark
        # and the trimmed name, beside the app as a record, never in the
        # output; the shell's own co-brand variables never reach cargo.
        with tempfile.TemporaryDirectory(prefix='relay-stage-windows-') as temporary:
            root = Path(temporary)
            env = self.fixture(root)
            env.update(DESKTOP_COBRAND_MARK=str(root / 'stale.svg'), DESKTOP_COBRAND_NAME='Stale')
            (root / 'branding').mkdir()
            mark = root / 'branding/cobrand.svg'
            mark.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8"/></svg>')
            (root / 'branding/fake.png').write_text('not an image')
            build = ['bash', str(root / 'scripts/build-windows.sh'), '--version', '2026.11.1', '--build-only', 'app']
            result = subprocess.run([*build, '--out', str(root / 'dist'), '--profile', 'example-7',
                '--cobrand-mark', 'branding/cobrand.svg', '--cobrand-name', '  Example Customer  '],
                cwd=root, env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn('Example Customer', result.stdout + result.stderr)
            [call] = (root / 'compile.log').read_text().splitlines()
            self.assertTrue(call.endswith(f',DESKTOP_COBRAND_MARK={mark.resolve()},DESKTOP_COBRAND_NAME=Example Customer'), call)
            self.assertEqual((root / 'dist/built/agent-relay/agent-relay-cobrand-name.txt').read_text(), 'Example Customer\n')
            # Without the options: no co-brand, and no record.
            result = subprocess.run([*build, '--out', str(root / 'plain'), '--profile', 'example-7'],
                cwd=root, env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            call = (root / 'compile.log').read_text().splitlines()[-1]
            self.assertNotIn('COBRAND', call)
            self.assertFalse((root / 'plain/built/agent-relay/agent-relay-cobrand-name.txt').exists())
            # A PNG mark, and a name of exactly 80 characters: in ASCII, and
            # in two-byte characters under LC_ALL=C, counted as characters
            # as gtk/build.rs counts them.
            png = root / 'branding/cobrand.png'
            png.write_bytes(b'\x89PNG\r\n\x1a\n')
            c_locale = {**env, 'LC_ALL': 'C'}
            for out, mark_path, name, run_env in [
                    ('png', 'branding/cobrand.png', 'Example Customer', env),
                    ('ascii', str(mark), 'A' * 80, env),
                    ('multibyte', str(mark), '\u00e9' * 80, c_locale)]:
                with self.subTest(accepted=out):
                    result = subprocess.run([*build, '--out', str(root / out), '--profile', 'example-7',
                        '--cobrand-mark', mark_path, '--cobrand-name', name],
                        cwd=root, env=run_env, text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    call = (root / 'compile.log').read_text(encoding='utf-8').splitlines()[-1]
                    target = png if out == 'png' else mark
                    self.assertTrue(call.endswith(f',DESKTOP_COBRAND_MARK={target.resolve()},DESKTOP_COBRAND_NAME={name}'), call)
                    self.assertEqual((root / out / 'built/agent-relay/agent-relay-cobrand-name.txt').read_text(encoding='utf-8'),
                        name + '\n')
            for refused, why, run_env in [
                    (['--profile', 'example-7', '--cobrand-mark', str(mark), '--cobrand-name', '\u00e9' * 81],
                     'at most 80 characters; it has 81', c_locale)] + [(*case, env) for case in [
                    (['--profile', 'example-7', '--cobrand-name', 'Example Customer'], '--cobrand-name needs --cobrand-mark'),
                    (['--profile', 'example-7', '--cobrand-mark', str(mark)], '--cobrand-mark needs --cobrand-name'),
                    (['--cobrand-mark', str(mark), '--cobrand-name', 'Example Customer'], 'need --profile'),
                    (['--profile', 'example-7', '--cobrand-mark', 'branding/fake.png', '--cobrand-name', 'Example Customer'],
                     '--cobrand-mark must be an SVG or a PNG'),
                    (['--profile', 'example-7', '--cobrand-mark', str(mark), '--cobrand-name', ' '],
                     '--cobrand-name must not be empty or blank'),
                    (['--profile', 'example-7', '--cobrand-mark', str(mark), '--cobrand-name', 'Example\tCustomer'],
                     '--cobrand-name must be one line'),
                    (['--profile', 'example-7', '--cobrand-mark', str(mark), '--cobrand-name', 'Example Customer ' + 'x' * 64],
                     'at most 80 characters; it has 81')]]:
                with self.subTest(refused=why, locale=run_env.get('LC_ALL')):
                    result = subprocess.run([*build, '--runtime-out', str(root / 'refused'), *refused],
                        cwd=root, env=run_env, text=True, capture_output=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(why, result.stderr)
                    self.assertNotIn('Example Customer', result.stdout + result.stderr)
                    self.assertFalse((root / 'refused').exists())
            self.assertEqual(len((root / 'compile.log').read_text(encoding='utf-8').splitlines()), 5)

    def test_a_profile_build_records_its_profile_and_names_its_packages(self):
        with tempfile.TemporaryDirectory(prefix='relay-stage-windows-') as temporary:
            root = Path(temporary)
            env = self.fixture(root)
            icon = root / 'icon.svg'
            icon.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8"/></svg>')
            profile = ['--site-url', 'HTTPS://Relay.Example/', '--url-scheme', 'Acme-Relay', '--profile', 'acme-7',
                       '--product-name', 'Acme Desktop', '--releases-repository', 'acme/desktop-releases',
                       '--icon', str(icon)]
            for component in ['core', 'app']:
                result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                    '--version', '2026.11.1', '--build-only', component,
                    '--runtime-out', str(root / component), *profile], env=env, text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('building the site profile acme-7 (artifacts named Desktop-acme-7-...)', result.stdout)
            # The .ico is rendered from the given mark, as is, and the tree
            # ships it as the app's icon.
            self.assertEqual(set((root / 'rsvg.log').read_text().splitlines()) - {''}, {icon.read_text()})
            self.assertEqual((root / 'app/files/share/icons/hicolor/scalable/apps/agent-relay.svg').read_text(), icon.read_text())
            # Each cargo build saw the profile, normalized, and nothing of the
            # shell's; the records beside each executable say the same.
            calls = (root / 'compile.log').read_text().splitlines()
            self.assertEqual(len(calls), 2)
            for call in calls:
                self.assertTrue(call.endswith(
                    ' env=AGENT_RELAY_SITE_URL=https://relay.example,AGENT_RELAY_URL_SCHEME=acme-relay,'
                    'DESKTOP_PRODUCT_NAME=Acme Desktop,RELEASES_REPOSITORY=acme/desktop-releases,'
                    f'DESKTOP_ASSET_PREFIX=Desktop-acme-7,DESKTOP_ICON={icon}'), call)
            for component in ['core', 'app']:
                manifest = json.loads((root / component / 'runtime.json').read_text())
                self.assertEqual(manifest['build'], {
                    'site_url': 'https://relay.example', 'url_scheme': 'acme-relay',
                    'product_name': 'Acme Desktop', 'asset_prefix': 'Desktop-acme-7',
                    'releases_repository': 'acme/desktop-releases',
                    'support_url': 'https://github.com/acme/desktop-releases'})
            self.assertIn('agent-relay.exe and agent-relay-probe.exe are Acme Desktop.',
                          (root / 'app/files/NOTICES.txt').read_text())
            # The staged records name the profile's site and scheme, so a
            # later run takes the executable back only for that profile.
            result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                '--version', '2026.11.1', '--build-only', 'core', '--out', str(root / 'dist'), *profile],
                env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            built = root / 'dist/built/probe'
            self.assertEqual((built / 'probe-site.txt').read_text(), 'https://relay.example\n')
            self.assertEqual((built / 'probe-url-scheme.txt').read_text(), 'acme-relay\n')
            result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                '--version', '2026.11.1', '--build-only', 'core', '--probe', str(built / 'agent-relay-desktop-probe.exe'),
                '--runtime-out', str(root / 'refused')], env=env, text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('is built for https://relay.example, not https://agentrelay.com', result.stderr)
            # The packages are named by the profile's opaque id, never the
            # product name, and the installer is compiled from the profile.
            iscc = root / 'ISCC.exe'
            iscc.write_text('#!' + sys.executable + '\nimport pathlib, sys\n'
                "defines = dict(arg[2:].split('=', 1) for arg in sys.argv[1:] if arg.startswith('/D'))\n"
                "pathlib.Path(defines['OutputDir'], defines['AssetPrefix'] + '-' + defines['Version'] + '-Windows-' + defines['Arch'] + '-Setup.exe').write_text(repr(defines))\n")
            iscc.chmod(0o755)
            result = subprocess.run([sys.executable, str(root / 'scripts/package-windows.py'),
                '--version', '2026.11.1', '--arch', 'x64', '--source-sha', 'a' * 40,
                '--app-runtime', str(root / 'app'), '--core-runtime', str(root / 'core'),
                '--out', str(root / 'out'), '--installer', '--iscc', str(iscc)], text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            names = sorted(path.name for path in (root / 'out').iterdir())
            self.assertEqual(names, [
                'AgentRelay-Windows-x64',
                'Desktop-acme-7-2026.11.1-Windows-x64-Setup.exe', 'Desktop-acme-7-2026.11.1-Windows-x64-Setup.exe.sha256',
                'Desktop-acme-7-2026.11.1-Windows-x64-probe.zip', 'Desktop-acme-7-2026.11.1-Windows-x64-probe.zip.sha256',
                'Desktop-acme-7-2026.11.1-Windows-x64.zip', 'Desktop-acme-7-2026.11.1-Windows-x64.zip.sha256'])
            defines = eval((root / 'out/Desktop-acme-7-2026.11.1-Windows-x64-Setup.exe').read_text())
            self.assertEqual({key: defines[key] for key in ('ProductName', 'UrlScheme', 'AssetPrefix', 'SiteUrl', 'SupportUrl')}, {
                'ProductName': 'Acme Desktop', 'UrlScheme': 'acme-relay', 'AssetPrefix': 'Desktop-acme-7',
                'SiteUrl': 'https://relay.example', 'SupportUrl': 'https://github.com/acme/desktop-releases'})
            with zipfile.ZipFile(root / 'out/Desktop-acme-7-2026.11.1-Windows-x64.zip') as archive:
                self.assertIn('AgentRelay-Windows-x64/bin/agent-relay.exe', archive.namelist())
            # An app and a core built for different profiles are not merged.
            result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                '--version', '2026.11.1', '--build-only', 'core', '--runtime-out', str(root / 'default-core')],
                env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([sys.executable, str(root / 'scripts/package-windows.py'),
                '--version', '2026.11.1', '--arch', 'x64', '--source-sha', 'a' * 40,
                '--app-runtime', str(root / 'app'), '--core-runtime', str(root / 'default-core'),
                '--out', str(root / 'mixed')], text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('built for different site profiles', result.stderr)
            self.assertFalse((root / 'mixed').exists())
            # A profile value the scripts refuse stops the build before cargo.
            for refused in [['--profile', 'Acme'], ['--product-name', 'Acme/Desktop'],
                            ['--releases-repository', 'acme'], ['--icon', str(root / 'missing.svg')],
                            ['--site-url', 'https://relay.example:8443']]:
                with self.subTest(refused=refused):
                    result = subprocess.run(['bash', str(root / 'scripts/build-windows.sh'),
                        '--version', '2026.11.1', '--build-only', 'core', '--runtime-out', str(root / 'refused'), *refused],
                        env=env, text=True, capture_output=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse((root / 'refused').exists())
            self.assertEqual(len((root / 'compile.log').read_text().splitlines()), 4)


if __name__ == '__main__':
    unittest.main()
