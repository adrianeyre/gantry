/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Gantry's version, from the workspace's Cargo.toml (vite.config.ts). */
  readonly VITE_APP_VERSION: string;
  /** Who made Gantry, for Credits. */
  readonly VITE_APP_AUTHOR: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
