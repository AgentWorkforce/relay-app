import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { KEY_BODY_LINE, LINE_CHARS, redactSecrets, SECRET_LINE, summarize, wholeLines } from "./change-summary.mjs";

/** A repo on `work`, one commit past `main`, with a tracked edit not yet committed and a lockfile in the commit. */
function repo() {
  const dir = mkdtempSync(join(tmpdir(), "change-summary-test-"));
  const git = (...args) => execFileSync("git", ["-C", dir, ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main");
  git("config", "user.email", "t@example.com");
  git("config", "user.name", "t");
  mkdirSync(join(dir, "src"));
  writeFileSync(join(dir, "src/a.txt"), "one\n");
  git("add", ".");
  git("commit", "-qm", "base");
  git("checkout", "-qb", "work");
  writeFileSync(join(dir, "src/b.txt"), "committed\n");
  writeFileSync(join(dir, "Cargo.lock"), "lock\n".repeat(50));
  git("add", ".");
  git("commit", "-qm", "Add b and a lockfile");
  writeFileSync(join(dir, "src/a.txt"), "two\n");
  return { dir, git };
}

const inDir = (dir, fn) => {
  const before = process.cwd();
  process.chdir(dir);
  try {
    return fn();
  } finally {
    process.chdir(before);
  }
};

test("the summary counts the commit and the uncommitted edit, lists the lockfile but keeps it out of the diff", () => {
  const { dir, git } = repo();
  const change = inDir(dir, () => summarize("main"));
  assert.equal(change.mergeBase, git("rev-parse", "main"));
  assert.deepEqual(change.files.map((f) => f.path).sort(), ["Cargo.lock", "src/a.txt", "src/b.txt"]);
  assert.equal(change.fileCount, 3);
  assert.equal(change.files.find((f) => f.path === "src/a.txt").deleted, 1, "the tracked edit is in the snapshot");
  assert.equal(change.lines, 50 + 1 + 2);
  assert.deepEqual(change.subjects, ["Add b and a lockfile"]);
  assert.equal(change.commitCount, 1);
  assert.ok(change.diff.includes("+two"), "the working-tree edit is in the diff");
  assert.ok(change.diff.includes("+committed"));
  assert.ok(!change.diff.includes("lock\n+lock"), "the lockfile's lines are not");
  assert.notEqual(change.snapshot, git("rev-parse", "HEAD"), "the snapshot is a commit past HEAD");
  assert.equal(change.tree, git("rev-parse", `${change.snapshot}^{tree}`));
});

test("an untracked file is counted only when it is in include, and the diff is cut to the given size", () => {
  const { dir } = repo();
  writeFileSync(join(dir, "src/new.txt"), "x\n".repeat(100));
  const without = inDir(dir, () => summarize("main"));
  assert.ok(!without.files.some((f) => f.path === "src/new.txt"));
  const withIt = inDir(dir, () => summarize("main", ["src/new.txt"], { diffChars: 40 }));
  assert.ok(withIt.files.some((f) => f.path === "src/new.txt" && f.added === 100));
  assert.equal(withIt.diff.length, 40);
});

test("the private core and non-ASCII paths: desktop-core is listed and counted but out of the diff; a path with accents comes back unquoted", () => {
  const { dir, git } = repo();
  mkdirSync(join(dir, "desktop-core"));
  writeFileSync(join(dir, "desktop-core/secret.rs"), "fn private() {}\n");
  writeFileSync(join(dir, "src/café.txt"), "crème\n");
  git("add", ".");
  const change = inDir(dir, () => summarize("main"));
  assert.ok(change.files.some((f) => f.path === "desktop-core/secret.rs" && f.added === 1));
  assert.ok(!change.diff.includes("fn private"), "the private core's lines never go to the classifier");
  writeFileSync(join(dir, "src/api_token.txt"), "tok_live_123\n");
  writeFileSync(join(dir, ".env"), "PASSWORD=hunter2\n");
  writeFileSync(join(dir, "src/Secrets.xcconfig"), "API_KEY = sk_live_456\n");
  writeFileSync(join(dir, "src/CloudCredentialRefresh.swift"), "let refreshSecret = \"rs_789\"\n");
  writeFileSync(join(dir, "src/tab\tname.txt"), "tabbed\n");
  git("add", ".");
  const more = inDir(dir, () => summarize("main"));
  assert.ok(more.files.some((f) => f.path === "src/api_token.txt") && more.files.some((f) => f.path === ".env"));
  assert.ok(!more.diff.includes("tok_live_123") && !more.diff.includes("hunter2"), "credential-shaped files stay out of the diff");
  assert.ok(!more.diff.includes("sk_live_456") && !more.diff.includes("rs_789"), "whatever their case");
  assert.ok(more.files.some((f) => f.path === "src/Secrets.xcconfig"), "but they are listed");
  for (const [name, body] of [["infra/terraform.tfstate", "tfstateplain"], [".s3cfg", "s3cfgbody"], ["AuthKey_ABC123.p8", "p8body"], [".npmrc", "//registry/:_authToken=npm_777"], [".netrc", "login x password netrcpw"], ["src/pw/password.rs", "const PW = \"pwrs\";"],
    ["secrets/config.json", "{\"k\":\"secdir\"}"], ["credentials/aws.json", "credsdir"], ["private/keys/x.txt", "privdir"], [".envrc", "export T=envrc1"],
    ["infra/prod.tfvars", "tfv1"], ["ops/kubeconfig.yaml", "kube1"], [".pypirc", "pypi1"], [".docker/config.json", "dock1"], ["src/api_key.txt", "apik1"], ["keys/a.ppk", "ppk1"]]) {
    mkdirSync(join(dir, name.includes("/") ? name.slice(0, name.lastIndexOf("/")) : "."), { recursive: true });
    writeFileSync(join(dir, name), `${body}\n`);
  }
  git("add", ".");
  const keys = inDir(dir, () => summarize("main"));
  for (const secret of ["tfstateplain", "s3cfgbody", "p8body", "npm_777", "netrcpw", "pwrs", "secdir", "credsdir", "privdir", "envrc1", "tfv1", "kube1", "pypi1", "dock1", "apik1", "ppk1"]) assert.ok(!keys.diff.includes(secret), secret);
  assert.ok(more.files.some((f) => f.path === "src/tab\tname.txt"), "a tab in a path survives");
  assert.ok(change.files.some((f) => f.path === "src/café.txt"), JSON.stringify(change.files));
});

test("the printed line stays under its byte bound: subjects and files are capped, the diff shrinks, then the file list goes", () => {
  const { dir, git } = repo();
  for (let i = 0; i < 5; i++) {
    writeFileSync(join(dir, `src/c${i}.txt`), `${"é".repeat(300)}\n`);
    git("add", ".");
    git("commit", "-qm", `Commit number ${i} ${"x".repeat(60)}`);
  }
  const raw = inDir(dir, () => summarize("main", [], { summaryBytes: 1e9 }));
  assert.equal(raw.commitCount, 6);
  assert.equal(raw.subjects.length, 6);
  const capped = inDir(dir, () => summarize("main", [], { listedSubjects: 2, listedFiles: 3, summaryBytes: 1e9 }));
  assert.equal(capped.subjects.length, 2);
  assert.equal(capped.commitCount, 6);
  assert.equal(capped.files.length, 3);
  assert.equal(capped.fileCount, 8);
  const rawBytes = Buffer.byteLength(JSON.stringify(raw), "utf8");
  assert.ok(rawBytes > 2000, `the unshrunk line is ${rawBytes} bytes`);
  const shrunk = inDir(dir, () => summarize("main", [], { summaryBytes: rawBytes - 500 }));
  assert.ok(Buffer.byteLength(JSON.stringify(shrunk), "utf8") <= rawBytes - 500);
  assert.ok(shrunk.diff.length > 0 && shrunk.diff.length < raw.diff.length, "the diff shrank");
  assert.equal(shrunk.files.length, raw.files.length, "the file list stayed");
  const tiny = inDir(dir, () => summarize("main", [], { summaryBytes: 700 }));
  assert.ok(Buffer.byteLength(JSON.stringify(tiny), "utf8") <= 700);
  assert.equal(tiny.diff, "");
  assert.deepEqual(tiny.files, []);
  assert.equal(tiny.fileCount, 8, "the counts stand");
  assert.ok(tiny.subjects.length > 0, "the subjects fit at this size");
  const tinier = inDir(dir, () => summarize("main", [], { summaryBytes: 300 }));
  assert.ok(Buffer.byteLength(JSON.stringify(tinier), "utf8") <= 300);
  assert.deepEqual(tinier.subjects, [], "the subjects go last");
  assert.equal(tinier.commitCount, 6);
});

test("a long subject is cut, merges are counted, and a cut never ends on half an emoji", () => {
  const { dir, git } = repo();
  writeFileSync(join(dir, "src/d.txt"), "😀".repeat(40) + "\n");
  git("add", ".");
  git("commit", "-qm", "x".repeat(500));
  git("checkout", "-q", "main");
  git("checkout", "-qb", "side");
  writeFileSync(join(dir, "src/e.txt"), "e\n");
  git("add", ".");
  git("commit", "-qm", "side");
  git("checkout", "-q", "work");
  git("merge", "-q", "--no-ff", "-m", "Merge side", "side");
  const change = inDir(dir, () => summarize("main"));
  assert.equal(change.subjects[0], "Merge side", "merges count, as context-review counts them");
  assert.equal(change.commitCount, 4);
  assert.equal(change.subjects.find((s) => s.startsWith("xxx")).length, 200);
  const at = change.diff.search(/[\uD800-\uDBFF]/);
  assert.ok(at > 0, "the diff holds an emoji");
  for (const chars of [at + 1, at + 3]) {
    const cut = inDir(dir, () => summarize("main", [], { diffChars: chars }));
    assert.ok(!/[\uD800-\uDBFF]$/.test(cut.diff), `no lone high surrogate at ${chars}`);
    assert.equal(cut.diff.length, chars - 1, "one short of the naive cut, which would end on half an emoji");
  }
  // One leading character, so the cut at 200 units would land between the halves of an emoji.
  git("commit", "-q", "--allow-empty", "-m", `a${"😀".repeat(150)}`);
  const emoji = inDir(dir, () => summarize("main"));
  assert.ok(!/[\uD800-\uDBFF]$/.test(emoji.subjects[0]) && emoji.subjects[0].length === 199, "a subject of emoji is cut on a pair");
});

test("the command prints one JSON line equal to summarize(), and refuses to run without a base", () => {
  const { dir } = repo();
  const script = fileURLToPath(new URL("./change-summary.mjs", import.meta.url));
  const run = spawnSync(process.execPath, [script, "main"], { cwd: dir, encoding: "utf8" });
  assert.equal(run.status, 0, run.stderr);
  const lines = run.stdout.split("\n").filter(Boolean);
  assert.equal(lines.length, 1);
  const printed = JSON.parse(lines[0]);
  const direct = inDir(dir, () => summarize("main"));
  assert.deepEqual({ ...printed, snapshot: "", tree: "" }, { ...direct, snapshot: "", tree: "" }, "the same change, bar the snapshot commit made for each call");
  assert.equal(printed.tree, direct.tree, "and the same tree");
  const usage = spawnSync(process.execPath, [script], { cwd: dir, encoding: "utf8" });
  assert.equal(usage.status, 2);
  assert.match(usage.stderr, /usage/);
});

test("a .gitattributes -diff line cannot hide a text file, while a real binary's bytes stay out of the diff", () => {
  const { dir, git } = repo();
  writeFileSync(join(dir, ".gitattributes"), "src/hidden.txt -diff\n*.png binary\n");
  writeFileSync(join(dir, "src/hidden.txt"), "line\n".repeat(600));
  mkdirSync(join(dir, "docs/img"), { recursive: true });
  writeFileSync(join(dir, "docs/img/shot.png"), Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 0, 0]), Buffer.from("PNGPAYLOAD".repeat(2000))]));
  git("add", ".");
  const change = inDir(dir, () => summarize("main"));
  const hidden = change.files.find((f) => f.path === "src/hidden.txt");
  assert.equal(hidden.binary, true, "numstat still reports it uncounted");
  assert.equal(hidden.added, 0);
  assert.ok(change.diff.includes("+line\n+line"), "the hidden file's text is in the diff anyway");
  assert.ok(!change.diff.includes("PNGPAYLOAD"), "a real binary is still binary, attribute or not");
  assert.ok(change.diff.includes("Binary files") || !change.diff.includes("shot.png\n@@"));
  assert.ok(!change.files.find((f) => f.path === ".gitattributes").binary);
});

