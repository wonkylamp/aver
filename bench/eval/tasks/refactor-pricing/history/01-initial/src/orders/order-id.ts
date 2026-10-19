// HB-20260301-000042: the shop prefix, the order date and a daily sequence.
export function formatOrderId(sequence: number, isoDate: string): string {
  if (!Number.isInteger(sequence) || sequence < 1 || sequence > 999999) {
    throw new Error(`order sequence out of range: ${sequence}`);
  }
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(isoDate);
  if (match === null) throw new Error(`not an ISO date: ${isoDate}`);
  return `HB-${match[1]}${match[2]}${match[3]}-${String(sequence).padStart(6, "0")}`;
}

export function parseOrderId(id: string): { date: string; sequence: number } {
  const match = /^HB-(\d{4})(\d{2})(\d{2})-(\d{6})$/.exec(id);
  if (match === null) throw new Error(`not an order id: ${id}`);
  return { date: `${match[1]}-${match[2]}-${match[3]}`, sequence: Number(match[4]) };
}
