export interface Line {
  sku: string;
  category: string;
  unitPriceCents: number;
  qty: number;
}

export interface Coupon {
  code: string;
  /** Whole percent, 1 to 100. */
  percentOff: number;
  /** Lines in this category only; every line when absent. */
  category?: string;
}

export interface Customer {
  id: string;
  /** In the trade programme: 3% off every line. */
  member: boolean;
}

export interface Cart {
  lines: Line[];
  /** Shipping and tax region, e.g. "US-CA" or "EU-DE". */
  region: string;
}
