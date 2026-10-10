//! Snapshots of what the pattern table makes of a command's output:
//! `try_specific_pattern` (the per-command compressor) and `compress_output`
//! (that compressor, then the json, log and test fallbacks).
//!
//! Every command here is one shell compression hands to the pattern table
//! (`reaches_pattern_table`), so a snapshot never pins a compressor
//! production does not run for its command.
//!
//! Run `cargo insta review` after intentional pattern changes to accept new snapshots.

use sobre::core::patterns::{compress_output, try_specific_pattern};
use sobre::shell::compress::reaches_pattern_table;

fn assert_reaches_pattern_table(name: &str, cmd: &str) {
    assert!(
        reaches_pattern_table(cmd),
        "[{name}] shell compression never hands {cmd:?} to the pattern table"
    );
}

fn assert_pattern(name: &str, cmd: &str, output: &str) {
    assert_reaches_pattern_table(name, cmd);
    let result = try_specific_pattern(cmd, output);
    let compressed = result
        .unwrap_or_else(|| panic!("[{name}] try_specific_pattern({cmd:?}, ...) returned None"));
    insta::assert_snapshot!(name, compressed);
}

fn assert_pipeline(name: &str, cmd: &str, output: &str) {
    assert_reaches_pattern_table(name, cmd);
    let result = compress_output(cmd, output);
    let compressed =
        result.unwrap_or_else(|| panic!("[{name}] compress_output({cmd:?}, ...) returned None"));
    insta::assert_snapshot!(name, compressed);
}

// ═══════════════════════════════════════════════════════════════════════
// git patterns
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_git_status() {
    assert_pattern(
        "git_status",
        "git status",
        "On branch main\n\
         Your branch is up to date with 'origin/main'.\n\n\
         Changes not staged for commit:\n\
         (use \"git add <file>...\" to update what will be committed)\n\
         (use \"git restore <file>...\" to discard changes in working directory)\n\
         \tmodified:   src/main.rs\n\
         \tmodified:   src/lib.rs\n\n\
         Untracked files:\n\
         (use \"git add <file>...\" to include in what will be committed)\n\
         \tnew_file.rs\n\n\
         no changes added to commit (use \"git add\" and/or \"git commit -a\")\n",
    );
}

#[test]
fn pattern_git_diff() {
    assert_pattern(
        "git_diff",
        "git diff",
        "diff --git a/src/main.rs b/src/main.rs\n\
         index abc1234..def5678 100644\n\
         --- a/src/main.rs\n\
         +++ b/src/main.rs\n\
         @@ -1,5 +1,7 @@\n\
         +use std::io;\n\
          fn main() {\n\
         -    println!(\"hello\");\n\
         +    println!(\"hello world\");\n\
         +    io::stdout().flush().unwrap();\n\
          }\n\
         diff --git a/src/lib.rs b/src/lib.rs\n\
         index 1111111..2222222 100644\n\
         --- a/src/lib.rs\n\
         +++ b/src/lib.rs\n\
         @@ -10,3 +10,5 @@\n\
          pub fn greet() -> &'static str {\n\
         -    \"hi\"\n\
         +    \"hello\"\n\
          }\n\
         +\n\
         +pub fn farewell() -> &'static str { \"bye\" }\n",
    );
}

