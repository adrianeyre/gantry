/**
 * The dashboard inside Gantry's desktop app (desktop/, ADR-0008). There, the
 * WebSocket asks for desktop Alerts, and each one is shown as a native
 * notification through Tauri's notification plugin. In a browser, none of
 * this does anything.
 */

/** An Alert sent to a desktop Alert channel. */
export interface DesktopAlert {
  title: string;
  message: string;
  level: "good" | "info" | "warning" | "bad";
  event: { kind: string; printer_id?: string; printer_name?: string; [key: string]: unknown };
}

/** The WebSocket message carrying one, sent only to desktop connections. */
export interface DesktopAlertMessage {
  type: "desktop_alert";
  alert: DesktopAlert;
}

interface TauriInternals {
  invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
}

function tauri(): TauriInternals | undefined {
  const internals = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__;
  return typeof internals?.invoke === "function" ? internals : undefined;
}

/** Whether this page is running inside the desktop app. */
export function inDesktopApp(): boolean {
  return typeof window !== "undefined" && tauri() !== undefined;
}

/** The WebSocket's path: the desktop app's connections get desktop Alerts. */
export function fleetSocketPath(): string {
  return inDesktopApp() ? "/api/ws?client=desktop" : "/api/ws";
}

/** Shows a desktop Alert as a native notification. Never throws. */
export async function showDesktopAlert(alert: DesktopAlert): Promise<void> {
  const internals = tauri();
  if (!internals) return;
  try {
    // `false` when notifications are turned off for the app.
    const granted = await internals.invoke("plugin:notification|is_permission_granted");
    if (granted === false) return;
    await internals.invoke("plugin:notification|notify", {
      options: { title: alert.title, body: alert.message },
    });
  } catch (error) {
    console.warn("Gantry could not show a desktop Alert", error);
  }
}

/**
 * Handles a WebSocket message if it is for the desktop app, and says whether
 * it was. A browser ignores desktop Alerts (it should never get one).
 */
export function handleDesktopMessage(message: { type: string }): boolean {
  if (message.type !== "desktop_alert") return false;
  void showDesktopAlert((message as DesktopAlertMessage).alert);
  return true;
}
