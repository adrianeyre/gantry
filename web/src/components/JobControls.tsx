import { useState } from "react";
import { api } from "../api";
import type { Command } from "../api";
import type { PrinterView } from "../fleet";
import { has } from "../fleet";
import { ConfirmDialog } from "./ConfirmDialog";

const COPY: Record<Command, { label: string; title: string; body: string; done: string }> = {
  pause: {
    label: "Pause",
    title: "Pause this Job?",
    body: "The Printer stops where it is and holds its temperatures.",
    done: "Paused",
  },
  resume: { label: "Resume", title: "Resume this Job?", body: "The Printer carries on printing.", done: "Resumed" },
  cancel: {
    label: "Cancel Job",
    title: "Cancel this Job?",
    body: "The print ends now and can't be resumed. The Printer will wait for Mark Clear.",
    done: "Cancelled",
  },
};

/** Pause, resume and Cancel on a card, each confirmed first (PRD §6.4). */
export function JobControls({ printer }: { printer: PrinterView }) {
  const [asking, setAsking] = useState<Command | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const { status } = printer;

  if (status.connection.state !== "connected") return null;
  const state = status.state;
  const offered: Command[] = [];
  if ((state === "printing" || state === "preparing") && has(status, "pause")) offered.push("pause");
  if (state === "paused" && has(status, "resume")) offered.push("resume");
  if (status.job && has(status, "cancel")) offered.push("cancel");

  async function confirm(command: Command) {
    setAsking(null);
    setBusy(true);
    setResult(null);
    try {
      await api.command(printer.id, command);
      setResult({ ok: true, text: COPY[command].done });
    } catch (e) {
      setResult({ ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  if (offered.length === 0 && !result) return null;
  return (
    <div className="controls" role="group" aria-label="Job controls">
      {offered.map((command) => (
        <button
          key={command}
          type="button"
          className={command === "cancel" ? "danger" : undefined}
          disabled={busy}
          onClick={() => setAsking(command)}
        >
          {COPY[command].label}
        </button>
      ))}
      {result && (
        <span role="status" className={`command-result ${result.ok ? "" : "error"}`}>
          {result.text}
        </span>
      )}
      <ConfirmDialog
        open={asking !== null}
        title={asking ? COPY[asking].title : ""}
        confirmLabel={asking ? COPY[asking].label : ""}
        cancelLabel={asking === "cancel" ? "Keep printing" : "Not now"}
        danger={asking === "cancel"}
        onConfirm={() => asking && confirm(asking)}
        onCancel={() => setAsking(null)}
      >
        {asking && `${printer.name}: ${COPY[asking].body}`}
      </ConfirmDialog>
    </div>
  );
}
