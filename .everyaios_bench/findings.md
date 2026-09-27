# Research Findings: ACP vs MCP Architecture

## Sources
- crates/everyaios-acp/src/registry.rs:185-235
- crates/everyaios-guard/src/netfloor.rs:40-110
- crates/everyaios-office/src/xlsx/recalc.rs:20-80

## Synthesis
1. Agent Client Protocol (ACP) drives external top-brain coding agents (OpenCode, Grok Build, Codex) over stdio JSON-RPC.
2. Model Context Protocol (MCP) acts as an external tool façade exposing EveryAIOS capabilities to outside tools.
3. Guard-2 mediates all file/network effects with cryptographic tickets; netfloor enforces SSRF blocking against RFC1918.
4. IronCalc 0.8.3 serves as the spreadsheet recalculation truth engine, ensuring numeric claims never hallucinate.
