#!/usr/bin/env node
// P71.9i — prompt-steering agreement gate.
//
// The tool-affinity block is a published contract (`ARCH/15-AGENT-PLANE.md`
// §7, the v1 owner of the engine/delegation contract after DEC-052 retired
// the v0 `ARCH/EXTERNAL-AGENTS.md` Agent X document) AND a shipped string
// (`crates/agentcowork-acp/src/chief.rs`). This gate keeps them one thing:
// the facade names and their order must match, the shell must route through
// the steering-aware builder, and the class of bug that shipped once —
// literal `\n` escapes instead of newlines — cannot come back.
//
// NOTE (2026-09-27): the v1 agent-plane document does not yet publish the
// affinity block's facade list, so the doc-side asserts below fail until the
// ARCH owner publishes it in §7. The code-side asserts (order, newlines,
// builder routing, regression test) still hold. Do not "fix" this gate by
// dropping the doc-side asserts — publish the block instead.

import { readFileSync } from 'node:fs';

const read = (p) => readFileSync(p, 'utf8');
const problems = [];
const fail = (m) => problems.push(m);
const need = (c, m) => { if (!c) fail(m); };

const doc = read('ARCH/15-AGENT-PLANE.md');
const chief = read('crates/agentcowork-acp/src/chief.rs');
const shell = read('src-tauri/src/acp_cmds.rs');

// 1. The doc still carries the section, with its four steering rules.
need(doc.includes('## 7. Delegation & subagents'), 'ARCH/15-AGENT-PLANE.md §7 lost its heading');
for (const facade of ['office.', 'browser.', 'computer_use.', 'delegate.spawn']) {
  need(doc.includes(facade), `ARCH/15-AGENT-PLANE.md §7 no longer names \`${facade}\``);
  need(chief.includes(facade), `COWORK_AFFINITY_STEERING no longer names \`${facade}\``);
}

// 2. Order agreement: the doc's published rules must run office → browser →
//    desktop → delegation, and the shipped constant must list them in the same
//    order — "tool ranking hierarchy" is the point of the block, so a
//    reordering is a contract change, not a formatting choice.
const orderInDoc = ['office.', 'browser.', 'computer_use.', 'delegate.spawn'].map((f) => {
  const i = doc.indexOf(f, doc.indexOf('## 7. Delegation & subagents'));
  return [f, i];
});
const orderInCode = ['office.', 'browser.', 'computer_use.', 'delegate.spawn'].map((f) => [
  f,
  chief.indexOf(f, chief.indexOf('COWORK_AFFINITY_STEERING')),
]);
const sorted = (pairs) => [...pairs].sort((a, b) => a[1] - b[1]).map(([f]) => f);
need(
  JSON.stringify(sorted(orderInDoc)) === JSON.stringify(sorted(orderInCode)),
  `the affinity block's facade order differs between the doc (${sorted(orderInDoc).join(' → ')}) and the shipped constant (${sorted(orderInCode).join(' → ')})`,
);
need(
  JSON.stringify(sorted(orderInCode)) === JSON.stringify(['office.', 'browser.', 'computer_use.', 'delegate.spawn']),
  'the shipped affinity block changed its ranking order',
);

// 3. The shipped string must use real newlines, not `\n` escapes.
const constStart = chief.indexOf('pub const COWORK_AFFINITY_STEERING');
need(constStart > 0, 'COWORK_AFFINITY_STEERING is gone');
const constBody = chief.slice(constStart, chief.indexOf(';', constStart));
need(!constBody.includes('\\\\n'), 'COWORK_AFFINITY_STEERING contains literal \\\\n escapes');

// 4. The shell must build the prompt through the steering-aware builder — the
//    regression this row fixes was a call site appending its own block after
//    the user turn, with literal escapes.
need(
  shell.includes('build_chief_prompt_with_steering'),
  'acp_cmds.rs no longer routes through build_chief_prompt_with_steering',
);
need(
  shell.includes('COWORK_AFFINITY_STEERING'),
  'acp_cmds.rs no longer passes the affinity steering block',
);
need(
  !/push_str\("\\\\\\\\n/.test(shell),
  'acp_cmds.rs is appending prompt text with literal backslash-n escapes again',
);

// 5. The order itself is pinned by a unit test (belt and braces: a test can be
//    deleted, but not silently).
need(
  /fn p71_9i_steering_blocks_follow_the_documented_order/.test(chief),
  'the order/newline regression test is gone (crates/agentcowork-acp/src/chief.rs)',
);

if (problems.length) {
  console.error('check-prompt-steering: FAILED');
  for (const p of problems) console.error(`  ✗ ${p}`);
  process.exit(1);
}
console.log('check-prompt-steering: ok (doc ↔ shipped affinity block agree, in order, with real newlines)');
