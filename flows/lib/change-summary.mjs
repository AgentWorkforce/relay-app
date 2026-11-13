// The change a review round is about, summed up for sizing it:
//
//   node flows/lib/change-summary.mjs <base> [include...]   -> one JSON line
//
// Measured on the same snapshot of the working tree the reviews read
// (flows/lib/remote-ci.mjs snapshot: tracked edits, staged files and the
// untracked paths in `include`), against the merge base with <base>. Prints a
// flows/lib/depth.ts ChangeSummary. The summary is what the flow sends to Jev,
// so the diff leaves out the private core (desktop-core/) and files shaped
// like credentials, which still count in the file list and the floors, as
// well as lockfiles, and any line that holds a secret is redacted whatever
// file it is in. The line is kept
// under SUMMARY_BYTES, since the flow body sees only the tail of a command's
// output: at most LISTED_FILES files and LISTED_SUBJECTS subjects (each cut
// to SUBJECT_CHARS) are listed, the diff is cut to DIFF_CHARS and then halved
// until the line fits, and if it still does not fit the file list goes, then
// the subjects, and the counts stand.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { DIFF_CHARS, LISTED_FILES, LISTED_SUBJECTS, SUBJECT_CHARS, SUMMARY_BYTES } from "./depth.ts";
import { snapshot } from "./remote-ci.mjs";
import { diffShowingHidden, NOT_LOCKFILES } from "./review-state.mjs";

const git = (args) => execFileSync("git", args, { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 }).trimEnd();
/** Never sent to the classifier: the private core, and files shaped like credentials. */
export const CREDENTIAL_GLOBS = [
  "**/*.pem", "**/*.key", "**/*.p8", "**/*.p12", "**/*.pfx", "**/*.jks", "**/*.keystore", "**/*.mobileprovision", "**/*.gpg", "**/*.keychain*",
  "**/.env", "**/.env.*", "**/.npmrc", "**/.netrc", "**/_netrc", "**/.pypirc", "**/id_rsa*", "**/id_ed25519*", "**/id_ecdsa*", "**/*.ppk",
  "**/api_key*", "**/apikey*", "**/*.tfvars", "**/kubeconfig*", "**/.docker/config.json",
  "**/*secret*", "**/*token*", "**/*credential*", "**/*password*", "**/*passwd*", "**/*private*", "**/.envrc",
  "**/*.tfstate", "**/*.tfstate.*", "**/.pgpass", "**/pgpass.conf", "**/.s3cfg",
  // A `*` in a git glob stays inside one path segment, so directories need their own patterns.
  "**/*secret*/**", "**/*token*/**", "**/*credential*/**", "**/*password*/**", "**/*private*/**",
];
const PRIVATE = [":(exclude)desktop-core/", ...CREDENTIAL_GLOBS.map((glob) => `:(exclude,glob,icase)${glob}`)];

// Case-sensitive: real tokens have fixed case, and an identifier such as AsiaPacific is not one.
const TOKEN_LINE = new RegExp(
  [
    "-----BEGIN (?:[A-Z ]*PRIVATE KEY|PGP PRIVATE KEY BLOCK)-----",
    // A PEM banner base64-encoded, as a Kubernetes Secret or a .env holds it.
    "LS0tLS1CRUdJTi[A-Za-z0-9+/=]{20,}",
    "\\b(?:sk_live|sk_test|rk_live|rk_test|ghp|gho|ghu|ghs|ghr|github_pat|xox[abpr]|glpat)[-_][A-Za-z0-9_-]{8,}",
    "\\b(?:AKIA|ASIA)[A-Z0-9]{16}\\b",
    "\\bAIza[A-Za-z0-9_-]{30,}",
    "\\bsk-(?:ant-|proj-)[A-Za-z0-9_-]{8,}",
    "\\bnpm_[A-Za-z0-9]{16,}",
    "\\bAGE-SECRET-KEY-1[A-Z0-9]{20,}",
    "hooks\\.slack\\.com/services/T[A-Za-z0-9]+/B[A-Za-z0-9]+/[A-Za-z0-9]{8,}",
    // A credential in a URL, an auth header, a JWT.
    "\\b[a-z][a-z0-9+.-]{0,30}://[^\\s/:@]*:[^\\s/@]{3,}@",
    // A header's value is base64 or a token: it has a lower-to-upper-or-digit and an upper-or-digit-to-lower
    // change inside, which "Bearer authentication" and "Basic credentials" have not.
    "\\bBearer\\s+(?=[A-Za-z0-9._~+/=-]*(?:[a-z][A-Z0-9]|[A-Z0-9][a-z]))[A-Za-z0-9._~+/=-]{16,}",
    "\\bBasic\\s+(?=[A-Za-z0-9+/]*[a-z][A-Z0-9])(?=[A-Za-z0-9+/]*[A-Z0-9][a-z])[A-Za-z0-9+/]{8,}={0,2}(?![A-Za-z0-9])",
    "\\bAuthorization:\\s*(?:Token|ApiKey|Api-Key|token|apikey)\\s+[A-Za-z0-9._~+/=-]{8,}",
    // Each JWT segment is matched once (lookahead capture, then back-reference), so a hyphenated run costs one pass.
    "\\beyJ(?=(?<j1>[A-Za-z0-9_-]{8,4096}))\\k<j1>\\.(?=(?<j2>[A-Za-z0-9_-]{8,4096}))\\k<j2>\\.(?=(?<j3>[A-Za-z0-9_-]{8,4096}))\\k<j3>",
  ].join("|"),
);
const cap = (part) => part[0].toUpperCase() + part.slice(1);
/**
 * The spellings a key word takes in a name: snake, SCREAMING and kebab, and
 * unless `camel` is off, Capitalised, camel and Pascal too. The option is an
 * object so the function is never handed an array index by mistake.
 */