test("a secret inside an ordinarily named file is redacted before the diff leaves the machine; names alone are not", () => {
  const { dir, git } = repo();
  writeFileSync(join(dir, "src/cloud.rs"), 'const KEY: &str = "sk_live_abcdefghijklmnop";\nlet token: String = read();\nconst PASSWORD = "hunter2hunter2";\n');
  writeFileSync(join(dir, "src/server.asc"), "-----BEGIN PGP PRIVATE KEY BLOCK-----\nascbody\n-----END PGP PRIVATE KEY BLOCK-----\n");
  writeFileSync(join(dir, "src/notes.txt"), "-----BEGIN RSA PRIVATE KEY-----\nMIIEkeybody\nMIIEkeybody2\n-----END RSA PRIVATE KEY-----\nafter the key\n");
  git("add", ".");
  const change = inDir(dir, () => summarize("main"));
  assert.ok(!change.diff.includes("sk_live_abcdefghijklmnop"), "a token literal is redacted");
  assert.ok(!change.diff.includes("hunter2hunter2"), "a password literal is redacted");
  assert.ok(change.diff.includes("+let token: String = read();"), "a name alone is kept");
  assert.ok(!change.diff.includes("ascbody"), "an armoured file is redacted by content, not hidden by its .asc name");
  assert.ok(!change.diff.includes("MIIEkeybody"), "a key block is redacted whole");
  assert.ok(change.diff.includes("+after the key"), "and the line after it is kept");
  assert.equal(change.redacted, 2 + 4 + 3, "two literals, a four-line key block and a three-line armoured block");
  assert.ok(change.files.some((f) => f.path === "src/server.asc"), "redacted files are still listed");
});

