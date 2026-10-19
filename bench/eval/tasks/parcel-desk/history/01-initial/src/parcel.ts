export type ContentsItem = {
  description: string;
  qty: number;
  /** Value of one item, in pence. */
  valuePence: number;
  /** Harmonized System code, for customs. */
  hsCode?: string;
};

export type Parcel = {
  weightKg: number;
  lengthCm: number;
  widthCm: number;
  heightCm: number;
  /** Contains lithium batteries (packed with equipment). */
  hazmat?: boolean;
  /** What is inside, for customs declarations. */
  contents?: ContentsItem[];
};

export class ParcelError extends Error {}

/** The carrier's limits for any service. */
export const MAX_KG = 30;
export const MAX_LENGTH_CM = 150;
export const MAX_GIRTH_CM = 300;

/** Length plus twice the width plus twice the height. */
export function girthCm(p: Parcel): number {
  return p.lengthCm + 2 * p.widthCm + 2 * p.heightCm;
}

export function volumeCm3(p: Parcel): number {
  return p.lengthCm * p.widthCm * p.heightCm;
}

export function checkParcel(p: Parcel): void {
  for (const [name, v] of [["weight", p.weightKg], ["length", p.lengthCm], ["width", p.widthCm], ["height", p.heightCm]] as const) {
    if (!(v > 0) || !Number.isFinite(v)) throw new ParcelError(`${name} must be positive`);
  }
  if (p.weightKg > MAX_KG) throw new ParcelError(`over ${MAX_KG} kg`);
  if (Math.max(p.lengthCm, p.widthCm, p.heightCm) > MAX_LENGTH_CM) throw new ParcelError(`longer than ${MAX_LENGTH_CM} cm`);
  if (girthCm(p) > MAX_GIRTH_CM) throw new ParcelError(`girth over ${MAX_GIRTH_CM} cm`);
  for (const item of p.contents ?? []) {
    if (!Number.isInteger(item.qty) || item.qty < 1) throw new ParcelError(`bad quantity for ${item.description}`);
  }
}

/** `2.5 kg`. */
export function formatKg(kg: number): string {
  return `${kg.toFixed(1)} kg`;
}
