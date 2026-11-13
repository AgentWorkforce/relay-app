#!/usr/bin/env python3
"""Stage hash-bound Windows runtimes, then package them without MSYS2 or Rust."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import zipfile

REPO = Path(__file__).resolve().parents[1]

# The default site profile's build values (docs/workspace-layout.md, "Site
# profiles"), as scripts/site-origin.sh names them. A runtime records the
# values it was built with; packaging names the zips and compiles the
# installer from them, and refuses an app and a core built for different ones.
DEFAULT_BUILD = {
    'site_url': 'https://agentrelay.com',
    'url_scheme': 'agentrelay',
    'product_name': 'Agent Relay',
    'asset_prefix': 'AgentRelay',
    'releases_repository': 'AgentWorkforce/relay-desktop-releases',
    'support_url': 'https://github.com/AgentWorkforce/relay-desktop-releases',
}
BUILD_PATTERNS = {
    'site_url': r'https?://[A-Za-z0-9.-]+(?::[0-9]+)?',
    'url_scheme': r'(?!(?:https?|wss?|ftp|file)$)[a-z][a-z0-9+.-]*',
    'product_name': r'[^\x00-\x1f/\\"\':*?<>|{},]{1,64}',
    'asset_prefix': r'AgentRelay|Desktop-(?=[a-z0-9-]{1,40}\Z)[a-z0-9]+(?:-[a-z0-9]+)*',
    # Empty for a cloud release feed, which has no releases repository.
    'releases_repository': r'(?:[\w.-]+/[\w.-]+)?',
    'support_url': r'https://[^\s"]+',
}
# The build's release feed (scripts/site-origin.sh, release_feed and
# feed_public_key): a github: feed is its releases repository's and is not
# recorded, so a runtime built for one records what it did before feeds; a
# cloud feed records itself and its public key.
FEED_PATTERNS = {
    'release_feed': r'github:[\w.-]+/[\w.-]+|cloud',
    'feed_public_key': r'(?:[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]=)?',
}


def feed_values(build):
    """A recorded build profile with its feed filled in: github: with the
    releases repository unless it records a cloud feed. Raises ValueError when
    the feed, repository and key disagree."""
    feed = build.get('release_feed') or f'github:{build["releases_repository"]}'
    key = build.get('feed_public_key', '')
    if feed == 'cloud':
        if build['releases_repository'] or not key:
            raise ValueError('A cloud release feed has a feed public key and no releases repository')
    elif key or feed.removeprefix('github:').casefold() != build['releases_repository'].casefold() \
            or not build['releases_repository']:
        raise ValueError('A github: release feed names the releases repository and has no feed public key')
    return {**build, 'release_feed': feed, 'feed_public_key': key}


def build_values(args):
    """The profile values given to --record, the default's where omitted."""
    build = dict(DEFAULT_BUILD)
    for key, pattern in {**BUILD_PATTERNS, **FEED_PATTERNS}.items():
        value = getattr(args, key)
        if value:
            if not re.fullmatch(pattern, value):
                raise ValueError(f'Invalid {key.replace("_", " ")}: {value}')
            build[key] = value
    feed = build.get('release_feed', '')
    if feed == 'cloud':
        build['releases_repository'] = args.releases_repository
    elif feed and not args.releases_repository:
        # An explicit github: feed names the repository it reads.
        build['releases_repository'] = feed.removeprefix('github:')
    # Validate the feed as given, then drop a github: feed, which is its
    # releases repository's and is not recorded.
    feed_values(build)
    if feed != 'cloud':
        build.pop('release_feed', None)
    return build


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def inventory(root):
    if root.is_symlink() or not root.is_dir():
        raise ValueError(f'Expected a regular runtime directory: {root}')
    files = {}
    names = set()
    for path in sorted(root.rglob('*')):
        if path.is_symlink() or (hasattr(path, 'is_junction') and path.is_junction()):
            raise ValueError(f'Links are not allowed in a runtime: {path}')
        relative = path.relative_to(root).as_posix()
        if relative.casefold() in names:
            raise ValueError(f'Case-insensitive runtime path collision: {relative}')
        names.add(relative.casefold())
        if path.is_file():
            files[relative] = {'sha256': digest(path), 'size': path.stat().st_size}
        elif not path.is_dir():
            raise ValueError(f'Unsupported runtime entry: {path}')
    return files