#[test]
fn pattern_git_log() {
    assert_pattern(
        "git_log",
        "git log",
        "commit abc1234567890abcdef1234567890abcdef123456\n\
         Author: Alice <alice@example.com>\n\
         Date:   Mon Jan 15 10:23:45 2024 +0100\n\n\
             feat: add user authentication\n\n\
         commit def5678901234567890abcdef1234567890abcdef\n\
         Author: Bob <bob@example.com>\n\
         Date:   Sun Jan 14 08:00:00 2024 +0100\n\n\
             fix: handle null pointer in config parser\n\n\
         commit 111aaaa234567890abcdef1234567890abcdef1234\n\
         Author: Charlie <charlie@example.com>\n\
         Date:   Sat Jan 13 12:30:00 2024 +0100\n\n\
             refactor: extract config module from main\n\n\
         commit 222bbbb234567890abcdef1234567890abcdef1234\n\
         Author: Alice <alice@example.com>\n\
         Date:   Fri Jan 12 09:15:00 2024 +0100\n\n\
             docs: update README with API examples\n\n\
         commit 333cccc234567890abcdef1234567890abcdef1234\n\
         Author: Bob <bob@example.com>\n\
         Date:   Thu Jan 11 14:00:00 2024 +0100\n\n\
             test: add integration tests for auth flow\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// gh / glab
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_gh_pr_list() {
    assert_pattern(
        "gh_pr_list",
        "gh pr list",
        "Showing 5 of 12 open pull requests in org/repo\n\n\
         #42  feat: add oauth    feature/oauth   OPEN\n\
         #41  fix: login crash   fix/login       OPEN\n\
         #40  docs: api guide    docs/api        OPEN\n\
         #39  refactor: db       refactor/db     OPEN\n\
         #38  chore: deps        chore/deps      OPEN\n",
    );
}

#[test]
fn pattern_glab_mr_list() {
    assert_pattern(
        "glab_mr_list",
        "glab mr list",
        "Showing 4 open merge requests on mygroup/myproject (Page 1)\n\n\
         !142 feat: add oauth support          feature/oauth    alice (alice@dev)  2024-01-15T10:23:45Z\n\
         !141 fix: login crash on empty token   fix/login        bob (bob@dev)      2024-01-14T08:00:00Z\n\
         !140 docs: add API reference guide     docs/api-ref     charlie (cdev)     2024-01-13T12:30:00Z\n\
         !139 refactor: database layer          refactor/db      diana (ddev)       2024-01-12T09:15:00Z\n\
         !138 chore: bump dependencies          chore/deps       eve (edev)         2024-01-11T14:00:00Z\n\
         !137 test: add E2E smoke tests         test/e2e         frank (fdev)       2024-01-10T16:45:00Z\n\
         !136 ci: add ARM64 runner              ci/arm-runner    grace (gdev)       2024-01-09T11:30:00Z\n\
         !135 perf: optimize query planner      perf/queries     hank (hdev)        2024-01-08T13:20:00Z\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// cargo
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_cargo_build() {
    assert_pattern(
        "cargo_build",
        "cargo build",
        "   Compiling proc-macro2 v1.0.86\n\
            Compiling unicode-ident v1.0.12\n\
            Compiling quote v1.0.36\n\
            Compiling syn v2.0.68\n\
            Compiling serde_derive v1.0.203\n\
            Compiling serde v1.0.203\n\
            Compiling serde_json v1.0.117\n\
            Compiling tokio-macros v2.3.0\n\
            Compiling tokio v1.38.0\n\
            Compiling axum v0.7.5\n\
            Compiling myapp v0.1.0 (/home/user/myapp)\n\
             Finished `dev` profile [unoptimized + debuginfo] target(s) in 45.12s\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// docker
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_docker_build() {
    assert_pattern(
        "docker_build",
        "docker build -t myapp .",
        "#1 [internal] load build definition from Dockerfile\n\
         #1 transferring dockerfile: 520B done\n\
         #1 DONE 0.0s\n\n\
         #2 [internal] load metadata for docker.io/library/rust:1.79\n\
         #2 DONE 1.2s\n\n\
         #3 [internal] load .dockerignore\n\
         #3 transferring context: 2B done\n\
         #3 DONE 0.0s\n\n\
         #4 [1/5] FROM docker.io/library/rust:1.79@sha256:abc123\n\
         #4 CACHED\n\n\
         #5 [2/5] WORKDIR /app\n\
         #5 CACHED\n\n\
         #6 [3/5] COPY Cargo.toml Cargo.lock ./\n\
         #6 DONE 0.1s\n\n\
         #7 [4/5] RUN cargo build --release\n\
         #7 3.456    Compiling myapp v0.1.0 (/app)\n\
         #7 15.23     Finished release [optimized] target(s) in 14.99s\n\
         #7 DONE 15.3s\n\n\
         #8 [5/5] COPY . .\n\
         #8 DONE 0.2s\n\n\
         #9 exporting to image\n\
         #9 exporting layers 0.3s done\n\
         #9 writing image sha256:def456 done\n\
         #9 naming to docker.io/library/myapp done\n\
         #9 DONE 0.4s\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// npm / pnpm
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_npm_install() {
    assert_pattern(
        "npm_install",
        "npm install",
        "\nadded 847 packages, and audited 848 packages in 12s\n\n\
         142 packages are looking for funding\n\
           run `npm fund` for details\n\n\
         3 vulnerabilities (1 moderate, 2 high)\n\n\
         To address all issues, run:\n\
           npm audit fix\n\n\
         Run `npm audit` for details.\n",
    );
}

#[test]
fn pattern_pnpm_install() {
    assert_pattern(
        "pnpm_install",
        "pnpm install",
        "Lockfile is up to date, resolution step is skipped\n\
         Already up to date\n\
         Done in 1.2s\n\
         Packages: +0\n\
         Progress: resolved 847, reused 847, downloaded 0, added 0, done\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// pip / poetry / mypy / ruff
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_pip_install() {
    assert_pattern(
        "pip_install",
        "pip install -r requirements.txt",
        "Collecting flask==3.0.0\n\
           Downloading flask-3.0.0-py3-none-any.whl (101 kB)\n\
         Collecting werkzeug>=3.0.0\n\
           Using cached werkzeug-3.0.1-py3-none-any.whl (226 kB)\n\
         Collecting jinja2>=3.1.2\n\
           Using cached Jinja2-3.1.3-py3-none-any.whl (133 kB)\n\
         Collecting click>=8.1.3\n\
           Using cached click-8.1.7-py3-none-any.whl (97 kB)\n\
         Collecting itsdangerous>=2.1.2\n\
           Using cached itsdangerous-2.1.2-py3-none-any.whl (15 kB)\n\
         Collecting markupsafe>=2.0\n\
           Using cached MarkupSafe-2.1.5-cp312-cp312-manylinux_2_17_x86_64.whl (23 kB)\n\
         Collecting blinker>=1.6.2\n\
           Using cached blinker-1.7.0-py3-none-any.whl (13 kB)\n\
         Installing collected packages: markupsafe, itsdangerous, click, blinker, werkzeug, jinja2, flask\n\
         Successfully installed blinker-1.7.0 click-8.1.7 flask-3.0.0 itsdangerous-2.1.2 jinja2-3.1.3 markupsafe-2.1.5 werkzeug-3.0.1\n",
    );
}

#[test]
fn pattern_poetry_install() {
    assert_pattern(
        "poetry_install",
        "poetry install",
        "Installing dependencies from lock file\n\n\
         Package operations: 15 installs, 2 updates, 0 removals\n\n\
           - Installing markupsafe (2.1.5)\n\
           - Installing jinja2 (3.1.3)\n\
           - Installing click (8.1.7)\n\
           - Installing werkzeug (3.0.1)\n\
           - Installing flask (3.0.0)\n\
           - Installing requests (2.31.0)\n\
           - Updating certifi (2023.11.17 -> 2024.2.2)\n\
           - Updating urllib3 (2.1.0 -> 2.2.0)\n\
           - Installing pytest (8.0.0)\n\
           - Installing coverage (7.4.0)\n\n\
         Installing the current project: myapp (0.1.0)\n",
    );
}

#[test]
fn pattern_mypy() {
    assert_pattern(
        "mypy_check",
        "mypy src/",
        "src/auth.py:12: error: Argument 1 to \"verify\" has incompatible type \"Optional[str]\"; expected \"str\"  [arg-type]\n\
         src/auth.py:25: error: Missing return statement  [return]\n\
         src/auth.py:40: error: Incompatible return value type (got \"None\", expected \"User\")  [return-value]\n\
         src/db.py:8: error: Incompatible types in assignment (expression has type \"None\", variable has type \"Connection\")  [assignment]\n\
         src/db.py:15: error: Name \"cursor\" is not defined  [name-defined]\n\
         src/db.py:33: error: \"Connection\" has no attribute \"execute_many\"  [attr-defined]\n\
         src/api.py:30: error: \"Dict[str, Any]\" has no attribute \"items\"  [attr-defined]\n\
         src/api.py:45: error: Argument 1 to \"loads\" has incompatible type \"bytes\"; expected \"str\"  [arg-type]\n\
         src/utils.py:5: error: Missing type parameters for generic type \"Dict\"  [type-arg]\n\
         src/utils.py:12: error: Need type annotation for \"cache\" (hint: \"cache: Dict[str, Any] = ...\")  [var-annotated]\n\
         Found 10 errors in 4 files (checked 12 source files)\n",
    );
}

#[test]
fn pattern_ruff() {
    assert_pattern(
        "ruff_check",
        "ruff check src/",
        "src/auth.py:3:1: F401 [*] `os` imported but unused\n\
         src/auth.py:15:5: E712 Comparison to `True` should be `if cond:` or `if cond is True:`\n\
         src/auth.py:22:1: F841 Local variable `result` is assigned to but never used\n\
         src/db.py:8:1: E501 Line too long (120 > 88)\n\
         src/db.py:22:5: W291 Trailing whitespace\n\
         src/db.py:35:1: E302 Expected 2 blank lines after class or function definition, found 1\n\
         src/api.py:1:1: I001 [*] Import block is un-sorted or un-formatted\n\
         src/api.py:15:5: B006 Do not use mutable data structures for argument defaults\n\
         src/api.py:28:1: E303 Too many blank lines (3)\n\
         src/utils.py:10:5: B006 Do not use mutable data structures for argument defaults\n\
         src/utils.py:18:9: SIM108 [*] Use ternary operator\n\
         src/config.py:5:1: F811 Redefinition of unused `load` from line 3\n\
         Found 12 errors.\n\
         [*] 3 fixable with the `--fix` option.\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// terraform / kubectl
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_terraform_plan() {
    assert_pattern(
        "terraform_plan",
        "terraform plan",
        "Terraform used the selected providers to generate the following execution plan.\n\
         Resource actions are indicated with the following symbols:\n\
           + create\n\
           ~ update in-place\n\n\
         Terraform will perform the following actions:\n\n\
           # aws_instance.web will be created\n\
           + resource \"aws_instance\" \"web\" {\n\
               + ami           = \"ami-0c55b159cbfafe1f0\"\n\
               + instance_type = \"t2.micro\"\n\
               + tags          = {\n\
                   + \"Name\" = \"web-server\"\n\
                 }\n\
             }\n\n\
           # aws_s3_bucket.data will be updated in-place\n\
           ~ resource \"aws_s3_bucket\" \"data\" {\n\
               ~ tags = {\n\
                   + \"Environment\" = \"production\"\n\
                 }\n\
             }\n\n\
         Plan: 1 to add, 1 to change, 0 to destroy.\n",
    );
}

#[test]
fn pattern_kubectl_get_pods() {
    assert_pattern(
        "kubectl_get_pods",
        "kubectl get pods -n default",
        "NAME                          READY   STATUS    RESTARTS   AGE\n\
         web-app-6b8c4d5f9-abc12      1/1     Running   0          3d\n\
         web-app-6b8c4d5f9-def34      1/1     Running   0          3d\n\
         web-app-6b8c4d5f9-ghi56      1/1     Running   1          3d\n\
         redis-master-0               1/1     Running   0          7d\n\
         redis-slave-6fc55b5b-jkl78   1/1     Running   0          7d\n\
         redis-slave-6fc55b5b-mno90   1/1     Running   0          7d\n\
         postgres-0                   1/1     Running   0          14d\n\
         celery-worker-pqr12          1/1     Running   2          3d\n\
         celery-beat-stu34            1/1     Running   0          3d\n\
         nginx-ingress-vwx56          1/1     Running   0          30d\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// prettier
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_prettier() {
    assert_pattern(
        "prettier",
        "prettier --check src/",
        "Checking formatting...\n\
         [warn] src/App.tsx\n\
         [warn] src/utils.ts\n\
         [warn] src/api.ts\n\
         [warn] src/config.ts\n\
         [warn] src/types.ts\n\
         [warn] Code style issues found in 5 files. Run Prettier to fix.\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// maven / make / just
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_maven_build() {
    assert_pattern(
        "maven_build",
        "mvn clean install",
        "[INFO] Scanning for projects...\n\
         [INFO] \n\
         [INFO] ----------------------< com.example:myapp >-----------------------\n\
         [INFO] Building myapp 1.0-SNAPSHOT\n\
         [INFO] --------------------------------[ jar ]---------------------------------\n\
         [INFO] \n\
         [INFO] --- maven-clean-plugin:3.2.0:clean (default-clean) @ myapp ---\n\
         [INFO] Deleting /home/user/myapp/target\n\
         [INFO] \n\
         [INFO] --- maven-resources-plugin:3.3.1:resources (default-resources) @ myapp ---\n\
         [INFO] Copying 3 resources\n\
         [INFO] \n\
         [INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---\n\
         [INFO] Changes detected - recompiling the module!\n\
         [INFO] Compiling 42 source files to /home/user/myapp/target/classes\n\
         [INFO] \n\
         [INFO] --- maven-surefire-plugin:3.2.2:test (default-test) @ myapp ---\n\
         [INFO] Tests run: 28, Failures: 0, Errors: 0, Skipped: 2\n\
         [INFO] \n\
         [INFO] --- maven-jar-plugin:3.3.0:jar (default-jar) @ myapp ---\n\
         [INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar\n\
         [INFO] \n\
         [INFO] --- maven-install-plugin:3.1.1:install (default-install) @ myapp ---\n\
         [INFO] Installing /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar\n\
         [INFO] ------------------------------------------------------------------------\n\
         [INFO] BUILD SUCCESS\n\
         [INFO] ------------------------------------------------------------------------\n\
         [INFO] Total time:  12.345 s\n\
         [INFO] Finished at: 2024-06-15T10:30:00+02:00\n\
         [INFO] ------------------------------------------------------------------------\n",
    );
}

#[test]
fn pattern_make() {
    assert_pattern(
        "make_build",
        "make -j4",
        "cc -Wall -O2 -c src/main.c -o build/main.o\n\
         cc -Wall -O2 -c src/parser.c -o build/parser.o\n\
         cc -Wall -O2 -c src/lexer.c -o build/lexer.o\n\
         cc -Wall -O2 -c src/codegen.c -o build/codegen.o\n\
         cc -Wall -O2 -c src/optimizer.c -o build/optimizer.o\n\
         cc -Wall -O2 -c src/utils.c -o build/utils.o\n\
         cc -Wall -O2 -c src/debug.c -o build/debug.o\n\
         cc -Wall -O2 -c src/error.c -o build/error.o\n\
         cc build/main.o build/parser.o build/lexer.o build/codegen.o build/optimizer.o build/utils.o build/debug.o build/error.o -o bin/compiler\n",
    );
}

#[test]
fn pattern_just() {
    assert_pattern(
        "just_list",
        "just --list",
        "Available recipes:\n\
             build         # Build the project\n\
             clean         # Remove build artifacts\n\
             dev           # Start development server\n\
             fmt           # Format source code\n\
             lint          # Run linters\n\
             release       # Create a release build\n\
             test          # Run all tests\n\
             test-e2e      # Run end-to-end tests\n\
             test-unit     # Run unit tests\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// playwright / next/vite build
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_playwright() {
    assert_pattern(
        "playwright",
        "playwright test",
        "\nRunning 12 tests using 4 workers\n\n\
           \u{2713}  1 [chromium] > tests/login.spec.ts:5:3 > should login (2.3s)\n\
           \u{2713}  2 [chromium] > tests/login.spec.ts:15:3 > should show error (1.2s)\n\
           \u{2713}  3 [firefox] > tests/login.spec.ts:5:3 > should login (3.1s)\n\
           \u{2713}  4 [firefox] > tests/login.spec.ts:15:3 > should show error (1.8s)\n\
           \u{2713}  5 [webkit] > tests/login.spec.ts:5:3 > should login (2.8s)\n\
           \u{2713}  6 [webkit] > tests/login.spec.ts:15:3 > should show error (1.5s)\n\
           \u{2713}  7 [chromium] > tests/dashboard.spec.ts:5:3 > should load (1.1s)\n\
           \u{2713}  8 [chromium] > tests/dashboard.spec.ts:12:3 > should filter (0.9s)\n\
           \u{2713}  9 [firefox] > tests/dashboard.spec.ts:5:3 > should load (1.4s)\n\
           \u{2713} 10 [firefox] > tests/dashboard.spec.ts:12:3 > should filter (1.2s)\n\
           \u{2713} 11 [webkit] > tests/dashboard.spec.ts:5:3 > should load (1.3s)\n\
           \u{2713} 12 [webkit] > tests/dashboard.spec.ts:12:3 > should filter (1.0s)\n\n\
           12 passed (15.6s)\n",
    );
}

#[test]
fn pattern_next_build() {
    assert_pattern(
        "next_build",
        "next build",
        "   Creating an optimized production build...\n\
            Compiled successfully.\n\n\
         Route (app)                              Size     First Load JS\n\
         \u{250c} \u{25cb} /                                    5.23 kB        89.2 kB\n\
         \u{251c} \u{25cb} /about                               2.14 kB        86.1 kB\n\
         \u{251c} \u{25cb} /api/health                          0 B            83.9 kB\n\
         \u{251c} \u{25cf} /dashboard                           12.4 kB        96.4 kB\n\
         \u{2514} \u{25cb} /login                               3.56 kB        87.5 kB\n\
         + First Load JS shared by all            83.9 kB\n\
           \u{251c} chunks/main-abc123.js                52.3 kB\n\
           \u{251c} chunks/pages/_app-def456.js          28.1 kB\n\
           \u{2514} other shared chunks (total)           3.50 kB\n\n\
         \u{25cb}  (Static)   prerendered as static content\n\
         \u{25cf}  (Dynamic)  server-rendered on demand\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// ruby / swift / zig / cmake / ninja / bazel
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_ruby_rubocop() {
    assert_pattern(
        "ruby_rubocop",
        "rubocop app/",
        "Inspecting 25 files\n\
         ..C..W..C.C..W....C..W..C\n\n\
         Offenses:\n\n\
         app/models/user.rb:3:5: C: Style/StringLiterals: Prefer single-quoted strings.\n\
         app/models/user.rb:15:3: W: Lint/UselessAssignment: Useless assignment to variable.\n\
         app/controllers/users_controller.rb:8:1: C: Metrics/MethodLength: Method has too many lines.\n\
         app/controllers/users_controller.rb:22:5: C: Style/GuardClause: Use a guard clause.\n\
         app/controllers/posts_controller.rb:12:3: W: Lint/UnusedMethodArgument: Unused method argument.\n\
         app/services/auth_service.rb:5:1: C: Layout/LineLength: Line is too long. [125/120]\n\
         app/services/auth_service.rb:18:3: C: Style/RedundantReturn: Redundant return.\n\
         app/helpers/application_helper.rb:3:5: W: Lint/SuppressedException: Do not suppress exceptions.\n\
         app/mailers/user_mailer.rb:10:1: C: Style/FrozenStringLiteralComment: Missing frozen string literal comment.\n\n\
         25 files inspected, 9 offenses detected\n",
    );
}

#[test]
fn pattern_swift_build() {
    assert_pattern(
        "swift_build",
        "swift build",
        "Building for debugging...\n\
         [1/8] Compiling MyApp main.swift\n\
         [2/8] Compiling MyApp Config.swift\n\
         [3/8] Compiling MyApp Router.swift\n\
         [4/8] Compiling MyApp Models.swift\n\
         [5/8] Compiling MyApp Database.swift\n\
         [6/8] Compiling MyApp Auth.swift\n\
         [7/8] Compiling MyApp Utils.swift\n\
         [8/8] Linking MyApp\n\
         Build complete! (5.23s)\n",
    );
}

#[test]
fn pattern_zig_build() {
    assert_pattern(
        "zig_build",
        "zig build",
        "info: Build succeeded.\n\
         info: Semantic Analysis: 0.234s\n\
         info: Code Generation: 1.456s\n\
         info: Linking: 0.789s\n\
         info: Total: 2.479s\n\
         info: Memory: 156 MiB\n",
    );
}

#[test]
fn pattern_cmake() {
    assert_pattern(
        "cmake_build",
        "cmake --build build",
        "[  8%] Building CXX object src/CMakeFiles/mylib.dir/parser.cpp.o\n\
         [ 16%] Building CXX object src/CMakeFiles/mylib.dir/lexer.cpp.o\n\
         [ 25%] Building CXX object src/CMakeFiles/mylib.dir/codegen.cpp.o\n\
         [ 33%] Building CXX object src/CMakeFiles/mylib.dir/optimizer.cpp.o\n\
         [ 41%] Linking CXX static library libmylib.a\n\
         [ 41%] Built target mylib\n\
         [ 50%] Building CXX object app/CMakeFiles/myapp.dir/main.cpp.o\n\
         [ 58%] Linking CXX executable myapp\n\
         [ 58%] Built target myapp\n\
         [ 66%] Building CXX object tests/CMakeFiles/tests.dir/test_parser.cpp.o\n\
         [ 75%] Building CXX object tests/CMakeFiles/tests.dir/test_lexer.cpp.o\n\
         [ 83%] Linking CXX executable tests\n\
         [ 83%] Built target tests\n\
         [100%] Built target all\n",
    );
}

#[test]
fn pattern_ninja() {
    assert_pattern(
        "ninja_build",
        "ninja -C build",
        "[1/10] CXX obj/src/main.o\n\
         [2/10] CXX obj/src/parser.o\n\
         [3/10] CXX obj/src/lexer.o\n\
         [4/10] CXX obj/src/codegen.o\n\
         [5/10] CXX obj/src/optimizer.o\n\
         [6/10] CXX obj/src/utils.o\n\
         [7/10] LINK mylib.a\n\
         [8/10] CXX obj/app/main.o\n\
         [9/10] LINK myapp\n\
         [10/10] STAMP build.stamp\n",
    );
}

#[test]
fn pattern_bazel_build() {
    assert_pattern(
        "bazel_build",
        "bazel build //...",
        "INFO: Analyzed 15 targets (3 packages loaded, 42 targets configured).\n\
         INFO: Found 15 targets...\n\
         [0 / 5] Compiling src/main.cc\n\
         [1 / 5] Compiling src/parser.cc\n\
         [2 / 5] Compiling src/lexer.cc\n\
         [3 / 5] Linking //src:myapp\n\
         [4 / 5] Compiling tests/test_parser.cc\n\
         [5 / 5] Linking //tests:all_tests\n\
         INFO: Elapsed time: 12.345s, Critical Path: 8.234s\n\
         INFO: 5 processes: 2 internal, 3 linux-sandbox.\n\
         INFO: Build completed successfully, 5 total actions\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// dotnet / flutter / ansible
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_dotnet_build() {
    assert_pattern(
        "dotnet_build",
        "dotnet build",
        "MSBuild version 17.8.3+195e7f5a3 for .NET\n\
           Determining projects to restore...\n\
           All projects are up-to-date for restore.\n\
           MyApp -> /home/user/MyApp/bin/Debug/net8.0/MyApp.dll\n\n\
         Build succeeded.\n\
             0 Warning(s)\n\
             0 Error(s)\n\n\
         Time Elapsed 00:00:03.45\n",
    );
}

#[test]
fn pattern_flutter_analyze() {
    assert_pattern(
        "flutter_analyze",
        "flutter analyze",
        "Analyzing myapp...\n\n\
           info - Unused import - lib/old_widget.dart:1:8 - unused_import\n\
           info - Unused import - lib/utils.dart:3:8 - unused_import\n\
           warning - The parameter 'context' is not used - lib/home.dart:15:30 - unused_element\n\
           error - The method 'build' isn't defined for the type 'Widget' - lib/broken.dart:22:5 - undefined_method\n\n\
         4 issues found. (1 error, 1 warning, and 2 infos)\n",
    );
}

#[test]
fn pattern_ansible_playbook() {
    assert_pattern(
        "ansible_playbook",
        "ansible-playbook deploy.yml",
        "\nPLAY [Deploy application] ****************************************************\n\n\
         TASK [Gathering Facts] ********************************************************\n\
         ok: [web1.example.com]\n\
         ok: [web2.example.com]\n\
         ok: [web3.example.com]\n\n\
         TASK [Install dependencies] ***************************************************\n\
         changed: [web1.example.com]\n\
         changed: [web2.example.com]\n\
         changed: [web3.example.com]\n\n\
         TASK [Deploy code] ************************************************************\n\
         changed: [web1.example.com]\n\
         changed: [web2.example.com]\n\
         changed: [web3.example.com]\n\n\
         TASK [Restart service] ********************************************************\n\
         changed: [web1.example.com]\n\
         changed: [web2.example.com]\n\
         changed: [web3.example.com]\n\n\
         PLAY RECAP *********************************************************************\n\
         web1.example.com           : ok=4    changed=3    unreachable=0    failed=0    skipped=0\n\
         web2.example.com           : ok=4    changed=3    unreachable=0    failed=0    skipped=0\n\
         web3.example.com           : ok=4    changed=3    unreachable=0    failed=0    skipped=0\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// prisma
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_prisma_migrate() {
    assert_pattern(
        "prisma_migrate",
        "prisma migrate dev",
        "Environment variables loaded from .env\n\
         Prisma schema loaded from prisma/schema.prisma\n\
         Datasource \"db\": PostgreSQL database \"mydb\", schema \"public\" at \"localhost:5432\"\n\n\
         Applying migration `20240115_add_users`\n\
         Applying migration `20240116_add_posts`\n\
         Applying migration `20240117_add_comments`\n\n\
         The following migration(s) have been applied:\n\n\
         migrations/\n\
           \u{2514}\u{2500} 20240115_add_users/\n\
             \u{2514}\u{2500} migration.sql\n\
           \u{2514}\u{2500} 20240116_add_posts/\n\
             \u{2514}\u{2500} migration.sql\n\
           \u{2514}\u{2500} 20240117_add_comments/\n\
             \u{2514}\u{2500} migration.sql\n\n\
         Your database is now in sync with your schema.\n\n\
         \u{2714} Generated Prisma Client (v5.8.0) to ./node_modules/@prisma/client in 234ms\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// composer / artisan
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_composer_install() {
    assert_pattern(
        "composer_install",
        "composer install",
        "Installing dependencies from lock file (including require-dev)\n\
         Verifying lock file contents can be installed on current platform.\n\
         Package operations: 42 installs, 0 updates, 0 removals\n\
           - Downloading laravel/framework (v11.0.0)\n\
           - Downloading nesbot/carbon (3.0.0)\n\
           - Downloading doctrine/dbal (4.0.0)\n\
           - Installing laravel/framework (v11.0.0): Extracting archive\n\
           - Installing nesbot/carbon (3.0.0): Extracting archive\n\
           - Installing doctrine/dbal (4.0.0): Extracting archive\n\
         Generating optimized autoload files\n\
         > Illuminate\\Foundation\\ComposerScripts::postAutoloadDump\n\
         > @php artisan package:discover --ansi\n\n\
            INFO  Discovering packages.\n\n\
           laravel/sail ............................................................. DONE\n\
           laravel/sanctum .......................................................... DONE\n\
           laravel/tinker ........................................................... DONE\n\n\
         80 packages you are using are looking for funding.\n\
         Use the `composer fund` command to find out more!\n",
    );
}

#[test]
fn pattern_artisan() {
    assert_pattern(
        "artisan_route_list",
        "php artisan route:list",
        "+--------+-----------+-------------------+------------------+-------------------------------------------------+\n\
         | Domain | Method    | URI               | Name             | Action                                          |\n\
         +--------+-----------+-------------------+------------------+-------------------------------------------------+\n\
         |        | GET|HEAD  | /                 | home             | App\\Http\\Controllers\\HomeController@index        |\n\
         |        | GET|HEAD  | api/users         | users.index      | App\\Http\\Controllers\\Api\\UserController@index    |\n\
         |        | POST      | api/users         | users.store      | App\\Http\\Controllers\\Api\\UserController@store    |\n\
         |        | GET|HEAD  | api/users/{user}  | users.show       | App\\Http\\Controllers\\Api\\UserController@show     |\n\
         |        | PUT|PATCH | api/users/{user}  | users.update     | App\\Http\\Controllers\\Api\\UserController@update   |\n\
         |        | DELETE    | api/users/{user}  | users.destroy    | App\\Http\\Controllers\\Api\\UserController@destroy  |\n\
         |        | GET|HEAD  | api/posts         | posts.index      | App\\Http\\Controllers\\Api\\PostController@index    |\n\
         |        | POST      | api/posts         | posts.store      | App\\Http\\Controllers\\Api\\PostController@store    |\n\
         +--------+-----------+-------------------+------------------+-------------------------------------------------+\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// grep
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_grep() {
    let mut lines = String::new();
    let files = &[
        "src/main.rs",
        "src/lib.rs",
        "src/utils.rs",
        "src/config.rs",
        "src/api.rs",
        "src/db.rs",
        "src/cache.rs",
        "src/auth.rs",
        "src/middleware.rs",
        "src/router.rs",
    ];
    for (fi, file) in files.iter().enumerate() {
        for j in 0..4 {
            let line_num = fi * 20 + j * 5 + 3;
            lines.push_str(&format!(
                "{file}:{line_num}: // TODO: task {}\n",
                fi * 4 + j + 1
            ));
        }
    }
    assert_pattern("grep_search", "grep -rn TODO src/", &lines);
}

// ═══════════════════════════════════════════════════════════════════════
// systemd
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_systemctl_status() {
    assert_pattern(
        "systemctl_status",
        "systemctl status nginx",
        "\u{25cf} nginx.service - A high performance web server and a reverse proxy server\n\
              Loaded: loaded (/lib/systemd/system/nginx.service; enabled; vendor preset: enabled)\n\
              Active: active (running) since Mon 2024-01-15 10:23:45 UTC; 3 days ago\n\
                Docs: man:nginx(8)\n\
             Process: 1234 ExecStartPre=/usr/sbin/nginx -t -q -g daemon on; (code=exited, status=0/SUCCESS)\n\
             Process: 1235 ExecStart=/usr/sbin/nginx -g daemon on; (code=exited, status=0/SUCCESS)\n\
            Main PID: 1236 (nginx)\n\
               Tasks: 5 (limit: 4915)\n\
              Memory: 12.3M\n\
                 CPU: 1.234s\n\
              CGroup: /system.slice/nginx.service\n\
                      \u{251c}\u{2500}1236 \"nginx: master process /usr/sbin/nginx -g daemon on;\"\n\
                      \u{251c}\u{2500}1237 \"nginx: worker process\"\n\
                      \u{251c}\u{2500}1238 \"nginx: worker process\"\n\
                      \u{251c}\u{2500}1239 \"nginx: worker process\"\n\
                      \u{2514}\u{2500}1240 \"nginx: worker process\"\n\n\
         Jan 15 10:23:44 server1 systemd[1]: Starting A high performance web server...\n\
         Jan 15 10:23:45 server1 systemd[1]: Started A high performance web server.\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Fallback patterns (via compress_output pipeline)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pattern_json_schema_fallback() {
    let large_json = r#"{"users":[{"id":1,"name":"Alice","email":"alice@example.com","role":"admin","active":true,"department":"engineering","location":"NYC","hire_date":"2020-01-15"},{"id":2,"name":"Bob","email":"bob@example.com","role":"user","active":true,"department":"marketing","location":"SF","hire_date":"2021-03-20"},{"id":3,"name":"Charlie","email":"charlie@example.com","role":"user","active":false,"department":"engineering","location":"NYC","hire_date":"2019-06-01"},{"id":4,"name":"Diana","email":"diana@example.com","role":"moderator","active":true,"department":"support","location":"London","hire_date":"2022-09-15"},{"id":5,"name":"Eve","email":"eve@example.com","role":"user","active":true,"department":"engineering","location":"Berlin","hire_date":"2023-01-10"},{"id":6,"name":"Frank","email":"frank@example.com","role":"user","active":false,"department":"marketing","location":"NYC","hire_date":"2020-07-22"},{"id":7,"name":"Grace","email":"grace@example.com","role":"admin","active":true,"department":"engineering","location":"SF","hire_date":"2018-11-30"},{"id":8,"name":"Hank","email":"hank@example.com","role":"user","active":true,"department":"support","location":"London","hire_date":"2023-05-15"},{"id":9,"name":"Irene","email":"irene@example.com","role":"user","active":true,"department":"engineering","location":"Berlin","hire_date":"2021-08-01"},{"id":10,"name":"Jack","email":"jack@example.com","role":"moderator","active":true,"department":"marketing","location":"NYC","hire_date":"2022-02-28"}],"total":10,"page":1,"per_page":20}"#;
    assert_pipeline("json_schema", "my-api-tool list-users", large_json);
}

#[test]
fn pattern_log_dedup_fallback() {
    let mut logs = String::new();
    for i in 0..30 {
        logs.push_str(&format!(
            "2024-01-15 10:23:{i:02} INFO  Processing request for /api/users\n"
        ));
    }
    logs.push_str("2024-01-15 10:24:00 WARN  High latency detected: 250ms\n");
    for i in 0..10 {
        logs.push_str(&format!(
            "2024-01-15 10:24:{:02} INFO  Processing request for /api/users\n",
            i + 1
        ));
    }
    assert_pipeline("log_dedup", "my-server run --log-level info", &logs);
}

#[test]
fn pattern_generic_test_output() {
    assert_pipeline(
        "generic_test",
        "my-test-runner run",
        " PASS  src/auth.test.ts\n\
         PASS  src/db.test.ts\n\
         PASS  src/cache.test.ts\n\
         FAIL  src/api.test.ts\n\
           \u{25cf} DELETE /users/:id returns 204\n\n\
             expect(received).toBe(expected)\n\n\
             Expected: 204\n\
             Received: 403\n\n\
               42 |     const response = await request(app).delete('/users/1');\n\
               43 |     expect(response.status).toBe(204);\n\
                  |                              ^\n\n\
         Test Suites: 1 failed, 3 passed, 4 total\n\
         Tests:       1 failed, 19 passed, 20 total\n\
         Snapshots:   0 total\n\
         Time:        3.456 s\n",
    );
}
