import assert from "node:assert/strict";
import { test } from "node:test";
import { AccountError, AccountStore } from "../src/accounts/store.ts";
import { checkStrength, hashPassword, verifyPassword } from "../src/accounts/passwords.ts";

const pw = "correct horse 42";

function code(fn: () => unknown): string {
  try {
    fn();
  } catch (err) {
    assert.ok(err instanceof AccountError, String(err));
    return err.code;
  }
  assert.fail("expected an AccountError");
}

test("signup creates an account that can log in", () => {
  const store = new AccountStore(() => new Date("2026-01-05T10:00:00Z"));
  const a = store.signup({ email: "dana@example.com", name: " Dana ", password: pw });
  assert.equal(a.id, "acct_000001");
  assert.equal(a.name, "Dana");
  assert.deepEqual(a.createdAt, new Date("2026-01-05T10:00:00Z"));
  assert.equal(store.login("dana@example.com", pw).id, a.id);
});

test("signup rejects bad input", () => {
  const store = new AccountStore();
  assert.equal(code(() => store.signup({ email: "dana", name: "Dana", password: pw })), "invalid");
  assert.equal(code(() => store.signup({ email: "dana@example.com", name: " ", password: pw })), "invalid");
  assert.equal(code(() => store.signup({ email: "dana@example.com", name: "Dana", password: "short1" })), "invalid");
});

test("the same address cannot sign up twice", () => {
  const store = new AccountStore();
  store.signup({ email: "dana@example.com", name: "Dana", password: pw });
  assert.equal(code(() => store.signup({ email: "dana@example.com", name: "Dana Two", password: pw })), "duplicate");
  assert.equal(code(() => store.signup({ email: "Dana Fox <dana@EXAMPLE.com>", name: "Dana Two", password: pw })), "duplicate");
  assert.equal(store.size, 1);
});

test("login rejects a wrong password or an unknown address the same way", () => {
  const store = new AccountStore();
  store.signup({ email: "dana@example.com", name: "Dana", password: pw });
  assert.equal(code(() => store.login("dana@example.com", "wrong password 1")), "credentials");
  assert.equal(code(() => store.login("nobody@example.com", pw)), "credentials");
  assert.equal(code(() => store.login("not an address", pw)), "credentials");
});

test("findByEmail ignores the domain's case and a display name", () => {
  const store = new AccountStore();
  const a = store.signup({ email: "lee@example.com", name: "Lee", password: pw });
  assert.equal(store.findByEmail("lee@Example.Com")?.id, a.id);
  assert.equal(store.findByEmail("Lee <lee@example.com>")?.id, a.id);
  assert.equal(store.findByEmail("lea@example.com"), undefined);
});

test("changeEmail moves the account to the new address", () => {
  const store = new AccountStore();
  const a = store.signup({ email: "lee@example.com", name: "Lee", password: pw });
  store.signup({ email: "kim@example.com", name: "Kim", password: pw });
  store.changeEmail(a.id, "lee@example.org");
  assert.equal(store.findByEmail("lee@example.com"), undefined);
  assert.equal(store.findByEmail("lee@example.org")?.id, a.id);
  assert.equal(code(() => store.changeEmail(a.id, "kim@example.com")), "duplicate");
});

test("passwords are salted and verified", () => {
  const a = hashPassword(pw);
  const b = hashPassword(pw);
  assert.notEqual(a, b);
  assert.equal(verifyPassword(pw, a), true);
  assert.equal(verifyPassword("correct horse 43", a), false);
  assert.equal(verifyPassword(pw, "md5$abc"), false);
});

test("password strength", () => {
  assert.equal(checkStrength("abc"), "at least 10 characters");
  assert.equal(checkStrength("abcdefghijkl"), "letters and digits");
  assert.equal(checkStrength("abcdefghij1"), null);
});
