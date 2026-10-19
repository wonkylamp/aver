import type { Line } from "../types.ts";

export interface Product {
  sku: string;
  name: string;
  category: string;
  unitPriceCents: number;
}

const PRODUCTS: Product[] = [
  { sku: "ROPE-12", name: "Mooring rope, 12 mm, per metre", category: "rigging", unitPriceCents: 349 },
  { sku: "ROPE-16", name: "Mooring rope, 16 mm, per metre", category: "rigging", unitPriceCents: 529 },
  { sku: "SHACKLE-8", name: "Bow shackle, 8 mm", category: "rigging", unitPriceCents: 1275 },
  { sku: "FENDER-M", name: "Fender, medium", category: "deck", unitPriceCents: 3450 },
  { sku: "CLEAT-150", name: "Cleat, 150 mm", category: "deck", unitPriceCents: 1899 },
  { sku: "VARNISH-1L", name: "Marine varnish, 1 l", category: "paint", unitPriceCents: 2799 },
  { sku: "BRUSH-50", name: "Brush, 50 mm", category: "paint", unitPriceCents: 450 },
];

const BY_SKU = new Map(PRODUCTS.map((p) => [p.sku, p]));

export function findProduct(sku: string): Product | undefined {
  return BY_SKU.get(sku.trim().toUpperCase());
}

export function listCategory(category: string): Product[] {
  return PRODUCTS.filter((p) => p.category === category).sort((a, b) => (a.sku < b.sku ? -1 : 1));
}

export function toLine(sku: string, qty: number): Line {
  const product = findProduct(sku);
  if (product === undefined) throw new Error(`unknown SKU: ${sku}`);
  if (!Number.isInteger(qty) || qty < 1) throw new Error(`quantity must be a positive whole number, got ${qty}`);
  return { sku: product.sku, category: product.category, unitPriceCents: product.unitPriceCents, qty };
}
