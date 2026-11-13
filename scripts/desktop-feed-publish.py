#!/usr/bin/env python3
"""Publish a release to a site profile's cloud release feed, or list it.

A thin CLI over desktop_feed.py, which follows the proposed Cloud contract
(relay-control docs/desktop-feed.md). The publisher token is read
from DESKTOP_FEED_PUBLISHER_TOKEN only and is never printed.

Only a profile whose RELEASE_FEED is cloud reaches this script; a github:
feed publishes to its repository and is refused here.

The release notes are signed inside the manifest. Before creating a draft,
publish reads the signer's limit on a manifest's canonical bytes from the
listing (the KMS default of 4096 when it gives none: null or absent is
never read as uncapped), measures the manifest with empty notes, cuts the
notes on whole lines to what fits (at most 300 canonical bytes, less when
the assets leave less room; cut notes end with a link to the full notes on
the site's downloads page, counted in that budget), measures again, and
stops, with nothing created or uploaded, when even empty notes do not fit
(desktop_feed.preflight). A dry run fits the notes, link included, under
the 4096 default, since it reads nothing, and plans the cut notes.

With --request-out, publish writes the exact bytes of its POST /releases
body (the declaration, notes already fitted) to that file before sending
it, on a rerun of a published version too: the pull model attaches it as
publish-request.json, which the customer's operator sends unchanged to the
customer's own site (scripts/agent-relay-desktop-update.sh).

A publish prints the envelope's key_id, the SHA-256 of the manifest's
canonical bytes (what the Control signed and the app verifies) and the
signature; with --envelope-out it also writes the signed envelope, as the
feed returned it, to that file. A replay prints and writes neither.

Exit status:
  0  done.
  1  the feed refused or failed a request (a malformed limits value
     included), other than manifest_too_large.
  2  a refusal: missing or malformed input, refused before any request; or
     a manifest too large for the signer, which is a refusal too: the
     pre-flight's measure (before any draft; nothing created), the feed's
     422 manifest_too_large at create (nothing created) or at publish (the
     draft stays with its assets uploaded, and the message names it). A
     manifest_too_large is never retried.
"""

import argparse
import importlib.util
import json
import os
import sys
from pathlib import Path