test("the secret-line shapes: real key formats and key-like assignments match, however prefixed or quoted; ordinary code does not", () => {
  for (const line of [
    "ghp_abcdefghijklmnopqrstuvwxyz", "xoxb-1234567890-abcdefghij", "glpat-abcdefghijklmnopqrst",
    'const AWS_ID = "AKIAIOSFODNN7EXAMPLE";', "ASIAY34FZKBOKMUTVHFA", 'const g = "AIzaSyA1b2C3d4E5f6G7h8I9j0KlMnOpQrStUv";',
    'api_key = "0123456789abcdef"', "password: supersecretvalue123456", "-----BEGIN EC PRIVATE KEY-----",
    "DB_PASSWORD=hunter2hunter2hunter2", 'GITHUB_TOKEN="0123456789abcdefghij"', '  "api_key": "0123456789abcdefghij",', "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    'const secretKey = "s3cr3tvalue12345";', 'SECRET_KEY_BASE="c9f8e7d6c5b4a3f2e1d0"', 'DB_PASSWORD_PROD="s3cr3t-p4ssw0rd-value"', 'private_key_pem = "MIIEvQIBADANBgkqhkiG"',
    "TOKEN_VALUE=abcdefghijklmnopqrstuvwxyz", 'password_hash = "$2b$12$abcdefghijklmnopqrstuv"', 'refresh_token_value = "0123456789abcdefghij"', 'AWS_SECRET_ACCESS_KEY="wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY"',
    'MEILI_MASTER_KEY = "9f8c7b6a5d4e3f2a1b0c"', 'deploy_key = "0123456789abcdef"', 'APP_KEY="base64:aBcDeFgHiJkLmNoPqRsT"', 'session_key: "0123456789abcdef"', 'hmac_key = "0123456789abcdef"', 'JWT_KEY="0123456789abcdef"',
    'signing_key = "0123456789abcdef"', 'ssh_key: "0123456789abcdef"', 'client_key = "0123456789abcdef"', 'license_key = "0123456789abcdef"',
    'PRODUCTION_DATABASE_PRIMARY_REPLICA_CONNECTION_PASSWORD="hunter2hunter2"', "RELAY_DESKTOP_REMOTE_CI_GITHUB_PERSONAL_ACCESS_TOKEN=abcdefghijklmnopqrst",
    'KEYSTORE_PASSWORD_ANDROID="s3cr3tvalue1234"', 'api_secret_valid = "0123456789abcdef"', 'api_key_profile = "0123456789abcdef"', 'token_guid = "0123456789abcdef"', 'apiKeyValue: "0123456789abcdef"',
    'const secrets = "sup3r-s3cret-v4lue";', 'export const apiKeys = "0123456789abcdefghij";', 'refreshTokens: "0123456789abcdef"', 'api_keys: "0123456789abcdef"', 'password2 = "hunter2hunter2"', "SECRET1=abcdefghijklmnopqrstuvwxyz",
    'API_KEY_FOR_THE_PROD_BUILD_AGENT_RUNNER = "0123456789abcdef"', 'TOKEN_FOR_THE_RELAY_DESKTOP_CI_BOT="0123456789abcdef"', 'password_mysupersecretdatabase = "hunter2hunter2"',
    'const deployKey = "AAAAB3NzaC1yc2EAAAADAQABAAABgQ";', 'signingKey = "0123456789abcdef"', 'masterKey: "0123456789abcdef"', 'hmacKey = "0123456789abcdef"', 'preshared_key = "iNqA3uhPbfZfCLYQNfNGLDyBe0Qf7wRCGWgCPIrZKnM="', 'preshared_key = "0123456789abcdef"',
    'DOCKER_REGISTRY_KEY="hunter2hunter2hunter2"', 'PROD_DOCKER_REGISTRY_KEY="hunter2hunter2hunter2"', 'settings_key = "0123456789abcdef"', 'defaults_key = "0123456789abcdef"', 'session_key = "0123456789abcdef"', 'ACCESS_KEY="0123456789abcdef"',
    'token12345 = "0123456789abcdef"', 'api_key_1234567890123456789012345678901234567890 = "0123456789abcdef"', `password${"1".repeat(41)} = "hunter2hunter2"`, `token${"9".repeat(60)} = "0123456789abcdef"`,
    'host_key = "0123456789abcdef"', 'HOST_KEY="0123456789abcdef"', 'const accessKey = "0123456789abcdef";', 'let sessionKey = "0123456789abcdef"', 'clientKey = "0123456789abcdef"', 'hostKey: "0123456789abcdef"', 'AppKey = "0123456789abcdef"',
    'static let apiKey: String = "0123456789abcdef"', 'const PASSWORD: &str = "hunter2hunter2";', 'pub const API_KEY: &str = "0123456789abcdef";', 'var secretKey: String? = "0123456789abcdef"', 'export const token: string = "0123456789abcdef";',
    'password := "hunter2hunter2"', 'token => "0123456789abcdef"',
    "postgres://relay:s3cr3tp4ss@db.example.internal/relay", "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
    "a bare sk-ant-api03-abcdefghijklmnop key", "npm_abcdefghijklmnopqrstuv", "AGE-SECRET-KEY-1QYQSZQGPQYQSZQGPQYQSZQGPQYQS", "https://hooks.slack.com/services/T0000000/B0000000/abcdefghijklmnop",
    "POSTGRES_PASSWORD=mysecretpassword", "MYSQL_ROOT_PASSWORD=rootpass123", "SMTP_PASSWORD=xk9#Lm2$pQ7vR4wN", "password: s3cr3t!pass", "Authorization: Basic dXNlcjpwYXNz",
    'PASSWORD="it\'s-a-secret-123"', "SECRET_KEY = b'_5#y2L\"F4Q8z\\n\\xec]/'", 'password = r"hunter2hunter2"', 'NSString *password = @"hunter2hunter2";', 'PASSWORD = "say \\"hi\\" to secrets"',
    'app.config["SECRET_KEY"] = "0123456789abcdef"', 'os.environ["GITHUB_TOKEN"] = "0123456789abcdef"', 'if password == "hunter2hunter2" {', 'credentials = "0123456789abcdefghij"', 'credentials: "user:hunter2hunter2"',
    'PASSWORD = """hunter2hunter2hunter2"""', "token = abcdefghijklmnop1234;", "api_key: c0ffee-c0ffee-c0ffee,", "export AWS_SECRET=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c", "sk-proj-abcdefghijklmnop", "const token = `0123456789abcdef`;", "token = 'abcdefghijklmnop1234'",
    "PASS=hunter2hunter2", "PGPASSWORD=hunter2hunter2", "passphrase = \"correct horse battery\"", "Authorization: Token 0123456789abcdef", "Authorization: ApiKey 0123456789abcdef",
    'const cs = "Server=db;User=sa;Password=hunter2hunter2;Database=x";', 'url = "https://example.com/v1?api_key=0123456789abcdef"', 'DATABASE_URL="Server=db;User=sa;Password=hunter2hunter2;"',
    'body: "client_id=x&client_secret=0123456789abcdef&grant_type=client_credentials"', "shared_key = \"0123456789abcdef\"", "primary_shared_key: 0123456789abcdef",
    "password: s3cr3t.pa$$word", "secret = abc&def!ghi1", 'defaults.set("password", "hunter2hunter2")', "put('token', '0123456789abcdef')", "CREATE USER app IDENTIFIED BY 'hunter2hunter2';", "ALTER ROLE app WITH PASSWORD 'hunter2hunter2';", "ENV DB_PASSWORD hunter2hunter2", "ENV API_TOKEN=0123456789abcdef", 'token = os.getenv("TOKEN", "0123456789abcdef")', "secret = env.get('SECRET', 'hunter2hunter2')", 'let token = Some("0123456789abcdef");', 'password: String::from("hunter2hunter2"),', "password: |2", "  secret: >-1", "ssh_key: |-2", "password: mysecretpassword", "password: MySecretPassword", "PASSWORD = '''hunter2hunter2'''", '<key>APIKey</key><string>0123456789abcdef</string>', "<password>hunter2hunter2</password>", '"password": |', "  - 'db_password': >", "passwd = CorrectHorse", "token: my-secret-token-value", "redis://:hunter2hunter2@cache.internal/0", "curl --password 'hunter2hunter2' https://x", "--token=\"0123456789abcdef\"", "db_password: supersecretvalue", "DB_PASS=hunter2hunter2", "DB_PASS: hunter2hunter2", "token: abcdefgh.ijklmnop1", "PASS = hunter2hunter2", 'secret_id = "0123456789abcdef"', 'Secret("0123456789abcdef")', 'new Password("hunter2hunter2")',
    "Authorization: Bearer abcdefghijklmnop1234",
    "machine api.example.com login deploy password netrcpw1", "curl --password hunter2hunter2 https://x", "psql --password=hunter2hunter2", "curl -u deploy:hunter2hunter2 https://x", "token: |", "  DB_PASSWORD: >-", "ssh_key: |-",
    `PASSWORD=${"a".repeat(1500)}`, `password = ${"a1".repeat(800)}`, "POSTGRES_PASSWORD: mysecretpassword", "  MYSQL_ROOT_PASSWORD: rootpassword", 'API_KEY: "0123456789abcdef"', "tls.key: LS0tLS1CRUdJTiBSU0EgUFJJVkFURSBLRVktLS0tLQpNSUlFdlFJQkFEQU5CZ2txaGtpRzl3MEJBUUVGQUFTQ0JLY3dnZ1NqQWdFQUFvSUJBUUM3YWJjZGVmCi0tLS0tRU5EIFJTQSBQUklWQVRFIEtFWS0tLS0t", "ssh.key: 0123456789abcdef",
  ]) {
    assert.ok(SECRET_LINE.test(line), line);
  }
  for (const line of ["token: string", "const token = parse(input);", "secret = os.environ['X']", "let password = prompt();", "// the API key is read from the keychain", "const t = /token (\\S+)/.exec(started)?.[1] ?? \"\";", "AKIA_ABCDEFGHIJKLMNOP is not a real key shape", "tokenizer = build()",
    'local api_key_path="$TMP_DIR/AuthKey.p8"', "const tokenPath = resolveStateDirectory(flow);", "token = resolveStateDirectory(flow);", 'APPLE_API_KEY_ID="${APPLE_API_KEY_ID:-}"',
    'cache_key = "profile-v2-abcdef"', 'primary_key = "id_of_the_row"', 'partition_key: "tenant-00000001"', "shared-key: ci-linux-probe-zig-v1-${{ matrix.arch }}-abcdef", '"public_key": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5"', 'translation_key = "settings.window.title"',
    'sort_key: "created_at_desc"', 'idempotency_key = "retry-after-crash"', 'token_uri = "https://oauth2.example/token"', 'secret_file = "/run/secrets/db"', 'SECRET_NAME="relay-prod-secret"', 'api_key_dir = "/etc/relay/keys"', 'token_url: "https://example.org/t"', 'apiKeyId: "ABCDEFGHIJKLMNOP"',
    'const tokenizer = "cl100k_base";', 'tokenizer_config = "sentencepiece-bpe-32k"', 'secretariat = "united nations org"', 'passwordless = "magic-link-login"',
    'foreign_key = "accounts.workspace_id"', 'routing_key = "relay.events.desktop"', 'index_key = "created_at_workspace"', 'caches_key = "profile-v2-abcdef"', 'cacheKey = "profile-v2-abcdef"',
    "INFOPLIST_KEY_UISupportedInterfaceOrientations_iPad: UIInterfaceOrientationPortrait UIInterfaceOrientationLandscapeLeft", 'locale_key = "settings.window.title"', 'accelerator_key = "<Primary>comma-and-more"',
    'UserDefaults.standard.string(forKey: "relay.mobileName")', 'monkey = "see-monkey-do-things"', "let token: String = read()", "case AsiaPacificSouthEast = 1", 'let akiaPrefix = "AKIA_literal_marker"',
    "https://relay.example.com/path?x=1", "Bearer tokens are read from the keychain", 'password = os.getenv("PASSWORD")', "token = resolveStateDirectory(flow)", "secret = short1",
    "token: self.token,", "let token = pausedWanted.token", "let token = changeToken", "guard token == self.token else {", "password: Option<String>,", "token: boolean;",
    "token: &Secret,", "pub view_key: ViewKey,", "legacy-key: probe-${{ matrix.arch }}-macos", "const token = process.env.GITHUB_TOKEN;", "secret: ${{ secrets.DEPLOY_SECRET }}",
    "token_ttl = 3600", "api_key_header = X-Api-Key-Header", "Basic authentication is off", "Use Basic credentials here", "Bearer token-from-keychain", "api_key: ${API_KEY}", "token: resolveToken(flow),",
    "PASSWORD=${{ secrets.PASSWORD }}", "cache-key=probe-${{ matrix.arch }}", "TOKEN=${TOKEN_FROM_ENV}", "token=resolveToken(flow)", "SECRET=<set-in-ci>", "password=\"\"", "PWD=/home/agent/src",
    "let bypassProxy = configured.bypass", "compassMode = headingTrue1", "trespass = notAllowed1", "api_key: $API_KEY_FROM_ENV", "token = $TOKEN_VALUE1", "secret_id_path = /run/secrets/id",
    "PASSWORD=$SECRET_VALUE_FROM_ENV", "`SECRET_KEY` (`other literal here`)", "TOKEN: string", "API_KEY_HEADER: X-Api-Key", "DB_PASSWORD: ${DB_PASSWORD}", "GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}", "TOKEN: process.env.GITHUB_TOKEN", "PASSWORD: Option<String>,", "SECRET_ENV: PRODUCTION",
    "CACHE_KEY: probe-linux-x64-v2", "RESTORE_KEYS: probe-linux-x64-", "PRIMARY_KEY: user_id_column", "the token expires after an hour", "--password-stdin reads it", "items: |", "description: >-", "secret_key_path: |",
    "token_expires_at = 1700000000", 'PASSWORD="$DB_PASSWORD"', "TOKEN='${TOKEN:-}'", 'password: "$PASSWORD_FROM_ENV"', "ENV DB_PASSWORD $DB_PASSWORD", "ENV PATH /usr/local/bin", 'password = os.getenv("PASSWORD")', "IDENTIFIED BY 'x'", "let token = &self.token;", "token: @escaping Completion", "password: unchanged", "password: disabled", '<key>APIKeyPath</key><string>/etc/keys/api</string>', "<password>short</password>", "api_key_header_name = x-api-key-header", "docker run -u 1000:1000 image", "--password <value> sets the password", "--token [token] is optional", "--token overrides the token", "password: String",
    "BYPASS_PROXY=0123456789abcdef", "COMPASS_MODE: headingTrue1234", "PASS_RATE = 0.9876543210", "PASSTHROUGH_ARGS=--foo-bar-baz", "token: session?.token,", 'if map.contains_key("0123456789abcdef") {', 'defaults.object(forKey: "0123456789abcdef")', "password: undefined", "password: required", "token: PasswordField",
    // The trade-off for the nouns Swift identifiers also use: whole-identifier camel forms only.
    'public static let contractorAccessKey = "ContractorAccessEnabled"', 'case sessionKeys = "session_keys"', 'let appKeyWindow = "main-window-key"', 'let mySessionKey = "window-session-key"',
    "        restore-keys: windows-cargo-sources-v2-", 'object_keys = "0123456789abcdef"',
    'plist_key = "NSCameraUsageDescription-x"', 'gsettings_key = "org.gnome.desktop.a11y"', 'accel_key = "<Primary>comma-plus"', 'menu_key = "file.open.recent"', 'i18n_key = "settings.window.title"', 'row_key = "tenant-00000001"', 'map_key = "tile-12-34-56"']) {
    assert.ok(!SECRET_LINE.test(line), line);
  }
  assert.ok(KEY_BODY_LINE.test("+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7"));
  assert.ok(!KEY_BODY_LINE.test("+const x = MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7;"));
  assert.ok(!KEY_BODY_LINE.test("+3b68f5cf0a924d94fb4db6136ce978f272fffa18574b6b8fc88221e104cf6f5c"), "a hex hash is not a key body");
  assert.ok(!KEY_BODY_LINE.test("x".repeat(500)), "nor a run of one letter");
  assert.ok(!KEY_BODY_LINE.test("ThisIsAVeryLongCamelCaseIdentifierNameWithoutDigits"), "nor mixed case without a digit");
  const started = Date.now();
  SECRET_LINE.test("a".repeat(200_000));
  SECRET_LINE.test("token".repeat(40_000));
  SECRET_LINE.test(`token${"1".repeat(2_000)}`);
  SECRET_LINE.test(`api_keyA${"9".repeat(500)}`);
  redactSecrets(Array.from({ length: 200 }, (_, i) => `+  { "user_tokenA${i}${"7".repeat(42)}": true },`).join("\n"));
  redactSecrets(`+${("token" + "1".repeat(48) + " ").repeat(50)}`);
  // Alternating segments with no assignment after them: the shapes that once re-partitioned for minutes.
  SECRET_LINE.test(`+token${("_" + "1".repeat(20)).repeat(12)}!`);
  SECRET_LINE.test(`token${("A" + "1".repeat(40)).repeat(6)}`);
  SECRET_LINE.test(`api_key${("_" + "1".repeat(40)).repeat(5)}`);
  SECRET_LINE.test(`api_key${("_" + "1".repeat(32)).repeat(6)}`);
  SECRET_LINE.test(`api_key${("A" + "1".repeat(40)).repeat(4)}`);
  SECRET_LINE.test(`+token${("_" + "1".repeat(20)).repeat(12)}${("A" + "2".repeat(30)).repeat(8)}`);
  SECRET_LINE.test("token-".repeat(20_000));
  SECRET_LINE.test("a-key-".repeat(20_000));
  SECRET_LINE.test(`PASSWORD=${"a".repeat(100_000)}`);
  SECRET_LINE.test(`PASSWORD=${"a".repeat(LINE_CHARS)}{`);
  // Lines of many key words inside one value run, at the length a line can have before it is replaced whole.
  SECRET_LINE.test(`${"password=a1".repeat(LINE_CHARS / 11)}(`);
  SECRET_LINE.test(`${"secret0=1,e.".repeat(LINE_CHARS / 12)})`);
  SECRET_LINE.test(`x="${"token=ab".repeat(LINE_CHARS / 8)}`);
  SECRET_LINE.test(`var x=${"a".repeat(LINE_CHARS)}{`);
  SECRET_LINE.test(`password: ${"a1".repeat(LINE_CHARS / 2)}`);
  SECRET_LINE.test(`password: ${"a1".repeat(LINE_CHARS / 2)}(`);
  SECRET_LINE.test(`${"_SECRET_A".repeat(LINE_CHARS / 9)}: x`);
  SECRET_LINE.test(`${"_SECRET_A".repeat(LINE_CHARS / 9)}: |`);
  SECRET_LINE.test(`${"secret.a".repeat(LINE_CHARS / 8)}: |`);
  SECRET_LINE.test(`token: ${" ".repeat(LINE_CHARS)}`);
  SECRET_LINE.test(`token:${" x".repeat(LINE_CHARS / 2)}`);
  SECRET_LINE.test(`${"eyJabcdefgh-".repeat(LINE_CHARS / 12)}`);
  SECRET_LINE.test(`--password ${"a".repeat(LINE_CHARS)}(`);
  SECRET_LINE.test(`${"--token ab".repeat(LINE_CHARS / 10)}"`);
  SECRET_LINE.test(`${": |#".repeat(LINE_CHARS / 4)}\r`);
  SECRET_LINE.test(`token${" ".repeat(LINE_CHARS)}`);
  SECRET_LINE.test(`password${" ".repeat(LINE_CHARS)}= x`);
  redactSecrets(`+"${" ".repeat(LINE_CHARS)}\n`);
  SECRET_LINE.test(`SECRET_KEY: ${"A".repeat(LINE_CHARS)}(`);
  redactSecrets(`+${"a ".repeat(LINE_CHARS / 2)}\\\n+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n`);
  // A generous bound for a loaded host: the shapes above cost seconds when the scan is quadratic.
  assert.ok(Date.now() - started < 3000, `no line, however shaped, costs more than a pass to scan (${Date.now() - started} ms)`);
  const long = redactSecrets(`+${"a".repeat(LINE_CHARS + 1)}\n+short\n`);
  assert.equal(long.text, "+[redacted]\n+short\n", "a line past the scan limit is replaced whole");
  assert.equal(long.redacted, 0, "replaced, but not counted as a secret");
  const { text, redacted } = redactSecrets("+x = 1\n+password = \"abcdefghijklmnop\"\n y = 2\n-ghp_abcdefghijklmnopqrstuvwxyz\n");
  assert.equal(text, "+x = 1\n+[redacted]\n y = 2\n-[redacted]\n");
  assert.equal(redacted, 2);
});

