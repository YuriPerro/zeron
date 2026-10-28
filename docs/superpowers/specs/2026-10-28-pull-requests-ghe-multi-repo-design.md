# Pull requests: GitHub Enterprise hosts and multi-repository boards

Fork-only (`zeron-fork`). Engine, RPC params and UI change together; every
device the UI talks to must run this fork.

## Problem

The pull requests board and the native pull request view only read
`github.com`. `GH_HOST=github.com` is fixed in `github_request()`
(`crates/engine/src/source_control.rs`), remote detection drops any other
host, and both URL validators (`validated_pull_request_url` in the engine,
`pull_request_link` / `open_on_device` in `crates/ui/src/pull_request_detail.rs`)
reject anything that is not `github.com`. Work repositories on a GitHub
Enterprise host (`leve-saude.ghe.com`) never show up, and their pull requests
open in the browser.

The board also scopes to exactly one repository.

## Goals

1. The board lists open pull requests from any host `gh` is signed in to.
2. One host per board. The user picks the host in the UI.
3. The board shows up to 5 repositories of that host at once.
4. The native pull request view (detail, diff, checks, comments, review
   threads) works for pull requests on those hosts.

Non-goals: mixing hosts in one board; GitHub Enterprise Server versions whose
GraphQL schema lacks fields the board queries (only `github.com` and
`*.ghe.com` are verified); auth flows inside Zeron (`gh auth login` stays the
way to add a host); resolving every project's host before the menu opens.

## Verified

Against `leve-saude.ghe.com` with `gh` 2.102.0:

- `GITHUB_SEARCH_QUERY` runs unchanged with `GH_HOST=leve-saude.ghe.com`:
  checks rollup, review decision and counts come back, no `errors`.
- `repo:A repo:B` in one search string ORs the repositories (10 results
  across both).
- `gh auth status --json hosts` lists each host with `state` and `active`.

## Engine (`crates/engine`)

### Host allowlist

`GitHubCli::known_hosts() -> Vec<String>`: hosts from
`gh auth status --json hosts` with an active account whose `state` is
`success`, lowercased. Cached 30 seconds like `store_account`. A token in
`GH_TOKEN` / `GITHUB_TOKEN` keeps `github.com` only, since `gh` uses that token
for every host.

Every request that names a host checks it against this list first and fails
with `UnsupportedRepository` otherwise. The list replaces each
`== "github.com"` check, so a crafted URL still cannot make the engine call an
arbitrary host.

### Requests

- `github_request()` becomes `github_request(host: &str)` and sets
  `GH_HOST=host`. Every caller passes the host of its scope or URL
  (`source_control.rs`, `source_control_review_threads.rs`).
- `validated_pull_request_url(raw, hosts)` accepts `https://<host>/owner/repo/pull/N`
  for `host` in the allowlist and returns `(host, canonical_url)`. The canonical
  URL keeps the host. Detail, diff, metadata, comment and review-thread calls
  take `GH_HOST` from it.
- `store_account(host)` reads `gh config get user -h <host>`; the cached value
  is per host.

### Board listing

`list_page(scope, filter, after, refresh, cached)` with
`BoardScope { host: String, repositories: Vec<String> }`:

- `repositories`: 1 to 5 entries, each `valid_pr_repository`, lowercased,
  deduplicated and sorted, so the same set is one cache entry.
- Search string: `is:pr is:open {qualifier}repo:a repo:b sort:updated-desc`.
  Rejected with `UnsupportedRepository` if longer than 256 characters (GitHub's
  search limit).
- The GraphQL `repository(owner, name)` field and the rename follow in
  `fetch_filtered_page` run only when one repository is selected. With several,
  the query passes the first repository as `owner`/`name` and ignores
  `canonical`.
- In-memory cache key: `list:{host}:{repos joined by ","}:{filter}:{after}`.
- On-disk store (`change_request_store.rs`): no schema change. `account` holds
  `{host}:{login}`, `repository` holds the sorted repositories joined by `,`.

### Repository detection

`github_slug` becomes `remote_identity(remote_url) -> Option<RepositoryRef>`
with `RepositoryRef { host, repository }`. It accepts a remote whose host, or
whose `ssh -G` hostname for an SSH alias, is in the allowlist.
`matching_repository_for_checkout` takes a `RepositoryRef` and only matches
remotes on the same host.

