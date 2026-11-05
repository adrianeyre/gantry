// The meshes the server reads from a .3mf, as the 3D viewer takes them
// (PRD §8.2, level 2). Positions and indices arrive as little-endian
// base64, which keeps a big model to one compact JSON document.

import { request } from "./api";

export interface ModelObjectJson {
  name: string | null;
  filament: number | null;
  colour: string | null;
  positions: string;
  indices: string;
}

export interface ModelJson {
  objects: ModelObjectJson[];
  triangles: number;
  original_triangles: number;
  simplified: boolean;
}

export interface ModelObject {
  name: string;
  filament: number | null;
  colour: string;
  positions: Float32Array;
  indices: Uint32Array;
}

/** The grey used for an object whose filament colour isn't known. */
export const DEFAULT_COLOUR = "#9E9E9E";

function bytes(base64: string): ArrayBuffer {
  const text = atob(base64);
  const out = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i++) out[i] = text.charCodeAt(i);
  return out.buffer;
}

/** Turns the server's JSON into typed arrays ready for the GPU. */
export function decodeModel(json: ModelJson): ModelObject[] {
  return json.objects.map((o, i) => ({
    name: o.name ?? `Object ${i + 1}`,
    filament: o.filament,
    colour: o.colour ?? DEFAULT_COLOUR,
    positions: new Float32Array(bytes(o.positions)),
    indices: new Uint32Array(bytes(o.indices)),
  }));
}

/** The box around every object, in model space. */
export function bounds(objects: ModelObject[]): { min: [number, number, number]; max: [number, number, number] } {
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (const o of objects) {
    for (let i = 0; i < o.positions.length; i += 3) {
      for (let a = 0; a < 3; a++) {
        min[a] = Math.min(min[a], o.positions[i + a]);
        max[a] = Math.max(max[a], o.positions[i + a]);
      }
    }
  }
  return { min, max };
}

export const fetchModel = (printFileId: number) => request<ModelJson>("GET", `/api/print-files/${printFileId}/model`);
