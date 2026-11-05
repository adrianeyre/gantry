// Test doubles for the server: a fetch that answers by method and path, and
// a WebSocket that sends a Fleet snapshot, as the real ones would.
import { vi } from "vitest";
import type { PrinterView } from "./fleet";

type Answer = unknown | ((init?: RequestInit) => unknown);

/**
 * Stubs `fetch` with answers keyed by "METHOD /path" (the query string is
 * ignored). An answer that is a function gets the request; `[status, body]`
 * sets the status. Returns the mock, to check the calls.
 */
export function stubApi(routes: Record<string, Answer>) {
  const fetch = vi.fn((input: string, init?: RequestInit) => {
    const method = (init?.method ?? "GET").toUpperCase();
    const path = String(input).split("?")[0];
    const key = `${method} ${path}`;
    if (!(key in routes)) return Promise.resolve(new Response("{}", { status: 404 }));
    let answer = routes[key];
    if (typeof answer === "function") answer = (answer as (init?: RequestInit) => unknown)(init);
    const [status, body] =
      Array.isArray(answer) && answer.length === 2 && typeof answer[0] === "number"
        ? (answer as [number, unknown])
        : [200, answer];
    return Promise.resolve(
      new Response(body === undefined ? "" : JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
      }),
    );
  });
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

/** Stubs `WebSocket` with one that sends this Fleet as its snapshot. */
export function stubFleet(printers: PrinterView[]) {
  class FakeSocket {
    onopen: (() => void) | null = null;
    onmessage: ((e: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    constructor() {
      setTimeout(() => {
        this.onopen?.();
        this.onmessage?.({ data: JSON.stringify({ type: "snapshot", printers }) });
      }, 0);
    }
    close() {}
  }
  vi.stubGlobal("WebSocket", FakeSocket);
}
