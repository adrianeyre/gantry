import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { fieldErrors } from "../api";
import { timelapses } from "../library";
import type { TimelapseMode, TimelapseSetting } from "../library";

export const MODE_LABELS: Record<TimelapseMode, string> = {
  off: "Off",
  layer: "A frame each layer",
  interval: "A frame on a timer",
};

/** "A frame every 30 s". */
export function describeTimelapse(s: TimelapseSetting): string {
  if (s.mode === "interval") return `A frame every ${s.interval_secs} s`;
  return MODE_LABELS[s.mode];
}

/**
 * A Printer's timelapse capture (PRD §7.5): off, a camera frame each layer,
 * or one on a timer. Admins change it; it applies from the next Job.
 */
export function TimelapseSettings({ printerId, editable }: { printerId: string; editable: boolean }) {
  const [setting, setSetting] = useState<TimelapseSetting | null>(null);
  const [mode, setMode] = useState<TimelapseMode>("off");
  const [interval, setIntervalSecs] = useState("30");
  const [status, setStatus] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    let live = true;
    timelapses.setting(printerId).then(
      (s) => {
        if (!live) return;
        setSetting(s);
        setMode(s.mode);
        setIntervalSecs(String(s.interval_secs));
      },
      (e: unknown) => live && setStatus({ ok: false, text: e instanceof Error ? e.message : String(e) }),
    );
    return () => {
      live = false;
    };
  }, [printerId]);

  async function save(e: FormEvent) {
    e.preventDefault();
    setStatus(null);
    try {
      const saved = await timelapses.set(printerId, mode, mode === "interval" ? Number(interval) : undefined);
      setSetting(saved);
      setStatus({ ok: true, text: "Saved. It applies from the next Job." });
    } catch (err) {
      const fields = fieldErrors(err);
      setStatus({
        ok: false,
        text: fields.length ? fields.map((f) => f.message).join(". ") : err instanceof Error ? err.message : String(err),
      });
    }
  }

  const line = status && (
    <p role={status.ok ? "status" : "alert"} className={status.ok ? "" : "error"}>
      {status.text}
    </p>
  );

  if (!setting) return line || <p className="loading">Loading…</p>;
  if (!editable) {
    return (
      <p data-testid="timelapse-setting">
        {describeTimelapse(setting)}.{" "}
        <span className="muted">Timelapses are kept with each Job in History.</span>
      </p>
    );
  }

  return (
    <form className="timelapse-form" aria-label="Timelapse" onSubmit={(e) => void save(e)}>
      <fieldset>
        <legend>Capture a camera frame</legend>
        {(Object.keys(MODE_LABELS) as TimelapseMode[]).map((m) => (
          <label key={m} className="check">
            <input type="radio" name="timelapse-mode" checked={mode === m} onChange={() => setMode(m)} />
            {MODE_LABELS[m]}
          </label>
        ))}
      </fieldset>
      {mode === "interval" && (
        <label>
          Seconds between frames
          <input
            type="number"
            min={5}
            max={3600}
            inputMode="numeric"
            value={interval}
            onChange={(e) => setIntervalSecs(e.target.value)}
            required
          />
          <span className="hint">5 to 3600.</span>
        </label>
      )}
      <p className="hint">It applies from the next Job. The timelapse is kept with the Job in History.</p>
      <button type="submit">Save timelapse</button>
      {line}
    </form>
  );
}