### RPC (`crates/engine/src/rpc.rs`, `crates/rpc/src/lib.rs`)

- New `LIST_CHANGE_REQUEST_HOSTS` → `Vec<String>`. Forwardable, like the other
  change request methods.
- `LIST_CHANGE_REQUEST_PAGE` params: `host` (default `github.com`) and
  `repositories: Vec<String>` replace `repository`.
- `GET_CHANGE_REQUEST_REPOSITORY` params: optional `host` next to
  `repository`. Returns `Option<RepositoryRef>` instead of `Option<String>`.

## UI (`crates/ui`)

### Board header

```
[◉ leve-saude.ghe.com ▾]  [📁 Infra-Dev/dbt-platform +2 ▾]
```

- Host chip: shown only when the target device reports 2 or more hosts.
  Options come from `LIST_CHANGE_REQUEST_HOSTS`, fetched per device when the
  board becomes visible or the device changes. Picking a host restores the last
  selection saved for it, or no selection.
- Repository trigger label: `owner/name` for one, `owner/name +N` for more.
  Tooltip lists all of them.

### Repository menu

- Rows toggle membership and the menu stays open. Selected rows show the
  selected state they show today.
- Search row: typing `owner/name` and pressing Enter or **Add** adds it to the
  selection. The button label changes from **Open** to **Add**.
- Project rows still resolve their repository on click. If it is on the current
  host, it toggles. If it is on another host, the board switches to that host
  with that repository as the whole selection.
- **Recent repositories** lists only repositories of the current host.
- A sixth repository is not added. Toast: "Choose up to 5 repositories."
- Removing the last repository leaves the board unscoped, as it is before the
  first choice today.

### State (`pull_requests.rs`)

- `repository: Option<String>` becomes `scope: Option<BoardScope>`.
- `snapshots`, `scope_filters` and `recent_repositories` key on
  `(target device, host, sorted repositories)`.
- `project_repositories` stores `RepositoryRef`.
- Automatic scope on open (`initialize_repository`) keeps its order and sets
  the host from the detected `RepositoryRef`.

### Settings (`settings.rs`)

`last_pull_request_host: Option<String>` and
`last_pull_request_repositories: BTreeMap<String, Vec<String>>` (host →
repositories) replace `last_pull_request_repository`. The old field is removed
and the last saved repository is lost once.

### Native view and handoff

- `AppState` keeps the host list per device from `LIST_CHANGE_REQUEST_HOSTS`.
- `open_on_device` opens the native view when the URL's host is in that list for
  the device, otherwise the browser, as today for unknown hosts.
- `pull_request_link` accepts `https://<host>/owner/repo/pull/N` for any host
  and keeps the host in the result. The engine still enforces the allowlist.
- `pull_request_handoff.rs` passes the pull request's host with `repository`
  to `GET_CHANGE_REQUEST_REPOSITORY` and compares both.
- Star keys already use the full URL; no change.

## Errors

- Host signed out or token expired: `classify_github_failure` already maps
  `gh` auth errors to `Authentication`; the board shows its existing auth
  error. The copy names `gh auth login`; it now includes `-h <host>` when the
  host is not `github.com`.
- Host missing from the allowlist (removed with `gh auth logout`): the board
  drops it from the chip and falls back to the first known host.

## Testing

Engine (`source_control.rs` tests, scripted `ProcessRunner`):

- `GH_HOST` equals the scope's host for list, and the URL's host for detail,
  diff, comment and review threads.
- Allowlist rejects an unknown host for list and for a crafted detail URL.
- Multi-repository search string, sorting/dedup of the key, 5-repo and
  256-character limits, rename follow skipped with 2+ repositories.
- `remote_identity` for an HTTPS GHE remote, an SSH alias resolving to a GHE
  host, and a host outside the allowlist.
- Store keys separate the same repository name on two hosts.

UI (`pull_requests.rs` tests with the fake engine):

- Toggling rows builds the selection and the `LIST_CHANGE_REQUEST_PAGE`
  params; the sixth repository is refused.
- Host switch restores that host's saved selection.
- Project on another host switches host and replaces the selection.
- `pull_request_link` and `open_on_device` with a GHE URL.

Manual: board on `leve-saude.ghe.com` with two `Infra-Dev` repositories, open
one pull request natively, post a comment.