test("a key block is redacted as far as the hunk shows it, keeps its markers, and never runs past a diff boundary or a one-line key", () => {
  const block = "+-----BEGIN RSA PRIVATE KEY-----\n+MIIEbody1\n+MIIEbody2\n+-----END RSA PRIVATE KEY-----\n+after\n";
  const whole = redactSecrets(block);
  assert.equal(whole.text, "+[redacted]\n+[redacted]\n+[redacted]\n+[redacted]\n+after\n");
  assert.equal(whole.redacted, 4);
  const oneLine = redactSecrets('+fixture = "-----BEGIN RSA PRIVATE KEY-----\\nMIIE-----END RSA PRIVATE KEY-----";\n+next line stays\n');
  assert.equal(oneLine.text, "+[redacted]\n+next line stays\n", "a BEGIN closed on its own line opens no block");
  const unclosed = redactSecrets("+-----BEGIN RSA PRIVATE KEY-----\n+MIIEbody\ndiff --git a/other b/other\n@@ -1 +1 @@\n+other file line\n");
  assert.equal(unclosed.text, "+[redacted]\n+[redacted]\ndiff --git a/other b/other\n@@ -1 +1 @@\n+other file line\n", "the block ends at the next diff boundary");
  assert.equal(unclosed.redacted, 2);
  const body = redactSecrets(" MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n-MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7ghijkl\r\n+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7mnopqr\n");
  assert.equal(body.text, " [redacted]\n-[redacted]\n+[redacted]\n", "key material with its BEGIN line outside the hunk is still redacted, CRLF or not");
  const boundaries = redactSecrets('@@ -6,3 +6,3 @@ password = "supersecretvalue123456"\n context\n--- password = "supersecretvalue123456"\n+++ api_key = "0123456789abcdefghij"\n--- a/src/x.sql\n+++ b/src/x.sql\n');
  assert.equal(boundaries.text, "@@ -6,3 +6,3 @@ [redacted]\n context\n-[redacted]\n+[redacted]\n--- a/src/x.sql\n+++ b/src/x.sql\n", "a header's trailer and a deleted comment are redacted; real headers stay");
  assert.equal(boundaries.redacted, 3);
  const bodyHeader = redactSecrets("@@ -5,5 +5,5 @@ MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC2abcdefgh\n context\n");
  assert.equal(bodyHeader.text, "@@ -5,5 +5,5 @@ [redacted]\n context\n", "a key body line as the header's context is redacted too");
  const literal = redactSecrets('+const fixture = `-----BEGIN RSA PRIVATE KEY-----\n+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7\n+const next = "ordinary code";\n+let more = 1;\n');
  assert.equal(literal.text, "+[redacted]\n+[redacted]\n+const next = \"ordinary code\";\n+let more = 1;\n", "a BEGIN inside a literal is left behind at the first line that cannot be a key");
  const concatenated = redactSecrets('+const pem = "-----BEGIN RSA PRIVATE KEY-----\\n" +\n+  "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7\\n" +\n+  "shortTail9=\\n" +\n+  "-----END RSA PRIVATE KEY-----";\n+let after = 1;\n');
  assert.equal(concatenated.text, "+[redacted]\n+[redacted]\n+[redacted]\n+[redacted]\n+let after = 1;\n", "a key as concatenated literals is redacted whole");
  const tail = redactSecrets(" MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7ghijkl\n+shortTail9=\n+-----END RSA PRIVATE KEY-----\n+after the key\n");
  assert.equal(tail.text, " [redacted]\n+[redacted]\n+[redacted]\n+[redacted]\n+after the key\n", "a key's short last line and END marker right after its body are redacted even with the BEGIN outside the hunk");
  const opening = redactSecrets("+shortTail9=\n+-----END RSA PRIVATE KEY-----\n+tls.crt: next\n");
  assert.equal(opening.text, "+[redacted]\n+[redacted]\n+tls.crt: next\n", "a key's tail that opens the hunk is reached back over from its END marker");
  const cTail = redactSecrets('+  "shortTail9=\\n" \\\n+  "-----END RSA PRIVATE KEY-----\\n"\n+int y;\n');
  assert.equal(cTail.text, "+[redacted]\n+[redacted]\n+int y;\n", "a C-literal tail that opens the hunk is reached back over from its C-literal END");
  const words = redactSecrets("+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n+done\n+else\n");
  assert.equal(words.text, "+[redacted]\n+done\n+else\n", "ordinary words after a base64 line are not key tail");
  const unpadded = redactSecrets("+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n+AbCdEfGhIjKlMnOpQrStUvWx\n+-----END RSA PRIVATE KEY-----\n+after\n");
  assert.equal(unpadded.text, "+[redacted]\n+[redacted]\n+[redacted]\n+after\n", "an unpadded 20+ base64 line continues the tail");
  const cstring = redactSecrets('+#define KEY "-----BEGIN RSA PRIVATE KEY-----\\n" \\\n+  "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\\n" \\\n+  "-----END RSA PRIVATE KEY-----\\n"\n+int x;\n');
  assert.equal(cstring.text, "+[redacted]\n+[redacted]\n+[redacted]\n+int x;\n", "a key written as backslash-continued C literals is redacted whole");
  const header = redactSecrets('@@ -6,4 +6,4 @@ const DATABASE_PASSWORD: &str = "hunter2hunter2hunter2hunter2hunter2hunter2hun\n unrelated\n');
  assert.equal(header.text, "@@ -6,4 +6,4 @@ [redacted]\n unrelated\n", "a quoted secret git cut at 80 bytes in the funcname trailer is redacted");
  for (const quote of ["'", "`"]) {
    const cut = redactSecrets(`@@ -6,4 +6,4 @@ const DATABASE_PASSWORD = ${quote}hunter2hunter2hunter2hunter2hunter2hunter2hunter2hun\n unrelated\n`);
    assert.equal(cut.text, "@@ -6,4 +6,4 @@ [redacted]\n unrelated\n", `cut inside a ${quote} literal`);
  }
  const checksum = redactSecrets("+wXYZ\n+=AbCd\n+-----END PGP PRIVATE KEY BLOCK-----\n+after\n");
  assert.equal(checksum.text, "+[redacted]\n+[redacted]\n+[redacted]\n+after\n", "a PGP checksum line is reached over from the END marker, and the tail before it");
  const complete = redactSecrets("+-----BEGIN RSA PRIVATE KEY-----\n+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n+shortTail9=\n+-----END RSA PRIVATE KEY-----\n+version: 3\n+ABCDEFGHIJKLMNOPQRSTUVWXYZ\n+done=\n");
  assert.equal(complete.text, "+[redacted]\n+[redacted]\n+[redacted]\n+[redacted]\n+version: 3\n+ABCDEFGHIJKLMNOPQRSTUVWXYZ\n+done=\n", "after a whole block, armour-like and base64-like lines are ordinary text again");
  const forward = redactSecrets("+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n+shortTail9=\n+Proc-Type: 4,ENCRYPTED\n+ordinary\n");
  assert.equal(forward.text, "+[redacted]\n+[redacted]\n+[redacted]\n+ordinary\n", "a body line reaches forward over its tail and an armour header without any END");
  const closes = redactSecrets("+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n+-----END RSA PRIVATE KEY-----\n+shortTail9=\n");
  assert.equal(closes.text, "+[redacted]\n+[redacted]\n+shortTail9=\n", "the END marker closes the reach-forward");
  const call = redactSecrets('+  "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef")\n');
  assert.equal(call.text, "+[redacted]\n", "a body line that closes a call is a body line");
  const inCode = redactSecrets("+done\n+else\n+marker = \"-----END RSA PRIVATE KEY-----\"\n");
  assert.equal(inCode.text, "+done\n+else\n+marker = \"-----END RSA PRIVATE KEY-----\"\n", "an END marker inside code does not reach back over words");
  const inHeader = redactSecrets("+done\n@@ -1,2 +1,2 @@ -----END RSA PRIVATE KEY-----\n+x\n");
  assert.equal(inHeader.text, "+done\n@@ -1,2 +1,2 @@ -----END RSA PRIVATE KEY-----\n+x\n", "nor one in a hunk header, which is no secret itself");
  assert.equal(redactSecrets("diff --git a/secret_key=0123456789abcdef b/x\n--- a/password=hunter2hunter2hunter2\n+++ b/x\n").redacted, 0, "file headers are never secret lines");
  const boundary = redactSecrets("+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\ndiff --git a/x b/x\n+shortTail9=\n");
  assert.equal(boundary.text, "+[redacted]\ndiff --git a/x b/x\n+shortTail9=\n", "a diff boundary ends the reach-forward");
  const open = redactSecrets("+-----BEGIN RSA PRIVATE KEY-----\n+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n");
  assert.equal(open.text, "+[redacted]\n+[redacted]\n", "a block open at the end of the diff leaves no stray line");
  assert.equal(open.redacted, 2);
  assert.equal(redactSecrets("@@ -6,4 +6,4 @@ fn connect(url: &str) -> Result<Connection> {\n").text, "@@ -6,4 +6,4 @@ fn connect(url: &str) -> Result<Connection> {\n", "an ordinary trailer stays");
  const indented = redactSecrets("+  tls.key: |\n+    -----BEGIN RSA PRIVATE KEY-----\n+    MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7\n+    shortTail9=\n+    -----END RSA PRIVATE KEY-----\n+  other: value\n");
  assert.equal(indented.text, "+[redacted]\n+[redacted]\n+[redacted]\n+[redacted]\n+[redacted]\n+  other: value\n", "an indented block is redacted whole, its block-scalar key line included");
  const quoted = redactSecrets('+const pem = [\n+  "-----BEGIN RSA PRIVATE KEY-----",\n+  "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7",\n+  "-----END RSA PRIVATE KEY-----",\n+];\n');
  assert.equal(quoted.text, "+const pem = [\n+[redacted]\n+[redacted]\n+[redacted]\n+];\n", "a quoted string array is redacted whole");
  const armoured = redactSecrets("+-----BEGIN RSA PRIVATE KEY-----\n+Proc-Type: 4,ENCRYPTED\n+DEK-Info: AES-128-CBC,0123456789ABCDEF\n+\n+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7\n+shortTail9=\n+-----END RSA PRIVATE KEY-----\n");
  assert.equal(armoured.redacted, 6, "an encrypted key's headers and short tail are part of the block; its blank line is kept and not counted");
  assert.ok(!armoured.text.includes("DEK-Info") && !armoured.text.includes("shortTail9"));
  const pgp = redactSecrets("+-----BEGIN PGP PRIVATE KEY BLOCK-----\n+Version: GnuPG v2\n+Comment: exported for the relay\n+MessageID: abc123\n+Hash: SHA256\n+Charset: utf-8\n+\n+lQOYBF9pgpbodyline1\n+=abcd\n+-----END PGP PRIVATE KEY BLOCK-----\n+after\n");
  assert.equal(pgp.redacted, 9, "every armour header, the body, the checksum and the end line; the blank line is kept and not counted");
  assert.ok(!pgp.text.includes("pgpbodyline1") && pgp.text.endsWith("+after\n"));
  assert.equal(redactSecrets('+  "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef",\n').redacted, 1, "an indented, quoted body line with its BEGIN outside the hunk is still key material");
  const prose = redactSecrets("+-----BEGIN RSA PRIVATE KEY-----\n+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7\n+type: kubernetes.io/tls\n+apiVersion: v1\n+Note: the banner above is quoted from the docs\n");
  assert.equal(prose.text, "+[redacted]\n+[redacted]\n+type: kubernetes.io/tls\n+apiVersion: v1\n+Note: the banner above is quoted from the docs\n", "an unterminated block ends at the first line that is not key material, YAML keys and prose included");
});

