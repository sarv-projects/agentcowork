#!/usr/bin/env node
// OpenCode Zen free-model probe (P56.6) — KEYLESS.
//
// Free ids do not need OPENCODE_API_KEY. The gate is x-opencode-session.
// Do not send Authorization on the free path (unrecognized Bearer 401s).
//
// Usage:
//   node desktop_app/scripts/zen-free-probe.mjs
//   node desktop_app/scripts/zen-free-probe.mjs mimo-v2.5-free

const MODEL = process.argv[2] || "ling-3.0-flash-fin-free";
const SESSION =
  process.env.OPENCODE_SESSION || `ses_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`;
const UA = "AgentCowork/0.7.2";

const body = JSON.stringify({
  model: MODEL,
  messages: [{ role: "user", content: "Reply with the single word pong." }],
  max_tokens: 32,
  stream: false,
});

const headers = {
  "content-type": "application/json",
  "user-agent": UA,
  "x-opencode-session": SESSION,
  "x-opencode-request": `msg-${Date.now().toString(36)}`,
  "x-opencode-client": "desktop",
};
if (process.env.OPENCODE_API_KEY) {
  headers.authorization = `Bearer ${process.env.OPENCODE_API_KEY}`;
}

const res = await fetch("https://opencode.ai/zen/v1/chat/completions", {
  method: "POST",
  headers,
  body,
});

const text = await res.text();
let parsed;
try {
  parsed = JSON.parse(text);
} catch {
  parsed = { raw: text.slice(0, 400) };
}

const content =
  parsed?.choices?.[0]?.message?.content ??
  parsed?.error?.message ??
  parsed?.error?.type ??
  "";

console.log(
  JSON.stringify(
    {
      ok: res.ok,
      status: res.status,
      model: MODEL,
      session: SESSION,
      userAgent: UA,
      finish: parsed?.choices?.[0]?.finish_reason ?? null,
      usage: parsed?.usage ?? null,
      content: typeof content === "string" ? content.slice(0, 240) : content,
      error: parsed?.error ?? null,
    },
    null,
    2,
  ),
);

process.exit(res.ok ? 0 : 1);
