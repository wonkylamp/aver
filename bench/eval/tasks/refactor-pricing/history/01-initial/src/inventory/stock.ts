export class Stock {
  #onHand = new Map<string, number>();
  #reserved = new Map<string, number>();

  receive(sku: string, qty: number): void {
    if (!Number.isInteger(qty) || qty <= 0) throw new Error(`cannot receive ${qty} of ${sku}`);
    this.#onHand.set(sku, (this.#onHand.get(sku) ?? 0) + qty);
  }

  available(sku: string): number {
    return (this.#onHand.get(sku) ?? 0) - (this.#reserved.get(sku) ?? 0);
  }

  // All or nothing: a reservation that cannot be met in full reserves nothing.
  reserve(items: Array<{ sku: string; qty: number }>): boolean {
    const wanted = new Map<string, number>();
    for (const { sku, qty } of items) wanted.set(sku, (wanted.get(sku) ?? 0) + qty);
    for (const [sku, qty] of wanted) {
      if (this.available(sku) < qty) return false;
    }
    for (const [sku, qty] of wanted) this.#reserved.set(sku, (this.#reserved.get(sku) ?? 0) + qty);
    return true;
  }

  release(sku: string, qty: number): void {
    const reserved = this.#reserved.get(sku) ?? 0;
    if (qty > reserved) throw new Error(`only ${reserved} of ${sku} reserved`);
    this.#reserved.set(sku, reserved - qty);
  }
}
