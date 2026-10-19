interface ShippingRule {
  flatCents: number;
  /** Subtotal at or above which shipping is free. */
  freeFromCents: number;
}

const RULES: Record<string, ShippingRule> = {
  US: { flatCents: 599, freeFromCents: 7500 },
  EU: { flatCents: 1499, freeFromCents: 15000 },
};

export function shippingCents(subtotalCents: number, region: string): number {
  const rule = RULES[region.split("-", 1)[0]];
  if (rule === undefined) throw new Error(`we do not ship to ${region}`);
  return subtotalCents >= rule.freeFromCents ? 0 : rule.flatCents;
}