test("a one-line edit inside an existing key, and a secret in a commit subject, never reach the summary; the new credential globs hold", () => {
  const { dir, git } = repo();
  // The key already exists on main, so the branch's edit shows only body lines in its hunk.
  const key = ["-----BEGIN RSA PRIVATE KEY-----", ...Array.from({ length: 12 }, (_, i) => `MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC${i}abcdefgh`), "-----END RSA PRIVATE KEY-----"];
  git("stash", "-q");
  git("checkout", "-q", "main");
  writeFileSync(join(dir, "src/notes.txt"), `${key.join("\n")}\n`);
  git("add", ".");
  git("commit", "-qm", "Add notes");
  git("checkout", "-q", "work");
  git("merge", "-q", "--no-edit", "main");
  git("stash", "pop", "-q");
  key[6] = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCEDITEDabcdefgh";
  writeFileSync(join(dir, "src/notes.txt"), `${key.join("\n")}\n`);
  mkdirSync(join(dir, "infra"), { recursive: true });
  mkdirSync(join(dir, "home"), { recursive: true });
  writeFileSync(join(dir, "infra/terraform.tfstate.backup"), "tfstatebody\n");
  writeFileSync(join(dir, "home/.pgpass"), "pgpassbody\n");
  writeFileSync(join(dir, "home/pgpass.conf"), "pgpassconfbody\n");
  git("add", ".");
  git("commit", "-qm", "rotate ghp_abcdefghijklmnopqrstuvwxyz0123456789 and edit the key");
  const change = inDir(dir, () => summarize("main"));
  assert.ok(!change.diff.includes("MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC"), "no key material in the diff");
  assert.equal(change.redacted, 7, "the hunk header, whose context was a body line, plus six key lines (two context, one removed, one added, two context)");
  assert.deepEqual(change.subjects.slice(0, 2), ["[redacted]", "Merge branch 'main' into work"], "the one subject holding a token is redacted, the next is not");
  assert.equal(change.redactedSubjects, 1);
  git("commit", "-q", "--allow-empty", "-m", "BumpVersion123");
  git("commit", "-q", "--allow-empty", "-m", "Strip the -----BEGIN RSA PRIVATE KEY----- banner from the fixture");
  git("commit", "-q", "--allow-empty", "-m", "An ordinary follow-up");
  const more = inDir(dir, () => summarize("main"));
  assert.deepEqual(more.subjects.slice(0, 3), ["An ordinary follow-up", "[redacted]", "BumpVersion123"], "a BEGIN in one subject never blanks the next, however key-like it looks");
  assert.equal(more.redactedSubjects, 2, "the token subject and the banner subject");
  git("commit", "-q", "--allow-empty", "-m", "- rotate ghp_abcdefghijklmnopqrstuvwxyz0123456789");
  git("commit", "-q", "--allow-empty", "-m", "- ordinary bullet subject");
  const dashed = inDir(dir, () => summarize("main"));
  assert.deepEqual(dashed.subjects.slice(0, 2), ["- ordinary bullet subject", "[redacted]"], "a subject is tested raw and replaced whole: a leading dash is no diff marker");
  git("commit", "-q", "--allow-empty", "-m", "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef0123456789");
  const bodyish = inDir(dir, () => summarize("main"));
  assert.equal(bodyish.subjects[0], "[redacted]", "a subject that reads as a key's body line is redacted");
  assert.ok(!change.diff.includes("tfstatebody") && !change.diff.includes("pgpassbody") && !change.diff.includes("pgpassconfbody"));
  assert.ok(change.files.some((f) => f.path === "infra/terraform.tfstate.backup") && change.files.some((f) => f.path === "home/.pgpass"));
});

