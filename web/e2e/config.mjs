// Shared by the Playwright config, the server launcher and the tests.
export const PORT = Number(process.env.GANTRY_E2E_PORT ?? 18181);
export const BASE_URL = `http://127.0.0.1:${PORT}`;
export const ADMIN = { username: "e2e-admin", password: "an end-to-end password" };
