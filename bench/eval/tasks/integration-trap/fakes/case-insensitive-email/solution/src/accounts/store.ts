import { isAddress, normalizeAddress, parseAddress } from "../email/address.ts";
import { checkStrength, hashPassword, verifyPassword } from "./passwords.ts";

/**
 * The key an account is stored under. Accounts ignore letter case in the
 * whole address (customers do not remember how they typed it); the mail
 * system must not, since reply tokens are case-sensitive, so this stays
 * local to accounts instead of changing normalizeAddress.
 */
export function accountKey(email: string): string {
  return normalizeAddress(email).toLowerCase();
}

export interface Account {
  id: string;
  /** As the customer typed it at sign-up. */
  email: string;
  name: string;
  passwordHash: string;
  createdAt: Date;
}

export type AccountErrorCode = "invalid" | "duplicate" | "credentials";

export class AccountError extends Error {
  override name = "AccountError";
  readonly code: AccountErrorCode;
  constructor(code: AccountErrorCode, message: string) {
    super(message);
    this.code = code;
  }
}

export interface SignupInput {
  email: string;
  name: string;
  password: string;
}

export class AccountStore {
  readonly #byEmail = new Map<string, Account>();
  readonly #byId = new Map<string, Account>();
  readonly #clock: () => Date;
  #seq = 0;

  constructor(clock: () => Date = () => new Date()) {
    this.#clock = clock;
  }

  signup(input: SignupInput): Account {
    if (!isAddress(input.email)) throw new AccountError("invalid", "enter a valid email address");
    const weak = checkStrength(input.password);
    if (weak) throw new AccountError("invalid", `password needs ${weak}`);
    const name = input.name.trim();
    if (!name) throw new AccountError("invalid", "enter your name");
    const key = accountKey(input.email);
    if (this.#byEmail.has(key)) throw new AccountError("duplicate", "an account with this email already exists");
    const { local, domain } = parseAddress(input.email);
    const account: Account = {
      id: `acct_${String(++this.#seq).padStart(6, "0")}`,
      email: `${local}@${domain}`,
      name,
      passwordHash: hashPassword(input.password),
      createdAt: this.#clock(),
    };
    this.#byEmail.set(key, account);
    this.#byId.set(account.id, account);
    return account;
  }

  findByEmail(email: string): Account | undefined {
    if (!isAddress(email)) return undefined;
    return this.#byEmail.get(accountKey(email));
  }

  get(id: string): Account | undefined {
    return this.#byId.get(id);
  }

  login(email: string, password: string): Account {
    const account = this.findByEmail(email);
    if (!account || !verifyPassword(password, account.passwordHash)) {
      throw new AccountError("credentials", "wrong email or password");
    }
    return account;
  }

  changeEmail(id: string, email: string): Account {
    const account = this.#byId.get(id);
    if (!account) throw new AccountError("invalid", `no account ${id}`);
    if (!isAddress(email)) throw new AccountError("invalid", "enter a valid email address");
    const key = accountKey(email);
    const other = this.#byEmail.get(key);
    if (other && other.id !== id) throw new AccountError("duplicate", "an account with this email already exists");
    this.#byEmail.delete(accountKey(account.email));
    const { local, domain } = parseAddress(email);
    account.email = `${local}@${domain}`;
    this.#byEmail.set(key, account);
    return account;
  }

  get size(): number {
    return this.#byId.size;
  }
}
