#!/usr/bin/env node
// P70.F5 · F6 · F8 (+ G) — the public surface and the release process.
// 2026-09-26: G4 (post-v1.md) / G5 (retrospective-pack.md) retired — both
// documents were archived with the v0 corpus and are re-authored post-v1.
//
// Two kinds of assertion:
//
//   * **presence + substance** — the legal/policy documents and the process
//     documents exist and still carry the sections the rows require;
//   * **the honesty property itself** — the telemetry claim in PRIVACY.md
//     ("no telemetry sender in this build") is checked against the dependency
//     tree, so the sentence cannot outlive its truth.

import { readFileSync, existsSync } from 'node:fs';

const read = (p) => readFileSync(p, 'utf8');
const has = (p) => existsSync(p);
const problems = [];
const fail = (m) => problems.push(m);
const need = (cond, msg) => { if (!cond) fail(msg); };

// ---------------------------------------------------------------- F5 legal
for (const [file, sections] of [
  ['LICENSE', []],
  ['LICENSE-MIT', []],
  ['LICENSE-APACHE', []],
  ['THIRD-PARTY-NOTICES.md', []],
  ['SECURITY.md', ['Reporting a vulnerability', 'The model', 'What is *not* contained']],
  ['PRIVACY.md', ['What leaves the machine', 'What never leaves the machine', 'Telemetry posture']],
  ['CONTRIBUTING.md', ['Running the checks', 'The invariants you must not break']],
]) {
  if (!has(file)) { fail(`F5: ${file} is missing`); continue; }
  const body = read(file);
  for (const s of sections) {
    if (!body.includes(s)) fail(`F5: ${file} lost the "${s}" section`);
  }
}

// The contribution/issue surface the row names.
for (const file of ['.github/ISSUE_TEMPLATE/bug_report.yml', '.github/ISSUE_TEMPLATE/feature_request.yml']) {
  need(has(file), `F5: ${file} is missing`);
}
const bugTemplate = has('.github/ISSUE_TEMPLATE/bug_report.yml') ? read('.github/ISSUE_TEMPLATE/bug_report.yml') : '';
need(/do \*\*not\*\* paste API keys|not included any credential/i.test(bugTemplate),
  'F5: the bug template no longer warns against pasting credentials');

// The licence declared in the bundle metadata must be one of the shipped ones.
const conf = JSON.parse(read('src-tauri/tauri.conf.json'));
const declared = conf.bundle?.license ?? '';
need(/MIT/.test(declared), `F5: bundle licence "${declared}" no longer names MIT`);

// ---------------------------------------------------------------- F6 telemetry
// The privacy statement says there is no telemetry sender. Enforce it against
// the dependency trees rather than trusting the sentence.
//
// "Sender" is the precise word: an *exporter or SDK* that can transmit is a
// sender; a wire-format type crate is not. `agentcowork-core/src/tracing.rs`
// depends on bare `opentelemetry` for the W3C `traceparent` types (TraceId /
// SpanId / SpanContext) and reports to console + a local log file — OTLP is
// post-v1 (SPEC J14). That distinction is enforced below instead of waived.
const TELEMETRY =
  /(^|["'\s/])(posthog|mixpanel|amplitude|segment-analytics|@segment\/analytics|sentry|@sentry\/[a-z-]+|datadog|dd-trace|newrelic|new-relic|app-insights|applicationinsights|opentelemetry-otlp|opentelemetry_sdk|opentelemetry-sdk|opentelemetry-exporter[a-z-]*|@opentelemetry\/(sdk-[a-z]+|exporter-[a-z]+)|google-analytics|gtag|matomo|plausible|umami|heap|fullstory|logrocket|bugsnag|rollbar|honeybadger|trackjs|cloudflare-insights|statsig|launchdarkly|firebase-analytics|@firebase\/analytics|sendgrid-analytics)(["'\s/@:]|$)/i;
for (const lock of ['pnpm-lock.yaml', 'crates/Cargo.lock', 'src-tauri/Cargo.lock']) {
  if (!has(lock)) continue;
  const body = read(lock);
  const hit = TELEMETRY.exec(body);
  need(!hit, `F6: ${lock} contains a telemetry/analytics sender (${hit?.[0]?.trim()}) — PRIVACY.md claims none exists`);
}
// The one allowed exception must stay an exception: the bare wire-format crate
// only, never an exporter that could turn it into a sender.
const tracingRs = read('crates/agentcowork-core/src/tracing.rs');
need(tracingRs.includes('opentelemetry::trace::{'),
  'F6: tracing.rs no longer uses the opentelemetry wire-format types (if the dependency was dropped, tighten this gate)');
need(/TraceReporter exports|console \+ log file/i.test(tracingRs),
  'F6: tracing.rs no longer documents its local-only export target');
const privacy = has('PRIVACY.md') ? read('PRIVACY.md') : '';
need(/No analytics\/telemetry SDK|no telemetry sender/i.test(privacy),
  'F6: PRIVACY.md no longer states the telemetry posture');
// The in-app posture must match: Settings → Privacy renders a disabled switch.
need(read('ui/src/components/panels/settings-sections-extra.tsx').includes('No telemetry sender exists in this build'),
  'F6: the in-app Privacy section no longer states the no-telemetry posture');

// ---------------------------------------------------------------- F8 checklist
const checklist = has('docs/release/launch-checklist.md') ? read('docs/release/launch-checklist.md') : '';
need(!!checklist, 'F8: docs/release/launch-checklist.md is missing');
for (const item of ['Machine-checked', 'Human, on a real host', 'release-qualify.mjs']) {
  need(checklist.includes(item), `F8: the launch checklist lost "${item}"`);
}

// ---------------------------------------------------------------- G processes
for (const [file, sections] of [
  ['docs/release/rollout-and-hotfix.md', ['Rollout monitoring', 'Hotfix process', 'Patch cadence and deprecation policy']],
]) {
  if (!has(file)) { fail(`G: ${file} is missing`); continue; }
  const body = read(file);
  for (const s of sections) {
    if (!body.includes(s)) fail(`G: ${file} lost the "${s}" section`);
  }
}

// ---------------------------------------------------------------- result
if (problems.length) {
  console.error('check-public-surface: FAILED');
  for (const p of problems) console.error(`  ✗ ${p}`);
  process.exit(1);
}
console.log('check-public-surface: ok (legal/policy docs, telemetry posture, launch checklist + release process)');
