import { useEffect, useState } from "react";
import { applyMessage } from "./fleet";
import type { PrinterView, ServerMessage } from "./fleet";
import { fleetSocketPath, handleDesktopMessage } from "./desktop";

export type LinkState = "connecting" | "open" | "closed";

/**
 * Keeps the Fleet live over the WebSocket. Each (re)connection starts with a
 * full snapshot, then deltas, so a browser that was away is never stale.
 */
export function useFleet(): { printers: PrinterView[]; link: LinkState } {
  const [fleet, setFleet] = useState<Map<string, PrinterView>>(new Map());
  const [link, setLink] = useState<LinkState>("connecting");

  useEffect(() => {
    let socket: WebSocket | null = null;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let delay = 500;
    let stopped = false;

    function connect() {
      const scheme = window.location.protocol === "https:" ? "wss" : "ws";
      socket = new WebSocket(`${scheme}://${window.location.host}${fleetSocketPath()}`);
      socket.onopen = () => {
        delay = 500;
        setLink("open");
      };
      socket.onmessage = (event) => {
        const message = JSON.parse(String(event.data)) as ServerMessage;
        if (handleDesktopMessage(message)) return;
        setFleet((current) => applyMessage(current, message));
      };
      socket.onclose = () => {
        if (stopped) return;
        setLink("closed");
        retry = setTimeout(connect, delay);
        delay = Math.min(delay * 2, 10_000);
      };
    }

    connect();
    return () => {
      stopped = true;
      clearTimeout(retry);
      socket?.close();
    };
  }, []);

  const printers = [...fleet.values()].sort((a, b) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
  );
  return { printers, link };
}
