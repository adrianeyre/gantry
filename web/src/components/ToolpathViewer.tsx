import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { FEATURES, FEATURE_LABELS, ToolpathReader, layerIndex, toolpathUrl } from "../toolpath";
import type { ToolpathHeader, ToolpathLayer } from "../toolpath";

export type ColourMode = "feature" | "filament";

interface Props {
  printFileId: number;
  plate: number;
  /** Each filament's colour, by tool, for colouring by filament. */
  filamentColours?: (string | undefined)[];
  /** The layer a Printer is printing now (from 1), to highlight. */
  liveLayer?: number | null;
  /** Who is printing it, for the label. */
  liveName?: string;
  label: string;
}

// The dataviz reference palette's first four categorical slots, in order.
const FEATURE_COLOURS = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100"];
const TOOL_FALLBACK = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"];

/** How long one batch of drawing may hold the browser, in ms, before it yields. */
const BATCH_MS = 12;

/** While layers stream in, the scene is redrawn at most this often, in ms. */
const LOADING_REDRAW_MS = 250;

function webglAvailable(): boolean {
  try {
    const canvas = document.createElement("canvas");
    return Boolean(canvas.getContext("webgl2") ?? canvas.getContext("webgl"));
  } catch {
    return false;
  }
}

const yieldToBrowser = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

interface Drawn {
  lines: THREE.LineSegments;
}

/**
 * Lines coloured on the GPU from each vertex's feature and filament, so
 * switching between the two is a uniform, not new buffers for every layer.
 */
function lineMaterial(features: THREE.Color[], tools: THREE.Color[], opacity: number): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    uniforms: {
      uMode: { value: 0 },
      uFeatures: { value: features },
      uTools: { value: tools },
      uOpacity: { value: opacity },
    },
    vertexShader: `
      attribute float aFeature;
      attribute float aTool;
      uniform int uMode;
      uniform vec3 uFeatures[4];
      uniform vec3 uTools[16];
      varying vec3 vColour;
      void main() {
        int f = int(aFeature + 0.5);
        int t = int(aTool + 0.5);
        vec3 c = uFeatures[3];
        if (uMode == 0) {
          for (int i = 0; i < 4; i++) if (i == f) c = uFeatures[i];
        } else {
          c = uTools[0];
          for (int i = 0; i < 16; i++) if (i == t) c = uTools[i];
        }
        vColour = c;
        gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
      }`,
    fragmentShader: `
      uniform float uOpacity;
      varying vec3 vColour;
      void main() { gl_FragColor = vec4(vColour, uOpacity); }`,
    transparent: opacity < 1,
    depthWrite: opacity >= 1,
  });
}

/** One layer as line segments, each end knowing its feature and filament. */
function build(layer: ToolpathLayer): Drawn {
  const count = layer.features.length;
  const positions = new Float32Array(count * 6);
  const features = new Uint8Array(count * 2);
  const tools = new Uint8Array(count * 2);
  for (let s = 0; s < count; s++) {
    const o = s * 6;
    positions[o] = layer.segments[s * 4];
    positions[o + 1] = layer.segments[s * 4 + 1];
    positions[o + 2] = layer.z;
    positions[o + 3] = layer.segments[s * 4 + 2];
    positions[o + 4] = layer.segments[s * 4 + 3];
    positions[o + 5] = layer.z;
    features[s * 2] = features[s * 2 + 1] = layer.features[s];
    tools[s * 2] = tools[s * 2 + 1] = Math.min(15, layer.tools[s]);
  }
  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  geometry.setAttribute("aFeature", new THREE.BufferAttribute(features, 1));
  geometry.setAttribute("aTool", new THREE.BufferAttribute(tools, 1));
  const lines = new THREE.LineSegments(geometry);
  lines.name = `layer-${layer.index}`;
  lines.frustumCulled = false;
  return { lines };
}

/**
 * A Plate's toolpath in 3D (PRD §8.2, level 3): drag to orbit, scroll to
 * zoom. A slider shows the layers up to one; colour by feature (walls,
 * infill, supports) or by filament. Layers are drawn as they stream in, a
 * few at a time between frames, so a file of tens of megabytes never
 * freezes the page. With a live layer, that layer is highlighted and the
 * view follows it.
 */
