// AgentCowork cockpit — one project, one session, one ticket, one timeline.
// Left = which job · center = talk + now-doing + approve · right = one lens.
// Never 9 peer tabs; never Chat/Cowork/Code as three apps.

import { useEffect, useRef, useState } from "react";
import { Menu } from "lucide-react";
import { TitleBar } from "@/components/shell/title-bar";
import { LeftSidebar } from "@/components/shell/left-sidebar";
import { CenterColumn } from "@/components/shell/center-column";
import { ActivityRail, RightViewport } from "@/components/shell/right-rail";
import { StatusBar } from "@/components/shell/status-bar";
import { CommandPalette } from "@/components/shell/command-palette";
import { CockpitSlideover } from "@/components/shell/cockpit-slideover";
import { KeyboardShortcuts } from "@/components/shell/keyboard-shortcuts";
import { AiPointer } from "@/components/shell/ai-pointer";
import { ToastBridge } from "@/components/shell/toast-bridge";
import VaultGate from "@/components/shell/vault-gate";
import { SetupGate } from "@/components/shell/setup-gate";
import { useAppStore } from "@/lib/store";
import { OnboardingModal } from "@/components/onboarding-modal";
import NpsPrompt from "@/components/nps-prompt";
import { startPerfMeasurement } from "@/lib/perf";
import { recordSessionEvent } from "@/lib/session-recording";
import { RuntimeStatusBanner } from "@/components/shell/runtime-status-banner";
import { cn } from "@/lib/utils";

export const NARROW_SHELL_BREAKPOINT = 900

export function isNarrowShellWidth(width: number): boolean {
  return width > 0 && width < NARROW_SHELL_BREAKPOINT
}

function useNarrowShell(): boolean {
  const [narrow, setNarrow] = useState(() =>
    typeof window === "undefined"
      ? false
      : isNarrowShellWidth(window.innerWidth),
  )

  useEffect(() => {
    if (typeof window === "undefined") return
    const update = () => setNarrow(isNarrowShellWidth(window.innerWidth))
    update()
    window.addEventListener("resize", update)
    return () => window.removeEventListener("resize", update)
  }, [])

  return narrow
}

/** One visible owner for first-run: onboarding first, setup only afterward. */
export function FirstRunSurfaces() {
  const onboardingDone = useAppStore((s) => s.onboardingDone)
  return onboardingDone ? <SetupGate /> : <OnboardingModal />
}

export default function App() {
  const powerMode = useAppStore((s) => s.powerMode);
  const cockpitOpen = useAppStore((s) => s.cockpitOpen);
  const setCockpitOpen = useAppStore((s) => s.setCockpitOpen);
  const sidebarCollapsed = useAppStore((s) => s.sidebarCollapsed);
  const narrow = useNarrowShell();
  const [narrowSidebarOpen, setNarrowSidebarOpen] = useState(false);
  const previousNarrow = useRef<boolean | null>(null);

  // The title bar remains the desktop sidebar owner. On a narrow window its
  // existing collapse control also opens/closes the drawer, while the first
  // render starts closed so a small window does not flash a full navigation
  // sheet over the first task.
  useEffect(() => {
    if (previousNarrow.current === null) {
      previousNarrow.current = narrow;
      setNarrowSidebarOpen(false);
      return;
    }
    if (previousNarrow.current !== narrow) {
      previousNarrow.current = narrow;
      setNarrowSidebarOpen(false);
      return;
    }
    if (!narrow) {
      setNarrowSidebarOpen(false);
      return;
    }
    setNarrowSidebarOpen(!sidebarCollapsed);
  }, [narrow, sidebarCollapsed]);

  // P11.4 — kick off LCP/TTI measurement at boot.
  useEffect(() => {
    startPerfMeasurement();
  }, []);

  // P11.6.5 — opt-in session recording: a single delegated listener records
  // clicks/navigation (content-free element identity only) when enabled.
  useEffect(() => {
    const onClick = (e: MouseEvent) => recordSessionEvent("click", e.target);
    document.addEventListener("click", onClick);
    return () => document.removeEventListener("click", onClick);
  }, []);

  // P44.5 — reconcile the composer autonomy level with the Rust GuardService
  // preset at boot (the applied preset wins over stale localStorage).
  useEffect(() => {
    void useAppStore.getState().syncAutonomyFromRust();
  }, []);

  return (
    <VaultGate>
      <div className="relative flex h-screen w-screen flex-col overflow-hidden bg-background">
        <KeyboardShortcuts />
        <TitleBar />
        <RuntimeStatusBanner />
        <main className={cn("flex min-h-0 min-w-0 flex-1 overflow-hidden", narrow ? "flex-col" : "flex-row")}>
          <LeftSidebar
            narrow={narrow}
            drawerOpen={narrow && narrowSidebarOpen}
            onDrawerOpenChange={setNarrowSidebarOpen}
          />
          <CenterColumn />
          {/* Power mode intentionally reveals the cockpit rail and active lens. */}
          {powerMode && <ActivityRail narrow={narrow} />}
          {powerMode && <RightViewport narrow={narrow} />}
        </main>
        <StatusBar />
        <CommandPalette />
        {/* P3.2 — multi-agent flight deck (was implemented but never mounted). */}
        <CockpitSlideover open={cockpitOpen} onClose={() => setCockpitOpen(false)} />
        <ToastBridge />
        <AiPointer />
        {narrow && !narrowSidebarOpen && (
          <button
            type="button"
            data-testid="narrow-sidebar-trigger"
            aria-label="Open work navigation"
            aria-expanded={false}
            onClick={() => setNarrowSidebarOpen(true)}
            className="no-drag fixed left-2 top-10 z-40 grid h-8 w-8 place-items-center rounded-md border border-border bg-card/90 text-muted-foreground shadow-sm backdrop-blur hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
          >
            <Menu aria-hidden className="h-4 w-4" />
          </button>
        )}
        <FirstRunSurfaces />
        {/* P11.6.2 — non-intrusive NPS prompt (after 7 days, at most once per 90). */}
        <NpsPrompt />
      </div>
    </VaultGate>
  );
}