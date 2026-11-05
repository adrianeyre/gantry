import { describe, expect, it } from "vitest";
import { DEFAULT_COLOUR, bounds, decodeModel } from "./model3d";

function base64(array: Float32Array | Uint32Array): string {
  const bytes = new Uint8Array(array.buffer);
  let text = "";
  for (const b of bytes) text += String.fromCharCode(b);
  return btoa(text);
}

describe("decodeModel", () => {
  it("turns the server's base64 arrays back into positions and indices", () => {
    const positions = new Float32Array([0, 0, 0, 10, 0, 0, 0, 10, 5]);
    const indices = new Uint32Array([0, 1, 2]);
    const [object] = decodeModel({
      objects: [{ name: null, filament: 2, colour: null, positions: base64(positions), indices: base64(indices) }],
      triangles: 1,
      original_triangles: 1,
      simplified: false,
    });
    expect([...object.positions]).toEqual([...positions]);
    expect([...object.indices]).toEqual([0, 1, 2]);
    expect(object.name).toBe("Object 1");
    expect(object.colour).toBe(DEFAULT_COLOUR);
    expect(bounds([object])).toEqual({ min: [0, 0, 0], max: [10, 10, 5] });
  });
});