test("the diff is redacted before it is cut, so a cut never leaves a secret's prefix", () => {
  const { dir, git } = repo();
  writeFileSync(join(dir, "src/config.rs"), 'const DB_PASSWORD: &str = "hunter2hunter2hunter2";\n');
  git("add", ".");
  const raw = inDir(dir, () => summarize("main", [], { summaryBytes: 1e9 }));
  assert.equal(raw.diff.indexOf("hunter2"), -1, "the password never reaches the diff");
  assert.ok(raw.diff.includes("[redacted]"), "the password line is in the diff, redacted");
  assert.equal(raw.redacted, 1);
  const cutAt = raw.diff.indexOf("[redacted]") + 4;
  const cut = inDir(dir, () => summarize("main", [], { diffChars: cutAt, summaryBytes: 1e9 }));
  assert.ok(!cut.diff.includes("hunter2hunte") && !cut.diff.includes("hunter2"), "cutting inside the redacted line leaves no prefix of the secret");
  assert.equal(cut.redacted, raw.redacted, "the count is of the scanned window, counted before the cut");
});

test("only whole lines a few times past the cut are scanned: a secret beyond the window is neither shown nor counted", () => {
  const { dir, git } = repo();
  writeFileSync(join(dir, "src/big.txt"), `${"filler line of ordinary text\n".repeat(400)}PASSWORD = "hunter2hunter2hunter2"\n`);
  git("add", ".");
  const narrow = inDir(dir, () => summarize("main", [], { diffChars: 1000, summaryBytes: 1e9 }));
  assert.equal(narrow.redacted, 0, "the password sits past four times the cut");
  assert.ok(!narrow.diff.includes("hunter2"));
  const wide = inDir(dir, () => summarize("main", [], { diffChars: 4000, summaryBytes: 1e9 }));
  assert.equal(wide.redacted, 1, "within the window it is scanned and counted");
  assert.ok(!wide.diff.includes("hunter2"));
});

