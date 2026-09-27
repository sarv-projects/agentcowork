// P4.2 — Excel bridge (D2): windowed sheet reads over the Rust calamine
// reader. Mirrors agentcowork-office/src/xlsx/read.rs types. In a plain-browser
// preview (no shell) the page falls back to a 100K-row demo grid so the
// virtualization is explorable.

import { invoke } from "./tauri";
import { nativeCall } from './runtime';

export type CellValue =
  | { Empty: null }
  | { Number: number }
  | { Text: string }
  | { Bool: boolean }
  | { Error: string };

export interface SheetMeta {
  name: string;
  rows: number;
  cols: number;
}

export interface SheetWindow {
  sheet: string;
  offset: number;
  total_rows: number;
  total_cols: number;
  rows: CellValue[][];
}

export interface XlsxWindowPayload extends SheetWindow {
  path: string;
  sheets: SheetMeta[];
}

/** IronCalc recalc (D2 truth engine): every engine-computed value + the
 * number of formula cells evaluated. "LLM never invents a number." */
export interface RecalcCell {
  row: number;
  col: number;
  value: CellValue;
}

export interface SheetValues {
  name: string;
  cells: RecalcCell[];
}

export interface RecalcResult {
  sheets: SheetValues[];
  formula_cells: number;
}

export async function xlsxRecalc(path: string): Promise<RecalcResult> {
  return nativeCall('spreadsheet recalculate', () => invoke<RecalcResult>("xlsx_recalc", { path }));
}

/** P4.7 — Guard-2 cell-edit split (plan-before-touch). Ticket-every-effect:
 * both `allow` and `ask` carry a single-use ticket the commit must consume. */
export interface XlsxEditRequest {
  action: "allow" | "ask";
  address: string;
  value: string;
  ticketId: string;
  approvalNonce: string;
}

export async function xlsxEditRequest(
  path: string,
  sheet: string,
  address: string,
  value: string,
): Promise<XlsxEditRequest> {
  return nativeCall('spreadsheet edit request', () => invoke<XlsxEditRequest>("xlsx_edit_request", { path, sheet, address, value }));
}

export async function xlsxEditCommit(
  path: string,
  sheet: string,
  address: string,
  value: string,
  ticketId: string,
): Promise<{ address: string; sheet: string; changedParts: string[] }> {
  return nativeCall('spreadsheet edit commit', () => invoke("xlsx_edit_commit", { path, sheet, address, value, ticketId }));
}

// ---------------------------------------------------------------------------
// P4.7 bulk edit + pivot (D2 DSL): FillRange / SortRange / ClearRange batches
// go through the same Guard-2 plan-before-touch split as single-cell edits;
// pivot is read-only (no ticket).
// ---------------------------------------------------------------------------

export type Scalar = { Number: number } | { Text: string } | { Bool: boolean };
export type CellRef = { row: number; col: number };
export type RangeRef = { start: CellRef; end: CellRef };

export type XlsxOperation =
  | { SetCell: { address: CellRef; value: Scalar } }
  | { FillRange: { range: RangeRef; mode: "Constant" | "CopyDown"; value: Scalar | null } }
  | { SortRange: { range: RangeRef; by_col: number; desc: boolean } }
  | { ClearRange: { range: RangeRef } }
  | {
      Shift: {
        sheet: string;
        kind: "InsertRow" | "DeleteRow" | "InsertCol" | "DeleteCol";
        at: number;
        count: number;
      };
    };

export interface WorkbookBatch {
  dsl_version: number;
  transaction_id: string;
  base_revision: number;
  summary: string;
  operations: XlsxOperation[];
}

export function newBatch(summary: string, ops: XlsxOperation[]): WorkbookBatch {
  return {
    dsl_version: 1,
    transaction_id: `txn-ui-${Date.now()}-${Math.floor(Math.random() * 1e6)}`,
    base_revision: 0,
    summary,
    operations: ops,
  };
}

