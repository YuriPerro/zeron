# Orchestrator Engine (Plan 1 of 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Zeron chat can be an orchestrator with its own prompt and associated projects. It creates role-based child chats whose model is resolved from a global role map, shares spec and plan files with them, and is woken when a child finishes. The existing chat UI is enough to use it.

**Architecture:** One neutral `instructions` field on the run request, translated by each driver. A new engine module `orchestration` owns roles, the model map, the orchestrator store and the instruction resolution, and is exposed through new engine RPCs. The MCP server gains role-aware `create_chat` and orchestrator/child tools that call those RPCs. A DocHost watcher wakes the parent orchestrator when a child's turn ends.

**Tech Stack:** Rust (stable, workspace at github.com/zeronsh/zeron @ `2d6895c`), tokio, serde, `toml = "0.8"` (already an engine dependency), clap (CLI).

**Spec:** `specs/2026-10-24-orquestrador-design.md` (copied to `docs/superpowers/specs/` in Task 0). Plan 1 covers spec sections 2, 3, 5, 6, 8 (except validation, notifications, cleanup), 11 and the matching items of 12. Plan 2 (sandbox, worktrees, cleanup, diff/PR tools) and Plan 3 (UI) are separate.

## Global Constraints

- Data paths: roles in `{data_dir}/roles/<name>.md`, model map in `{data_dir}/roles/models.toml`, orchestrators in `{data_dir}/orchestrators/<id>/`. `data_dir` is the engine's `EngineConfig.data_dir`.
- File formats are TOML (`+++` frontmatter for role files). No YAML dependency.
- The MCP crate never touches the filesystem (`crates/mcp/src/lib.rs:6`). All file access goes through engine RPCs.
- New code lives in new files: `crates/engine/src/orchestration/` and `crates/mcp/src/orchestration.rs`. Edits to existing upstream files stay minimal (field additions, one match arm, one call site).
- Default role names are Portuguese: `explorador`, `planejador`, `executor`, `revisor`. Aliases: `@forte`, `@medio`, `@rapido`. Tool names, tool descriptions and agent-facing text are English, like the rest of the MCP server.
- No `//` narration comments. `///` only on public items, following the surrounding style.
- Commits: English messages, no co-author or tool trailers.
- Tests: `cargo test --locked -p <crate> --lib` for unit tests, `cargo test --locked -p zeron-harness --test claude --test codex` for driver tests (unix only), `cargo test --locked -p zeron-engine --test orchestration` for the engine integration test.

## Review Focus

- A demand id or child id containing `..`, `/` or uppercase must be rejected, not written outside the orchestrator folder. Pinned in Task 3.
- Every candidate for an alias missing from the device catalog must fail with an explicit error naming the alias, harness and tried candidates, never fall back silently. Pinned in Task 2.
- Changing the model of an orchestrator or role chat from the composer must keep `orchestrator` and `role` in its config (`SetChatConfig` replaces the whole config today). Pinned in Task 4.
- The parent is woken once per child turn and only when the parent is an orchestrator. An orchestrator's own turn end, or a child of a normal chat, wakes nobody. Pinned in Task 7.
- An orchestrator asking for a child in a project it is not associated with gets a refusal listing the associated projects. Pinned in Task 6.

---

### Task 0: Fork, baseline and docs

**Files:**
- Create: `docs/superpowers/specs/2026-10-24-orquestrador-design.md`, `docs/superpowers/plans/2026-10-24-orquestrador-plano-1-motor.md` (copies of the session files)

- [ ] **Step 1:** Ask the user to confirm the fork owner, then run `gh repo fork zeronsh/zeron --clone --remote` and `git checkout -b feat/orchestrator-engine`.
- [ ] **Step 2:** Run `cargo test --locked -p zeron-mcp --lib` and `cargo test --locked -p zeron-harness --lib`. Expected: both pass on the untouched fork. Record any pre-existing failure in the PR notes and do not fix it.
- [ ] **Step 3:** Copy the spec and this plan into `docs/superpowers/`, commit `docs: add orchestrator design and plan 1`.