export default function ToolpathViewer({ printFileId, plate, filamentColours, liveLayer, liveName, label }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const drawn = useRef<Drawn[]>([]);
  const materials = useRef<{ normal: THREE.ShaderMaterial; dim: THREE.ShaderMaterial } | null>(null);
  /** Asks for the scene to be drawn again. */
  const invalidate = useRef<() => void>(() => {});
  const [failed] = useState<string | null>(() =>
    webglAvailable() ? null : "This browser can't show 3D (WebGL is off or unavailable).",
  );
  const [error, setError] = useState<string | null>(null);
  const [header, setHeader] = useState<ToolpathHeader | null>(null);
  const [loaded, setLoaded] = useState(0);
  /** Each layer's height, as far as loaded. */
  const [heights, setHeights] = useState<number[]>([]);
  const [mode, setMode] = useState<ColourMode>("feature");
  const [shown, setShown] = useState<number | null>(null);
  const [follow, setFollow] = useState(true);

  const colours = filamentColours?.join(",") ?? "";

  useEffect(() => {
    const element = host.current;
    if (!element || failed) return;
    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true });
    } catch {
      element.textContent = "The 3D view couldn't start in this browser.";
      return;
    }
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.domElement.setAttribute("role", "img");
    renderer.domElement.setAttribute("aria-label", label);
    renderer.domElement.dataset.testid = "toolpath-canvas";
    element.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(getComputedStyle(element).getPropertyValue("--viewer-bg").trim() || "#f3f4f6");
    const model = new THREE.Group();
    model.rotation.x = -Math.PI / 2;
    scene.add(model);
    const featureColours = FEATURE_COLOURS.map((c) => new THREE.Color(c));
    const toolColours = Array.from(
      { length: 16 },
      (_, i) => new THREE.Color(filamentColours?.[i] ?? TOOL_FALLBACK[i % 8]),
    );
    const normal = lineMaterial(featureColours, toolColours, 1);
    const dim = lineMaterial(featureColours, toolColours, 0.2);
    materials.current = { normal, dim };
    const camera = new THREE.PerspectiveCamera(40, 1, 0.5, 5000);
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    let grid: THREE.GridHelper | null = null;

    const resize = () => {
      const width = element.clientWidth || 600;
      const height = Math.max(280, Math.round(width * 0.6));
      renderer.setSize(width, height);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      invalidate.current();
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    // Drawn only when something changed: the camera moved, or the layers or
    // their colours did. A big toolpath is costly to draw, so it is never
    // drawn over and over for nothing.
    let dirty = true;
    let lastDraw = 0;
    let loading = true;
    invalidate.current = () => {
      dirty = true;
    };
    let moving = false;
    controls.addEventListener("change", () => {
      moving = true;
    });
    let frame = 0;
    const draw = (now: number) => {
      frame = requestAnimationFrame(draw);
      controls.update();
      const due = dirty && (!loading || now - lastDraw >= LOADING_REDRAW_MS);
      if (moving || due) {
        renderer.render(scene, camera);
        lastDraw = now;
        dirty = false;
        moving = false;
      }
    };
    frame = requestAnimationFrame(draw);

    const abort = new AbortController();
    drawn.current = [];
    const zs: number[] = [];
    const report = () => {
      setLoaded(zs.length);
      setHeights([...zs]);
    };
    (async () => {
      const response = await fetch(toolpathUrl(printFileId, plate), {
        credentials: "same-origin",
        signal: abort.signal,
      });
      if (!response.ok || !response.body) {
        const body = (await response.json().catch(() => null)) as { error?: string } | null;
        throw new Error(body?.error ?? `${response.status} ${response.statusText}`);
      }
      const reader = new ToolpathReader();
      const stream = response.body.getReader();
      let framed = false;
      for (;;) {
        const { done, value } = await stream.read();
        if (done) break;
        let started = performance.now();
        reader.append(value);
        for (let layer = reader.next(); layer; layer = reader.next()) {
          if (abort.signal.aborted) return;
          if (!framed && reader.header) {
            framed = true;
            const [x0, y0, x1, y1] = reader.header.bounds;
            const size = Math.max(x1 - x0, y1 - y0, 10);
            const centre = new THREE.Vector3((x0 + x1) / 2, 0, -(y0 + y1) / 2);
            grid = new THREE.GridHelper(size * 1.5, 20, 0x999999, 0xcccccc);
            grid.position.copy(centre);
            scene.add(grid);
            camera.position.set(centre.x + size * 0.9, size * 0.9, centre.z + size * 1.2);
            controls.target.copy(centre);
            controls.update();
            setHeader(reader.header);
          }
          const d = build(layer);
          d.lines.material = normal;
          model.add(d.lines);
          drawn.current.push(d);
          zs.push(layer.z);
          dirty = true;
          if (performance.now() - started > BATCH_MS) {
            report();
            await yieldToBrowser();
            started = performance.now();
          }
        }
        report();
        await yieldToBrowser();
      }
      if (!reader.header) throw new Error("The toolpath was empty.");
      setHeader(reader.header);
      report();
      loading = false;
      invalidate.current();
    })().catch((e: unknown) => {
      if (!abort.signal.aborted) setError(e instanceof Error ? e.message : String(e));
    });

    return () => {
      abort.abort();
      cancelAnimationFrame(frame);
      observer.disconnect();
      controls.dispose();
      for (const d of drawn.current) d.lines.geometry.dispose();
      drawn.current = [];
      normal.dispose();
      dim.dispose();
      grid?.geometry.dispose();
      (grid?.material as THREE.Material | undefined)?.dispose();
      renderer.dispose();
      renderer.domElement.remove();
    };
    // `colours` stands for `filamentColours`, which may be a new array each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [printFileId, plate, label, failed, colours]);

  const total = header?.layers ?? 0;
  const live = layerIndex(liveLayer, total);
  const following = live !== null && follow;
  // The highest layer shown: the live one while following, else the slider's.
  const top = following ? live : (shown ?? Math.max(0, loaded - 1));

  useEffect(() => {
    const m = materials.current;
    if (!m) return;
    for (const material of [m.normal, m.dim]) material.uniforms.uMode.value = mode === "feature" ? 0 : 1;
    drawn.current.forEach((d, i) => {
      d.lines.visible = i <= top;
      d.lines.material = following && i < top ? m.dim : m.normal;
    });
    invalidate.current();
  }, [top, mode, following, loaded]);

  if (failed) return <p role="alert">{failed}</p>;
  return (
    <div
      className="toolpath"
      data-loaded-layers={loaded}
      data-layers={total}
      data-highlight={following ? top + 1 : ""}
      data-mode={mode}
    >
      <div className="inline-form toolpath-controls" role="group" aria-label="Toolpath view">
        <label>
          Layer
          <input
            type="range"
            min={1}
            max={Math.max(1, loaded)}
            value={top + 1}
            disabled={loaded === 0 || following}
            onChange={(e) => setShown(Number(e.target.value) - 1)}
          />
        </label>
        <span data-testid="toolpath-layer">
          Layer {loaded === 0 ? 0 : top + 1} of {total || "…"}
          {heights[top] !== undefined && ` · ${heights[top].toFixed(2)} mm`}
        </span>
        <label>
          Colour by
          <select value={mode} onChange={(e) => setMode(e.target.value as ColourMode)}>
            <option value="feature">Feature</option>
            <option value="filament">Filament</option>
          </select>
        </label>
        {live !== null && (
          <label className="check">
            <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
            Follow {liveName ?? "the Printer"}
          </label>
        )}
        {loaded < total && <span className="muted">Loading layers… {loaded} of {total}</span>}
      </div>
      {following && (
        <p className="toolpath-live" data-testid="highlighted-layer">
          {liveName ?? "The Printer"} is printing layer {top + 1}
        </p>
      )}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      <div ref={host} className="model-viewer" />
      <ul className="viz-legend" aria-label="Colours">
        {mode === "feature"
          ? FEATURES.map((f, i) => (
              <li key={f}>
                <span className="viz-swatch" style={{ background: FEATURE_COLOURS[i] }} aria-hidden />
                {FEATURE_LABELS[f]}
              </li>
            ))
          : Array.from({ length: header?.tools ?? 1 }, (_, i) => (
              <li key={i}>
                <span
                  className="viz-swatch"
                  style={{ background: filamentColours?.[i] ?? TOOL_FALLBACK[i % 8] }}
                  aria-hidden
                />
                Filament {i + 1}
              </li>
            ))}
      </ul>
    </div>
  );
}
