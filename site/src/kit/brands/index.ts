/**
 * Stageshow's local brand registry, emptied for Gantry: the site shows no
 * other company's marks, so none of that artwork is vendored. `BrandMark`
 * still works with a caller-supplied `name` and `src`.
 */
export type BrandAsset = {
  name: string
  category: 'Payments' | 'Banks' | 'Apps' | 'Crypto'
  light: string
  dark: string
  tileArtwork?: string
  tileBackground?: string
  proportion: number
  scale: number
}

export const brandMarks: Record<string, BrandAsset> = {}

export type BrandName = never