export function scalar(value: string): Scalar {
  const t = value.trim();
  if (t.toLowerCase() === "true") return { Bool: true };
  if (t.toLowerCase() === "false") return { Bool: false };
  const n = Number(t);
  if (t !== "" && Number.isFinite(n)) return { Number: n };
  return { Text: value };
}

function parseCellRef(s: string): CellRef | null {
  const m = /^\$?([A-Za-z]+)\$?([1-9][0-9]*)$/.exec(s.trim());
  if (!m) return null;
  let col = 0;
  for (const ch of m[1].toUpperCase()) col = col * 26 + (ch.charCodeAt(0) - 64);
  return { row: Number(m[2]), col };
}

export function parseRangeRef(s: string): RangeRef | null {
  const [a, b] = s.split(":");
  const start = parseCellRef(a);
  const end = b ? parseCellRef(b) : start;
  if (!start || !end) return null;
  return { start, end };
}

export interface XlsxBatchRequest {
  action: "allow" | "ask";
  summary: string;
  ticketId: string;
  approvalNonce: string;
}

export async function xlsxBatchRequest(
  path: string,
  sheet: string,
  batch: WorkbookBatch,
): Promise<XlsxBatchRequest> {
  return nativeCall('spreadsheet batch request', () => invoke<XlsxBatchRequest>("xlsx_batch_request", { path, sheet, batch }));
}

export async function xlsxBatchCommit(
  path: string,
  sheet: string,
  batch: WorkbookBatch,
  ticketId: string,
): Promise<{ summary: string; sheet: string; changedParts: string[] }> {
  return nativeCall('spreadsheet batch commit', () => invoke("xlsx_batch_commit", { path, sheet, batch, ticketId }));
}

export interface PivotRow {
  key: string;
  value: number;
  count: number;
}

export async function xlsxPivot(
  path: string,
  sheet: string,
  source: string,
  groupBy: number,
  aggregate: number,
  agg: "sum" | "count" | "avg",
): Promise<PivotRow[]> {
  return nativeCall('spreadsheet pivot', () => invoke<PivotRow[]>("xlsx_pivot", {
    path,
    sheet,
    source,
    groupBy,
    aggregate,
    agg,
  }));
}

/** Read one windowed slice of a sheet from a workbook path. */
export async function xlsxOpen(
  path: string,
  sheet: string | null,
  offset: number,
  limit: number,
): Promise<XlsxWindowPayload> {
  return nativeCall('spreadsheet open', () => invoke<XlsxWindowPayload>("xlsx_open", {
    path,
    sheet,
    offset,
    limit,
  }));
}

export function cellDisplay(v: CellValue | undefined): string {
  if (!v) return "";
  if ("Empty" in v) return "";
  if ("Number" in v) {
    const n = v.Number as number;
    if (Number.isInteger(n) && Math.abs(n) < 1e15) return String(n);
    return String(n);
  }
  if ("Text" in v) return v.Text as string;
  if ("Bool" in v) return String(v.Bool);
  return `#${v.Error}`;
}

/** Column letter for a 1-based index (1→A, 27→AA). */
export function colLetter(idx: number): string {
  let n = idx;
  let out = "";
  while (n > 0) {
    const rem = (n - 1) % 26;
    out = String.fromCharCode(65 + rem) + out;
    n = Math.floor((n - 1) / 26);
  }
  return out;
}

// ---------------------------------------------------------------------------
// demo fallback (plain-browser preview): 100K rows × 6 cols, virtualized
// ---------------------------------------------------------------------------

export function demoRow(row: number): CellValue[] {
  return [
    { Number: row + 1 },
    { Text: `Region ${String.fromCharCode(65 + (row % 5))}` },
    { Text: row % 7 === 0 ? "Quarterly" : "Monthly" },
    { Number: Math.round(((row * 7919) % 9973) * 10) / 10 },
    { Number: Math.round(((row * 104729) % 987654) * 100) / 100 },
    { Bool: row % 3 === 0 },
  ];
}
