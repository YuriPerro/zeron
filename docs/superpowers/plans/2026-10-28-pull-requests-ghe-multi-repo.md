# Pull requests: GHE hosts and multi-repository boards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The pull requests board and native pull request view work on any host `gh` is signed in to, and the board shows up to 5 repositories of one host at once.

**Architecture:** The engine learns an allowlist of hosts from `gh auth status --json hosts` and passes `GH_HOST` per request (board scope host, or the pull request URL's host). The board scope becomes `{ host, repositories[] }`, searched with one GitHub query (`repo:a repo:b`). The UI gets a host chip, a multi-select repository menu, and a global host list that decides whether a pull request opens natively.

**Tech Stack:** Rust, gpui, tokio, `gh` CLI, rusqlite (existing store).

**Spec:** `docs/superpowers/specs/2026-10-28-pull-requests-ghe-multi-repo-design.md`

## Global Constraints

- One host per board; at most 5 repositories; search string at most 256 characters.
- Allowlist: hosts from `gh auth status --json hosts` with an account where `active == true && state == "success"`, lowercased, cached 30 s. With non-empty `GH_TOKEN` or `GITHUB_TOKEN`, the allowlist is `["github.com"]` and `gh` is not called.
- Every host-bearing engine request fails with `ChangeRequestError::UnsupportedRepository` when the host is not in the allowlist.
- Product copy is English, matching the board: toast "Choose up to 5 repositories.", button "Add".
- No inline `//` comments in new code beyond what the surrounding code already does; doc comments on new pub items.
- Commit messages in English, conventional style used by the repo (`feat(engine): …`, `feat(ui): …`), no co-author trailer.
- Remove replaced code (`github_request()` without host, `last_pull_request_repository`, `repository` RPC param). No compatibility path.

## Review Focus

- A crafted pull request URL on a host outside the allowlist reaches the native view: the engine must refuse it before running `gh` (Task 3 test `detail_refuses_url_on_unknown_host`).
- Same `owner/name` on two hosts: board cache, on-disk store and snapshots must not mix them (Task 4 test `store_separates_same_repository_on_two_hosts`, Task 7 test `snapshots_key_on_host`).
- The repositories set in a different order (`b,a` vs `a,b`) must hit the same cache and store entry (Task 4 test `scope_key_ignores_order_and_case`).
- A host signed out after it was chosen: board falls back to the first known host instead of looping on auth errors (Task 7 test `missing_host_falls_back_to_first_known`).
- Clicking a GHE pull request pill in a chat before ever opening the board must open natively: the local host list is fetched on engine attach, not only by the board (Task 6 test `hosts_fetched_on_engine_attach`).

---

### Task 1: Shared types and RPC method

**Files:**
- Modify: `crates/proto/src/entities.rs` (next to `ChangeRequestListItem`)
- Modify: `crates/rpc/src/lib.rs:221-226`
- Modify: `crates/engine/src/rpc.rs:2015-2016` (forwardable list), `:5037-5040` (test)

**Interfaces:**
- Produces: `zeron_proto::RepositoryRef { pub host: String, pub repository: String }`, `#[serde(rename_all = "camelCase")]`, derives `Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize`.
- Produces: `zeron_proto::BoardScope { pub host: String, pub repositories: Vec<String> }`, same derives, plus `impl BoardScope { pub fn key(&self) -> String }` returning `"{host}:{sorted lowercased repos joined by ','}"` with host lowercased.
- Produces: `methods::LIST_CHANGE_REQUEST_HOSTS: &str = "ListChangeRequestHosts"`, forwardable, not a stream.

- [ ] **Step 1: Write failing tests**

In `entities.rs` tests: `board_scope_key_is_order_and_case_insensitive` asserting
`BoardScope { host: "GHE.example".into(), repositories: vec!["B/x".into(), "a/y".into()] }.key() == "ghe.example:a/y,b/x"`.
In `rpc.rs` test at `:5037`: `assert!(forwardable(methods::LIST_CHANGE_REQUEST_HOSTS)); assert!(!is_stream_method(methods::LIST_CHANGE_REQUEST_HOSTS));`

- [ ] **Step 2: Run, expect compile failure**

Run: `cargo test -p zeron-proto board_scope_key && cargo test -p zeron-engine --lib rpc::tests`
Expected: FAIL, `BoardScope` / `LIST_CHANGE_REQUEST_HOSTS` not found.

- [ ] **Step 3: Add the types, the constant (doc comment: "Hosts `gh` is signed in to on this device.") and the forwardable entry.**

- [ ] **Step 4: Run, expect PASS** (same command).

- [ ] **Step 5: Commit** `feat(proto): repository ref, board scope and host list method`

---

### Task 2: Engine host allowlist and per-host `gh` requests

**Files:**
- Modify: `crates/engine/src/source_control.rs` (`GitHubCli` struct `:352-360`, `with_runner` `:769`, `store_account` `:790-830`, `github_request` `:1096-1109`, every `..github_request()` caller)
- Modify: `crates/engine/src/source_control_review_threads.rs:20`
- Test: `crates/engine/src/source_control.rs` tests module

**Interfaces:**
- Produces: `GitHubCli::known_hosts(&self) -> Vec<String>` (async). New field `hosts: Arc<tokio::sync::Mutex<Option<(Instant, Vec<String>)>>>`.
- Produces: `#[cfg(test)] fn with_hosts(self, hosts: &[&str]) -> Self` that pre-fills `hosts` so tests never script `gh auth status`. All existing tests constructing `GitHubCli::with_runner` add `.with_hosts(&["github.com"])`.
- Produces: `fn github_request(host: &str) -> ProcessRequest` with env `GH_PROMPT_DISABLED=1`, `GH_HOST=host`.
- Produces: `GitHubCli::store_account(&self, host: &str) -> Option<String>` returning `"{host}:{login}"`; cache becomes `HashMap<String, (Instant, Option<String>)>`.

- [ ] **Step 1: Write failing tests**

- `known_hosts_reads_signed_in_hosts`: fake runner returns
  `{"hosts":{"github.com":[{"state":"success","active":true,"login":"a"}],"Acme.GHE.com":[{"state":"success","active":true,"login":"b"}],"old.example":[{"state":"error","active":true,"login":"c"}]}}`;
  assert `known_hosts() == ["acme.ghe.com", "github.com"]` (sorted) and the request args are `["auth","status","--json","hosts"]`. Second call within 30 s issues no new request.
- `allowed_hosts_with_token_is_github_only`: test the pure helper `fn allowed_hosts(token_in_env: bool, status: Option<&[u8]>) -> Vec<String>` (no process env mutation in tests): `allowed_hosts(true, Some(<json above>)) == ["github.com"]`, `allowed_hosts(false, None) == ["github.com"]`. `known_hosts` checks the env first and skips `gh` when a token is set.
- `store_account_is_per_host`: scripted `gh config get user -h acme.ghe.com` → `b\n`; assert `store_account("acme.ghe.com") == Some("acme.ghe.com:b")` and the request args end with `"-h", "acme.ghe.com"`.

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p zeron-engine --lib source_control::tests::known_hosts source_control::tests::store_account`

- [ ] **Step 3: Implement.** Parse with a private `#[derive(Deserialize)] struct GhAuthStatus { hosts: BTreeMap<String, Vec<GhAuthAccount>> }`. A failed or undecodable `gh auth status` yields `["github.com"]` (today's behavior) and is cached like a success. Replace `github_request()` with `github_request(host)`; existing callers temporarily pass `"github.com"` (Tasks 3 and 4 thread the real host). Update the `GH_HOST` assertions at `:2555` and `:3569` only if their expected value changes (it does not yet).

- [ ] **Step 4: Run the whole engine suite, expect PASS**

Run: `cargo test -p zeron-engine --lib source_control`

- [ ] **Step 5: Commit** `feat(engine): learn gh hosts and pass GH_HOST per request`

---

### Task 3: Engine pull request URLs on any allowed host (native view)

**Files:**
- Modify: `crates/engine/src/source_control.rs` (`validated_pull_request_url` `:1956-1983`, callers `:462, :494, :589, :665`, `fetch_revision_diff` `:556`, `fetch_pr_metadata` `:660`, `post_comment` `:489`)
- Modify: `crates/engine/src/source_control_review_threads.rs:20, :47-49`

**Interfaces:**
- Consumes: `known_hosts`, `github_request(host)` (Task 2).
- Produces: `struct PullRequestUrl { host: String, owner: String, name: String, number: u64, url: String }` where `url = "https://{host}/{owner}/{name}/pull/{number}"`, host lowercased.
- Produces: `fn parse_pull_request_url(raw: &str, hosts: &[String]) -> Result<PullRequestUrl, ChangeRequestError>`, same rules as today's validator with `host ∈ hosts` replacing `== "github.com"`.
- Produces: `GitHubCli::pull_request_url(&self, raw: &str) -> Result<PullRequestUrl, ChangeRequestError>` (async, calls `known_hosts`). Every detail-path caller uses it and passes `&url.host` to `github_request`. `fetch_review_threads` uses `url.owner/name/number` instead of splitting the string.

- [ ] **Step 1: Write failing tests**

- Extend the existing rejection table at `:2700-2712`: call `parse_pull_request_url(url, &hosts)` with `hosts = ["github.com","acme.ghe.com"]`; add accepted `https://ACME.ghe.com/a/b/pull/3#x` → `url == "https://acme.ghe.com/a/b/pull/3"`, `host == "acme.ghe.com"`, `number == 3`; add rejected `https://evil.example/a/b/pull/3`.
- `detail_uses_url_host`: `GitHubCli::with_runner(runner).with_hosts(&["github.com","acme.ghe.com"])`, call `detail("https://acme.ghe.com/a/b/pull/1", false)` with the fixture responses the existing detail test uses; assert every recorded request has `("GH_HOST","acme.ghe.com")`.
- `detail_refuses_url_on_unknown_host`: hosts `["github.com"]`, `detail("https://acme.ghe.com/a/b/pull/1", false)` → `Err(UnsupportedRepository)` and `runner.requests().is_empty()`.

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p zeron-engine --lib source_control::tests::detail_ source_control::tests::validated`

- [ ] **Step 3: Implement.** Cache keys built from the URL already contain the host once the canonical URL keeps it; verify `detail_request`'s key uses `url.url`.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p zeron-engine --lib source_control`

- [ ] **Step 5: Commit** `feat(engine): native pull request view on any signed-in gh host`

---

### Task 4: Engine board listing by `BoardScope`

**Files:**
- Modify: `crates/engine/src/source_control.rs` (`list_page` `:837`, `fetch_list_page` `:873`, `fetch_filtered_page` `:895`, `fetch_scoped_search` `:918`)
- Modify: `crates/engine/src/change_request_store.rs` (doc comment only: `account` is `host:login`, `repository` is the sorted set)

**Interfaces:**
- Consumes: `BoardScope` (Task 1), `known_hosts`, `store_account(host)` (Task 2).
- Produces: `GitHubCli::list_page(&self, scope: &BoardScope, filter, after: Option<&str>, refresh: bool, cached: bool) -> Result<ChangeRequestPage, ChangeRequestError>`.
- Produces: `pub fn valid_board_scope(scope: &BoardScope) -> bool`: 1..=5 repositories, each `valid_pr_repository`, host non-empty.
- Search string, exact: `format!("is:pr is:open {qualifier}{repos}sort:updated-desc")` with `repos` = each sorted lowercased repository as `"repo:{r} "`. Over 256 chars → `UnsupportedRepository`.
- Store: `account = store_account(&scope.host)`, `repository = scope.key()` minus the `host:` prefix (sorted repos joined by `,`).
- In-memory key: `format!("list:{}:{filter:?}:{}", scope.key(), after.unwrap_or_default())`.
- Rename follow and `canonical` only when `repositories.len() == 1`; otherwise GraphQL `owner`/`name` = first sorted repository and `canonical` is ignored.

- [ ] **Step 1: Write failing tests**

- `board_search_names_every_repository`: scope `{host:"acme.ghe.com", repositories:["Infra/b","infra/a"]}`, filter `Authored`; assert the `search=` arg equals `"search=is:pr is:open author:@me repo:infra/a repo:infra/b sort:updated-desc"` and env has `GH_HOST=acme.ghe.com`.
- `board_rejects_six_repositories_and_long_queries`: 6 repos → `Err(UnsupportedRepository)`; 5 repos of 50-char names → `Err(UnsupportedRepository)`; no requests recorded.
- `board_rejects_unknown_host`: hosts `["github.com"]`, scope host `acme.ghe.com` → `Err(UnsupportedRepository)`, no requests.
- `scope_key_ignores_order_and_case`: two `list_page` calls with `["b/x","a/y"]` then `["A/Y","b/x"]` and the same filter issue one `gh` request (cache hit).
- `rename_follow_skipped_with_several_repositories`: empty first page whose `repository.nameWithOwner` differs → no second request when 2 repositories.
- `store_separates_same_repository_on_two_hosts`: with `with_store(dir)`, save a page for `{github.com, [a/b]}` and load `{acme.ghe.com, [a/b]}` with `cached: true` → goes to `gh` (no stored page returned). Adapt the existing store test at `:3204`.

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p zeron-engine --lib source_control::tests::board_ source_control::tests::scope_key source_control::tests::store_`

- [ ] **Step 3: Implement.** Update existing list tests to pass a `BoardScope { host: "github.com".into(), repositories: vec![…] }`.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p zeron-engine --lib source_control`

- [ ] **Step 5: Commit** `feat(engine): list pull requests for several repositories of one host`

---

### Task 5: Engine repository detection and RPC params

**Files:**
- Modify: `crates/engine/src/source_control.rs` (`github_slug` `:261-290`, `repository_for_checkout` `:196`, `matching_repository_for_checkout` `:215`)
- Modify: `crates/engine/src/rpc.rs:3223-3282` and a new `LIST_CHANGE_REQUEST_HOSTS` arm
- Test: end-to-end test at `source_control.rs:2233` and the dispatch tests near `:2340-2525`

**Interfaces:**
- Consumes: `RepositoryRef`, `BoardScope`, `LIST_CHANGE_REQUEST_HOSTS` (Task 1); `known_hosts` (Task 2); `list_page` (Task 4).
- Produces: `ChangeRequestResolver::remote_identity(&self, remote_url: &str, hosts: &[String]) -> Option<RepositoryRef>` replacing `github_slug`. Accepts the remote's host, or the `ssh -G` hostname of an SSH alias, when it is in `hosts`.
- Produces: `repository_for_checkout(cwd, hosts) -> Option<RepositoryRef>`, `matching_repository_for_checkout(cwd, wanted: &RepositoryRef, github) -> Option<RepositoryRef>` (same host only; `canonical_repository` gets a `host` param).
- RPC `GET_CHANGE_REQUEST_REPOSITORY` params `{ cwd, repository?: String, host?: String (default "github.com") }` → `Option<RepositoryRef>`.
- RPC `LIST_CHANGE_REQUEST_PAGE` params `{ host?: String (default "github.com"), repositories: Vec<String>, filter, after, refresh, cached }`; `BadParams("repositories must be 1 to 5 owner/repo")` when `!valid_board_scope`.
- RPC `LIST_CHANGE_REQUEST_HOSTS` params `{}` → `Vec<String>` from `self.github.known_hosts()`.

- [ ] **Step 1: Write failing tests**

- `remote_identity_accepts_allowed_hosts`: hosts `["github.com","acme.ghe.com"]`; `https://acme.ghe.com/Infra/dbt.git` → `RepositoryRef{host:"acme.ghe.com", repository:"Infra/dbt"}`; `git@work:Infra/dbt.git` with scripted `ssh -G work` output `hostname acme.ghe.com\n` → same; `https://gitlab.com/a/b.git` → `None`.
- In the end-to-end test (`:2233`): `GET_CHANGE_REQUEST_REPOSITORY` returns `{"host":"github.com","repository":"acme/zeron"}`; `LIST_CHANGE_REQUEST_PAGE` called with `{"repositories":["acme/zeron"]}` succeeds; `{"repositories":[]}` → `BadParams`.
- `list_hosts_rpc`: dispatch `LIST_CHANGE_REQUEST_HOSTS` with hosts seeded → `["acme.ghe.com","github.com"]`.

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p zeron-engine --lib source_control::tests rpc::tests`

- [ ] **Step 3: Implement.** `self.github` in `rpc.rs` is the shared `GitHubCli`; pass `self.github.known_hosts().await` into the resolver calls. Update the rpc method doc comments in `crates/rpc/src/lib.rs` (`LIST_CHANGE_REQUEST_PAGE`: "One page of open pull requests for 1 to 5 repositories of one host").

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p zeron-engine`

- [ ] **Step 5: Commit** `feat(engine): detect repositories on any gh host`

---

### Task 6: UI host list, native open and handoff

**Files:**
- Create: `crates/ui/src/pull_request_hosts.rs` (register in `lib.rs` next to `pull_request_detail`)
- Modify: `crates/ui/src/state.rs:3541-3563` (`attach_engine`)
- Modify: `crates/ui/src/pull_request_detail.rs:75-123`
- Modify: `crates/ui/src/pull_request_handoff.rs:285-310` and its test at `:598`

**Interfaces:**
- Consumes: `LIST_CHANGE_REQUEST_HOSTS`, `RepositoryRef` (Tasks 1, 5).
- Produces: `pub(crate) struct PullRequestHosts { by_device: HashMap<Option<String>, Vec<String>> }` as a gpui `Global` with
  `pub(crate) fn hosts(cx: &App, device: Option<&str>) -> Vec<String>` (defaults to `["github.com"]` when unknown),
  `pub(crate) fn set(cx: &mut App, device: Option<String>, hosts: Vec<String>)`,
  `pub(crate) fn fetch(engine: EngineHandle, device: Option<String>, cx: &mut App) -> Task<()>` (calls the RPC with `params_for_target`, stores on success, keeps the old value on error).
- Produces: `pull_request_link(url) -> Option<String>` accepting any https host, result `https://{host}/{owner}/{repo}/pull/{n}` with host lowercased.
- `open_on_device` opens natively when the URL host is in `hosts(cx, device)` and destination is not External.
- Produces: `pub(crate) fn pull_request_repository(url: &str) -> Option<RepositoryRef>` in `pull_request_detail.rs`, built on the same parsing as `pull_request_link` (`https://acme.ghe.com/a/b/pull/1` → `{host:"acme.ghe.com", repository:"a/b"}`).
- `matching_checkout(engine, spaces, device, wanted: &RepositoryRef)` sends `host` and `repository`, compares both case-insensitively. The caller gets `wanted` from `pull_request_repository`.
- Auth error copy names the host when it is not `github.com`: `pull_request_detail.rs:59` becomes `"Run gh auth login -h {host} on this device, then refresh."` for such hosts, unchanged for `github.com`.

- [ ] **Step 1: Write failing tests**

- `pull_request_link_keeps_host`: `pull_request_link("https://Acme.ghe.com/a/b/pull/12/files") == Some("https://acme.ghe.com/a/b/pull/12")`; keep the existing rejection table (`http://`, `/issues/`, port, userinfo).
- `opens_ghe_natively_when_host_known` / `opens_unknown_host_in_browser`: set `PullRequestHosts` for `None` to `["github.com","acme.ghe.com"]`, call `open_on_device` and assert the dispatched action vs `cx.open_url` (use the existing test harness in `pull_request_detail.rs` tests).
- `hosts_fetched_on_engine_attach`: `ScriptedRpc` answering `LIST_CHANGE_REQUEST_HOSTS` with `["acme.ghe.com","github.com"]`; after attach, `hosts(cx, None)` contains `"acme.ghe.com"`.
- Handoff test at `:598`: expects params `{"host":"acme.ghe.com","repository":"a/b"}` and a `RepositoryRef` reply.

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo nextest run -p zeron-ui --lib pull_request_detail pull_request_handoff pull_request_hosts state::tests::hosts_fetched`

- [ ] **Step 3: Implement.** In `attach_engine`, spawn `pull_request_hosts::fetch(handle.clone(), None, cx)` and hold its task with the other watch tasks. Existing `ScriptedRpc` handlers that panic on unknown methods must answer `LIST_CHANGE_REQUEST_HOSTS`; add `pull_request_test_support::hosts_reply(method) -> Option<RpcReply>` returning `["github.com"]` and call it first in those handlers.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo nextest run -p zeron-ui --lib`

- [ ] **Step 5: Commit** `feat(ui): open pull requests natively on signed-in gh hosts`

---

### Task 7: UI board scope, host chip and multi-select menu

**Files:**
- Modify: `crates/ui/src/pull_requests.rs` (struct `:293-363`, `initialize_repository` `:621`, `apply_initial_repository` `:748`, `reset_for_target` `:816`, `restore_snapshot` `:842`, `select_repository` `:864`, `remember_filter` `:955`, `select_project` `:1002`, `project_is_selected` `:1067`, `recent_repositories` `:1076`, `render_repository_menu` `:1093-1470`, `load` params `:1638, :1768`, test fixture URL `:3641`, tests)
- Modify: `crates/ui/src/settings.rs:1180-1181, :1302, :1975, :3061`

**Interfaces:**
- Consumes: `BoardScope`, `RepositoryRef` (Task 1); `PullRequestHosts` (Task 6).
- Settings: `last_pull_request_host: Option<String>`, `last_pull_request_repositories: BTreeMap<String, Vec<String>>` replace `last_pull_request_repository`; `last_pull_request_device` stays.
- Page state: `scope: Option<BoardScope>` replaces `repository`; `host: String` (current host, default `"github.com"`); `project_repositories: HashMap<ProjectRef, RepositoryRef>`; `snapshots` key `(Option<String>, String /* scope.key() */, ChangeRequestFilter)`; `scope_filters` key `(Option<String>, String /* scope.key() */)`.
- Methods:
  - `fn toggle_repository(&mut self, repository: String, cx)`: lowercases, removes if present, else adds; 6th → `toast::error(cx, "Choose up to 5 repositories.")` and no change; empty → `scope = None`, board unscoped. Saves settings, resets and loads like `select_repository` does today.
  - `fn add_typed_repository(&mut self, cx)`: reads `repository_input`, validates with `valid_repository_filter` (toast "Enter a repository as owner/name." unchanged), calls `toggle_repository` only when not already selected, clears the input.
  - `fn select_host(&mut self, host: String, cx)`: sets `host`, scope = saved `last_pull_request_repositories[host]` or `None`, resets and loads.
  - `select_project` result: same host → `toggle_repository`; other host → `select_host(ref.host)` then scope `[ref.repository]`.
  - `fn scope_label(&self) -> String`: `"owner/name"` or `"owner/name +N"` (first sorted repository).
- `load` params: `params["host"] = scope.host`, `params["repositories"] = scope.repositories`.
- Host chip: rendered left of the repository trigger only when `PullRequestHosts::hosts(cx, target).len() >= 2`; a `popover` menu like the device menu, one row per host. Fetch hosts with `PullRequestHosts::fetch` in `on_visible` and `set_target_device` for the target.
- Menu: project and recent rows call `toggle_repository` and do not close the menu; selected state = membership in `scope`. Search button label "Add", aria-label "Add repository". Recent rows: repositories of the current host from snapshots plus `last_pull_request_repositories[host]`, max 5.
- Trigger tooltip: all repositories, one per line.
- Auth error copy at `:3492`: `"Run gh auth login -h {host}, then refresh this page."` when the board host is not `github.com`, unchanged otherwise. Test `auth_error_names_ghe_host`.
- If `host` is no longer in `PullRequestHosts::hosts`, switch to the first known host via `select_host`.

- [ ] **Step 1: Write failing tests** (existing `ScriptedRpc` style, `fixture::settings`)

- `toggling_rows_builds_scope_params`: toggle `infra/a`, `infra/b` on host `acme.ghe.com`; last `LIST_CHANGE_REQUEST_PAGE` params `{"host":"acme.ghe.com","repositories":["infra/a","infra/b"]}`; menu still open.
- `sixth_repository_is_refused`: five toggled, sixth → toast "Choose up to 5 repositories.", scope length 5.
- `host_switch_restores_saved_selection`: settings `last_pull_request_repositories = {"acme.ghe.com": ["infra/a"], "github.com": ["owner/repo"]}`; `select_host("acme.ghe.com")` → scope repositories `["infra/a"]`, settings host saved.
- `project_on_other_host_switches_host`: board on `github.com` with `owner/repo`; project resolves `{"host":"acme.ghe.com","repository":"infra/a"}` → host `acme.ghe.com`, scope `["infra/a"]`.
- `snapshots_key_on_host`: snapshot for `github.com:a/b` is not restored for `acme.ghe.com:a/b`.
- `missing_host_falls_back_to_first_known`: hosts `["github.com"]`, page host `acme.ghe.com` → `on_visible` sets host `github.com`.
- `host_chip_hidden_with_one_host`: no element with debug selector `pr-host` when hosts are `["github.com"]`.
- Update every existing test that sets `page.repository` or asserts `params["repository"]` to the new shape.

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo nextest run -p zeron-ui --lib pull_requests settings`

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run the UI suite and the fixture build, expect PASS**

Run: `cargo nextest run -p zeron-ui --lib && cargo build -p zeron-ui --example pull-request-fixture`

- [ ] **Step 5: Commit** `feat(ui): pick the gh host and several repositories on the pull requests board`

---

### Task 8: Workspace check and manual verification

- [ ] **Step 1:** `cargo clippy --workspace --all-targets -- -D warnings` → no warnings. `cargo test -p zeron-engine -p zeron-proto -p zeron-rpc` and `cargo nextest run -p zeron-ui --lib` → all pass.
- [ ] **Step 2:** Run the app (`run` skill). Board: host chip shows `github.com` and `leve-saude.ghe.com`; pick `leve-saude.ghe.com`, add `Infra-Dev/dbt-platform` and `Infra-Dev/dbt-mkt-cloud-journeys`; list shows pull requests from both, label `infra-dev/dbt-mkt-cloud-journeys +1`.
- [ ] **Step 3:** Open one GHE pull request: native view loads diff and checks. Post a comment on a pull request of your own, confirm it on the web.
- [ ] **Step 4:** Switch back to `github.com`: previous `yuriperro/arch-live` selection returns.
- [ ] **Step 5:** Commit any fixes from manual testing with a message describing the fix.
