import { describe, expect, it } from "vitest";
import { classifyAuthError, isLoginAttemptPath } from "./fetchInterceptor";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

describe("classifyAuthError", () => {
  it("returns null for non-401 responses", async () => {
    expect(await classifyAuthError(jsonResponse(200, { ok: true }))).toBeNull();
    expect(await classifyAuthError(jsonResponse(403, { error: "x" }))).toBeNull();
    expect(await classifyAuthError(jsonResponse(500, { error: "x" }))).toBeNull();
  });

  it("classifies 401 login_required as login_required", async () => {
    const res = jsonResponse(401, {
      error: "login_required",
      message: "Passphrase login required",
    });
    expect(await classifyAuthError(res)).toBe("login_required");
  });

  it("classifies 401 unauthorized as unauthorized", async () => {
    const res = jsonResponse(401, {
      error: "unauthorized",
      message: "Invalid or missing auth token",
    });
    expect(await classifyAuthError(res)).toBe("unauthorized");
  });

  it("falls back to unauthorized on non-JSON 401 body", async () => {
    const res = new Response("not json", { status: 401 });
    expect(await classifyAuthError(res)).toBe("unauthorized");
  });

  it("falls back to unauthorized on JSON 401 without an error field", async () => {
    const res = jsonResponse(401, { message: "no error key" });
    expect(await classifyAuthError(res)).toBe("unauthorized");
  });

  it("leaves the original response body readable", async () => {
    const res = jsonResponse(401, { error: "login_required" });
    await classifyAuthError(res);
    const body = await res.json();
    expect(body).toEqual({ error: "login_required" });
  });
});

describe("isLoginAttemptPath", () => {
  it("recognizes the login and elevate endpoints", () => {
    expect(isLoginAttemptPath("/api/login")).toBe(true);
    expect(isLoginAttemptPath("/api/login/elevate")).toBe(true);
  });

  it("does not match unrelated /api/login/* paths", () => {
    expect(isLoginAttemptPath("/api/login/status")).toBe(false);
    expect(isLoginAttemptPath("/api/logout")).toBe(false);
    expect(isLoginAttemptPath("/api/sessions")).toBe(false);
    expect(isLoginAttemptPath("/api/login/something-else")).toBe(false);
  });
});
