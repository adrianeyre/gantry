import { describe, expect, it } from "vitest";
import { ToolpathReader, encodeToolpath, layerIndex } from "./toolpath";

const sample = encodeToolpath(
  [
    {
      z: 0.2,
      paths: [
        {
          feature: 0,
          tool: 0,
          points: [
            [0, 0],
            [10, 0],
            [10, 10],
          ],
        },
        {
          feature: 1,
          tool: 1,
          points: [
            [1, 1],
            [2, 2],
          ],
        },
      ],
    },
    {
      z: 0.4,
      paths: [
        {
          feature: 2,
          tool: 0,
          points: [
            [5, 5],
            [6, 5],
          ],
        },
      ],
    },
  ],
  2,
);

describe("ToolpathReader", () => {
  it("reads every layer as segments, with each segment's feature and tool", () => {
    const reader = new ToolpathReader();
    const layers = reader.push(sample);
    expect(reader.header).toMatchObject({ layers: 2, tools: 2 });
    expect(reader.done).toBe(true);
    expect(layers).toHaveLength(2);
    expect(layers[0].z).toBeCloseTo(0.2);
    expect(Array.from(layers[0].segments)).toEqual([0, 0, 10, 0, 10, 0, 10, 10, 1, 1, 2, 2]);
    expect(Array.from(layers[0].features)).toEqual([0, 0, 1]);
    expect(Array.from(layers[0].tools)).toEqual([0, 0, 1]);
    expect(Array.from(layers[1].features)).toEqual([2]);
  });

  it("hands back each layer only once all its bytes have arrived, however they are split", () => {
    for (const size of [1, 3, 7, 13, 64]) {
      const reader = new ToolpathReader();
      const seen: number[] = [];
      for (let at = 0; at < sample.length; at += size) {
        for (const layer of reader.push(sample.subarray(at, at + size))) seen.push(layer.index);
      }
      expect(seen, `chunks of ${size}`).toEqual([0, 1]);
      expect(reader.done).toBe(true);
    }
  });

  it("refuses what isn't a toolpath", () => {
    expect(() => new ToolpathReader().push(new Uint8Array(40))).toThrow(/toolpath/);
  });
});

describe("layerIndex", () => {
  it("maps a reported layer (from 1) onto the toolpath, within its layers", () => {
    expect(layerIndex(1, 10)).toBe(0);
    expect(layerIndex(10, 10)).toBe(9);
    expect(layerIndex(25, 10)).toBe(9);
    expect(layerIndex(0, 10)).toBe(0);
    expect(layerIndex(null, 10)).toBeNull();
    expect(layerIndex(3, 0)).toBeNull();
  });
});
