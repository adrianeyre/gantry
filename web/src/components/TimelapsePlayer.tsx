import { useEffect, useState } from "react";
import { timelapseFrameUrl } from "../library";

/** Frames a second to choose from. */
export const SPEEDS = [2, 5, 10, 20, 30];

/** Whether the user has asked for less motion. */
export function prefersReducedMotion(): boolean {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Plays a Job's timelapse frame by frame: play and pause, a slider to any
 * frame, and a speed. It starts playing by itself unless the user prefers
 * reduced motion; then it waits for Play.
 */
export function TimelapsePlayer({ jobId, frames, label }: { jobId: number; frames: number; label: string }) {
  const [frame, setFrame] = useState(1);
  const [playing, setPlaying] = useState(() => frames > 1 && !prefersReducedMotion());
  const [speed, setSpeed] = useState(10);

  useEffect(() => {
    if (!playing) return;
    const timer = setInterval(() => {
      setFrame((f) => {
        if (f >= frames) {
          setPlaying(false);
          return f;
        }
        return f + 1;
      });
    }, 1000 / speed);
    return () => clearInterval(timer);
  }, [playing, speed, frames]);

  // The next frame loads while this one shows, so playback doesn't stutter.
  useEffect(() => {
    if (frame < frames && typeof Image !== "undefined") new Image().src = timelapseFrameUrl(jobId, frame + 1);
  }, [frame, frames, jobId]);

  function toggle() {
    if (!playing && frame >= frames) setFrame(1);
    setPlaying((p) => !p);
  }

  return (
    <div className="timelapse-player">
      <img
        className="timelapse-frame"
        src={timelapseFrameUrl(jobId, frame)}
        alt={`${label}, frame ${frame} of ${frames}`}
      />
      <div className="controls toolpath-controls">
        <button type="button" onClick={toggle} disabled={frames <= 1}>
          {playing ? "Pause" : "Play"}
        </button>
        <label>
          Frame
          <input
            type="range"
            min={1}
            max={frames}
            value={frame}
            aria-valuetext={`Frame ${frame} of ${frames}`}
            onChange={(e) => {
              setPlaying(false);
              setFrame(Number(e.target.value));
            }}
          />
        </label>
        <span className="num" data-testid="frame-count">
          {frame} / {frames}
        </span>
        <label>
          Speed
          <select value={speed} onChange={(e) => setSpeed(Number(e.target.value))}>
            {SPEEDS.map((s) => (
              <option key={s} value={s}>
                {s} frames a second
              </option>
            ))}
          </select>
        </label>
      </div>
    </div>
  );
}