test("the scan window ends on a whole line", () => {
  assert.equal(wholeLines("abc\ndef\nghi\n", 5), "abc\ndef\n", "the line crossing the edge is kept whole");
  assert.equal(wholeLines("abc\ndef\nghi\n", 100), "abc\ndef\nghi\n", "a short text is whole");
  assert.equal(wholeLines("abcdefgh", 3), "abcdefgh", "a last line without a newline is kept whole");
});

test("a YAML block scalar under a secret key is redacted with its indented lines", () => {
  const { text, redacted } = redactSecrets("+config:\n+  password: |\n+    hunter2hunter2\n+    second line\n+\n+  other: value\n+  tls.key: >-\n+    MIIEvQIB\n+next: 1\n");
  assert.equal(text, "+config:\n+[redacted]\n+[redacted]\n+[redacted]\n+\n+  other: value\n+[redacted]\n+[redacted]\n+next: 1\n");
  assert.equal(redacted, 5);
  assert.equal(redactSecrets("+  password: |\n+    hunter2hunter2\ndiff --git a/x b/x\n+    not the value\n").text, "+[redacted]\n+[redacted]\ndiff --git a/x b/x\n+    not the value\n", "a diff boundary ends the block");
  const blockMarker = redactSecrets(" password: |\n-    oldpassword1\n\\ No newline at end of file\n+    newpassword2\n\\ No newline at end of file\n");
  assert.equal(blockMarker.text, " [redacted]\n-[redacted]\n\\ No newline at end of file\n+[redacted]\n\\ No newline at end of file\n", "git's no-newline marker does not close a block scalar");
  assert.equal(redactSecrets("+<key>APIKey</key>\n+<!-- rotated -->\n+<string>0123456789abcdef</string>\n").text, "+[redacted]\n+<!-- rotated -->\n+[redacted]\n", "an XML comment between a plist key and its string keeps the pair open");
  assert.equal(redactSecrets("+- password: |\n+    hunter2hunter2\n+- other: 1\n").text, "+[redacted]\n+[redacted]\n+- other: 1\n", "a block scalar under a list item key is redacted past the dash");
  assert.equal(redactSecrets('+"password": |\n+  hunter2hunter2\n+next: 1\n').text, "+[redacted]\n+[redacted]\n+next: 1\n", "a quoted key opens a block too");
  const twoLine = redactSecrets("+<key>APIKey</key>\n+<string>0123456789abcdef</string>\n+<key>APIKeyPath</key>\n+<string>/etc/keys</string>\n+- name: DB_PASSWORD\n+  value: hunter2hunter2\n+- name: DB_HOST\n+  value: db\n");
  assert.equal(twoLine.text, "+[redacted]\n+[redacted]\n+<key>APIKeyPath</key>\n+<string>/etc/keys</string>\n+[redacted]\n+[redacted]\n+- name: DB_HOST\n+  value: db\n", "a secret named on one line is redacted with its value on the next; a path key and an ordinary env item are not");
  assert.equal(twoLine.redacted, 4);
  const rotated = redactSecrets(" - name: DB_PASSWORD\n-    value: oldpassword1\n+    value: newpassword2\n <key>APIKey</key>\n-<string>0123456789abcdef</string>\n+<string>fedcba9876543210</string>\n");
  assert.equal(rotated.text, " [redacted]\n-[redacted]\n+[redacted]\n [redacted]\n-[redacted]\n+[redacted]\n", "a rotated value under a context key line is redacted old and new");
  const marker = redactSecrets(" - name: DB_PASSWORD\n-    value: oldpassword1\n\\ No newline at end of file\n+    value: newpassword2\n\\ No newline at end of file\n");
  assert.equal(marker.text, " [redacted]\n-[redacted]\n\\ No newline at end of file\n+[redacted]\n\\ No newline at end of file\n", "git's no-newline marker between the old and new value keeps the pair open");
  const commented = redactSecrets("+- name: DB_PASSWORD\n+  # rotated today\n+\n+  value: newpassword2\n+- name: DB_HOST\n+  value: db\n");
  assert.equal(commented.text, "+[redacted]\n+  # rotated today\n+\n+[redacted]\n+- name: DB_HOST\n+  value: db\n", "a comment or blank line between the key and its value keeps the pair open; another key closes it");
  assert.equal(redactSecrets("@@ -1,2 +1,2 @@ <key>APIKey</key>\n+<string>0123456789abcdef</string>\n").text, "@@ -1,2 +1,2 @@ <key>APIKey</key>\n+<string>0123456789abcdef</string>\n", "a key-name trailer is no secret itself, keeps the hunk header's structure and opens no pair");
  assert.equal(redactSecrets("+- name: DB_PASSWORD_FILE\n+  value: /run/secrets/db\n").text, "+- name: DB_PASSWORD_FILE\n+  value: /run/secrets/db\n", "an env item naming a secret's file is not a secret");
  const longBlockKey = redactSecrets(`+${"x".repeat(100)}_password: | # ${"comment ".repeat(60)}\n+  hunter2hunter2\n`);
  assert.equal(longBlockKey.text, "+[redacted]\n+[redacted]\n", "a long block-scalar key line (a 100-char key and a long comment, past 400 chars) still opens its block");
  const blanks = redactSecrets("+MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7abcdef\n+\n+-----END RSA PRIVATE KEY-----\n");
  assert.equal(blanks.text, "+[redacted]\n+\n+[redacted]\n", "a blank line inside a block stays blank");
  assert.equal(blanks.redacted, 2, "and is not counted");
  assert.equal(redactSecrets("@@ -3,3 +3,3 @@   password: |\n     hunter2hunter2\n+    hunter2hunter3\n   other: 1\n").text, "@@ -3,3 +3,3 @@ [redacted]\n [redacted]\n+[redacted]\n   other: 1\n", "a block-scalar key reaching the hunk only as its header's trailer still covers the value lines");
});
