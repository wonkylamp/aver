export type TaxClass = "standard" | "reduced" | "zero";

export interface Product {
  sku: string;
  name: string;
  /** Minor units, as the store shows it. */
  priceCents: number;
  taxClass: TaxClass;
  weightGrams: number;
  /** Gift cards are money, not merchandise: no tax, no discounts, no shipping weight. */
  giftCard?: boolean;
}

export const CATALOG: Record<string, Product> = {
  "MUG-01": { sku: "MUG-01", name: "Enamel mug", priceCents: 1800, taxClass: "standard", weightGrams: 350 },
  "TEE-02": { sku: "TEE-02", name: "Organic tee", priceCents: 2500, taxClass: "standard", weightGrams: 200 },
  "BOOK-03": { sku: "BOOK-03", name: "Field guide", priceCents: 1999, taxClass: "reduced", weightGrams: 600 },
  "TEA-04": { sku: "TEA-04", name: "Loose leaf tea", priceCents: 899, taxClass: "reduced", weightGrams: 120 },
  "LAMP-05": { sku: "LAMP-05", name: "Desk lamp", priceCents: 7900, taxClass: "standard", weightGrams: 1800 },
  "SEED-06": { sku: "SEED-06", name: "Seed packet", priceCents: 350, taxClass: "zero", weightGrams: 20 },
  "GIFT-25": { sku: "GIFT-25", name: "Gift card 25", priceCents: 2500, taxClass: "zero", weightGrams: 0, giftCard: true },
};

export class CatalogError extends Error {
  override name = "CatalogError";
}

export function product(sku: string): Product {
  const p = CATALOG[sku];
  if (!p) throw new CatalogError(`unknown product ${sku}`);
  return p;
}
