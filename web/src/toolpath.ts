// A Plate's toolpath (PRD §8.2, level 3), as the server streams it: the
// binary of gantry-server's print_files::toolpath, read layer by layer as
// the bytes arrive, so the first layers can be drawn while the rest load.

/** What a path prints. The numbers match the server's. */
export type Feature = "wall" | "infill" | "support" | "other";

export const FEATURES: Feature[] = ["wall", "infill", "support", "other"];

export const FEATURE_LABELS: Record<Feature, string> = {
  wall: "Walls",
  infill: "Infill",
  support: "Supports",
  other: "Skirt, brim and other",
};

/** One layer, ready to draw: every path as line segments. */
export interface ToolpathLayer {
  index: number;
  z: number;
  /** x, y pairs: each segment's two ends, one after the other. */
  segments: Float32Array;
  /** Each segment's feature (by index into FEATURES) and tool. */
  features: Uint8Array;
  tools: Uint8Array;
}

export interface ToolpathHeader {
  layers: number;
  tools: number;
  /** min x, min y, max x, max y. */
  bounds: [number, number, number, number];
}

const HEADER = 28;

/**
 * Reads the format incrementally: `push` bytes as they arrive, and it hands
 * back each layer once all of its bytes are in.
 */
export class ToolpathReader {
  private buffer = new Uint8Array(0);
  private offset = 0;
  header: ToolpathHeader | null = null;
  private read = 0;

  /** Takes in more bytes; returns every layer they complete. */
  push(chunk: Uint8Array): ToolpathLayer[] {
    this.append(chunk);
    const out: ToolpathLayer[] = [];
    for (let layer = this.next(); layer; layer = this.next()) out.push(layer);
    return out;
  }

  /** Takes in more bytes without reading them yet. */
  append(chunk: Uint8Array) {
    const rest = this.buffer.subarray(this.offset);
    const joined = new Uint8Array(rest.length + chunk.length);
    joined.set(rest);
    joined.set(chunk, rest.length);
    this.buffer = joined;
    this.offset = 0;
  }

  /**
   * The next complete layer, or `null` until more bytes arrive. One at a
   * time, so a caller can yield to the browser between layers.
   */
  next(): ToolpathLayer | null {
    if (!this.header) {
      if (this.buffer.length < HEADER) return null;
      const view = new DataView(this.buffer.buffer, this.buffer.byteOffset, HEADER);
      const magic = String.fromCharCode(...this.buffer.subarray(0, 4));
      if (magic !== "GTP1") throw new Error("This isn't a Gantry toolpath.");
      this.header = {
        layers: view.getUint32(4, true),
        tools: view.getUint8(8),
        bounds: [
          view.getFloat32(12, true),
          view.getFloat32(16, true),
          view.getFloat32(20, true),
          view.getFloat32(24, true),
        ],
      };
      this.offset = HEADER;
    }
    return this.readLayer();
  }

  /** Whether every layer has been read. */
  get done(): boolean {
    return this.header !== null && this.read >= this.header.layers;
  }

  private readLayer(): ToolpathLayer | null {
    if (!this.header || this.read >= this.header.layers) return null;
    const bytes = this.buffer;
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    let at = this.offset;
    if (bytes.length - at < 8) return null;
    const z = view.getFloat32(at, true);
    const paths = view.getUint32(at + 4, true);
    at += 8;
    // First, check the whole layer is here, and count its segments.
    let segments = 0;
    let scan = at;
    for (let p = 0; p < paths; p++) {
      if (bytes.length - scan < 8) return null;
      const points = view.getUint32(scan + 4, true);
      scan += 8 + points * 8;
      if (scan > bytes.length) return null;
      segments += Math.max(0, points - 1);
    }
    const out = new Float32Array(segments * 4);
    const features = new Uint8Array(segments);
    const tools = new Uint8Array(segments);
    let s = 0;
    for (let p = 0; p < paths; p++) {
      const feature = view.getUint8(at);
      const tool = view.getUint8(at + 1);
      const points = view.getUint32(at + 4, true);
      at += 8;
      let px = view.getFloat32(at, true);
      let py = view.getFloat32(at + 4, true);
      for (let i = 1; i < points; i++) {
        const x = view.getFloat32(at + i * 8, true);
        const y = view.getFloat32(at + i * 8 + 4, true);
        out[s * 4] = px;
        out[s * 4 + 1] = py;
        out[s * 4 + 2] = x;
        out[s * 4 + 3] = y;
        features[s] = feature;
        tools[s] = tool;
        s++;
        px = x;
        py = y;
      }
      at += points * 8;
    }
    this.offset = at;
    return { index: this.read++, z, segments: out, features, tools };
  }
}

/** Builds a toolpath binary, for tests. */
export function encodeToolpath(
  layers: { z: number; paths: { feature: number; tool: number; points: [number, number][] }[] }[],
  tools = 1,
): Uint8Array {
  const points = layers.flatMap((l) => l.paths).reduce((n, p) => n + p.points.length, 0);
  const paths = layers.reduce((n, l) => n + l.paths.length, 0);
  const buffer = new ArrayBuffer(HEADER + layers.length * 8 + paths * 8 + points * 8);
  const view = new DataView(buffer);
  const bytes = new Uint8Array(buffer);
  bytes.set([71, 84, 80, 49], 0);
  view.setUint32(4, layers.length, true);
  view.setUint8(8, tools);
  let at = HEADER;
  for (const layer of layers) {
    view.setFloat32(at, layer.z, true);
    view.setUint32(at + 4, layer.paths.length, true);
    at += 8;
    for (const path of layer.paths) {
      view.setUint8(at, path.feature);
      view.setUint8(at + 1, path.tool);
      view.setUint32(at + 4, path.points.length, true);
      at += 8;
      for (const [x, y] of path.points) {
        view.setFloat32(at, x, true);
        view.setFloat32(at + 4, y, true);
        at += 8;
      }
    }
  }
  return bytes;
}

/** The layer a Printer reports (from 1) as an index into the toolpath's layers. */
export function layerIndex(reported: number | null | undefined, layers: number): number | null {
  if (reported == null || layers === 0) return null;
  return Math.min(layers - 1, Math.max(0, Math.round(reported) - 1));
}

/** The toolpath's address. */
export function toolpathUrl(printFileId: number, plate: number): string {
  return `/api/print-files/${printFileId}/plates/${plate}/toolpath`;
}