const forms = (word, { camel = true } = {}) => {
  const parts = word.split("_");
  const out = new Set([word, word.toUpperCase()]);
  if (camel) for (const form of [cap(word), parts.map(cap).join(""), parts[0] + parts.slice(1).map(cap).join("")]) out.add(form);
  if (parts.length > 1) for (const form of [...out]) out.add(form.replace("_", "-"));
  return [...out];
};
/** The camel and Pascal forms of a word, matched only as a whole identifier. */
const wholeCamel = (word) => {
  const parts = word.split("_");
  return `(?<![A-Za-z0-9])(?:${parts.map(cap).join("")}|${parts[0] + parts.slice(1).map(cap).join("")})(?![A-Za-z])`;
};
// The credential key words carry their camel forms too (deployKey, hmacKey); a bare `Key` would
// catch every `forKey:` in the Swift sources, so the generic arm keeps its separator. Nouns that
// also appear inside ordinary Swift identifiers (contractorAccessKey, sessionKeys) match in camel
// form only as a whole identifier: accessKey = "..." is a secret, contractorAccessKey is not.
const KEY_WORDS = [
  ...["api_key", "apikey", "secret", "secret_id", "token", "password", "passwd", "passphrase", "credential", "private_key", "privatekey",
    "master_key", "deploy_key", "hmac_key", "jwt_key", "signing_key", "license_key", "ssh_key",
    "auth_key", "encryption_key", "consumer_key", "preshared_key", "aes_key", "gpg_key", "rsa_key"].flatMap((word) => forms(word)),
  ...["session_key", "app_key", "client_key", "access_key", "host_key"].flatMap((word) => [...forms(word, { camel: false }), wholeCamel(word)]),
  // `pass` only as a whole word: bypass, compass and trespass are not credentials.
  "(?<![A-Za-z])(?:pass|PASS|Pass)(?![A-Za-z0-9_])",
].join("|");
// Keys that name a lookup, not a credential: a cache or primary key, a plist or GSettings key, a
// CI cache's restore key. Registry, settings and defaults keys stay secrets: they can hold one.
const BENIGN_WORDS = [
  "cache", "primary", "partition", "public", "translation", "sort", "foreign", "idempotency", "routing", "index",
  "infoplist", "plist", "gsettings", "accel", "accelerator", "menu", "locale", "i18n", "row", "map", "restore", "object", "legacy",
];
const BENIGN_KEYS = BENIGN_WORDS.flatMap((word) => forms(word)).join("|");
// A segment after the key word that names a property of the secret rather than the secret: its path,
// id, header, prefix, length, lifetime or version.
const NOT_A_SECRET_WORDS = ["path", "file", "dir", "url", "uri", "id", "name", "header", "prefix", "suffix", "ttl", "timeout", "expiry", "expires",
  "expiration", "length", "len", "size", "count", "limit", "version", "type", "kind", "format"];
