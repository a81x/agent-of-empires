// Worktree session creation through POST /api/sessions against a real git repo.

import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { test, expect } from "../helpers/liveTest";
import { gitEnv, initWorkingRepo } from "../helpers/gitFixture";

const CIVILIZATION_NAMES = (
  "Armenians Aztecs Bengalis Berbers Bohemians Britons Bulgarians Burgundians Burmese Byzantines Celts Chinese " +
  "Cumans Dravidians Ethiopians Franks Georgians Goths Gurjaras Hindustanis Huns Incas Italians Japanese Jurchens " +
  "Khitans Khmer Koreans Lithuanians Magyars Malay Malians Mayans Mongols Persians Poles Portuguese Romans Saracens " +
  "Shu Sicilians Slavs Spanish Tatars Teutons Turks Vietnamese Vikings Wei Wu"
).split(" ");

const createSession = (baseUrl: string, body: object) =>
  fetch(`${baseUrl}/api/sessions`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });

test("duplicate worktree branch returns the real collision error, not a generic one", async ({ spawnServe }) => {
  // #1649
  const serve = await spawnServe({ seedFn: ({ home, env }) => void initWorkingRepo(join(home, "project"), env) });
  const payload = {
    path: join(serve.home, "project"),
    tool: "claude",
    title: "dup-session",
    worktree_branch: "dup-branch",
    create_new_branch: true,
  };
  expect((await createSession(serve.baseUrl, payload)).status).toBe(201);
  const second = await createSession(serve.baseUrl, payload);
  expect(second.status).toBe(400);
  const body = await second.json();
  expect(body.message).toContain("Worktree already exists");
  expect(body.message).not.toBe("Failed to create session");
});

test("auto-generated worktree branch avoids civilization branch collisions", async ({ spawnServe }) => {
  const serve = await spawnServe({
    seedFn: ({ home, env }) => {
      const { path } = initWorkingRepo(join(home, "project"), env);
      for (const civ of CIVILIZATION_NAMES) spawnSync("git", ["branch", civ], { cwd: path, env: gitEnv(env) });
    },
  });
  const res = await createSession(serve.baseUrl, {
    path: join(serve.home, "project"),
    tool: "claude",
    worktree_enabled: true,
    create_new_branch: true,
  });
  expect(res.status).toBe(201);
  const body = await res.json();
  expect(body.title).toMatch(/\bII\b/);
  expect(body.branch).toBeTruthy();
  expect(CIVILIZATION_NAMES.map((civ) => civ.toLowerCase())).not.toContain(String(body.branch).toLowerCase());
});