def load_feed():
    path = Path(__file__).resolve().parent / "desktop_feed.py"
    spec = importlib.util.spec_from_file_location("desktop_feed", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


feed = load_feed()


def parse(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("list", "publish"):
        command = commands.add_parser(name)
        command.add_argument("--site", default="", help="the profile's site origin, https://host")
        command.add_argument("--profile", default="", help="the site profile id")
        command.add_argument("--feed", default="", help="the profile's RELEASE_FEED; must be cloud")
        command.add_argument("--dry-run", action="store_true", help="print the requests and send nothing")
    listing = commands.choices["list"]
    listing.add_argument("--channel", choices=["stable", "beta"], default="")
    listing.add_argument("--output", type=Path, help="write the listing JSON here instead of stdout")
    listing.add_argument("--missing-feed-empty", action="store_true",
                         help="for a prepare-only run: a site answering HTTP 404 not_found (no feed route yet)"
                              " lists as an empty feed, with a warning; other failures still fail")
    publishing = commands.choices["publish"]
    publishing.add_argument("--asset-prefix", required=True)
    publishing.add_argument("--version", required=True)
    publishing.add_argument("--channel", choices=["stable", "beta"], required=True)
    publishing.add_argument("--notes", type=Path, required=True, help="the release notes file")
    publishing.add_argument("--directory", type=Path, required=True, help="where the assets are")
    publishing.add_argument("--assets", type=Path, required=True,
                            help="a file naming every asset to declare, one per line")
    publishing.add_argument("--unlisted", action="store_true",
                            help="the candidate's version was taken as given, never checked against the feed:"
                                 " refuse it if the feed already holds it")
    publishing.add_argument("--request-out", type=Path,
                            help="write the exact POST /releases body sent (notes already fitted) here, for the"
                                 " pull model's publish-request.json; written on a rerun too")
    publishing.add_argument("--envelope-out", type=Path,
                            help="write the signed envelope the feed returned here, for an observer to compare"
                                 " with what the installed app verifies; not written on a replay")
    return parser.parse_args(argv)


def read_names(path):
    names = [line.strip() for line in path.read_text(encoding="utf-8").splitlines()]
    return [name for name in names if name]


def run(args, environ, open_url, out):
    feed.check_feed(args.feed)
    token = environ.get(feed.TOKEN_ENV, "")
    client = feed.Client(args.site, args.profile, token, open_url=open_url, out=out)
    if args.command == "list":
        query = "include_drafts=true" + (f"&channel={args.channel}" if args.channel else "")
        if args.dry_run:
            feed.show_plan(client, [("GET", f"/releases?{query}", None, {}, None)])
            return 0
        listed = client.listing(args.channel, missing_feed_empty=args.missing_feed_empty)
        if listed is None:
            client.say(f"::warning title=Desktop feed not enabled::{args.site} does not serve the desktop feed"
                       f" route (GET {feed.API_PATH}/releases answered HTTP 404 not_found); the feed is listed"
                       " as empty and the version resolves as for a first release.")
            listed = {"versions": []}
        text = json.dumps(listed, indent=2, sort_keys=True) + "\n"
        if args.output:
            args.output.write_text(text, encoding="utf-8")
            client.say(f"Wrote the feed listing to {args.output}.")
        else:
            out(text.rstrip("\n"))
        return 0
    feed.check_release(args.version, args.channel)
    notes = args.notes.read_text(encoding="utf-8")
    if len(notes) > feed.MAX_NOTES:
        raise feed.Refusal(f"the release notes exceed {feed.MAX_NOTES} characters")
    assets = feed.expected_assets(args.directory, read_names(args.assets), args.asset_prefix, args.version)
    if args.dry_run:
        client.say(f"Dry run: nothing is sent. v{args.version} ({args.channel}) declares {len(assets)} assets.")
        notes, size = feed.fit_notes(client.say, args.profile, args.version, args.channel, notes, assets,
                                     feed.DEFAULT_MAX_CANONICAL_BYTES, feed.full_notes_line(args.site, args.version))
        client.say(f"Manifest canonical size: at most {size} bytes; signer limit: not read in a dry run"
                   f" (notes fitted under the default of {feed.DEFAULT_MAX_CANONICAL_BYTES}).")
        listing = [("GET", "/releases?include_drafts=true", None, {}, None)] if args.unlisted else []
        feed.show_plan(client, listing + feed.plan(args.version, args.channel, notes, assets))
        return 0
    on_request = None
    if args.request_out:
        def on_request(data):
            args.request_out.write_bytes(data)
            client.say(f"Wrote the create request body to {args.request_out}.")
    envelope = feed.publish(client, args.directory, args.version, args.channel, notes, assets, args.unlisted,
                            on_request)
    if envelope is None:
        return 0
    manifest = envelope["manifest"]
    if args.envelope_out:
        # The feed has accepted the publication: a failed write must not turn
        # it into a failed run (a retry is refused as a replay).
        try:
            args.envelope_out.write_text(json.dumps(envelope, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            client.say(f"Wrote the published envelope to {args.envelope_out}.")
        except OSError as error:
            print(f"Warning: could not write the published envelope to {args.envelope_out}: {feed.redact(error, '')}",
                  file=sys.stderr)
    client.say(f"Published v{manifest['version']} at {manifest.get('published_at')}.")
    client.say(f"Signed by key_id {envelope.get('key_id')}.")
    client.say(f"Signed manifest canonical SHA-256: {feed.manifest_digest(manifest)}")
    client.say(f"Signature: {envelope['signature']}")
    for asset in manifest["assets"]:
        client.say(f"  {asset['id']}  {asset['name']}")
    return 0


def main(argv=None, environ=None, open_url=None):
    args = parse(sys.argv[1:] if argv is None else argv)
    environ = os.environ if environ is None else environ
    token = environ.get(feed.TOKEN_ENV, "")

    def out(text):
        print(feed.redact(text, token), flush=True)

    try:
        return run(args, environ, open_url or feed.default_open, out)
    except feed.FeedError as error:
        print(f"desktop-feed-publish: {feed.redact(error, token)}", file=sys.stderr)
        return 2 if isinstance(error, feed.Refusal) else 1
    except Exception as error:  # never a traceback: it could carry the token
        print(f"desktop-feed-publish: {type(error).__name__}: {feed.redact(error, token)}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
