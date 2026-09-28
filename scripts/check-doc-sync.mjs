#!/usr/bin/env node
// Check the live requirement → matrix → task chain. Historical v0 files are
// deliberately outside this gate; ARCH/00-INDEX.md defines current authority.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(join(root, path), "utf8");
const failures = [];

const requirementsText = read("ARCH/08-REQUIREMENTS.md");
const matrixText = read("ARCH/09-FEATURE-MATRIX.md");
const todoText = read("TODO.md");
const indexText = read("ARCH/00-INDEX.md");
const specText = read("AGENTCOWORK-SPEC.md");

const requirementMatches = [
  ...requirementsText.matchAll(/^#### (REQ-[A-Z]+-\d{3})\b[^\n]*$/gm),
];
const requirementIds = requirementMatches.map((match) => match[1]);
const requirementLineById = new Map(
  requirementMatches.map((match) => [match[1], requirementsText.slice(0, match.index).split("\n").length]),
);
const matrixRows = matrixText.split("\n").filter((line) => /^\| `REQ-[A-Z]+-\d{3}` \|/.test(line));
const matrixIds = matrixRows.map((row) => /^\| `(REQ-[A-Z]+-\d{3})`/.exec(row)[1]);

function duplicates(ids) {
  const seen = new Set();
  const repeated = new Set();
  for (const id of ids) {
    if (seen.has(id)) repeated.add(id);
    seen.add(id);
  }
  return [...repeated];
}

for (const [label, ids] of [
  ["ARCH/08 requirement headings", requirementIds],
  ["ARCH/09 matrix rows", matrixIds],
]) {
  const repeated = duplicates(ids);
  if (repeated.length) failures.push(`${label} contain duplicate IDs: ${repeated.join(", ")}`);
}

const requirementSet = new Set(requirementIds);
const matrixSet = new Set(matrixIds);
const missingRows = requirementIds.filter((id) => !matrixSet.has(id));
const orphanRows = matrixIds.filter((id) => !requirementSet.has(id));
if (missingRows.length) failures.push(`requirements absent from matrix: ${missingRows.join(", ")}`);
if (orphanRows.length) failures.push(`matrix rows without requirements: ${orphanRows.join(", ")}`);

const requiredFields = ["Statement", "Priority", "Source", "Acceptance", "Failure cases", "Tests", "Status"];
for (let index = 0; index < requirementMatches.length; index++) {
  const match = requirementMatches[index];
  const end = requirementMatches[index + 1]?.index ?? requirementsText.length;
  const block = requirementsText.slice(match.index, end).split(/^## /m, 1)[0];
  const missing = requiredFields.filter((field) => !block.includes(`- **${field}:**`));
  if (missing.length) failures.push(`${match[1]} lacks ${missing.join(", ")}`);
}

// A matrix reference must resolve to a task in the live TODO, including a
// deliberate retirement tombstone. This checks existence, not completion.
const taskRefs = new Set(matrixRows.flatMap((row) => [...row.matchAll(/TASK-[A-Z]+-\d{3}/g)].map((match) => match[0])));
const missingTasks = [...taskRefs].filter((id) => !todoText.includes(id));
if (missingTasks.length) failures.push(`matrix task references absent from TODO.md: ${missingTasks.join(", ")}`);

// Active TODO rows promise exact navigation to the first referenced requirement.
// History is explicitly archival and may retain its original line snapshots.
const activeTodo = todoText.split(/^## History\s*$/m, 1)[0];
for (const [index, line] of activeTodo.split("\n").entries()) {
  const pathRef = /ARCH\/08-REQUIREMENTS\.md:(\d+)/.exec(line);
  if (!pathRef) continue;
  const firstRequirement = /REQ-[A-Z]+-\d{3}/.exec(line)?.[0];
  const expectedLine = firstRequirement && requirementLineById.get(firstRequirement);
  if (!expectedLine) {
    failures.push(`TODO.md:${index + 1} has a requirement line reference without a live requirement ID`);
  } else if (Number(pathRef[1]) !== expectedLine) {
    failures.push(
      `TODO.md:${index + 1} points to ARCH/08 line ${pathRef[1]} for ${firstRequirement}; current heading is line ${expectedLine}`,
    );
  }
}

for (const wave of ["W0", "W1", "W2", "W3", "W4", "W5", "W6"]) {
  if (!new RegExp(`^### ${wave}\\b`, "m").test(todoText)) failures.push(`TODO.md lacks ${wave} delivery wave`);
}

if (!indexText.includes("AGENTCOWORK-SPEC.md") || !indexText.includes("ARCH/08-REQUIREMENTS.md")) {
  failures.push("ARCH/00-INDEX.md does not identify the live spec and requirement authority");
}
if (!specText.includes("ARCH/08-REQUIREMENTS.md")) {
  failures.push("AGENTCOWORK-SPEC.md does not link to the live requirement registry");
}

const matrixCountClaim = /Current inventory[^\n]*?(\d+) unique requirement rows/.exec(matrixText)?.[1];
if (matrixCountClaim && Number(matrixCountClaim) !== matrixIds.length) {
  failures.push(`ARCH/09 claims ${matrixCountClaim} rows; found ${matrixIds.length}`);
}
const indexCountClaim = /each contain (\d+) active requirements\/rows/.exec(indexText)?.[1];
if (indexCountClaim && Number(indexCountClaim) !== requirementIds.length) {
  failures.push(`ARCH/00 claims ${indexCountClaim} requirements; found ${requirementIds.length}`);
}

if (failures.length) {
  console.error("check-doc-sync: failed");
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exitCode = 1;
} else {
  console.log(
    `check-doc-sync: ok — ${requirementIds.length} requirements, ${matrixIds.length} matrix rows, ` +
      `${taskRefs.size} referenced task IDs; live authority chain present`,
  );
}
