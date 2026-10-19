export interface Vendor {
  id: string;
  name: string;
  email?: string;
  phone?: string;
  vatNumber?: string;
  paymentTermsDays: number;
  active: boolean;
  /** Open balance we owe, in cents. */
  balanceCents: number;
  lastPaidAt?: Date;
}

export class VendorError extends Error {
  override name = "VendorError";
}

const VAT = /^[A-Z]{2}[0-9A-Z]{2,13}$/;

export function validateVendor(v: Vendor): void {
  if (!v.name.trim()) throw new VendorError("a vendor needs a name");
  if (v.vatNumber !== undefined && !VAT.test(v.vatNumber)) throw new VendorError(`bad VAT number ${v.vatNumber}`);
  if (!Number.isInteger(v.paymentTermsDays) || v.paymentTermsDays < 0 || v.paymentTermsDays > 120) {
    throw new VendorError(`payment terms are 0-120 days, not ${v.paymentTermsDays}`);
  }
}

export function dueDate(invoiceDate: Date, v: Vendor): Date {
  return new Date(invoiceDate.getTime() + v.paymentTermsDays * 86_400_000);
}