const NOT_A_SECRET = NOT_A_SECRET_WORDS.flatMap((word) => forms(word)).join("|");
// Case-sensitive, so a camel-case segment is told from the rest of a word. After the key word a name may
// go on with separated segments, capitalised segments, a plural s or digit runs, at length. Each arm
// takes its whole segment and the tail is matched once (a lookahead capture, then its back-reference),
// so a line can never be re-partitioned: the scan is linear whatever the line holds.
const TAIL = "(?:[_-][A-Za-z0-9]+(?![A-Za-z0-9])|[A-Z][a-z0-9]*(?![a-z0-9])|[0-9]+(?![0-9])|s(?![A-Za-z])){0,12}";
/** A quoted literal of 8+ in any of the three quote kinds, escapes and the other quote kinds inside allowed. */
const QUOTED = "(?![\"']\\$[A-Za-z_{])(?:[bBrRuUfF]{0,2}|@)?(?:\"\"\".{8,}|'''.{8,}|\"(?:[^\"\\\\]|\\\\.){8,}\"|'(?:[^'\\\\]|\\\\.){8,}'|`(?:[^`\\\\]|\\\\.){8,}`)";
/**
 * A bare value of 8+ that reads as a value rather than code, with an optional trailing `,` or `;`:
 * letters, digits and the symbols a password or token holds, at least one of them a digit or a symbol,
 * so a plain identifier (`changeToken`), a member path (`self.token`), a type (`Option<String>`), a
 * template (`${{ secrets.X }}`), a variable (`$API_KEY`) and a call (`prompt()`) are not values. A quoted
 * literal also counts as the argument of a constructor or call named for the key (`Secret("…")`).
 */
const BARE =
  "(?!\\$)(?=[A-Za-z0-9_\\-+/=!#%^*@~:.$&]{0,1024}[0-9!#%^*~+/=$-])(?=(?<bare>[A-Za-z0-9_\\-+/=!#%^*@~:.$&]{8,1024}))\\k<bare>(?:[,;]?(?=[\\s\"'`;,)\\]]|$)|(?<=[A-Za-z0-9_\\-+/=!#%^*@~:.$&]{1024}))" +
  // Or a lowercase word of 10+ letters on its own (`password: mysecretpassword`): no type, identifier or
  // member path looks like that, and the words that do (undefined, required) are shorter.
  "|(?=(?<word>[a-z]{10,64}))\\k<word>(?=[\\s\"'`;,)\\]]|$)";
/** Under a password-family key any word of 8+ letters is the password (`password: MySecretPassword`). */
const PASSWORD_WORD = "(?!(?:undefined|required|optional|nullable|readonly|password|passwords|unchanged|redacted|disabled|enabled|unknown|missing|unset)(?![A-Za-z]))(?=(?<pw>[A-Za-z]{8,64}))\\k<pw>(?=[\\s\"'`;,)\\]]|$)";
/**
 * The tight `NAME=value` form of an env file, a compose file, a shell line, a connection string or a
 * query: any word of 8+ is the value, ending at whitespace, a quote or a separator. The run is matched
 * once (a lookahead capture, then its back-reference) and capped, so a key word costs one bounded scan
 * however long the line and whatever follows the run.
 */
const TIGHT = "=(?=(?<tight>[^\\s\"'`=(){}<>$][^\\s\"'`(){}<>]{7,1023}))\\k<tight>(?:(?=[\\s\"'`&;,)\\]]|$)|(?<=[^\\s\"'`(){}<>]{1024}))";
const SCREAMING_BENIGN = BENIGN_WORDS.map((word) => word.toUpperCase()).join("|");
/**
 * A YAML or compose map entry under a SCREAMING name (`POSTGRES_PASSWORD: mysecretpassword`): the value
 * may be letters only. The name's segments after the key word must not say it is a path, id or the like,
 * and the value is one word with no template, variable or member path in it.
 */
const SCREAMING = new RegExp(
  // Each quantified part is matched once (lookahead capture, then back-reference) and bounded, so a long
  // SCREAMING run costs one pass: the name's segments after the key word, and the value.
  // The name starts only where a word does (not after an underscore), so a long run is scanned once.
  // PASS counts only as the whole last segment (PASS=, DB_PASS:), never inside BYPASS_, COMPASS_ or PASS_RATE.
  `(?<![A-Za-z0-9_])[A-Z0-9_]{0,120}(?:API_?KEY|SECRET|TOKEN|PASSWORD|PASSWD|PASSPHRASE|(?<![A-Z])PASS(?=[\"']?\\s*[:=])|CREDENTIALS?|PRIVATE_?KEY|(?<!(?:^|[^A-Z0-9])(?:${SCREAMING_BENIGN})S?)_KEY)` +
    "(?=(?<segments>(?:_[A-Z0-9]+){0,12}))\\k<segments>" +
    `(?<!_(?:${[...NOT_A_SECRET_WORDS, "env", "var", "variable"].map((w) => w.toUpperCase()).join("|")}))` +
    "(?![A-Z0-9_])[\"']?\\s*[:=]\\s*(?=(?<screaming>[A-Za-z][A-Za-z0-9_\\-/]{7,1023}))\\k<screaming>(?=[\\s\"'`;,)\\]]|$)",
);
/**
 * Secrets given with a space instead of an assignment: a netrc line (`login x password y`), a CLI flag
 * (`--password y`, `-p y` is too short a flag to trust), or a YAML block scalar whose value starts on
 * the next lines (`password: |`), which redactSecrets follows by indentation.
 */
