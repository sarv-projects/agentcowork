// Calendar & AI Event scheduling client bridge (Open WebUI parity & cowork automations).
// Thin wrappers over Tauri calendar_* commands with vault persistence.

import { invoke } from "./tauri";
import { bridgeCall } from "./runtime";

/** Wire shape owned by agentcowork-vault::CalendarRow. */
export interface CalendarRow {
  id: string;
  name: string;
  color: string;
  visible: boolean;
  created_at: number;
  updated_at: number;
}

/** Wire shape owned by agentcowork-vault::CalendarEventRow. */
export interface CalendarEventRow {
  id: string;
  calendar_id: string;
  title: string;
  description: string;
  start_time: number;
  end_time: number;
  all_day: boolean;
  rrule: string | null;
  automation_id: string | null;
  created_at: number;
  updated_at: number;
}

// Preview-only fixtures. Native shell reads always come from SQLCipher and
// return an empty list for a fresh vault; these values are never persisted.
const DEMO_CALENDARS: CalendarRow[] = [
  {
    id: "cal-preview",
    name: "Preview calendar",
    color: "blue",
    visible: true,
    created_at: 0,
    updated_at: 0,
  },
];

const DEMO_EVENTS: CalendarEventRow[] = [
  {
    id: "evt-preview",
    calendar_id: "cal-preview",
    title: "Preview event",
    description: "Preview-only event; not native runtime data.",
    start_time: 0,
    end_time: 3600,
    all_day: false,
    rrule: null,
    automation_id: null,
    created_at: 0,
    updated_at: 0,
  },
];

export async function calendarList(): Promise<CalendarRow[]> {
  return bridgeCall({
    operation: "calendar_list",
    live: async () => {
      const out = (await invoke("calendar_list")) as { calendars?: CalendarRow[] };
      return out.calendars ?? [];
    },
    preview: () => Promise.resolve(DEMO_CALENDARS),
  });
}

export async function calendarPut(calendar: CalendarRow): Promise<boolean> {
  return bridgeCall({
    operation: "calendar_put",
    live: async () => {
      return (await invoke("calendar_put", { calendar })) as boolean;
    },
    preview: () => Promise.resolve(true),
  });
}

export async function calendarDelete(id: string): Promise<boolean> {
  return bridgeCall({
    operation: "calendar_delete",
    live: async () => {
      return (await invoke("calendar_delete", { id })) as boolean;
    },
    preview: () => Promise.resolve(true),
  });
}

/** Exact argument names accepted by the native `calendar_event_list` command. */
export function calendarEventListArgs(
  calendarId?: string,
  startTs?: number,
  endTs?: number,
): Record<string, string | number> {
  const args: Record<string, string | number> = {};
  if (calendarId !== undefined) args.calendar_id = calendarId;
  if (startTs !== undefined) args.start_ts = startTs;
  if (endTs !== undefined) args.end_ts = endTs;
  return args;
}

export async function calendarEventList(
  calendarId?: string,
  startTs?: number,
  endTs?: number,
): Promise<CalendarEventRow[]> {
  return bridgeCall({
    operation: "calendar_event_list",
    live: async () => {
      const out = (await invoke(
        "calendar_event_list",
        calendarEventListArgs(calendarId, startTs, endTs),
      )) as {
        events?: CalendarEventRow[];
      };
      return out.events ?? [];
    },
    preview: () => Promise.resolve(DEMO_EVENTS),
  });
}

export async function calendarEventPut(event: CalendarEventRow): Promise<boolean> {
  return bridgeCall({
    operation: "calendar_event_put",
    live: async () => {
      return (await invoke("calendar_event_put", { event })) as boolean;
    },
    preview: () => Promise.resolve(true),
  });
}

export async function calendarEventDelete(id: string): Promise<boolean> {
  return bridgeCall({
    operation: "calendar_event_delete",
    live: async () => {
      return (await invoke("calendar_event_delete", { id })) as boolean;
    },
    preview: () => Promise.resolve(true),
  });
}