def required_files(component):
    required = {'LICENSE.txt', 'NOTICES.txt', 'bin/agent-relay-probe.exe'}
    if component == 'app':
        required.remove('bin/agent-relay-probe.exe')
        required.update({
            'bin/agent-relay.exe', 'bin/gdbus.exe', 'bin/gspawn-win64-helper.exe',
            'bin/gspawn-win64-helper-console.exe',
            'lib/gdk-pixbuf-2.0/2.10.0/loaders.cache',
            'share/glib-2.0/schemas/gschemas.compiled', 'share/icons/Adwaita/index.theme',
            'share/icons/hicolor/scalable/apps/agent-relay.svg',
        })
    return required


def record(directory, component, version, arch, source_sha, build=None):
    files = inventory(directory / 'files')
    missing = required_files(component) - files.keys()
    if missing:
        raise ValueError(f'Missing {component} runtime files: {sorted(missing)}')
    manifest = dict(schema=1, component=component, version=version, arch=arch,
                    source_sha=source_sha, build=dict(build or DEFAULT_BUILD), files=files)
    (directory / 'runtime.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    return manifest


def verify(directory, component, version, arch, source_sha):
    manifest_path = directory / 'runtime.json'
    if manifest_path.is_symlink():
        raise ValueError('Runtime manifest must not be a link')
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    expected = dict(schema=1, component=component, version=version, arch=arch)
    if any(manifest.get(key) != value for key, value in expected.items()):
        raise ValueError(f'Wrong {component} runtime identity')
    if source_sha and manifest.get('source_sha') != source_sha:
        raise ValueError(f'Wrong {component} runtime source revision')
    files = inventory(directory / 'files')
    if manifest.get('files') != files or required_files(component) - files.keys():
        raise ValueError(f'Changed or incomplete {component} runtime inventory')
    build = manifest.get('build')
    patterns = {**BUILD_PATTERNS, **FEED_PATTERNS}
    if (not isinstance(build, dict) or not set(DEFAULT_BUILD) <= set(build) <= set(patterns)
            or any(not isinstance(value, str) or not re.fullmatch(patterns[key], value)
                   for key, value in build.items())):
        raise ValueError(f'Missing or malformed {component} runtime build profile')
    try:
        feed_values(build)
    except ValueError as error:
        raise ValueError(f'Malformed {component} runtime build profile: {error}') from None
    return manifest


def archive(source, output, prefix=''):
    with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as zipped:
        for path in sorted(source.rglob('*')):
            if path.is_file():
                zipped.write(path, prefix + path.relative_to(source).as_posix())
    output.with_name(output.name + '.sha256').write_text(f'{digest(output)}  {output.name}\n', encoding='utf-8')


# A Windows command line holds at most 32767 characters; signing batches stay
# well below that.
SIGN_COMMAND_LIMIT = 30000
# The Inno Setup sign tool --sign-command defines (AgentRelay.iss, SignTool).
INNO_SIGN_TOOL = 'AgentRelaySign'


def portable_executable(path):
    """Whether a file is a PE image (an exe or dll, whatever its name): an MZ
    header whose e_lfanew points at a PE signature."""
    with path.open('rb') as stream:
        header = stream.read(64)
        if len(header) < 64 or header[:2] != b'MZ':
            return False
        stream.seek(int.from_bytes(header[0x3C:0x40], 'little'))
        return stream.read(4) == b'PE\0\0'


def sign(command, files):
    """Run the sign command on the files, each an absolute path appended as
    one quoted argument, in as few runs as the command line allows. The command
    signs in place and fails (non-zero) unless every file it was given is
    signed."""
    files = [str(path.resolve()) for path in files]
    if not files:
        raise ValueError('No portable executables to sign')
    batch = []
    for name in files:
        if batch and len(subprocess.list2cmdline([command, *batch, name])) > SIGN_COMMAND_LIMIT:
            subprocess.run(f'{command} {subprocess.list2cmdline(batch)}', check=True)
            batch = []
        batch.append(name)
    subprocess.run(f'{command} {subprocess.list2cmdline(batch)}', check=True)


def inno_sign_tool(command):
    """ISCC's /S definition of a sign tool that runs the sign command on the
    file Inno names ($f). Inno reads $q as a quote and $$ as a dollar sign; a
    literal quote would not survive ISCC's command-line parsing."""
    return f'/S{INNO_SIGN_TOOL}=' + command.replace('$', '$$').replace('"', '$q') + ' $f'


def find_iscc(explicit):
    candidates = [explicit, os.environ.get('ISCC'), shutil.which('ISCC.exe')]
    for base in [os.environ.get('ProgramFiles(x86)'), os.environ.get('ProgramFiles'),
                 str(Path(os.environ.get('LOCALAPPDATA', '')) / 'Programs')]:
        if base:
            candidates.append(str(Path(base) / 'Inno Setup 6' / 'ISCC.exe'))
    return next((Path(p) for p in candidates if p and Path(p).is_file()), None)


def package(args):
    app = verify(args.app_runtime, 'app', args.version, args.arch, args.source_sha)
    core = verify(args.core_runtime, 'core', args.version, args.arch, args.source_sha)
    if app.get('source_sha') != core.get('source_sha'):
        raise ValueError('App and core runtime source revisions differ')
    if app['build'] != core['build']:
        raise ValueError('App and core runtimes are built for different site profiles')
    build = feed_values(app['build'])
    # Validate the merge before changing any output. Native libraries must be
    # byte-identical when both components carry them; rolling MSYS2 updates
    # must never silently mix incompatible copies.
    app_names = {name.casefold(): name for name in app['files']}
    for name, value in core['files'].items():
        previous = app_names.get(name.casefold())
        if previous and name != 'NOTICES.txt':
            if previous != name or app['files'][previous] != value:
                raise ValueError(f'Conflicting component runtime file: {name}')
    args.out.mkdir(parents=True, exist_ok=True)
    tree_name = f'AgentRelay-Windows-{args.arch}'
    tree = args.out / tree_name
    if tree.exists():
        shutil.rmtree(tree)
    shutil.copytree(args.app_runtime / 'files', tree)
    for name in core['files']:
        if name == 'NOTICES.txt':
            continue
        destination = tree / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(args.core_runtime / 'files' / name, destination)
    app_notices = (tree / 'NOTICES.txt').read_text(encoding='utf-8')
    core_notices = (args.core_runtime / 'files/NOTICES.txt').read_text(encoding='utf-8')
    with (tree / 'NOTICES.txt').open('w', encoding='utf-8', newline='\n') as notices:
        notices.write(app_notices)
        if core_notices != app_notices:
            notices.write('\nCore runtime notices\n====================\n')
            notices.write(core_notices)
    # Sign every shipped PE file (the app, the core, GTK and its helpers and
    # loaders) in the staged tree, never in the hash-bound runtimes, and
    # before anything is archived.
    sign_command = getattr(args, 'sign_command', None)
    if sign_command:
        sign(sign_command, [path for path in sorted(tree.rglob('*'))
                            if path.is_file() and portable_executable(path)])
    prefix = f'{build["asset_prefix"]}-{args.version}-Windows-{args.arch}'
    archive(tree, args.out / (prefix + '.zip'), tree_name + '/')
    # Build the headless archive from exactly the core's captured closure,
    # taken from the staged tree, where it is signed when signing is on. No GTK
    # libraries enter it through the merged application tree.
    with tempfile.TemporaryDirectory(prefix='relay-probe-package-') as temporary:
        probe = Path(temporary) / 'agent_relay'
        helpers = probe / 'helpers'
        helpers.mkdir(parents=True)
        for name in core['files']:
            if name.startswith('bin/'):
                shutil.copyfile(tree / name, helpers / Path(name).name)
        for name in ['LICENSE.txt', 'NOTICES.txt']:
            shutil.copyfile(args.core_runtime / 'files' / name, probe / name)
        archive(probe, args.out / (prefix + '-probe.zip'), 'agent_relay/')
    if args.installer:
        iscc = find_iscc(args.iscc)
        if not iscc:
            raise ValueError('Inno Setup 6 ISCC.exe is required for --installer')
        setup = args.out / (prefix + '-Setup.exe')
        setup.unlink(missing_ok=True)
        # With a sign command, Inno signs Setup and the uninstaller it embeds.
        signing = [inno_sign_tool(sign_command), f'/DSignTool={INNO_SIGN_TOOL}'] if sign_command else []
        subprocess.run([str(iscc), '/Q', *signing, f'/DVersion={args.version}', f'/DArch={args.arch}',
                        f'/DSourceDir={tree.resolve()}', f'/DOutputDir={args.out.resolve()}',
                        f'/DProductName={build["product_name"]}', f'/DUrlScheme={build["url_scheme"]}',
                        f'/DAssetPrefix={build["asset_prefix"]}', f'/DSiteUrl={build["site_url"]}',
                        f'/DSupportUrl={build["support_url"]}', f'/DReleaseFeed={build["release_feed"]}',
                        str(REPO / 'windows/installer/AgentRelay.iss')], check=True)
        if not setup.is_file():
            raise ValueError(f'Inno Setup did not create {setup.name}')
        setup.with_name(setup.name + '.sha256').write_text(f'{digest(setup)}  {setup.name}\n', encoding='utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--record', type=Path)
    parser.add_argument('--component', choices=['core', 'app'])
    parser.add_argument('--version', required=True)
    parser.add_argument('--arch', choices=['x64', 'arm64'], required=True)
    parser.add_argument('--source-sha', default=os.environ.get('GITHUB_SHA', ''))
    parser.add_argument('--app-runtime', type=Path)
    parser.add_argument('--core-runtime', type=Path)
    parser.add_argument('--out', type=Path, default=Path('dist/windows'))
    parser.add_argument('--installer', action='store_true')
    parser.add_argument('--iscc')
    # A Windows command line that signs the files appended to it in place
    # (Authenticode); every PE file in the package, Setup and its uninstaller.
    parser.add_argument('--sign-command')
    # With --record: the site profile the component was built for.
    for key in [*DEFAULT_BUILD, *FEED_PATTERNS]:
        parser.add_argument('--' + key.replace('_', '-'), default='')
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.-]*', args.version):
        parser.error('Invalid package version')
    if args.source_sha and not re.fullmatch(r'[0-9a-f]{40}', args.source_sha):
        parser.error('Source revision must be a full Git SHA')
    if args.sign_command is not None and (args.record or not args.sign_command.strip()
                                          or any(c in args.sign_command for c in '\r\n\0')):
        parser.error('--sign-command is a one-line command, and only for packaging')
    if args.record and not args.component:
        parser.error('--record requires --component')
    if not args.record and (not args.app_runtime or not args.core_runtime):
        parser.error('Packaging requires --app-runtime and --core-runtime')
    if not args.record and any(getattr(args, key) for key in [*DEFAULT_BUILD, *FEED_PATTERNS]):
        parser.error('The site profile is read from the recorded runtimes; give it to --record')
    try:
        if args.record:
            record(args.record, args.component, args.version, args.arch, args.source_sha,
                   build_values(args))
        else:
            package(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'package-windows: {error}\n')


if __name__ == '__main__':
    main()