### Task 1: Neutral run instructions

**Files:**
- Modify: `crates/proto/src/agent.rs:300-389` (`RunRequest` field and its hand-written `Debug`)
- Modify: every `RunRequest {` literal reported by the compiler (about 124, mostly tests): add `instructions: None`
- Modify: `crates/harness/src/claude/mod.rs:273` (`launch_args`)
- Modify: `crates/harness/src/codex/mod.rs:1280-1324` (`start_params`)
- Create: `crates/engine/src/orchestration/mod.rs`, `crates/engine/src/orchestration/instructions.rs`
- Modify: `crates/engine/src/sessions.rs:3531-3535` (per-harness prompt rewrite in `drive_run`)
- Test: `crates/harness/tests/claude.rs` + `tests/fixtures/fake-claude.sh`, `crates/harness/tests/codex.rs` + `fake-codex.sh`, unit test in `instructions.rs`

**Interfaces:**
- Produces: `RunRequest.instructions: Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Produces: `pub fn prefix_first_prompt(instructions: &str, prompt: &str) -> String` in `orchestration::instructions`.

- [ ] **Step 1: Write the failing driver tests.** Claude: add fixture scenario `instructions-check` that fails unless `"$@"` contains `--append-system-prompt` followed by `ORCH-RULES`. Test `instructions_reach_claude_as_append_system_prompt` runs a request with `instructions: Some("ORCH-RULES".into())` and expects the turn to complete. Codex: fixture scenario `instructions` greps the `thread/start` line for `"developerInstructions":"ORCH-RULES"`. Test `instructions_reach_codex_as_developer_instructions`, modeled on `title_run_preserves_read_only_and_replaces_coding_instructions` (`tests/codex.rs:1621`). Also assert a title run still sends `TITLE_INSTRUCTIONS` and not the chat's instructions.
- [ ] **Step 2: Write the failing unit test** `prefix_wraps_instructions_before_the_prompt`: `prefix_first_prompt("A", "B") == "<instructions>\nA\n</instructions>\n\nB"`.
- [ ] **Step 3:** Run the three tests. Expected: compile error on the missing field, then FAIL.
- [ ] **Step 4: Implement.** Add the field and the `Debug` entry. Claude: in `launch_args`, push `--append-system-prompt <text>` when `instructions` is `Some` (title runs keep their own `--system-prompt` path in `build_command`). Codex: insert `developerInstructions` in `start_params` when `Some` and not `title_only`. Engine: in `drive_run`, for harnesses other than `ClaudeCode` and `Codex`, when `request.resume.is_none()` and `instructions` is `Some`, replace `wire_request.prompt` with `prefix_first_prompt(..)`.
- [ ] **Step 5:** Run `cargo test --locked -p zeron-harness --test claude --test codex` and `cargo test --locked -p zeron-engine --lib orchestration`. Expected: PASS, plus the full `zeron-harness --lib` suite still green.
- [ ] **Step 6:** Commit `feat(harness): carry per-chat instructions to every driver`.

### Task 2: Roles and model map

**Files:**
- Create: `crates/engine/src/orchestration/roles.rs`, `crates/engine/src/orchestration/models.rs`, `crates/engine/src/orchestration/defaults.rs`

**Interfaces:**
- Produces in `roles.rs`:
  - `pub enum RoleAccess { ReadOnly, Write }` (serde `kebab-case`, default `Write`)
  - `pub struct Role { pub name: String, pub description: String, pub model: String, pub access: RoleAccess, pub prompt: String }`
  - `pub fn parse_role(text: &str) -> Result<Role, RoleError>` (`+++` TOML frontmatter, body is the prompt, trimmed)
  - `pub fn load_roles(dir: &Path) -> (Vec<Role>, Vec<RoleError>)` (sorted by name, invalid files reported, not fatal)
  - `pub fn seed_defaults(dir: &Path) -> std::io::Result<()>` (writes the four default roles and `models.toml` only when the folder has no `.md` file)
- Produces in `models.rs`:
  - `pub enum RelativeEffort { Low, Medium, High }` (serde lowercase)
  - `pub struct ModelMap { pub aliases: BTreeMap<String, BTreeMap<HarnessId, Vec<String>>> }`, parsed from `[alias.<name>]` tables
  - `pub fn parse_model_map(text: &str) -> Result<ModelMap, ModelMapError>`
  - `pub struct ResolvedModel { pub model: String, pub reasoning: Option<ReasoningLevel> }`
  - `pub fn resolve(map: &ModelMap, alias: &str, harness: HarnessId, catalog: &[Model], effort: Option<RelativeEffort>) -> Result<ResolvedModel, ResolveError>`
  - `pub enum ResolveError { UnknownAlias(String), NoCandidate { alias: String, harness: HarnessId, tried: Vec<String> } }` with `Display` naming all three
- `defaults.rs` holds `DEFAULT_ROLES: [(&str, &str); 4]` (file name, content) and `DEFAULT_MODELS_TOML` with exactly the map in spec section 5. Role prompts: explorador "Read the code relevant to the brief and report what you found. Never edit files." planejador "Write the spec and the plan the brief asks for. Do not implement." executor "Implement exactly what the brief and the spec ask. Prove it works as the brief says. Do not change the contract; ask the orchestrator instead." revisor "Check the diff and the tests against the spec. Report concrete findings with file and line. Do not fix them."

Resolution rules: strip a leading `@` from `alias`. Candidates are `"<model>:<level>"` or `"<model>"`. A candidate is available when its model id is in `catalog`. The level ladder is `[Minimal, Low, Medium, High, XHigh, Max]` filtered to the model's `reasoning_levels`. A level not supported by the model clamps to the nearest supported ladder level below, else above. `RelativeEffort::Low` moves one step down the filtered ladder, `High` one step up, `Medium` keeps it, clamped at the ends. A model with empty `reasoning_levels` resolves with `reasoning: None`.

- [ ] **Step 1: Write failing tests** in `roles.rs` and `models.rs`:
  - `parses_role_frontmatter_and_body`: name, description, `model == "@rapido"`, `access == ReadOnly`, prompt equals the trimmed body.
  - `role_without_frontmatter_is_an_error` and `load_roles_skips_invalid_files` (one valid, one broken file: 1 role, 1 error).
  - `seed_writes_defaults_once`: first call creates 4 `.md` + `models.toml`; a second call after editing `executor.md` leaves the edit intact.
  - `resolves_first_available_candidate`: catalog without `claude-opus-5-5` resolves `@forte` on `ClaudeCode` to `claude-fable-5-1` / `High`.
  - `no_candidate_names_alias_harness_and_tried`: empty catalog, error text contains `forte`, `claude-code`, `claude-opus-5-5:high`, `claude-fable-5-1:high`.
  - `unknown_alias_is_an_error`.
  - `relative_effort_moves_one_step`: model levels `[Low, Medium, High, Max]`, base `Medium`: `Low → Low`, `High → High`; base `Max` with `High → Max`.
  - `unsupported_level_clamps_down`: levels `[Low, High]`, base `Medium` resolves to `Low`.
  - `model_without_levels_resolves_without_reasoning`.
- [ ] **Step 2:** Run `cargo test --locked -p zeron-engine --lib orchestration`. Expected: FAIL (items not defined).
- [ ] **Step 3:** Implement the items above.
- [ ] **Step 4:** Run the same command. Expected: PASS.
- [ ] **Step 5:** Commit `feat(engine): roles and model map with catalog resolution`.

### Task 3: Orchestrator store

**Files:**
- Create: `crates/engine/src/orchestration/store.rs`
- Modify: `crates/engine/src/orchestration/defaults.rs` (add `DEFAULT_ORCHESTRATOR_PROMPT`, the exact text of spec section 11 as updated)

**Interfaces:**
- Produces:
  - `pub struct OrchestratorConfig { pub id: String, pub name: String, pub avatar: Option<String>, pub harness: HarnessId, pub model: Option<String>, pub reasoning: Option<ReasoningLevel>, pub child_harness: HarnessId, pub projects: Vec<String> }` (serialized as `orchestrator.toml`; `projects` are space ids)
  - `pub struct Store { root: PathBuf }` with `pub fn new(data_dir: &Path) -> Store` (root `{data_dir}/orchestrators`)
  - `create(&self, cfg: &OrchestratorConfig) -> io::Result<()>` writes `orchestrator.toml`, `prompt.md` (default prompt), empty `memoria.md` and `historico.md`
  - `get(&self, id: &str) -> io::Result<OrchestratorConfig>`, `list(&self) -> io::Result<Vec<OrchestratorConfig>>`, `prompt(&self, id: &str) -> io::Result<String>`, `memory(&self, id: &str) -> io::Result<String>`
  - `create_demand(&self, id: &str, title: &str, today: NaiveDate) -> io::Result<String>` returns `"<YYYY-MM-DD>-<slug>"`, slug = lowercase ASCII, non-alphanumerics collapsed to `-`, max 48 chars, `-2`, `-3`... on collision
  - `write_spec(&self, id, demand, text: &str)`, `write_plan(&self, id, demand, items: &[PlanItem])`, `submit_report(&self, id, demand, child_id: &str, text: &str)`, `read_demand(&self, id, demand) -> io::Result<DemandView>`, `close_demand(&self, id, demand, summary: &str, today: NaiveDate)`, `remember(&self, id, note: &str)`
  - `pub enum PlanStatus { Pending, InProgress, Completed }`, `pub struct PlanItem { pub text: String, pub status: PlanStatus, pub role: Option<String>, pub project: Option<String> }`
  - `pub struct DemandView { pub demand: String, pub spec: String, pub plan: String, pub reports: Vec<(String, String)> }`
- `plano.md` rendering, one line per item: `- [ ] text` / `- [~] text` / `- [x] text`, followed by ` · <role>` and ` · <project>` when present.
- `close_demand` moves the folder to `demandas/arquivo/<demand>/` and appends `- <YYYY-MM-DD> · <demand> · <summary>` to `historico.md`. `remember` appends `- <note>`.
- Every `id`, `demand` and `child_id` must match `^[a-z0-9][a-z0-9-]{0,79}$`, else `io::ErrorKind::InvalidInput`.

- [ ] **Step 1: Write failing tests** with a temp dir:
  - `create_writes_config_prompt_and_empty_notes` (prompt equals `DEFAULT_ORCHESTRATOR_PROMPT`).
  - `demand_slug_is_dated_and_unique`: two "Cupom expirado no checkout" demands on 2026-10-24 give `2026-10-24-cupom-expirado-no-checkout` and `...-2`.
  - `plan_renders_status_role_and_project`.
  - `read_demand_returns_spec_plan_and_reports`.
  - `close_moves_to_archive_and_appends_history`.
  - `rejects_traversal_ids`: `"../x"`, `"a/b"`, `"Upper"` each give `InvalidInput` and create nothing outside the root.
- [ ] **Step 2:** Run `cargo test --locked -p zeron-engine --lib orchestration::store`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(engine): orchestrator store for demands, spec, plan and reports`.

