# 37 — Workflow, skill and plugin lifecycle

> Status: accepted target architecture 2026-09-28 under DEC-054; implementation pending. Detailed execution remains in `20-WORKFLOW`; skill/plugin manifests remain in `31-SKILLS-PLUGINS`.

## Three distinct objects

Mission is an adaptive outcome and plan graph. Workflow is a version-pinned executable process with triggers, typed nodes, waits, retries and receipts. Skill is portable procedural guidance about how to accomplish a class of tasks. A skill can call a workflow; a workflow can invoke an agent task with a skill; neither becomes a permission grant. Plugin is a packaging/dependency and distribution object, not an all-powerful runtime. One user goal may combine all three without duplicating their state stores.

## Capture and promotion

“Record and replay” starts with an opt-in, privacy-scoped capture of a human demonstration or successful agent execution. Store a bounded event/action trace and referenced artifacts; redact secrets, tokens, personal data not needed for replay and native agent private context. The candidate generator separates deterministic steps (typed workflow nodes) from adaptive decisions (skill instructions). It emits a draft workflow and/or `SKILL.md`, declared capabilities, input/output schema, preconditions, side effects, permission scopes, failure/rollback notes, provenance and fixtures. A workflow-to-skill conversion writes a concise procedure that invokes the pinned workflow for deterministic work and describes when the agent should intervene; it does not paste raw click logs into a skill.

Promotion states: captured → candidate → reviewed → dry_run → evaluated → published → deprecated. Dry-run/replay uses non-production accounts or mocks where needed. Evals measure outcome, side effects, brittleness and recovery on representative examples. A user reviews connector grants and consequential effects before publish. Published versions are immutable and rollbackable. Repeated corrections propose a new version; there is no live self-edit of installed trusted skill/plugin/workflow behavior. A workflow export/import preserves typed semantics, declares unsupported nodes, and fails explicitly on partial conversion. Native agent skill imports are copies by explicit request, not transparent mutation of native directories.

## Workflow providers and connector reuse

AgentCowork's embedded workflow supports local deterministic orchestration and host governance. External n8n/Activepieces definitions remain in those systems. `ExternalWorkflowAdapter` has discover/schema, invoke(version,input,idempotency), status, cancel, callback verification and result/evidence retrieval. A host Work item records provider run ID, version, origin, grant, callback nonce and terminal result; the external provider owns its internal node execution and credential store. Provider callbacks are authenticated and idempotently reconciled, including resume-before-pause races. Explicit connector/MCP bridges let a host task call a service without copying an entire marketplace. For local/cloud continuity, trigger ownership must be singular: either host schedule/event subscription or external provider trigger; never both for one logical automation.

## Failure cases

Reject a captured procedure whose inputs contain unrecoverable secrets, whose permissions cannot be scoped, whose effects cannot be observed, or whose site/app state is too unstable for safe replay. Mark adaptive steps as guidance and keep human takeover. If an external workflow is unavailable, preserve host Work in `waiting_dependency`; do not fabricate completion. If a published skill updates while an agent is running, pin the old version for that Work and use the new version only on new activation.
