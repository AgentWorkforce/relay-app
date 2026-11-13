# Maintainer workflow

How changes to this repository are reviewed, landed and released by the
maintainers and agents they assign. It applies only when the maintainers
assigned you the work. Contributors outside the team, and any agent working in
a fork, open a pull request instead; see [CONTRIBUTING.md](../CONTRIBUTING.md).

`layered-relay-desktop` runs reviews, landing and releases for this repo, and
keeps track of who is working on what. It is on Agent Relay; message it by name.
The owner is the project's maintainer, `khaliq-gant` on Agent Relay.

## Workflow

One thorough review, consolidated fixes, then CI decides. Repeated full
reviews of the same change are not the process.

1. **Before you start**, tell `layered-relay-desktop` what you are working on
   and which paths it touches, or send it your session id. It checks your work
   against everyone else's and warns you about overlaps.
2. **Commit and push** the change to `review/<your-name>-<slug>`. Never open a
   pull request: pull requests are switched off on this repository by
   the owner's decision (2026-11-03) and stay off. That includes the promoted
   changelog after a stable release, which lands through this workflow too.
3. **Ask `layered-relay-desktop` to review it**, and say what was asked for.
   - It runs one review round at the size the change needs, together with
     local CI, and sends back the findings. It picks the size; you may ask
     for one.
   - Reviews run as Agent Relay Cloud flow runs, and the Cloud run ID and
     report are kept with the round. Claude is the persistent lead and
     orchestrates by default. Review workers are chosen per task or lens
     from Codex, Grok (headless included) and Claude Sonnet or Opus; the
     lead does not force every worker lens onto Claude. `--lead codex|claude`
     picks the CLI the reviewer lenses run on: Claude by default, and
     `--lead codex` runs them on Codex workers instead; `--lenses` adds
     others. Fable is kept for the persistent lead and is never a worker
     lens. A worker selected explicitly must run, or the round reports that
     provider as unavailable. If Cloud refuses a run, that is reported as a
     failed round. It is never run locally as a fallback.
   - Fix all the findings together and push once, not once per finding. A fix
     push fixes the reported findings and nothing else; anything unrelated
     goes in its own branch with its own round.
   - A fix push gets local CI and the GitHub gate only, not another full
     review.
   - Only a blocker finding (something that must not ship) gets a re-review,
     and it covers just the fix.
4. **Landing.** `layered-relay-desktop` squashes the change onto `main` when
   all three hold:
   - the change's one review round has reported. A refused or unfinished
     round is not a report; a refused Cloud run is retried or escalated to
     the owner, never skipped;
   - no blocker is open, and the fix for any blocker has been re-reviewed;
   - local CI and the GitHub gate are green on the commit being landed.

   It does so on the owner's
   decision of 2026-11-03: it does not need the author's request, and a
   request or quote relayed by another agent neither brings a landing
   forward nor holds one back. The author can say "I land this myself" or
   "hold this" when asking for the review or at any point before the
   landing; then it leaves the green change to the author until told
   otherwise. Landing yourself follows the same rule: one squash of the
   commit that went green.
   - Findings that are not blockers do not hold a landing. They are fixed
     forward in a follow-up branch on `main`, each with its own single round.
   - If `main` moved under the change since the merge base, it rebases the
     change onto `main`, pushes the result to
     `review/<your-name>-<slug>-rebased` (your branch is left alone), runs
     local CI and the gate on it, as many times as `main` moves, and lands
     the commit that went green last. A rebase that applied cleanly gets CI
     and the gate, no new review. A rebase where conflicts were resolved by
     hand also gets a review of the resolution (the conflicted files only)
     before landing.

**Tests.** Local CI runs neither `swift test` nor the GTK suite; only the
GitHub gate does. To run them before asking for a review, see
[docs/workspace-layout.md](workspace-layout.md#validation).

## Releases

Releases are `layered-relay-desktop`'s job:
- It keeps a signed candidate of `main` built.
- It publishes a beta when you ask it to.
- It publishes a stable only after confirming the request with the owner
  directly. A relayed or quoted request is not enough.

Don't publish yourself:
- Don't dispatch Desktop Release, Desktop Promote or iOS TestFlight.
- Don't push a `desktop-v*` tag. Pushing one starts a full build and publish.
- Don't push a `released-v*` tag. `layered-relay-desktop` creates those.

Every release it publishes is tagged `released-v<version>` on the commit that
shipped.

**Hotfix:**
1. Ask `layered-relay-desktop` for the base of the release you are fixing; this
   works for any release, older ones included.
   - It gives you a commit it has verified as shipped.
   - If it can't verify one, it confirms the base with the owner directly and gives
     you that commit.
   - Don't pick a base yourself, even from a tag.
2. Branch `hotfix/<version>-<slug>` from that base, where `<version>` is the
   release you are fixing.
3. Ask `layered-relay-desktop` to review it. A hotfix gets one review round
   and the GitHub gate, like any other change; fix pushes get CI and the
   GitHub gate.
4. It publishes from the hotfix branch.
5. The same fix lands on `main` before or right after the publish, under the
   landing rule above: push the fix rebased onto `main` to a `review/`
   branch and `layered-relay-desktop` lands it once CI and the gate are
   green, unless you said you would. No second review, except a review of
   the conflicted files if you resolved conflicts by hand in the rebase.