### Task 4: Chat config fields and instruction resolution

**Files:**
- Modify: `crates/proto/src/entities.rs:165-174` (`ChatConfig`), plus every `ChatConfig {` literal the compiler reports (about 32): add `role: None, orchestrator: None`
- Modify: `crates/engine/src/workspace_host.rs:1374` (`set_chat_config`)
- Modify: `crates/engine/src/sessions.rs` in `drive_run` before the driver call (next to the `mcp` fill at `:3373`)
- Modify: `crates/engine/src/orchestration/instructions.rs`

**Interfaces:**
- Produces: `ChatConfig.role: Option<String>` and `ChatConfig.orchestrator: Option<String>`, both `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Produces: `pub fn instructions_for(chat: &Chat, parent: Option<&Chat>, data_dir: &Path) -> Option<String>` in `orchestration::instructions`.
- Produces: `pub const CHILD_RULES: &str` = "You are working for a Zeron orchestrator. Your brief names a demand: read it with read_demand before starting. Finish with submit_report: what you did, how you proved it, what is left. If the contract in the spec must change, call request_contract_change instead of changing it."

Rules for `instructions_for`: orchestrator chat → `prompt.md` text, then `"\n\n## Memory\n" + memoria.md` when memory is not empty. Role chat → the role prompt, then `"\n\n" + CHILD_RULES` when `parent` is an orchestrator chat. Otherwise `None`. A missing role file gives `None` and a `tracing::warn!`. `drive_run` sets `request.instructions` from it only when the request has none.

`set_chat_config` merge: when the incoming config has `role` or `orchestrator` as `None` and the stored config has them, keep the stored values.

- [ ] **Step 1: Write failing tests:**
  - `orchestrator_chat_gets_prompt_and_memory`, `role_child_of_orchestrator_gets_role_prompt_and_child_rules`, `role_chat_without_orchestrator_parent_gets_only_role_prompt`, `plain_chat_gets_no_instructions` (unit, temp data dir).
  - `set_chat_config_keeps_role_and_orchestrator` (engine): store a config with `orchestrator: Some("o1")`, set a new config with a different model and `orchestrator: None`, read back `orchestrator == Some("o1")` and the new model.
- [ ] **Step 2:** Run `cargo test --locked -p zeron-engine --lib`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS. Also run `cargo test --locked -p zeron-mcp --lib` (literal updates compile).
- [ ] **Step 5:** Commit `feat(engine): role and orchestrator chat config with instruction resolution`.

### Task 5: Engine RPCs

**Files:**
- Modify: `crates/rpc/src/lib.rs` (`methods` module)
- Modify: `crates/engine/src/rpc.rs` (local `handle` match near `:2389`, and the method group at `:1941` that `GET_TITLE_SETTINGS` belongs to)
- Create: `crates/engine/src/orchestration/rpc.rs` (param structs and handlers)
- Test: `crates/engine/tests/orchestration.rs`

**Interfaces:**
- Produces method names: `ListRoles`, `ResolveRoleModel`, `CreateOrchestrator`, `GetOrchestrator`, `ListOrchestrators`, `OrchCreateDemand`, `OrchWriteSpec`, `OrchWritePlan`, `OrchReadDemand`, `OrchSubmitReport`, `OrchCloseDemand`, `OrchRemember`.
- Params and results (camelCase JSON):
  - `ListRoles {}` → `{ roles: [{name, description, model, access}], errors: [string] }` (seeds defaults first)
  - `ResolveRoleModel { role, harness, effort?, targetDeviceId? }` → `{ model, reasoning, sandbox }` (`sandbox` is `read-only` for `ReadOnly` roles, else `workspace-write`), using the same catalog source as `LIST_MODELS`
  - `CreateOrchestrator { name, avatar?, harness, model?, reasoning?, childHarness?, projects: [spaceId] }` → `{ orchestratorId, chatId }`. Validates every project id against the registry spaces. Creates the store entry and a projectless top-level chat titled `name` with `ChatConfig { harness, model, reasoning, sandbox: ReadOnly, orchestrator: Some(id), role: None, .. }`, using the same code path as the `createChat` mutate op.
  - `GetOrchestrator { orchestratorId }`, `ListOrchestrators {}` → config(s)
  - `OrchCreateDemand { orchestratorId, title }` → `{ demand }`; `OrchWriteSpec { orchestratorId, demand, text }`; `OrchWritePlan { orchestratorId, demand, items }`; `OrchReadDemand { orchestratorId, demand }` → `DemandView`; `OrchSubmitReport { orchestratorId, demand, childId, text }`; `OrchCloseDemand { orchestratorId, demand, summary }`; `OrchRemember { orchestratorId, note }` → `{}`
- Store errors map to `RpcError::Failed(<message>)`.

- [ ] **Step 1: Write failing integration test** `orchestration_rpcs_round_trip` in `tests/orchestration.rs`, following `tests/projectless.rs` setup: create a space, call `CreateOrchestrator` with it, assert the chat row has `config.orchestrator == orchestratorId` and `sandbox == read-only`; create a demand, write spec and plan, submit a report, read the demand back and assert all three; `CreateOrchestrator` with an unknown space id fails with a message containing that id.
- [ ] **Step 2:** Run `cargo test --locked -p zeron-engine --test orchestration`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(engine): orchestration RPCs`.