const SPACED = new RegExp(
  "(?:\\b(?:login|machine)\\s+\\S+\\s+password\\s+[^\\s\"'`]{6,}" +
    // A flag's value: bare with a digit or symbol in it, or 12+ long (help prose says `--token sets the token`),
    // or quoted after a space or an equals sign; never a placeholder (`<value>`, `[token]`).
    "|(?<![A-Za-z0-9-])--?(?:password|passwd|passphrase|token|secret|api-?key|access-?key|client-?secret)(?:[ =])(?:(?!-)(?=(?<flagv>[^\\s\"'`$<>\\[\\]]{6,1024}))\\k<flagv>(?<=[0-9!#%^*@~+/=&.][^\\s\"'`$<>\\[\\]]{0,1024}|[^\\s]{12})(?![^\\s])|[\"'][^\"'\\s]{6,}[\"'])" +
    // `-u user:password`, with a letter in the password: `-u 1000:1000` is a uid and gid.
    "|-u\\s+[^\\s:]+:(?=[^\\s\"'`]*[A-Za-z])[^\\s\"'`]{4,}(?![^\\s])" +
    // SQL: IDENTIFIED BY '…', WITH PASSWORD '…', PASSWORD '…'; a Dockerfile ENV with a space.
    "|\\b(?:IDENTIFIED BY|identified by|WITH PASSWORD|with password|PASSWORD|password)\\s+[\"'][^\"'\\s]{4,}[\"']" +
    "|^\\s*ENV\\s+[A-Za-z0-9_]{0,60}(?:PASSWORD|PASSWD|SECRET|TOKEN|API_?KEY|PASS|password|secret|token)[A-Za-z0-9_]{0,60}\\s+(?!\\$)[^\\s\"'`]{8,}(?![^\\s])" +
    // An environment lookup with a literal fallback: os.getenv("TOKEN", "…"), env.get('KEY', '…').
    "|(?:getenv|environ\\.get|env\\.get|lookupEnv|Getenv)\\(\\s*[\"'][^\"']+[\"']\\s*,\\s*[\"'][^\"']{8,}[\"'])",
);
/** A YAML key whose value is a block scalar on the following, more indented lines. */
const BLOCK_WORDS = ["password", "passwd", "passphrase", "secret", "token", "api_key", "apikey", "private_key", "privatekey", "credential", "credentials"].flatMap((w) => forms(w)).join("|");
const BLOCK_NOT = NOT_A_SECRET_WORDS.flatMap((w) => forms(w)).join("|");
const BLOCK_KEY = new RegExp(
  `^(\\s*)(- +)?["']?(?:[A-Za-z0-9_.-]{0,120}(?:${BLOCK_WORDS})[A-Za-z0-9_.-]{0,120}|[A-Za-z0-9_.-]{0,120}[._-](?:key|KEY|Key))(?<![._-](?:${BLOCK_NOT}))["']?\\s*:\\s*[|>](?:[+-]?[0-9]?|[0-9]?[+-]?)\\s*(?:#.*)?$`,
);
/** The cheap test first: only a line ending in a block indicator is matched against the key shape. */
const BLOCK_SCALAR = {
  exec: (line) => (/:[ \t]{0,8}[|>][+0-9-]{0,2}[ \t]{0,8}(?:#[^\n]{0,2000})?$/.test(line) && line.length <= 2000 ? BLOCK_KEY.exec(line) : null),
  test: (line) => BLOCK_SCALAR.exec(line) !== null,
};
const NAME_LINE = new RegExp(
  `\\b[A-Za-z0-9_]{0,120}(?:${KEY_WORDS}|(?<!(?:^|[^A-Za-z0-9])(?:${BENIGN_KEYS})s?)[_.-](?:key|KEY|Key))` +
    `(?!(?:[A-Za-z0-9]*[_-]){0,13}(?:${NOT_A_SECRET})(?![A-Za-z0-9]))(?=(${TAIL}))\\1` +
    // Then the assignment: a quote closing a JSON or index key (and its `]`), an optional bounded type
    // annotation (`: &str`, `: String?`), `=`, `:`, `:=`, `=>` or `==`, and the value.
    `[\"'\`]?\\s*(?:\\]\\s*)?(?:(?::(?=(?<ann>\\s*[&'A-Za-z0-9_?<>\\[\\]., ]{1,40}))\\k<ann>(?==))?(?::=|=>|===?|[:=])\\s*(?:(?:Some|Ok|new [A-Za-z_][A-Za-z0-9_]{0,40}|[A-Z][A-Za-z0-9_]{0,40}(?:::(?:new|from))?)\\(\\s*)?(?:${QUOTED}|${BARE})|${TIGHT}|(?<!contains_key|has_key|get_key|remove_key|insert_key|by_key|for_key|forKey|byKey|containsKey|hasKey|getKey|removeKey)\\(\\s*${QUOTED.replace("|`(?:[^`\\\\]|\\\\.){8,}`", "")})`,
);
/**
 * A line that unmistakably holds a secret, whatever file it is in: a private
 * key block's opening line; a well-known token (GitHub, GitLab, Slack and
 * Slack webhooks, Stripe, AWS, Google, Anthropic, OpenAI, npm, age), a
 * credential in a URL, a Bearer, Basic, Token or ApiKey authorization
 * header, a JWT; or a key-like name,
 * however qualified, quoted or indexed (`SECRET_KEY_BASE`, `secretKey`,
 * `"api_key":`, `config["SECRET_KEY"]`), assigned, directly or through a
 * bounded type annotation, with `=`, `:`, `:=`, `=>` or `==`, a quoted
 * literal of 8+ (escapes, apostrophes, triple quotes and `b''`/`r""`/`@""`
 * prefixes included) or a bare value of 8+ that reads as a value, not as
 * code: any word in the tight `NAME=value` form of an env file, a compose
 * file or a shell line, and otherwise a word with a digit or a symbol in it
 * that is no member path, type, call or template. Any `*_key` counts
 * but a cache, primary, partition or similar key. A name whose segment names
 * a path, file, dir, URL or id (`api_key_path`, `APPLE_API_KEY_ID`), a word
 * that merely starts with a key word (`tokenizer`, `secretariat`), a name
 * alone (`token: string`) and a call (`password = prompt()`,
 * `token = resolve(flow)`) do not match. Both sides of the key word are
 * bounded, so a long word costs nothing to scan.
 */
const PASSWORD_LINE = new RegExp(`\\b[A-Za-z0-9_]{0,120}(?:${["password", "passwd", "passphrase"].flatMap((w) => forms(w)).join("|")})(?![A-Za-z0-9_])[\"'\`]?\\s*(?::=|=>|===?|[:=])\\s*${PASSWORD_WORD}`);
/** A plist or XML secret: `<key>password</key><string>…</string>` on one line, or `<password>…</password>`. */
/** A secret given as the value argument after its quoted key name: set("password", "…"), put('token', '…'). */
const KEYED_CALL = new RegExp(`[\"'](?:${["password", "passwd", "passphrase", "secret", "token", "api_key", "apikey", "private_key", "credential"].flatMap((w) => forms(w)).join("|")})[\"']\\s*,\\s*${QUOTED}`);
const XML_LINE = /<key>[^<]{0,80}(?:[Pp]assword|[Ss]ecret|[Tt]oken|[Aa][Pp][Ii]_?[Kk]ey|[Pp]assphrase|[Cc]redential)(?![^<]*(?:[Pp]ath|[Ff]ile|[Dd]ir|URL|[Uu]rl|[Nn]ame))[^<]{0,80}<\/key>\s*<string>[^<]{8,}<\/string>|<(?:password|secret|token|apiKey|api_key|passphrase|credential)>[^<]{8,}<\//;
export const SECRET_LINE = {
  test: (line) =>
    TOKEN_LINE.test(line) || NAME_LINE.test(line) || PASSWORD_LINE.test(line) || SCREAMING.test(line) || SPACED.test(line) || BLOCK_SCALAR.test(line) || XML_LINE.test(line) || KEYED_CALL.test(line),
};
/** The first `chars` of `text`, never ending on half a surrogate pair. */
const cut = (text, chars) => text.slice(0, chars).replace(/[\uD800-\uDBFF]$/, "");
/** The text's whole lines up to the first newline at or after `chars`: a line is never cut in two. */
export const wholeLines = (text, chars) => {
  if (text.length <= chars) return text;
  const end = text.indexOf("\n", chars);
  return end === -1 ? text : text.slice(0, end + 1);
};
/**
 * A line that is nothing but key material: the base64 body of a PEM block
 * whose opening line fell outside the hunk. Base64 of key bytes mixes upper
 * and lower case with digits; a hex hash or a run of one letter does not.
 */
export const KEY_BODY_LINE = /^[+\- ]?(?=[A-Za-z0-9+/=]*[A-Z])(?=[A-Za-z0-9+/=]*[a-z])(?=[A-Za-z0-9+/=]*[0-9])[A-Za-z0-9+/=]{40,}$/;
/** A unified diff's structural lines: a key block never runs past them. */
const DIFF_BOUNDARY = /^(?:diff --git |@@ |\+\+\+ |--- )/;
/** A hunk header's trailing context (git's funcname line) can hold a secret; the header itself cannot. */
const HUNK_HEADER = /^(@@ [^@]* @@)(.*)$/;
const REDACTED = "[redacted]";
const redactLine = (line) => (/^[+\- ]/.test(line) ? `${line[0]}${REDACTED}` : REDACTED);
/** A line's content without its diff marker or CR: what the secret tests look at. */
const content = (line) => line.replace(/^[+\- ]/, "").replace(/\r$/, "");
/**
 * The content trimmed and unquoted: how a key's body reads inside an indented
 * block, a string array, a heredoc or a run of concatenated literals
 * (`"MIIE...\n" +`).
 */
const inside = (body) => {
  let core = body.trim();
  // A trailing line continuation, stripped without a regex scan of the line's whitespace.
  if (core.endsWith("\\")) core = core.slice(0, -1).trimEnd();
  return core.replace(/^["'`]/, "").replace(/(?:\\n)?["'`]\s{0,8}(?:[+,;]|\))?\s{0,8}[;,]?$/, "");
};
/** A PEM or PGP armour header line (`Proc-Type: 4,ENCRYPTED`, `Version: GnuPG v2`), part of the block; any other `Word: value` is not. */
const ARMOUR_HEADER = /^(?:Proc-Type|DEK-Info|Version|Comment|MessageID|Hash|Charset): /;
/** Whether a line could belong to a key block: base64, an armour header, or blank. */
const keyLike = (body) => {
  const core = inside(body);
  return core === "" || ARMOUR_HEADER.test(core) || /^[A-Za-z0-9+/=]+$/.test(core);
};

/** A line that could be the base64 tail of a key: base64 only, 20+ chars or padded, or a PGP armour checksum (`=AbCd`). */
const KEY_TAIL = /^(?:[A-Za-z0-9+/]{20,}={0,2}|[A-Za-z0-9+/]{4,}={1,2}|=[A-Za-z0-9+/]{4})$/;

/**
 * Lines an END marker reaches back over: the consecutive base64 or armour
 * lines right before it, when a key's tail opens a hunk with no 40+ body
 * line to announce it.
 */
function keyTailsBeforeEnd(lines) {
  const marked = new Set();
  for (let i = 0; i < lines.length; i++) {
    // Only an END marker on its own line reaches back: one inside a hunk header or a line of code does not.
    if (DIFF_BOUNDARY.test(lines[i]) || !/^["'`]?-----END [A-Z ]+-----(?:\\n)?["'`]?\s*(?:[+,;]|\))?\s*[;,]?(?:\s*\\)?$/.test(content(lines[i]).trim())) continue;
    for (let j = i - 1; j >= 0; j--) {
      const core = inside(content(lines[j]));
      // The line right before the END, or before its checksum, may be a short base64 word; further up only key-shaped lines count.
      const last = j === i - 1 || /^=[A-Za-z0-9+/]{4}$/.test(inside(content(lines[j + 1])));
      const short = last && /^[A-Za-z0-9+/]{4,}={0,2}$/.test(core);
      if (DIFF_BOUNDARY.test(lines[j]) || !(core === "" || ARMOUR_HEADER.test(core) || short || KEY_TAIL.test(core))) break;
      marked.add(j);
    }
  }
  return marked;
}

/** A line longer than this is replaced whole rather than scanned: nothing that long is shown, and a scan of it could cost seconds. */
export const LINE_CHARS = 262_144;

/**
 * The text with every secret-holding line replaced, a private key block
 * replaced whole as far as the hunk shows it, so no secret leaves the machine
 * for the classifier. Diff markers and hunk headers stay, so the hunks still
 * read as a change. Returns the text and how many lines went.
 */
export function redactSecrets(text) {
  let redacted = 0;
  let inKey = false;
  // The last line was key material shown without its BEGIN: a short final
  // line, an armour header or the END marker right after it belongs to it.
  let afterBody = false;
  // A YAML block scalar under a secret key: its value is every following line indented deeper than the key.
  let blockIndent = -1;
  // A key named on one line whose value is the next: a plist <key>password</key> then <string>…</string>,
  // or a Kubernetes env item `- name: DB_PASSWORD` then `value: …`.
  let valueNext = false;
  const KEY_ONLY = /<key>[^<]{0,80}(?:[Pp]assword|[Ss]ecret|[Tt]oken|[Aa][Pp][Ii]_?[Kk]ey|[Pp]assphrase|[Cc]redential)(?![^<]*(?:[Pp]ath|[Ff]ile|[Dd]ir|URL|[Uu]rl|[Nn]ame))[^<]{0,80}<\/key>\s*$|^\s*-?\s*name:\s*["']?[A-Za-z0-9_.-]{0,80}(?:PASSWORD|PASSWD|SECRET|TOKEN|API_?KEY|PASSPHRASE|password|secret|token|api_?key|passphrase)(?![A-Za-z0-9_.-]*(?:PATH|FILE|DIR|URL|NAME|path|file|dir|url|name))[A-Za-z0-9_.-]{0,80}["']?\s*$/;
  const VALUE_LINE = /^\s*<string>[^<]{4,}<\/string>\s*$|^\s*value:\s*\S/;
  const all = text.split("\n");
  const tails = keyTailsBeforeEnd(all);
  const lines = all.map((line, index) => {
    // The split's empty last segment is the text's final newline, not a line of the key.
    if (line === "" && index === all.length - 1) return line;
    // Replaced, not counted: it is not known to be a secret, only too long to scan.
    if (line.length > LINE_CHARS && !DIFF_BOUNDARY.test(line)) return redactLine(line);
    const body = content(line);
    // A file header names files; it is never a secret line itself (a hunk's trailer is checked below). Only
    // git's own forms count: a removed `-- comment` or an added `++ x` line is content, and is scanned.
    if (/^(?:diff --git |\+\+\+ (?:b\/|\/dev\/null)|--- (?:a\/|\/dev\/null))/.test(line)) {
      inKey = afterBody = false;
      blockIndent = -1;
      return line;
    }
    if (DIFF_BOUNDARY.test(line)) {
      blockIndent = -1;
      valueNext = false;
    }
    // Every value line that follows the key is the secret: a rotation shows the old and the new one. A blank
    // line, a comment or git's "No newline at end of file" marker in between keeps the pair open.
    if (valueNext) {
      if (VALUE_LINE.test(body)) {
        redacted++;
        return redactLine(line);
      }
      if (!(body.trim() === "" || /^\s*(?:#|<!--)/.test(body) || /^\\ No newline at end of file/.test(line))) valueNext = false;
    }
    // A hunk header keeps its structure whatever its trailer says; only a content line names a key.
    if (!HUNK_HEADER.test(line) && KEY_ONLY.test(body)) {
      // The key line goes with its value: a secret's name alone still says what the next line is.
      valueNext = true;
      redacted++;
      return redactLine(line);
    }
    if (blockIndent >= 0) {
      // git's "No newline at end of file" marker is no line of the file: the block goes on past it.
      if (/^\\ No newline at end of file/.test(line)) return line;
      const indent = body.length - body.trimStart().length;
      if (body.trim() === "" || indent > blockIndent) {
        if (body.trim() !== "") redacted++;
        return body.trim() === "" ? line : redactLine(line);
      }
      blockIndent = -1;
    }
    const block = BLOCK_SCALAR.exec(body);
    if (block) {
      // Under a list item the value is indented past the dash.
      blockIndent = block[1].length + (block[2] ? block[2].length : 0);
      redacted++;
      return redactLine(line);
    }
    if (tails.has(index)) {
      afterBody = true;
      // A blank line inside the block is kept as it is, and not counted.
      if (content(line).trim() === "") return line;
      redacted++;
      return redactLine(line);
    }
    // A block never runs past a diff boundary, and ends at the first line that could not belong
    // to a key (code after a BEGIN marker inside a string literal); indentation, quotes, armour
    // headers and blank lines are part of a block.
    if (DIFF_BOUNDARY.test(line) || (inKey && !/-----END /.test(body) && !keyLike(body))) inKey = false;
    if (DIFF_BOUNDARY.test(line)) afterBody = false;
    if (inKey) {
      redacted++;
      // The END line closes the block and the tail reached from it; what follows is ordinary text.
      if (/-----END /.test(body)) inKey = afterBody = false;
      return redactLine(line);
    }
    if (afterBody) {
      const core = inside(body);
      const tail = /-----END /.test(core) || ARMOUR_HEADER.test(core) || KEY_TAIL.test(core);
      afterBody = tail && !/-----END /.test(core);
      if (tail) {
        redacted++;
        return redactLine(line);
      }
    }
    const header = HUNK_HEADER.exec(line);
    if (header) {
      // git's context is the nearest earlier line: it may be a secret, or a key's body.
      // git cuts the trailer at 80 bytes, so a quoted secret may have lost its closing quote: test it closed too.
      const trailer = header[2].trim();
      // A block-scalar key as the trailer: the hunk's first lines are its value.
      const block = BLOCK_SCALAR.exec(header[2].replace(/^ /, ""));
      if (block) blockIndent = block[1].length + (block[2] ? block[2].length : 0);
      const closed = [trailer, `${trailer}"`, `${trailer}'`, `${trailer}\``];
      if (!trailer || (!block && !closed.some((t) => SECRET_LINE.test(t)) && !KEY_BODY_LINE.test(inside(trailer)))) return line;
      redacted++;
      return `${header[1]} ${REDACTED}`;
    }
    if (!SECRET_LINE.test(body) && !KEY_BODY_LINE.test(inside(body))) return line;
    redacted++;
    // A block opened here runs until its END line, unless this line closes it too.
    if (/-----BEGIN /.test(body) && !/-----END /.test(body)) inKey = true;
    if (KEY_BODY_LINE.test(inside(body))) afterBody = true;
    return redactLine(line);
  });
  return { text: lines.join("\n"), redacted };
}

export function summarize(base, include = [], limits = {}) {
  const { diffChars = DIFF_CHARS, summaryBytes = SUMMARY_BYTES, listedFiles = LISTED_FILES, listedSubjects = LISTED_SUBJECTS } = limits;
  const mergeBase = git(["merge-base", base, "HEAD"]);
  const snap = snapshot(include);
  const tree = git(["rev-parse", `${snap}^{tree}`]);
  // NUL-separated, so a path with a non-ASCII byte comes back as itself, not quoted.
  const all = git(["diff", "--numstat", "--no-renames", "-z", mergeBase, snap])
    .split("\0")
    .filter(Boolean)
    .map((entry) => {
      const [added, deleted, ...path] = entry.split("\t");
      // "-" means git did not count lines: a binary file, or one a .gitattributes `-diff` line hides.
      return { path: path.join("\t"), added: Number(added) || 0, deleted: Number(deleted) || 0, ...(added === "-" ? { binary: true } : {}) };
    });
  const lines = all.reduce((sum, file) => sum + file.added + file.deleted, 0);
  // Merges count too, as context-review counts them when it looks for an intent.
  const commits = git(["log", "--format=%s", `${mergeBase}..HEAD`]).split("\n").filter(Boolean);
  // Subjects go to the classifier too: each on its own, so one can never blank the next, and whole,
  // since a subject has no diff marker to keep.
  let redactedSubjects = 0;
  let subjects = commits.slice(0, listedSubjects).map((subject) => {
    if (subject.length > LINE_CHARS || SECRET_LINE.test(subject) || KEY_BODY_LINE.test(subject.trim())) {
      redactedSubjects++;
      return REDACTED;
    }
    return cut(subject, SUBJECT_CHARS);
  });
  // As the reviews read it: a `-diff` attribute cannot turn a source file into "Binary files differ",
  // while a real binary still is one and its bytes stay out of the diff.
  // Redacted before the cut, so a cut cannot split a key block around the check.
  // Only what can be shown is scanned: whole lines up to a few times the cut, since the cut is of the
  // redacted text and nothing past it leaves the machine. The counts are of those lines.
  const { text: clean, redacted } = redactSecrets(wholeLines(diffShowingHidden(mergeBase, snap, [...NOT_LOCKFILES, ...PRIVATE], ["--unified=2"]), 4 * diffChars));
  let diff = cut(clean, diffChars);
  let files = all.slice(0, listedFiles);
  const summary = () => ({ mergeBase, snapshot: snap, tree, files, fileCount: all.length, lines, subjects, commitCount: commits.length, redactedSubjects, diff, redacted });
  const bytes = () => Buffer.byteLength(JSON.stringify(summary()), "utf8");
  // JSON escaping can grow the diff; halve it until the line fits, then let the file list go, then the subjects.
  while (diff.length > 0 && bytes() > summaryBytes) diff = cut(diff, Math.floor(diff.length / 2));
  if (bytes() > summaryBytes) files = [];
  if (bytes() > summaryBytes) subjects = [];
  return summary();
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [base, ...include] = process.argv.slice(2);
  if (!base) {
    console.error("usage: change-summary.mjs <base> [include...]");
    process.exit(2);
  }
  console.log(JSON.stringify(summarize(base, include)));
}