### Task 6: MCP tools

**Files:**
- Modify: `crates/mcp/src/zeron.rs` (client methods for Task 5 RPCs)
- Create: `crates/mcp/src/orchestration.rs` (tool defs, arg structs, handlers)
- Modify: `crates/mcp/src/tools.rs` (`ToolDef.description` becomes `Cow<'static, str>`; `list` and `has` become `async`; `call` routes the new names; `CreateChatArgs` and `create_chat`)
- Modify: `crates/mcp/src/jsonrpc.rs:184` and the `tools/list` handler (await `list`/`has`); add one paragraph to `INSTRUCTIONS` about roles

**Interfaces:**
- Consumes: Task 5 RPCs.
- `Zeron` gains: `list_roles()`, `resolve_role_model(role, harness, effort, device)`, `get_orchestrator(id)`, and one method per `Orch*` RPC, all `async fn ... -> anyhow::Result<T>`.
- `CreateChatArgs` gains `role: Option<String>`, `effort: Option<String>` (accepts only `low`, `medium`, `high`; the MCP crate does not depend on the engine, so it passes the string to `ResolveRoleModel`), `demand: Option<String>`.
- Tool visibility, computed in `list()` from the origin chat (`resolve_chat(origin)`):
  - Orchestrator chat (`config.orchestrator` is `Some`): `list_roles`, `create_demand`, `write_spec`, `update_plan`, `read_demand`, `close_demand`, `remember`.
  - Child of an orchestrator (parent's `config.orchestrator` is `Some`): `read_demand`, `submit_report`, `request_contract_change`.
  - Everyone else: unchanged list.
- The orchestrator id for child tools comes from the parent's config, never from tool arguments.

Behavior:
- `create_chat` with `role`: resolve harness (explicit arg, else orchestrator's `childHarness`, else current default), then `ResolveRoleModel` for model, reasoning and sandbox. Explicit `model`/`reasoning` args win over the role. The new chat's config carries `role`. With `demand`, the first prompt is prefixed with `Demand: <demand>. Read it with read_demand before starting.\n\n`.
- `create_chat` from an orchestrator: the resolved project's space id must be in the orchestrator's `projects`, else error `Project <name> is not associated with this orchestrator. Associated: <names>.` A projectless child is refused the same way.
- In an orchestrator chat, `create_chat`'s description is extended with `Roles: <name> (<description>); ...` built from `ListRoles`.
- `update_plan` takes `{ demand, items: [{text, status, role?, project?}] }` and calls `OrchWritePlan`.
- `request_contract_change { demand, reason }` queues a message to the parent: `[Contract change requested by <title> (<id>) for demand <demand>]\n\n<reason>`.

- [ ] **Step 1: Write failing tests** with the `World` fake (`tools.rs:1600`), extended to answer the new RPCs:
  - `orchestrator_sees_orchestrator_tools_only_in_orchestrator_chats` (plain chat lacks `write_spec`; orchestrator chat has it; child has `submit_report` but not `write_spec`).
  - `create_chat_with_role_uses_resolved_model_and_records_role`: `writes[0].1["config"]["model"]`, `["role"] == "executor"`, `["sandbox"]` from the resolution.
  - `explicit_model_beats_role_model`.
  - `orchestrator_cannot_create_children_outside_its_projects`: error text contains `is not associated` and the associated project name.
  - `demand_prefix_is_added_to_the_first_prompt`.
  - `create_chat_description_lists_roles_in_orchestrator_chats`.
  - `child_tools_use_the_parents_orchestrator_id`.
- [ ] **Step 2:** Run `cargo test --locked -p zeron-mcp --lib`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again, plus the existing `show_visual_is_offered_only_inside_a_chat` and `delegation_is_one_level_deep`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(mcp): role-aware create_chat and orchestrator tools`.

### Task 7: Wake the orchestrator when a child stops

**Files:**
- Create: `crates/engine/src/orchestration/wake.rs`
- Modify: `crates/engine/src/doc_host.rs` (spawn the watcher next to `spawn_queue_flush_watcher` at `:1245`)
- Test: `crates/engine/tests/orchestration.rs`

**Interfaces:**
- Produces: `pub fn wake_message(child_title: &str, child_id: &str, transition: ChildStop) -> String` and `pub enum ChildStop { Finished, WaitingForAnswer, Failed }`.
- Text: `[Zeron: <title> (<id>) finished its turn. Read it with read_chat.]`, `... is waiting for an answer. Read its question with get_chat.]`, `... stopped with an error. Read it with read_chat.]`.

Watcher rule: subscribe to `SessionsEngine::watch_sessions()`. On a transition from working to idle, awaiting input or error for a chat whose `parent_chat_id` points to a chat with `config.orchestrator` set, call `DocHost::queue_message(parent, wake_message(..), vec![])`. Deduplicate by the child's completed turn id so one turn wakes the parent once.

- [ ] **Step 1: Write failing tests:**
  - Unit: `wake_messages_name_the_child_and_the_next_tool` (three variants).
  - Integration (mock harness, as in `tests/message_queue.rs`): `child_turn_end_queues_one_message_to_orchestrator`; `child_of_plain_chat_wakes_nobody`; `orchestrator_turn_end_wakes_nobody`.
- [ ] **Step 2:** Run `cargo test --locked -p zeron-engine --test orchestration`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(engine): wake the orchestrator when a child stops`.

### Task 8: CLI to create orchestrators and check roles

**Files:**
- Modify: `apps/zeron/src/main.rs` (`Command` enum at `:32`)
- Create: `apps/zeron/src/orchestrator_cli.rs`

**Interfaces:**
- Consumes: Task 5 RPCs over the local IPC client, as `auth_cli.rs`/`update_cli.rs` reach the engine.
- Commands:
  - `zeron orchestrator create --name <NAME> --project <PROJECT>... [--harness <ID>] [--model <ID>] [--reasoning <LEVEL>] [--child-harness <ID>]` prints `Created <name>: chat <chatId>`. `<PROJECT>` accepts a space id, path or name, resolved like the MCP `project` argument.
  - `zeron orchestrator list` prints `<name>  <chatId>  <projects>` per line.
  - `zeron roles check` prints, per role and per connected harness, the resolved `model · reasoning` or the `ResolveError` text, and exits non-zero when any role fails on every harness.

- [ ] **Step 1: Write failing tests:** clap parsing tests `parses_orchestrator_create_with_projects` (two `--project` values) and `parses_roles_check`.
- [ ] **Step 2:** Run `cargo test --locked -p zeron --bin zeron` (adjust to the app's test target). Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(cli): create orchestrators and check roles`.

### Task 9: End-to-end check

- [ ] **Step 1:** Build and start the app from the fork. Run `zeron roles check`. Expected: every default role resolves on Claude Code and Codex, or a clear error names the missing model.
- [ ] **Step 2:** Run `zeron orchestrator create --name Teste --project <two real projects>`. Open the printed chat in the app.
- [ ] **Step 3:** Ask it for a small cross-repo investigation. Expected: it creates a demand, writes spec and plan, starts one `explorador` child per project with the resolved model, is woken when each finishes, and asks for approval before any `executor`.
- [ ] **Step 4:** Confirm the files under `~/.zeron/orchestrators/<id>/demandas/` match what the chat reported. Note anything off as input for Plans 2 and 3.
